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
use crate::serve::{Running, Settings, now};

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
        let (route, query) = path.split_once('?').unwrap_or((path, ""));
        let own = matches!(
            (method, route),
            ("GET", "/office/outgoing")
                | (
                    "POST",
                    "/office/name"
                        | "/office/lookup"
                        | "/office/licences"
                        | "/office/mailboxes/check"
                )
                | ("GET" | "PUT", "/office/settings")
        ) || (method == "GET" && route.starts_with("/office/lookup/"))
            || matches!(
                (method, route),
                ("GET" | "POST", "/office/health")
                    | ("GET", "/office/jobs" | "/office/backups")
                    | ("POST", "/office/backup")
            )
            || (method == "POST" && route.starts_with("/office/jobs/"));
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
        if let Some(answer) = self.desk_care(method, route, &who, body) {
            return Some(answer);
        }
        let text = |k: &str| body.get(k).and_then(Json::as_str).unwrap_or("").to_owned();
        Some(match (method, route) {
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
            ("POST", "/office/lookup") => match self.look_up(&text("application")) {
                Some(entry) => ok(&entry),
                None => refused(ErrorCode::UnknownMessage, 404),
            },
            ("GET", _) if route.starts_with("/office/lookup/") => {
                let call = route.trim_start_matches("/office/lookup/").to_uppercase();
                let name = query
                    .split('&')
                    .find_map(|p| p.strip_prefix("name="))
                    .map(unescape)
                    .unwrap_or_default();
                ok(&self.evidence_for(&call, &name, None))
            }
            ("POST", "/office/licences") => {
                let started = self.fetch_fcc();
                eprintln!("radiopost-po: {who} asked for the FCC licence copy to be fetched");
                ok(&json!({"started": started, "fcc": self.fcc_status()}))
            }
            ("GET", "/office/settings") => ok(&self.desk_settings()),
            ("POST", "/office/mailboxes/check") => ok(&self.check_mailboxes()),
            _ => match self.put_settings(body) {
                Ok(said) => {
                    eprintln!("radiopost-po: {who} changed the settings: {said}");
                    ok(&self.desk_settings())
                }
                Err(400) => refused(ErrorCode::Malformed, 400),
                Err(_) => refused(ErrorCode::Internal, 500),
            },
        })
    }

    /// The operator's port's share of the desk, taken before it locks the post office:
    /// `GET`/`PUT /settings`, `GET /lookup?call=…&name=…`, `POST /licences` (fetch the copy).
    pub(crate) fn admin_desk(
        &self,
        method: &str,
        path: &str,
        body: &Json,
    ) -> Option<(u16, String)> {
        let (route, query) = path.split_once('?').unwrap_or((path, ""));
        let param = |k: &str| {
            query
                .split('&')
                .find_map(|p| p.strip_prefix(&format!("{k}=")))
                .map(unescape)
                .unwrap_or_default()
        };
        Some(match (method, route) {
            ("GET", "/settings") => (200, self.desk_settings().to_string()),
            ("PUT", "/settings") => match self.put_settings(body) {
                Ok(_) => (200, self.desk_settings().to_string()),
                Err(_) => (
                    400,
                    json!({"error": "a setting of the wrong kind"}).to_string(),
                ),
            },
            ("GET", "/lookup") => (
                200,
                self.evidence_for(&param("call").to_uppercase(), &param("name"), None)
                    .to_string(),
            ),
            ("POST", "/mailboxes/check") => (200, self.check_mailboxes().to_string()),
            ("POST", "/health") => (200, self.health_now().to_string()),
            ("POST", "/licences") => (
                200,
                json!({"started": self.fetch_fcc(), "fcc": self.fcc_status()}).to_string(),
            ),
            _ => return None,
        })
    }

    /// The desk's share of the server's care: health, jobs, backups.
    fn desk_care(&self, method: &str, route: &str, who: &str, body: &Json) -> Option<Answer> {
        let text = |k: &str| body.get(k).and_then(Json::as_str).unwrap_or("").to_owned();
        Some(match (method, route) {
            ("GET", "/office/health") => ok(&self.health().unwrap_or_else(|| self.health_now())),
            ("POST", "/office/health") => ok(&self.health_now()),
            ("GET", "/office/jobs") => ok(&self.jobs_status()),
            ("POST", _) if route.starts_with("/office/jobs/") => {
                let name = route.trim_start_matches("/office/jobs/");
                match self.ask_job(name, who) {
                    Ok(true) => ok(&self.jobs_status()),
                    Ok(false) => refused(ErrorCode::UnknownMessage, 404),
                    Err(e) => {
                        eprintln!("radiopost-po: the job {name}: {e}");
                        refused(ErrorCode::Internal, 500)
                    }
                }
            }
            ("GET", "/office/backups") => {
                let list: Vec<Json> = crate::backups::list(self.files().dir())
                    .into_iter()
                    .map(|(name, bytes, at)| json!({"name": name, "bytes": bytes, "at": at}))
                    .collect();
                ok(&json!({"backups": list}))
            }
            ("POST", "/office/backup") => {
                let made = crate::backups::archive(self.files().dir())
                    .and_then(|zip| crate::backups::seal(&zip, &text("passphrase")));
                match made {
                    Ok(sealed) => {
                        eprintln!("radiopost-po: {who} took a sealed backup");
                        let domain = self.settings().domains.first().cloned().unwrap_or_default();
                        let name = format!("{domain}-{}.rpbackup", crate::lookups::day(now()));
                        ok(&json!({"name": name, "bytes": sealed.len(),
                                   "data": radiopost::base64url::encode(&sealed)}))
                    }
                    Err(e) => (
                        400,
                        "application/problem+json",
                        json!({"type": "about:blank", "title": e, "status": 400, "code": 2,
                               "name": "malformed"})
                        .to_string(),
                    ),
                }
            }
            _ => return None,
        })
    }

    /// What `/office/settings` shows: never the QRZ password, only whether there is one.
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
            "fcc": self.fcc_status(),
            "qrz": {"username": settings.lookups.qrz_username,
                    "password": settings.lookups.qrz_password.is_some()},
            "mailboxes": settings.mailboxes.shown(),
            "dkim": settings.health.dkim,
            "smtp": settings.smtp.is_some(),
        })
    }

    /// Whether the way to make mailboxes works, without making one: Stalwart asked for the
    /// post office's domain, or the command found.
    pub(crate) fn check_mailboxes(&self) -> Json {
        let settings = self.settings();
        let m = &settings.mailboxes;
        let domain = settings.domains.first().cloned().unwrap_or_default();
        match m.kind.as_str() {
            "stalwart" => match crate::mailboxes::stalwart_domain(m, &domain) {
                Ok(id) => json!({"ok": true,
                                 "said": format!("Stalwart answers, and has {domain} (domain {id})")}),
                Err(e) => json!({"ok": false, "said": e}),
            },
            "command" => match m.command.first() {
                Some(p) if std::path::Path::new(p).is_file() || which(p) => {
                    json!({"ok": true, "said": format!("{p} is there; it runs at each approval")})
                }
                Some(p) => json!({"ok": false, "said": format!("{p} is not found")}),
                None => json!({"ok": false, "said": "no command set"}),
            },
            _ => json!({"ok": false, "said": "no way to make mailboxes is set"}),
        }
    }

    /// `PUT /office/settings`: any of `applications` (`open`, `closed`), `fcc` (true, false),
    /// `qrz_username` and `qrz_password` (empty clears). Returns what changed, for the log, or
    /// 400 for a member of the wrong kind, 500 when `config.json` could not be written.
    fn put_settings(&self, body: &Json) -> Result<String, u16> {
        let mut said = Vec::new();
        let applications = match body.get("applications") {
            None => None,
            Some(Json::String(s)) if s == "open" || s == "closed" => Some(s == "open"),
            Some(_) => return Err(400),
        };
        let fcc = match body.get("fcc") {
            None => None,
            Some(Json::Bool(b)) => Some(*b),
            Some(_) => return Err(400),
        };
        let string = |k: &str| -> Result<Option<String>, u16> {
            match body.get(k) {
                None => Ok(None),
                Some(Json::String(s)) => Ok(Some(s.trim().to_owned())),
                Some(_) => Err(400),
            }
        };
        let (user, pass) = (string("qrz_username")?, string("qrz_password")?);
        let dkim = match body.get("dkim") {
            None => None,
            Some(Json::Array(list)) => Some(
                list.iter()
                    .filter_map(Json::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
            ),
            Some(_) => return Err(400),
        };
        let mailboxes = match body.get("mailboxes") {
            None => None,
            Some(m) if m.is_object() => {
                let kind = m["kind"].as_str().unwrap_or("").to_owned();
                if !matches!(kind.as_str(), "" | "stalwart" | "command") {
                    return Err(400);
                }
                Some((kind, m.clone()))
            }
            Some(_) => return Err(400),
        };
        let mut fetch = false;
        self.change_settings(|s| {
            if let Some(open) = applications {
                s.info.applications = open.then(|| "open".to_owned());
                s.info.updated = now();
                said.push(format!(
                    "applications {}",
                    if open { "open" } else { "closed" }
                ));
            }
            if let Some(on) = fcc {
                fetch = on && !s.lookups.fcc;
                s.lookups.fcc = on;
                said.push(format!("FCC copy {}", if on { "on" } else { "off" }));
            }
            if let Some(u) = user {
                said.push("QRZ account".into());
                s.lookups.qrz_username = (!u.is_empty()).then_some(u);
            }
            if let Some(p) = pass {
                s.lookups.qrz_password = (!p.is_empty()).then_some(p);
            }
            if let Some(d) = dkim {
                said.push(format!("DKIM selectors {}", d.join(", ")));
                s.health.dkim = d;
            }
            if let Some((kind, m)) = mailboxes {
                let text = |k: &str| m[k].as_str().map(str::trim).map(str::to_owned);
                s.mailboxes.kind = kind;
                if let Some(url) = text("url") {
                    s.mailboxes.url = url;
                }
                if let Some(key) = text("api_key") {
                    s.mailboxes.api_key = (!key.is_empty()).then_some(key);
                }
                if let Some(login) = text("login") {
                    s.mailboxes.login = (!login.is_empty()).then_some(login);
                }
                if let Some(cmd) = text("command") {
                    s.mailboxes.command = cmd.split_whitespace().map(str::to_owned).collect();
                }
                if let Some(q) = m.get("quota_mb") {
                    s.mailboxes.quota_mb = q.as_u64();
                }
                let how = if s.mailboxes.kind.is_empty() {
                    "hand"
                } else {
                    s.mailboxes.kind.as_str()
                };
                said.push(format!("mailboxes by {how}"));
            }
        })
        .map_err(|e| -> u16 {
            eprintln!("radiopost-po: cannot save config.json: {e}");
            500
        })?;
        if fetch && self.fcc_stale() {
            self.fetch_fcc();
        }
        Ok(said.join(", "))
    }

    /// Changes the settings at once, and keeps them in `config.json`.
    ///
    /// # Errors
    ///
    /// `config.json` could not be written.
    pub fn change_settings(&self, change: impl FnOnce(&mut Settings)) -> Result<(), String> {
        let mut settings = self.settings();
        change(&mut settings);
        self.files().write(
            "config.json",
            &serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?,
        )?;
        self.office().config.applications_open =
            settings.info.applications.as_deref() == Some("open");
        self.set_settings(settings);
        Ok(())
    }

    /// Opens or closes applications (RP-5 §4.1, §6.7) at once, and keeps it in `config.json`.
    ///
    /// # Errors
    ///
    /// `config.json` could not be written.
    pub fn take_applications(&self, take: bool) -> Result<(), String> {
        self.change_settings(|s| {
            s.info.applications = take.then(|| "open".to_owned());
            s.info.updated = now();
        })
    }

    /// The jobs table for this post office and program.
    fn jobs_table(&self) -> Vec<crate::jobs::Job> {
        let exe = std::env::current_exe().unwrap_or_else(|_| "/usr/local/bin/radiopost-po".into());
        crate::jobs::table(self.files().dir(), &exe)
    }

    /// Where the jobs stand (`jobs`).
    #[must_use]
    pub fn jobs_status(&self) -> Json {
        crate::jobs::status(self.files().dir(), &self.jobs_table())
    }

    /// Asks the runner for `name`, for `who`.
    ///
    /// # Errors
    ///
    /// The request could not be written.
    pub fn ask_job(&self, name: &str, who: &str) -> Result<bool, String> {
        let asked = crate::jobs::request(self.files().dir(), &self.jobs_table(), name, who, now())?;
        if asked {
            eprintln!("radiopost-po: {who} asked for the job {name}");
        }
        Ok(asked)
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
        drop(office);
        if let Some(h) = self.health() {
            status["health"] = json!({"fails": h["fails"], "at": h["at"]});
        }
        if let Some((name, _, at)) = crate::backups::list(self.files().dir()).first() {
            status["backup"] = json!({"name": name, "at": at});
        }
    }
}

/// A query value with `+` and `%XX` undone.
fn unescape(v: &str) -> String {
    let b = v.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(x) => {
                        out.push(x);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether `program` is on the PATH.
fn which(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
}
