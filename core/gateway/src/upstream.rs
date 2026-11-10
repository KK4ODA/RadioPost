// SPDX-License-Identifier: MIT OR Apache-2.0
//! The user's post office, as the gateway's engine reaches it (RP-5 §5): in this process
//! ([`InProcess`], for tests and for a gateway that runs beside its post office) or over HTTPS
//! ([`Http`]), logging in with the gateway's own credential (RP-5 §3).

use std::sync::{Arc, Mutex, PoisonError};

use radiopost::ErrorCode;
use radiopost::base64url;
use radiopost::crypto::ContentId;
use radiopost::session::gateway::Upstream;
use radiopost_postoffice::{PostOffice, Request};
use serde_json::{Value as Json, json};

pub use radiopost_api::{Account, Http};
use radiopost_api::{Answer, code_of};

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

impl Transport for Http {
    fn request(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&Json>,
    ) -> Result<Answer, ErrorCode> {
        self.call(method, path, body)
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
