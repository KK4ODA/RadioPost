// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-5 over HTTPS, as gateways and clients use it: logging in with a credential (RP-5 §3) and
//! renewing the token when it runs out, requests and their problem codes (§1), enrollment (§6.1),
//! and a domain's post office found in DNS (RP-1 §7.1) by DNS over HTTPS.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use radiopost::ErrorCode;
use radiopost::base64url;
use radiopost::crypto::{self, context};
use serde_json::{Value as Json, json};

/// Where DNS is asked when nothing else is set: a DNS-over-HTTPS JSON resolver.
pub const RESOLVER: &str = "https://dns.google/resolve";

/// What one RP-5 request comes to: a status and a body.
pub type Answer = (u16, Option<Json>);

/// The code a problem body carries (RP-5 §1), or `internal`.
#[must_use]
pub fn code_of(body: Option<&Json>) -> ErrorCode {
    body.and_then(|b| b.get("code"))
        .and_then(Json::as_u64)
        .and_then(ErrorCode::from_code)
        .unwrap_or(ErrorCode::Internal)
}

/// A credential and the device key it is for, to log in with.
pub struct Account {
    /// The credential (RP-1 §4).
    pub credential: Vec<u8>,
    /// Its device key.
    pub key: SigningKey,
}

/// A post office's API at its base URL, with the token logged in for, renewed when it runs out
/// (RP-5 §3).
pub struct Http {
    base: String,
    account: Option<Arc<Account>>,
    token: Arc<Mutex<Option<String>>>,
    agent: ureq::Agent,
}

impl Http {
    /// The API at `base` (its base URL, RP-5 §1), logged in as `account`; `token` may be shared by
    /// every user of that post office's API in the process.
    #[must_use]
    pub fn new(base: &str, account: Arc<Account>, token: Arc<Mutex<Option<String>>>) -> Http {
        Http {
            base: base.trim_end_matches('/').to_owned(),
            account: Some(account),
            token,
            agent: agent(),
        }
    }

    /// The API at `base` with no credential: enrollment and the public documents.
    #[must_use]
    pub fn anonymous(base: &str) -> Http {
        Http {
            base: base.trim_end_matches('/').to_owned(),
            account: None,
            token: Arc::new(Mutex::new(None)),
            agent: agent(),
        }
    }

