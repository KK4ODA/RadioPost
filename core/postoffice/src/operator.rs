// SPDX-License-Identifier: MIT OR Apache-2.0
//! Applications for accounts (RP-5 §6.7) and the operator endpoints (RP-5 §7).
//!
//! Someone with a licence applies from a new device; the post office keeps the application, and
//! its operators, logged in with their own credentials, approve it, decline it or send a code by
//! post. The same endpoints show the accounts, make codes and revoke devices. Like the rest of
//! [`PostOffice`], this is sans-IO: what the service adds (its own checks, a licence database,
//! the mail server) it adds around these answers, never inside them, so the vectors replay.

use radiopost::ErrorCode;
use radiopost::base64url;
use radiopost::credential::flags;
use radiopost::crypto::{self, context};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json, json};

use crate::office::{CODE_S, PostOffice, Response, domain_of, is_entry, problem, text};

/// How long a code sent by post is good for (RP-5 §7.3: at least 30 days).
pub const POSTCARD_S: u64 = 30 * 86_400;
/// How long a pending application is kept (RP-5 §6.7: at least 14 days).
pub const APPLICATION_S: u64 = 30 * 86_400;
/// How long a decided application is answered (RP-5 §6.7: at least 7 days).
pub const DECIDED_S: u64 = 7 * 86_400;
/// The letters of an enrollment code, read out by people: no 0, O, 1 or I.
const CODE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";

/// An application for an account (RP-5 §6.7).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Application {
    /// Its ID, `ap_` and 16 random bytes in base64url.
    pub application: String,
    /// `pending`, `approved`, `declined` or `postcard`.
    pub status: String,
    /// The account asked for.
    pub address: String,
    /// The applicant's callsign.
    pub operator: String,
    /// The name on the licence, as the applicant gave it.
    pub name: String,
    /// Where to write to the applicant.
    pub contact: String,
    /// The new device's key, base64url.
    pub device: String,
    /// When it came.
    pub received: u64,
    /// When it was decided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided: Option<u64>,
    /// The flags it was approved with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flags: Option<u64>,
    /// Why it was declined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The code sent by post.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// That code's end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_expires: Option<u64>,
    /// The credential issued, base64url, for the applicant to collect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
    /// What the service found of the callsign for the operators (a licence database, a
    /// callbook): its own members of the entry (RP-5 §7.3), never part of a decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Json>,
    /// The mailbox made for an approved application: `{"made"}` or `{"error"}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mailbox: Option<Json>,
    /// The email about the decision: `{"to", "at"}`, with `error` when it did not go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub told: Option<Json>,
}

impl Application {
    /// Kept: pending for 30 days, answered for 7 after a decision.
    fn alive(&self, now: u64) -> bool {
        let until = self
            .decided
            .map_or(self.received + APPLICATION_S, |d| d + DECIDED_S);
        now < until
    }

    fn pending(&self, now: u64) -> bool {
        self.status == "pending" && self.alive(now)
    }

    /// The entry the operators see (RP-5 §7.3): everything but the credential.
    #[must_use]
    pub fn entry(&self) -> Json {
        let mut entry = serde_json::to_value(self).unwrap_or_else(|_| json!({}));
        if let Some(map) = entry.as_object_mut() {
            map.remove("credential");
            if self.status == "pending" {
                map.insert("expires".into(), json!(self.received + APPLICATION_S));
            }
        }
        entry
    }
}

/// A credential the post office issued, for its operators' list of accounts (RP-5 §7.4).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Issued {
    /// The account.
    pub address: String,
    /// The callsign.
    pub operator: String,
    /// The device key, hex.
    pub device: String,
    /// Its flags.
    pub flags: u64,
    /// Its end.
    pub expires: u64,
}

/// RP-0 §6.2's local part for an account a post office makes, at a lowercase domain.
fn account_address(a: &str) -> bool {
    let Some((local, domain)) = a.split_once('@') else {
        return false;
    };
    (1..=64).contains(&local.len())
        && local
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c))
        && labels(domain, |c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
        })
}

fn labels(domain: &str, ok: impl Fn(char) -> bool) -> bool {
    let parts: Vec<&str> = domain.split('.').collect();
    parts.len() >= 2 && parts.iter().all(|l| !l.is_empty() && l.chars().all(&ok))
}

/// An address the applicant can be written to at.
fn contact_address(a: &str) -> bool {
    let Some((local, domain)) = a.split_once('@') else {
        return false;
    };
    a.chars().count() <= 254
        && (1..=64).contains(&local.chars().count())
        && !local.contains(char::is_whitespace)
        && labels(domain, |c| c.is_ascii_alphanumeric() || c == '-')
}

