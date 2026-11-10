// SPDX-License-Identifier: MIT OR Apache-2.0
//! The user's post office, as the gateway's engine reaches it (RP-5 §5): in this process
//! ([`InProcess`], for tests and for a gateway that runs beside its post office) or over HTTPS
//! ([`Http`]), logging in with the gateway's own credential (RP-5 §3).

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use radiopost::ErrorCode;
use radiopost::base64url;
use radiopost::crypto::{self, ContentId};
use radiopost::session::gateway::Upstream;
use radiopost_postoffice::{PostOffice, Request};
use serde_json::{Value as Json, json};

/// The code a problem body carries (RP-5 §1), or `internal`.
fn code_of(body: Option<&Json>) -> ErrorCode {
    body.and_then(|b| b.get("code"))
        .and_then(Json::as_u64)
        .and_then(ErrorCode::from_code)
        .unwrap_or(ErrorCode::Internal)
}

/// What one RP-5 request comes to: a status and a body.
pub type Answer = (u16, Option<Json>);

/// How a gateway talks to one post office: one request at a time.
trait Transport: Send {
    fn request(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&Json>,
    ) -> Result<Answer, ErrorCode>;
}

/// The session calls of RP-5 §5, over any transport.
struct Session<T: Transport> {
    transport: T,
    session: Option<String>,
}

impl<T: Transport> Upstream for Session<T> {
    fn open(&mut self, body: &Json) -> Result<Json, ErrorCode> {
        match self.transport.request("POST", "/sessions", Some(body))? {
            (201, Some(answer)) => {
                self.session = answer
                    .get("session")
                    .and_then(Json::as_str)
                    .map(str::to_owned);
                Ok(answer)
            }
            (_, body) => Err(code_of(body.as_ref())),
        }
    }

    fn fetch(&mut self, serial: u64) -> Result<Vec<u8>, ErrorCode> {
        let sid = self.session.clone().ok_or(ErrorCode::UnknownMessage)?;
        match self
            .transport
            .request("GET", &format!("/sessions/{sid}/messages/{serial}"), None)?
        {
            (200, Some(m)) => m
                .get("air")
                .and_then(Json::as_str)
                .and_then(base64url::decode)
                .ok_or(ErrorCode::Malformed),
            (_, body) => Err(code_of(body.as_ref())),
        }
    }

    fn hand_in(
        &mut self,
        messages: &[(ContentId, Vec<u8>)],
    ) -> Result<Vec<Result<(), ErrorCode>>, ErrorCode> {
        let sid = self.session.clone().ok_or(ErrorCode::UnknownMessage)?;
        let body = json!({"messages": messages.iter()
            .map(|(id, air)| json!({"id": id.text(), "air": base64url::encode(air)}))
            .collect::<Vec<_>>()});
        match self
            .transport
            .request("POST", &format!("/sessions/{sid}/messages"), Some(&body))?
        {
            (200, Some(answer)) => Ok(answer
                .get("results")
                .and_then(Json::as_array)
                .map(|r| {
                    r.iter()
                        .map(|x| match x.get("status").and_then(Json::as_str) {
                            Some("accepted" | "duplicate") => Ok(()),
                            _ => Err(code_of(Some(x))),
                        })
                        .collect()
                })
                .unwrap_or_default()),
            (_, body) => Err(code_of(body.as_ref())),
        }
    }

    fn picked_up(&mut self, serials: &[u64]) -> Result<(), ErrorCode> {
        let sid = self.session.clone().ok_or(ErrorCode::UnknownMessage)?;
        match self.transport.request(
            "POST",
            &format!("/sessions/{sid}/pickups"),
            Some(&json!({"serials": serials})),
        )? {
            (204, _) => Ok(()),
            (_, body) => Err(code_of(body.as_ref())),
        }
    }

    fn close(&mut self) {
        if let Some(sid) = self.session.take() {
            let _ = self
                .transport
                .request("DELETE", &format!("/sessions/{sid}"), None);
        }
    }
}

