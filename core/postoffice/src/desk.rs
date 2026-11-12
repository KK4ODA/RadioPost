// SPDX-License-Identifier: MIT OR Apache-2.0
//! The operator's desk: what `radiopost-po` adds under `/office` beside RP-5 §7.
//!
//! RP-5 §7 is what any post office may offer an operator's client. This service adds its own, as
//! RP-5 §7.2 and §7.3 allow: more members in `/office/status`, and endpoints a client may use or
//! ignore (`/office/outgoing`, `/office/name`, `/office/settings`). They answer only to a token
//! RP-5 §7.1 lets in, and they live here, around [`PostOffice::handle`], so that the vectors'
//! replay of the post office itself stays exact.
//!
//! [`PostOffice::handle`]: crate::office::PostOffice::handle

use radiopost::ErrorCode;
use serde_json::{Value as Json, json};

use crate::office::problem;
use crate::serve::{Running, now};

/// An answer: status, content type, body.
pub type Answer = (u16, &'static str, String);

fn ok(body: &Json) -> Answer {
    (200, "application/json", body.to_string())
}

fn refused(code: ErrorCode, status: u16) -> Answer {
    let p = problem(code, Some(status));
    (
        status,
        "application/problem+json",
        p.body.unwrap_or_default().to_string(),
    )
}

impl Running {
    /// RP-5 §7.1 for the desk's own endpoints: the operator's callsign, or the status to refuse
    /// with (404 when the post office has no operators, 401 for no token, 403 for another's).
    fn operator(&self, token: Option<&str>) -> Result<String, u16> {
        let office = self.office();
        if office.config.operators.is_empty() {
            return Err(404);
        }
        let Some(token) = token else {
            return Err(401);
        };
        if let Some(op) = office.operator_token(token) {
            return Ok(op);
        }
        let valid = office
            .state
            .tokens
            .get(token)
            .is_some_and(|h| h.expires > now());
        Err(if valid { 403 } else { 401 })
    }

    /// The desk's answer to a request under `/office`, or `None` for RP-5 §7's own.
    pub(crate) fn desk(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&Json>,
    ) -> Option<Answer> {
        let own = matches!(
            (method, path),
            ("GET", "/office/outgoing")
                | ("POST", "/office/name")
                | ("GET" | "PUT", "/office/settings")
        );
        if !own {
            return None;
        }
        let who = match self.operator(token) {
            Ok(who) => who,
            Err(404) => return Some(refused(ErrorCode::UnknownMessage, 404)),
            Err(status) => return Some(refused(ErrorCode::NotAuthorized, status)),
        };
        let empty = json!({});
        let body = body.unwrap_or(&empty);
        let text = |k: &str| body.get(k).and_then(Json::as_str).unwrap_or("").to_owned();
        Some(match (method, path) {
            ("GET", "/office/outgoing") => ok(&json!({"outgoing": self.office().state.outgoing})),
            ("POST", "/office/name") => {
                let (address, name) = (text("address"), text("name"));
                let mut office = self.office();
                if !office.state.accounts.contains(&address.to_lowercase()) || name.is_empty() {
                    return Some(refused(ErrorCode::Malformed, 400));
                }
                office.state.names.insert(address.clone(), name.clone());
                self.save(&office);
                eprintln!("radiopost-po: {who} named {address} {name:?}");
                ok(&json!({"address": address, "name": name}))
            }
            ("GET", "/office/settings") => ok(&self.desk_settings()),
            _ => {
                let take = match body.get("applications").and_then(Json::as_str) {
                    Some("open") => true,
                    Some("closed") => false,
                    _ => return Some(refused(ErrorCode::Malformed, 400)),
                };
                if let Err(e) = self.take_applications(take) {
                    eprintln!("radiopost-po: cannot save config.json: {e}");
                    return Some(refused(ErrorCode::Internal, 500));
                }
                eprintln!(
                    "radiopost-po: {who} {} applications",
                    if take { "opened" } else { "closed" }
                );
                ok(&self.desk_settings())
            }
        })
    }

    /// What `/office/settings` shows.
    pub(crate) fn desk_settings(&self) -> Json {
        let settings = self.settings();
        json!({
            "applications": if settings.info.applications.as_deref() == Some("open") {
                "open"
            } else {
                "closed"
            },
            "operators": settings.operators(),
            "domains": settings.domains,
            "contact": settings.info.contact,
            "base_url": settings.base_url,
        })
    }

    /// Opens or closes applications (RP-5 §4.1, §6.7) at once, and keeps it in `config.json`.
    ///
    /// # Errors
    ///
    /// `config.json` could not be written.
    pub fn take_applications(&self, take: bool) -> Result<(), String> {
        let mut settings = self.settings();
        settings.info.applications = take.then(|| "open".to_owned());
        settings.info.updated = now();
        self.files().write(
            "config.json",
            &serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?,
        )?;
        self.set_settings(settings);
        self.office().config.applications_open = take;
        Ok(())
    }

    /// The members `/office/status` gains from the service (RP-5 §7.2 allows them).
    pub(crate) fn status_extras(&self, status: &mut Json) {
        let settings = self.settings();
        let office = self.office();
        status["version"] = json!(env!("CARGO_PKG_VERSION"));
        status["update"] = json!(self.update_status());
        status["outgoing"] = json!(office.state.outgoing.len());
        status["applications_open"] = json!(settings.info.applications.as_deref() == Some("open"));
        status["operators"] = json!(settings.operators());
        status["domains"] = json!(settings.domains);
    }
}