/// RP-0 §6.1's `base`: 1 to 10 uppercase letters and digits.
fn base_callsign(c: &str) -> bool {
    (1..=10).contains(&c.len())
        && c.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// RP-0 §6.1: `[prefix "/"] base ["/" suffix] ["-" ssid]`, at most 20 characters, uppercase.
fn callsign(c: &str) -> bool {
    let (body, ssid) = c.split_once('-').map_or((c, None), |(b, s)| (b, Some(s)));
    let ssid_ok = ssid.is_none_or(|s| {
        s.len() == 1 && s.as_bytes()[0].is_ascii_digit()
            || s.len() == 2 && s.starts_with('1') && (b'0'..=b'5').contains(&s.as_bytes()[1])
    });
    let parts: Vec<&str> = body.split('/').collect();
    c.len() <= 20
        && ssid_ok
        && (1..=3).contains(&parts.len())
        && parts.into_iter().all(base_callsign)
}

fn display_name(name: &str) -> bool {
    (1..=64).contains(&name.chars().count()) && !name.trim().is_empty()
}

/// RP-5 §7.3: `send`, `receive` and `role` by name; none given, `send` and `receive`.
/// RP-5 §7.1: refused because it would make someone an operator; `operates` names the callsign.
fn operates(callsign: &str) -> Response {
    let mut r = problem(ErrorCode::RefusedByPolicy, None);
    if let Some(body) = r.body.as_mut() {
        body["operates"] = json!(callsign);
    }
    r
}

fn grantable(given: Option<&Json>) -> Option<u64> {
    let list = match given {
        None | Some(Json::Null) => return Some(flags::SEND | flags::RECEIVE),
        Some(Json::Array(list)) => list,
        Some(_) => return None,
    };
    list.iter().try_fold(0, |f, name| {
        Some(
            f | match name.as_str()? {
                "send" => flags::SEND,
                "receive" => flags::RECEIVE,
                "role" => flags::ROLE,
                _ => return None,
            },
        )
    })
}

impl PostOffice {
    /// RP-5 §7.1: a credential this post office issued, to one of its operators, not a gateway's.
    #[must_use]
    pub fn is_operator(&self, address: &str, operator: &str, credential_flags: u64) -> bool {
        self.config.domains.iter().any(|d| d == domain_of(address))
            && self.config.operators.iter().any(|o| o == operator)
            && credential_flags & flags::GATEWAY == 0
    }

    /// Whether a token may use the operator endpoints, for what the service adds under `/office`.
    #[must_use]
    pub fn operator_token(&self, token: &str) -> Option<String> {
        let held = self.state.tokens.get(token)?;
        let good = held.expires > self.now()
            && !self.state.revoked.contains(&held.serial)
            && self.is_operator(&held.address, &held.operator, held.flags);
        good.then(|| held.operator.clone())
    }

    /// A new enrollment code: eight characters in two groups, `7Q4F-AZ9K`.
    pub fn new_code(&mut self) -> String {
        let raw = self.fresh("code", 8);
        let chars: String = raw
            .iter()
            .map(|b| char::from(CODE_ALPHABET[usize::from(*b) % CODE_ALPHABET.len()]))
            .collect();
        format!("{}-{}", &chars[..4], &chars[4..])
    }

    /// RP-5 §6.7: an application, checked in its order; it waits for the operators.
    pub(crate) fn apply(&mut self, body: &Json) -> Response {
        let string = |k: &str| body.get(k).and_then(Json::as_str);
        let (address, operator) = (string("address"), string("operator"));
        let (name, contact) = (string("name"), string("contact"));
        let device = string("device")
            .and_then(base64url::decode)
            .and_then(|d| <[u8; 32]>::try_from(d).ok());
        let signature = string("signature").and_then(base64url::decode);
        let (
            Some(address),
            Some(operator),
            Some(name),
            Some(contact),
            Some(device),
            Some(signature),
        ) = (address, operator, name, contact, device, signature)
        else {
            return problem(ErrorCode::Malformed, None);
        };
        if !(account_address(address)
            && base_callsign(operator)
            && display_name(name)
            && contact_address(contact))
        {
            return problem(ErrorCode::Malformed, None);
        }
        let ours = self.config.domains.iter().any(|d| d == domain_of(address));
        if !self.config.applications_open || !ours {
            return problem(ErrorCode::RefusedByPolicy, None);
        }
        if self.operates(operator) {
            return operates(operator); // RP-5 §7.1: nobody becomes an operator by applying
        }
        let now = self.now();
        let asked = self
            .state
            .applications
            .iter()
            .any(|a| a.address == address && a.pending(now));
        if self.state.accounts.contains(address) || asked {
            return problem(ErrorCode::AlreadyHave, None);
        }
        let signed = format!("{address}\n{operator}\n{}", self.config.base_url);
        if !crypto::verify(context::API_APPLY, signed.as_bytes(), &signature, &device) {
            return problem(ErrorCode::SignatureInvalid, None);
        }
        let id = format!("ap_{}", base64url::encode(&self.fresh("application", 16)));
        self.state.applications.push(Application {
            application: id.clone(),
            status: "pending".into(),
            address: address.to_owned(),
            operator: operator.to_owned(),
            name: name.to_owned(),
            contact: contact.to_owned(),
            device: base64url::encode(&device),
            received: now,
            ..Application::default()
        });
        Response::ok(
            202,
            json!({"application": id, "status": "pending", "expires": now + APPLICATION_S}),
        )
    }

    /// RP-5 §6.7: what the applicant may see of an application.
    pub(crate) fn application(&self, id: &str) -> Response {
        let now = self.now();
        let Some(app) = self
            .state
            .applications
            .iter()
            .find(|a| a.application == id && a.alive(now))
        else {
            return problem(ErrorCode::UnknownMessage, Some(404));
        };
        let mut answer = json!({"application": id, "status": app.status});
        match app.status.as_str() {
            "pending" => answer["expires"] = json!(app.received + APPLICATION_S),
            "approved" => answer["credential"] = json!(app.credential),
            "declined" => answer["text"] = json!(app.text),
            _ => {}
        }
        Response::ok(200, answer)
    }

    /// Whether a credential naming `callsign` gives the operator endpoints (RP-5 §7.1).
    #[must_use]
    pub fn operates(&self, callsign: &str) -> bool {
        self.config.operators.iter().any(|o| o == callsign)
    }

    /// RP-5 §7: what an operator asks, after RP-5 §7.1's check. `asking` is the token's account
    /// and callsign; the operator's port, which skips the check, asks as nobody.
    pub fn office(
        &mut self,
        method: &str,
        rest: &[&str],
        body: &Json,
        asking: Option<(&str, &str)>,
    ) -> Response {
        let now = self.now();
        match (method, rest) {
            ("GET", ["status"]) => Response::ok(
                200,
                json!({
                    "accounts": self.state.accounts.len(),
                    "applications": self.state.applications.iter()
                        .filter(|a| a.pending(now)).count(),
                    "codes": self.state.codes.values().filter(|c| c.expires > now).count(),
                    "sessions": self.state.sessions.values().filter(|s| s.expires > now).count(),
                }),
            ),
            ("GET", ["applications"]) => {
                let list: Vec<Json> = self
                    .state
                    .applications
                    .iter()
                    .filter(|a| a.alive(now))
                    .map(Application::entry)
                    .collect();
                Response::ok(200, json!({"applications": list}))
            }
            ("POST", ["applications", id]) => self.decide(id, body),
            ("GET", ["accounts"]) => {
                let list: Vec<Json> = self
                    .state
                    .accounts
                    .iter()
                    .map(|a| self.account(a))
                    .collect();
                Response::ok(200, json!({"accounts": list}))
            }
            ("POST", ["codes"]) => self.make_code(body, asking),
            ("POST", ["revoke"]) => {
                let device = body
                    .get("device")
                    .and_then(Json::as_str)
                    .and_then(base64url::decode)
                    .and_then(|d| <[u8; 32]>::try_from(d).ok());
                match device {
                    Some(d) => Response::ok(200, json!({"revoked": self.revoke_device(&d)})),
                    None => problem(ErrorCode::Malformed, None),
                }
            }
            _ => problem(ErrorCode::UnknownMessage, Some(404)),
        }
    }

    /// RP-5 §7.3: approve, decline or send a code by post.
    fn decide(&mut self, id: &str, body: &Json) -> Response {
        let now = self.now();
        let Some(i) = self
            .state
            .applications
            .iter()
            .position(|a| a.application == id && a.alive(now))
        else {
            return problem(ErrorCode::UnknownMessage, Some(404));
        };
        let decision = text(body, "decision");
        let granted = if decision == "approve" {
            grantable(body.get("flags"))
        } else {
            Some(0)
        };
        let reason = body.get("text").and_then(Json::as_str).unwrap_or("");
        if !matches!(decision, "approve" | "decline" | "postcard")
            || granted.is_none()
            || (decision == "decline" && reason.is_empty())
        {
            return problem(ErrorCode::Malformed, None);
        }
        let app = self.state.applications[i].clone();
        if app.status != "pending"
            || (decision != "decline" && self.state.accounts.contains(&app.address))
        {
            return problem(ErrorCode::AlreadyHave, None);
        }
        if decision != "decline" && self.operates(&app.operator) {
            return operates(&app.operator); // RP-5 §7.3: its callsign became an operator's since
        }
        let mut app = app;
        app.decided = Some(now);
        match decision {
            "approve" => {
                let device = base64url::decode(&app.device)
                    .and_then(|d| <[u8; 32]>::try_from(d).ok())
                    .unwrap_or([0; 32]);
                let f = granted.unwrap_or(0);
                let credential = self.issue(&app.address, &app.operator, device, f);
                self.state
                    .names
                    .insert(app.address.clone(), app.name.clone());
                app.status = "approved".into();
                app.flags = Some(f);
                app.credential = Some(base64url::encode(&credential));
            }
            "decline" => {
                app.status = "declined".into();
                app.text = Some(reason.to_owned());
            }
            _ => {
                let code = self.new_code();
                let both = flags::SEND | flags::RECEIVE;
                self.add_code_for(&code, &app.address, &app.operator, both, POSTCARD_S);
                self.state
                    .names
                    .insert(app.address.clone(), app.name.clone());
                app.status = "postcard".into();
                app.code = Some(code);
                app.code_expires = Some(now + POSTCARD_S);
            }
        }
        let entry = app.entry();
        self.state.applications[i] = app;
        Response::ok(200, entry)
    }

    /// RP-5 §7.4: an account, its waiting mail and its credentials not yet expired.
    fn account(&self, address: &str) -> Json {
        let now = self.now();
        let credentials: Vec<Json> = self
            .state
            .credentials
            .iter()
            .filter(|(_, c)| c.address == address && c.expires > now)
            .map(|(serial, c)| {
                let device: Vec<u8> = (0..c.device.len())
                    .step_by(2)
                    .filter_map(|i| u8::from_str_radix(c.device.get(i..i + 2)?, 16).ok())
                    .collect();
                json!({"serial": serial, "operator": c.operator,
                       "device": base64url::encode(&device), "flags": c.flags,
                       "expires": c.expires, "revoked": self.state.revoked.contains(serial)})
            })
            .collect();
        let waiting = self
            .state
            .mailboxes
            .get(address)
            .map_or(0, |b| b.iter().filter(|w| !w.picked).count());
        let mut entry = Map::new();
        entry.insert("address".into(), json!(address));
        if let Some(name) = self.state.names.get(address) {
            entry.insert("name".into(), json!(name));
        }
        entry.insert("waiting".into(), json!(waiting));
        entry.insert("credentials".into(), json!(credentials));
        Json::Object(entry)
    }

    /// RP-5 §7.5: a code for a new account, or a new device of one; one that makes an operator
    /// only when asked for with `operates`, unless it is the asking operator's own.
    fn make_code(&mut self, body: &Json, asking: Option<(&str, &str)>) -> Response {
        let address = body.get("address").and_then(Json::as_str);
        let operator = body.get("operator").and_then(Json::as_str);
        let name = match body.get("name") {
            None | Some(Json::Null) => Some(None),
            Some(Json::String(n)) if display_name(n) => Some(Some(n.clone())),
            Some(_) => None,
        };
        let granted = grantable(body.get("flags"));
        let confirmed = match body.get("operates") {
            None => Some(false),
            Some(Json::Bool(b)) => Some(*b),
            Some(_) => None,
        };
        let (Some(address), Some(operator), Some(name), Some(f), Some(confirmed)) =
            (address, operator, name, granted, confirmed)
        else {
            return problem(ErrorCode::Malformed, None);
        };
        let ours = self.config.domains.iter().any(|d| d == domain_of(address));
        if !(is_entry(address) && !address.starts_with('@') && ours && callsign(operator)) {
            return problem(ErrorCode::Malformed, None);
        }
        let gives = self.operates(operator); // RP-5 §7.1: whoever enrolls with it is an operator
        if gives && !confirmed && asking != Some((address, operator)) {
            return operates(operator);
        }
        let code = self.new_code();
        self.add_code(&code, address, operator, f);
        if let Some(name) = name {
            self.state.names.insert(address.to_owned(), name);
        }
        let expires = self.now() + CODE_S;
        let mut answer = json!({"code": code, "address": address, "operator": operator, "flags": f,
                                "expires": expires});
        if gives {
            answer["operates"] = json!(true);
        }
        Response::ok(200, answer)
    }
}