/// A post office in this process, reached with a gateway's token.
pub struct InProcess {
    office: Arc<Mutex<PostOffice>>,
    token: String,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl Transport for InProcess {
    fn request(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&Json>,
    ) -> Result<Answer, ErrorCode> {
        let mut office = self.office.lock().unwrap_or_else(PoisonError::into_inner);
        let answer = office.handle(
            (self.clock)(),
            &Request {
                method: method.into(),
                path: path.into(),
                token: Some(self.token.clone()),
                body: body.cloned(),
            },
        );
        Ok((answer.status, answer.body))
    }
}

/// The session calls to a post office in this process, as a gateway logged in with `token`.
#[must_use]
pub fn in_process(
    office: Arc<Mutex<PostOffice>>,
    token: String,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
) -> Box<dyn Upstream + Send> {
    Box::new(Session {
        transport: InProcess {
            office,
            token,
            clock,
        },
        session: None,
    })
}

/// The gateway's own credential and device key, which it logs in to post offices with.
pub struct Account {
    /// Its credential, with the `gateway` flag (RP-5 §6.4).
    pub credential: Vec<u8>,
    /// Its device key.
    pub key: SigningKey,
}

/// A post office's API over HTTPS, with the token the gateway logged in for, renewed when it
/// runs out (RP-5 §3).
pub struct Http {
    base: String,
    account: Arc<Account>,
    token: Arc<Mutex<Option<String>>>,
    agent: ureq::Agent,
}

impl Http {
    /// The API at `base` (its base URL, RP-5 §1), logged in as `account`; `token` is shared by
    /// every session to that post office.
    #[must_use]
    pub fn new(base: &str, account: Arc<Account>, token: Arc<Mutex<Option<String>>>) -> Http {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .into();
        Http {
            base: base.trim_end_matches('/').to_owned(),
            account,
            token,
            agent,
        }
    }

    fn send(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&Json>,
    ) -> Result<Answer, ErrorCode> {
        let url = format!("{}{path}", self.base);
        let auth = token.map(|t| format!("Bearer {t}"));
        let result = match (method, body) {
            ("GET", _) => {
                let mut r = self.agent.get(&url);
                if let Some(a) = &auth {
                    r = r.header("Authorization", a);
                }
                r.call()
            }
            ("DELETE", _) => {
                let mut r = self.agent.delete(&url);
                if let Some(a) = &auth {
                    r = r.header("Authorization", a);
                }
                r.call()
            }
            (m, body) => {
                let mut r = if m == "PUT" {
                    self.agent.put(&url)
                } else {
                    self.agent.post(&url)
                };
                if let Some(a) = &auth {
                    r = r.header("Authorization", a);
                }
                r = r.header("Content-Type", "application/json");
                r.send(body.map(Json::to_string).unwrap_or_default())
            }
        };
        let mut response = result.map_err(|_| ErrorCode::PostOfficeUnreachable)?;
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().unwrap_or_default();
        Ok((status, serde_json::from_str(&text).ok()))
    }

    /// RP-5 §3: a challenge, then a token for the gateway's credential.
    fn login(&self) -> Result<String, ErrorCode> {
        let (_, challenge) = self.send("POST", "/auth/challenge", None, None)?;
        let nonce = challenge
            .as_ref()
            .and_then(|c| c.get("nonce"))
            .and_then(Json::as_str)
            .ok_or(ErrorCode::PostOfficeUnreachable)?
            .to_owned();
        let mut signed = base64url::decode(&nonce).ok_or(ErrorCode::Malformed)?;
        signed.extend_from_slice(self.base.as_bytes());
        let signature = crypto::sign(crypto::context::API_LOGIN, &signed, &self.account.key);
        let body = json!({"credential": base64url::encode(&self.account.credential),
                          "nonce": nonce, "signature": base64url::encode(&signature)});
        match self.send("POST", "/auth/token", None, Some(&body))? {
            (200, Some(t)) => t
                .get("token")
                .and_then(Json::as_str)
                .map(str::to_owned)
                .ok_or(ErrorCode::Internal),
            (_, body) => Err(code_of(body.as_ref())),
        }
    }
}

impl Http {
    /// One request as the logged-in account: its status and body, logging in first, and again
    /// when the token ran out.
    ///
    /// # Errors
    ///
    /// The post office could not be reached, or refused the login.
    pub fn call(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&Json>,
    ) -> Result<Answer, ErrorCode> {
        self.request(method, path, body)
    }
}

impl Transport for Http {
    fn request(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&Json>,
    ) -> Result<Answer, ErrorCode> {
        let held = self
            .token
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let token = match held {
            Some(t) => t,
            None => self.login()?,
        };
        let mut answer = self.send(method, path, Some(&token), body)?;
        let mut current = token;
        if answer.0 == 401 {
            // the token ran out (RP-5 §3.2): log in again, once
            current = self.login()?;
            answer = self.send(method, path, Some(&current), body)?;
        }
        *self.token.lock().unwrap_or_else(PoisonError::into_inner) = Some(current);
        Ok(answer)
    }
}

/// The session calls to a post office over HTTPS.
#[must_use]
pub fn http(
    base: &str,
    account: Arc<Account>,
    token: Arc<Mutex<Option<String>>>,
) -> Box<dyn Upstream + Send> {
    Box::new(Session {
        transport: Http::new(base, account, token),
        session: None,
    })
}