    /// The base URL, as signed at login and enrollment.
    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }

    /// One request with no token, or `token`: its status and its body, read as JSON when it is.
    ///
    /// # Errors
    ///
    /// `post-office-unreachable`: no answer.
    pub fn send(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&Json>,
    ) -> Result<Answer, ErrorCode> {
        let (status, text) = self.send_text(method, path, token, body)?;
        Ok((status, serde_json::from_str(&text).ok()))
    }

    /// As [`Http::send`], the body as text (a JWS document, RP-5 §4).
    ///
    /// # Errors
    ///
    /// `post-office-unreachable`: no answer.
    pub fn send_text(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&Json>,
    ) -> Result<(u16, String), ErrorCode> {
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
        Ok((status, text))
    }

    /// RP-5 §3: a challenge, then a token for the account's credential.
    ///
    /// # Errors
    ///
    /// No account, no answer, or the post office refused the credential or the signature.
    pub fn login(&self) -> Result<String, ErrorCode> {
        let account = self.account.as_ref().ok_or(ErrorCode::CredentialRequired)?;
        let (_, challenge) = self.send("POST", "/auth/challenge", None, None)?;
        let nonce = challenge
            .as_ref()
            .and_then(|c| c.get("nonce"))
            .and_then(Json::as_str)
            .ok_or(ErrorCode::PostOfficeUnreachable)?
            .to_owned();
        let mut signed = base64url::decode(&nonce).ok_or(ErrorCode::Malformed)?;
        signed.extend_from_slice(self.base.as_bytes());
        let signature = crypto::sign(context::API_LOGIN, &signed, &account.key);
        let body = json!({"credential": base64url::encode(&account.credential),
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

    /// One request as the logged-in account: logging in first, and again when the token ran
    /// out (RP-5 §3.2).
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
            current = self.login()?;
            answer = self.send(method, path, Some(&current), body)?;
        }
        *self.token.lock().unwrap_or_else(PoisonError::into_inner) = Some(current);
        Ok(answer)
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into()
}

/// RP-5 §6.1: enrolls the device key `key` with a one-time code, and returns the credential.
///
/// # Errors
///
/// No answer, or the post office refused the code or the signature: the reason, in words.
pub fn enroll(
    base: &str,
    address: &str,
    operator: &str,
    code: &str,
    key: &SigningKey,
) -> Result<Vec<u8>, String> {
    let http = Http::anonymous(base);
    let mut signed = code.as_bytes().to_vec();
    signed.extend_from_slice(http.base().as_bytes());
    let signature = crypto::sign(context::API_ENROLL, &signed, key);
    let body = json!({"address": address, "operator": operator, "code": code,
                      "device": base64url::encode(key.verifying_key().as_bytes()),
                      "signature": base64url::encode(&signature)});
    match http.send("POST", "/enroll", None, Some(&body)) {
        Ok((200, Some(answer))) => answer
            .get("credential")
            .and_then(Json::as_str)
            .and_then(base64url::decode)
            .ok_or_else(|| "the post office's answer held no credential".to_owned()),
        Ok((status, body)) => Err(format!(
            "the post office refused ({status} {}): {}",
            code_of(body.as_ref()).name(),
            body.as_ref()
                .and_then(|b| b.get("title"))
                .and_then(Json::as_str)
                .unwrap_or("")
        )),
        Err(_) => Err(format!("cannot reach {}", http.base())),
    }
}

/// RP-5 §6.7: asks for an account from the device key `key`. Returns the post office's answer:
/// the application's ID, `pending`, and when it is forgotten.
///
/// # Errors
///
/// No answer, or the post office refused: the reason, in words.
pub fn apply(
    base: &str,
    address: &str,
    operator: &str,
    name: &str,
    contact: &str,
    key: &SigningKey,
) -> Result<Json, String> {
    let http = Http::anonymous(base);
    let signed = format!("{address}\n{operator}\n{}", http.base());
    let signature = crypto::sign(context::API_APPLY, signed.as_bytes(), key);
    let body = json!({"address": address, "operator": operator, "name": name,
                      "contact": contact,
                      "device": base64url::encode(key.verifying_key().as_bytes()),
                      "signature": base64url::encode(&signature)});
    match http.send("POST", "/applications", None, Some(&body)) {
        Ok((202, Some(answer))) => Ok(answer),
        Ok((status, body)) => Err(refusal(status, body.as_ref(), "application")),
        Err(_) => Err(format!("cannot reach {}", http.base())),
    }
}

/// RP-5 §6.7: where an application stands: `pending`, `approved` with its `credential`,
/// `declined` with its `text`, or `postcard`.
///
/// # Errors
///
/// No answer, or the post office does not know the application (forgotten, or never made).
pub fn application(base: &str, id: &str) -> Result<Json, String> {
    let http = Http::anonymous(base);
    match http.send("GET", &format!("/applications/{id}"), None, None) {
        Ok((200, Some(answer))) => Ok(answer),
        Ok((status, body)) => Err(refusal(status, body.as_ref(), "application")),
        Err(_) => Err(format!("cannot reach {}", http.base())),
    }
}

/// A refusal in words: what RP-5 §6.7 means by each.
fn refusal(status: u16, body: Option<&Json>, what: &str) -> String {
    let code = code_of(body);
    let why = match (what, code) {
        ("application", ErrorCode::RefusedByPolicy) => {
            "it takes no applications, or not for that domain".to_owned()
        }
        ("application", ErrorCode::AlreadyHave) => {
            "that address is taken, or already asked for".to_owned()
        }
        ("application", ErrorCode::Malformed) => "a field is not as it should be: the address \
            in lowercase, the callsign as licensed, with no SSID, your name and an email address"
            .to_owned(),
        ("application", ErrorCode::UnknownMessage) => {
            "it does not know the application: it was forgotten after 30 days, or never made"
                .to_owned()
        }
        _ => code.name().to_owned(),
    };
    format!("the post office refused ({status} {}): {why}", code.name())
}

/// The record types `query` asks for, by name and RFC 1035 number.
const TYPES: [(&str, u64); 6] = [
    ("A", 1),
    ("CNAME", 5),
    ("PTR", 12),
    ("MX", 15),
    ("TXT", 16),
    ("AAAA", 28),
];

/// One question to a DNS-over-HTTPS resolver that answers in JSON (Google's and Cloudflare's
/// form): the data of each answer of type `kind`, a TXT record's strings joined into one. A
/// name that does not exist, or has no such record, gives an empty list.
///
/// # Errors
///
/// The resolver cannot be reached, does not answer in JSON, or reports a failure other than a
/// name that does not exist.
pub fn query(resolver: &str, name: &str, kind: &str) -> Result<Vec<String>, String> {
    let number = TYPES
        .iter()
        .find(|(n, _)| *n == kind)
        .map(|(_, t)| *t)
        .ok_or_else(|| format!("{kind}: not a record type asked here"))?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    let answer: Json = agent
        .get(resolver)
        .query("name", name)
        .query("type", kind)
        .header("Accept", "application/dns-json")
        .call()
        .map_err(|e| format!("{resolver}: {e}"))?
        .body_mut()
        .read_to_string()
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .ok_or_else(|| format!("{resolver} gave no JSON for {name} {kind}"))?;
    match answer.get("Status").and_then(Json::as_u64) {
        Some(0 | 3) => {}
        other => return Err(format!("{name} {kind}: the resolver's status is {other:?}")),
    }
    Ok(answer
        .get("Answer")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter(|a| a.get("type").and_then(Json::as_u64) == Some(number))
        .filter_map(|a| a.get("data")?.as_str())
        .map(|d| {
            if kind == "TXT" {
                d.split('"')
                    .filter(|s| !s.trim().is_empty())
                    .collect::<String>()
            } else {
                d.to_owned()
            }
        })
        .collect())
}

/// RP-1 §7.1: a domain's `_radiopost` TXT records, read by DNS over HTTPS, each as its tags.
#[must_use]
pub fn records(resolver: &str, domain: &str) -> Vec<std::collections::BTreeMap<String, String>> {
    query(resolver, &format!("_radiopost.{domain}"), "TXT")
        .unwrap_or_default()
        .iter()
        .filter_map(|txt| radiopost::dns::parse(txt))
        .collect()
}

/// RP-1 §7.1: a domain's post office API, from its `t=api` record.
#[must_use]
pub fn lookup(resolver: &str, domain: &str) -> Option<String> {
    records(resolver, domain)
        .into_iter()
        .find(|t| t.get("t").map(String::as_str) == Some("api"))
        .and_then(|t| t.get("url").cloned())
}

/// RP-1 §7.1: a domain's post office keys, from its `t=key` records.
#[must_use]
pub fn keys(resolver: &str, domain: &str) -> Vec<[u8; 32]> {
    records(resolver, domain)
        .iter()
        .filter(|t| t.get("t").map(String::as_str) == Some("key"))
        .filter_map(radiopost::dns::key_of)
        .collect()
}
