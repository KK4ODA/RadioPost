// SPDX-License-Identifier: MIT OR Apache-2.0
//! The post office as a service: its directory (configuration, key, state), the RP-5 API over
//! HTTP, and a port on the loopback interface for its operator's commands.
//!
//! The service speaks plain HTTP on the address it is given; HTTPS (RP-5 §1) comes from the web
//! server in front of it, which also serves the mail server. It answers the public documents
//! itself (RP-5 §4), passes everything else to [`PostOffice::handle`], and saves the state after
//! each request that may change it, so a restart loses nothing a client was told.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use ed25519_dalek::SigningKey;
use radiopost::base64url;
use radiopost::credential::{PostOfficeKey, Trust, flags};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use tiny_http::{Header, Method, Response as Http, Server};

use crate::auth::Checker;
use crate::documents::Info;
use crate::lookups::{LookupSettings, Lookups};
use crate::mailboxes::MailboxSettings;
use crate::office::{Config, Draw, PostOffice, Request, Response, State};

/// A post office's settings, as `config.json` holds them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    /// The domains it serves.
    pub domains: Vec<String>,
    /// Its API's base URL, as published in DNS (RP-1 §7.1).
    pub base_url: String,
    /// Where it listens for the API, behind the HTTPS front.
    pub listen: String,
    /// Where it listens for its operator's commands: the loopback interface only.
    pub admin: String,
    /// What the info document says.
    pub info: Info,
    /// The other post offices its operators accepted (RP-1 §7.5): each `domain`, with an `api`
    /// and `keys` (`public`, `not_before`, `not_after`) when they were typed rather than found.
    /// What is fetched of them is kept in the state, and serves before what was typed.
    #[serde(default)]
    pub known: Vec<Json>,
    /// Post offices its operators blocked (RP-1 §7.5): unknown, and not offered as seen.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked: Vec<String>,
    /// Where it asks DNS over HTTPS (Google's JSON form) for other post offices' records; none,
    /// `radiopost_api::RESOLVER`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolver: Option<String>,
    /// Where the mail server's MTA hooks reach the door (RP-6 §2.1); none, no door.
    #[serde(default)]
    pub hooks: Option<String>,
    /// The bearer token the mail server sends with each hook; needed off the loopback interface.
    #[serde(default)]
    pub hook_token: Option<String>,
    /// Where email leaves (RP-6 §3): the mail server's SMTP submission, as a URL
    /// (`smtp://user:password@127.0.0.1:587?tls=required`); none, mail waits in `outgoing`.
    #[serde(default)]
    pub smtp: Option<String>,
    /// More callsigns that operate it, besides the info document's `operator` (RP-5 §7.1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operators: Vec<String>,
    /// What the service looks callsigns up in for its operators: the FCC copy, QRZ.com.
    #[serde(default, skip_serializing_if = "LookupSettings::is_empty")]
    pub lookups: LookupSettings,
    /// How it makes an approved account's mailbox in the mail server.
    #[serde(default, skip_serializing_if = "MailboxSettings::is_empty")]
    pub mailboxes: MailboxSettings,
    /// What its hourly check asks besides the rest.
    #[serde(default, skip_serializing_if = "HealthSettings::is_empty")]
    pub health: HealthSettings,
    /// Whether the messages it makes for the radio are DEFLATE when that is fewer bytes (RP-5
    /// §5.3, [`crate::air_form`]); `false` makes every one uncompressed. On unless the file says
    /// `false`. A message already made keeps the air form it was made with.
    #[serde(default = "on", skip_serializing_if = "is_on")]
    pub compress: bool,
}

fn on() -> bool {
    true
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if takes &T"
)]
fn is_on(value: &bool) -> bool {
    *value
}

/// What the hourly check asks besides the rest (`health`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HealthSettings {
    /// The DKIM selectors to check, as Stalwart's zone file names them.
    pub dkim: Vec<String>,
}

impl HealthSettings {
    /// Nothing set: `config.json` leaves the member out.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.dkim.is_empty()
    }
}

impl Settings {
    /// Every callsign that operates the post office: the info document's first (RP-5 §7.1).
    #[must_use]
    pub fn operators(&self) -> Vec<String> {
        let mut all = vec![self.info.operator.to_uppercase()];
        for o in &self.operators {
            let o = o.to_uppercase();
            if !all.contains(&o) {
                all.push(o);
            }
        }
        all
    }
}

/// The files in a post office's directory.
pub struct Files {
    dir: PathBuf,
}

impl Files {
    /// The directory.
    #[must_use]
    pub fn new(dir: &Path) -> Files {
        Files {
            dir: dir.to_path_buf(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// The directory itself.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The settings.
    ///
    /// # Errors
    ///
    /// The file is missing or not settings.
    pub fn settings(&self) -> Result<Settings, String> {
        read_json(&self.path("config.json"))
    }

    /// The key that signs credentials and documents.
    ///
    /// # Errors
    ///
    /// The file is missing or holds no key.
    pub fn key(&self) -> Result<SigningKey, String> {
        let text = std::fs::read_to_string(self.path("key.hex"))
            .map_err(|e| format!("{}: {e}", self.path("key.hex").display()))?;
        let bytes: Vec<u8> = (0..text.trim().len())
            .step_by(2)
            .filter_map(|i| u8::from_str_radix(text.trim().get(i..i + 2)?, 16).ok())
            .collect();
        let seed: [u8; 32] = bytes
            .try_into()
            .map_err(|_| "key.hex holds no 32-byte key")?;
        Ok(SigningKey::from_bytes(&seed))
    }

    /// The state, or a new one when there is none yet.
    ///
    /// # Errors
    ///
    /// The file is there but is not a state.
    pub fn state(&self) -> Result<State, String> {
        if self.path("state.json").exists() {
            read_json(&self.path("state.json"))
        } else {
            Ok(State {
                next_serial: 1,
                ..State::default()
            })
        }
    }

    /// Writes a file whole, so a crash leaves the old one or the new one.
    ///
    /// # Errors
    ///
    /// It could not be written.
    pub fn write(&self, name: &str, text: &str) -> Result<(), String> {
        self.write_bytes(name, text.as_bytes())
    }

    /// Writes a file whole, as [`Files::write`], making its directory if need be.
    ///
    /// # Errors
    ///
    /// It could not be written.
    pub fn write_bytes(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let path = self.path(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let file = path
            .file_name()
            .map_or_else(String::new, |f| f.to_string_lossy().into_owned());
        let fresh = path.with_file_name(format!(".{file}.new"));
        std::fs::write(&fresh, bytes).map_err(|e| format!("{}: {e}", fresh.display()))?;
        std::fs::rename(&fresh, &path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The clock, Unix seconds.
#[must_use]
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Random bytes from the system, whatever they are for.
///
/// # Panics
///
/// When the system has no random numbers to give.
pub fn system_draw() -> Draw {
    Box::new(|_, n| {
        let mut b = vec![0u8; n];
        getrandom::fill(&mut b).expect("the system's random numbers");
        b
    })
}

/// The post offices a post office knows: itself and those its operators accepted and did not
/// block (RP-1 §7.5), with the keys typed for them. Those fetched are laid over these by
/// [`PostOffice::trust`](crate::office::PostOffice::trust).
#[must_use]
pub fn trust_of(settings: &Settings, key: &SigningKey) -> Trust {
    let mut trust = Trust::default();
    for domain in &settings.domains {
        trust
            .keys
            .entry(domain.clone())
            .or_default()
            .push(PostOfficeKey {
                public: key.verifying_key().to_bytes(),
                not_before: settings.info.not_before,
                not_after: settings.info.not_after,
            });
    }
    for k in &settings.known {
        let Some(domain) = k
            .get("domain")
            .and_then(Json::as_str)
            .map(str::to_lowercase)
        else {
            continue;
        };
        if settings.blocked.contains(&domain) || settings.domains.contains(&domain) {
            continue;
        }
        // known, even before anything is fetched: its keys come when they are
        trust.keys.entry(domain.clone()).or_default();
        for key in k.get("keys").and_then(Json::as_array).into_iter().flatten() {
            let public = key
                .get("public")
                .and_then(Json::as_str)
                .and_then(base64url::decode)
                .and_then(|p| <[u8; 32]>::try_from(p).ok());
            if let Some(public) = public {
                trust
                    .keys
                    .entry(domain.clone())
                    .or_default()
                    .push(PostOfficeKey {
                        public,
                        not_before: key.get("not_before").and_then(Json::as_u64).unwrap_or(0),
                        not_after: key
                            .get("not_after")
                            .and_then(Json::as_u64)
                            .unwrap_or(u64::MAX),
                    });
            }
        }
    }
    trust
}

/// A post office being served: it, its settings and where it saves.
pub struct Running {
    office: Mutex<PostOffice>,
    settings: Mutex<Settings>,
    files: Files,
    checker: OnceLock<Result<Checker, String>>,
    updates: OnceLock<Arc<radiopost_update::Watch>>,
    lookups: Lookups,
    /// The other post offices' worker and what waits for it (RP-1 §7.5).
    pub(crate) known: crate::known::Known,
    pub(crate) care_state: crate::health::Care,
}

impl Running {
    /// The lookups' queue and state (`lookups`).
    pub(crate) fn lookups(&self) -> &Lookups {
        &self.lookups
    }

    /// What the update channel offers, for the operator's `status` (the program asks it).
    pub fn watch_updates(&self, watch: Arc<radiopost_update::Watch>) {
        let _ = self.updates.set(watch);
    }

    /// The post office in `dir`, loaded.
    ///
    /// # Errors
    ///
    /// A file is missing or unreadable.
    pub fn load(dir: &Path, draw: Draw) -> Result<Running, String> {
        let files = Files::new(dir);
        let settings = files.settings()?;
        let key = files.key()?;
        let config = Config {
            domains: settings.domains.clone(),
            base_url: settings.base_url.clone(),
            trust: trust_of(&settings, &key),
            key,
            operators: settings.operators(),
            applications_open: settings.info.applications.as_deref() == Some("open"),
            compress: settings.compress,
        };
        let mut office = PostOffice::new(config, files.state()?, draw);
        // RP-6 §2.5: the door keeps held attachments as `held/<ref in hex>`
        let held_dir = dir.join("held");
        office.keep_held(
            Box::new(move |r| std::fs::read(held_dir.join(radiopost::cbor::hex(r))).ok()),
            settings.info.max_air_bytes,
        );
        Ok(Running {
            office: Mutex::new(office),
            settings: Mutex::new(settings),
            files,
            checker: OnceLock::new(),
            updates: OnceLock::new(),
            lookups: Lookups::default(),
            known: crate::known::Known::default(),
            care_state: crate::health::Care::default(),
        })
    }

    /// The door's sender checks (RP-6 §2.1), made ready at their first use: for a mail server
    /// that gives the door no verdicts.
    pub(crate) fn checker(&self) -> Result<&Checker, String> {
        self.checker
            .get_or_init(|| {
                let base = self.settings().base_url;
                let host = base
                    .split_once("://")
                    .map_or(base.as_str(), |(_, rest)| rest)
                    .split(['/', ':'])
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                Checker::new(&host)
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// The post office, locked.
    pub fn office(&self) -> MutexGuard<'_, PostOffice> {
        self.office
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Changes the post office in this process and saves its state: what a program that runs it
    /// beside other parts does (the public test service's codes, mail and stations' credentials).
    pub fn change<T>(&self, f: impl FnOnce(&mut PostOffice) -> T) -> T {
        let mut office = self.office();
        office.at(now());
        let t = f(&mut office);
        self.save(&office);
        t
    }

    /// Its settings, as they stand.
    pub(crate) fn settings(&self) -> Settings {
        self.settings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Its directory's files.
    pub(crate) fn files(&self) -> &Files {
        &self.files
    }

    pub(crate) fn save(&self, office: &PostOffice) {
        let text = serde_json::to_string_pretty(&office.state).unwrap_or_default();
        if let Err(e) = self.files.write("state.json", &text) {
            eprintln!("radiopost-po: cannot save the state: {e}");
        }
    }

    /// What the door judges with, for a backup mail server whose token this is (W38): `401`
    /// to any other.
    fn door_snapshot(&self, method: &str, token: Option<&str>) -> (u16, &'static str, String) {
        let hash = token.map(crate::backup_mx::token_hash);
        let mut office = self.office();
        office.at(now());
        let known = hash.is_some_and(|h| office.state.backup_mx.iter().any(|b| b.token_hash == h));
        if !known {
            return (401, "application/problem+json", problem_text(401));
        }
        if method != "GET" {
            return (405, "application/problem+json", problem_text(405));
        }
        let settings = self.settings();
        let snapshot = crate::backup_mx::Snapshot {
            issued: office.now(),
            domains: settings.domains.clone(),
            max_email_bytes: settings.info.max_email_bytes,
            accounts: office.door_accounts(),
            known_post_offices: office.known_post_offices(),
            sent: office.state.sent_ids.keys().cloned().collect(),
        };
        (
            200,
            "application/json",
            serde_json::to_string(&snapshot).unwrap_or_default(),
        )
    }

    /// Answers one API request: a public document, or [`PostOffice::handle`]'s answer.
    #[must_use]
    pub fn api(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: &[u8],
    ) -> (u16, &'static str, String) {
        let base = self.base_path();
        let Some(path) = path.strip_prefix(&base).filter(|p| p.starts_with('/')) else {
            return (404, "application/problem+json", problem_text(404));
        };
        let at = now();
        if method == "GET" {
            let settings = self
                .settings
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let mut info = settings.info.clone();
            if path == "/post-offices" {
                info.post_offices = self.published_post_offices(&settings);
            }
            if let Some(jws) = self.office().signed_document(&info, path, at) {
                return (200, "application/jose", jws);
            }
        }
        let body = if body.is_empty() {
            None
        } else if let Ok(j) = serde_json::from_slice(body) {
            Some(j)
        } else {
            let p = crate::office::problem(radiopost::ErrorCode::Malformed, None);
            return (
                400,
                "application/problem+json",
                p.body.unwrap_or_default().to_string(),
            );
        };
        if path == crate::backup_mx::SNAPSHOT_PATH {
            return self.door_snapshot(method, token);
        }
        if path.starts_with("/office/")
            && let Some(answer) = self.desk(method, path, token, body.as_ref())
        {
            return answer;
        }
        let request = Request {
            method: method.to_owned(),
            path: path.to_owned(),
            token: token.map(str::to_owned),
            body,
        };
        let waits = long_poll(method, path);
        let mut office = self.office();
        // RP-5 §7.5's code for an address that is no account yet begins one: its mailbox is made
        // once the code is (`crate::aftercare`), outside the lock and the replayed answer
        let begins = ((method, path) == ("POST", "/office/codes"))
            .then(|| request.body.as_ref().and_then(|b| b["address"].as_str()))
            .flatten()
            .filter(|a| !office.state.accounts.contains(&a.to_lowercase()))
            .map(str::to_owned);
        let mut answer = office.handle(at, &request);
        // RP-5 §5.8: a poll with nothing to give waits, up to `wait` seconds, for items to come
        let until = std::time::Instant::now() + std::time::Duration::from_secs(waits);
        while answer.status == 200
            && answer
                .body
                .as_ref()
                .and_then(|b| b["items"].as_array())
                .is_some_and(Vec::is_empty)
            && std::time::Instant::now() < until
        {
            drop(office);
            std::thread::sleep(std::time::Duration::from_millis(500));
            office = self.office();
            answer = office.handle(now(), &request);
        }
        self.save(&office);
        let met = office.take_met();
        drop(office);
        for domain in met {
            self.met(&domain, "credential"); // RP-1 §7.5: a credential it was shown
        }
        let kind = if answer.status >= 400 {
            "application/problem+json"
        } else {
            "application/json"
        };
        self.follow_up(&request, &mut answer, begins);
        (
            answer.status,
            kind,
            answer.body.map(|b| b.to_string()).unwrap_or_default(),
        )
    }

    /// What the service does once [`PostOffice::handle`] has answered, outside its lock and its
    /// replayed answer: lookups and aftercare (`crate::aftercare`), and the members the desk's
    /// answers gain from the service. `begins` is the address a code began an account for.
    fn follow_up(&self, request: &Request, answer: &mut Response, begins: Option<String>) {
        let (method, path) = (request.method.as_str(), request.path.as_str());
        let named = |k: &str| answer.body.as_ref().and_then(|b| b[k].as_str());
        match (method, answer.status) {
            ("POST", 202) if path == "/applications" => {
                if let Some(id) = named("application") {
                    self.queue_lookup(id, true);
                }
            }
            ("POST", 200) if path.starts_with("/office/applications/") => {
                if let Some(id) = named("application") {
                    self.after_decision(id);
                }
            }
            ("POST", 200) if path == "/enroll" => {
                if let Some(address) = request.body.as_ref().and_then(|b| b["address"].as_str()) {
                    self.after_enrollment(address);
                }
            }
            // RP-5 §6.4, §7.7: a registration withdrawn is logged, by whom it was
            ("DELETE", 200)
                if path.starts_with("/gateways/") || path.starts_with("/office/gateways/") =>
            {
                let by = if path.starts_with("/office/") {
                    "an operator, on the desk"
                } else {
                    "its operator"
                };
                eprintln!(
                    "radiopost-po: gateway {}'s registration withdrawn by {by}, revoking {}",
                    named("callsign").unwrap_or("?"),
                    answer
                        .body
                        .as_ref()
                        .map_or(Json::Null, |b| b["revoked"].clone())
                );
            }
            _ => {}
        }
        let Some(body) = answer.body.as_mut().filter(|_| answer.status == 200) else {
            return;
        };
        if path == "/office/status" {
            self.status_extras(body);
        }
        if let Some(address) = begins {
            body["mailbox"] = self.mailbox_for(&address).unwrap_or(Json::Null);
        }
        if (method, path) == ("GET", "/office/accounts") {
            self.mark_mailboxes(body);
        }
    }

    /// Replaces the settings the service runs with (the caller has written `config.json`).
    pub fn set_settings(&self, settings: Settings) {
        *self
            .settings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = settings;
    }

    /// What the update channel says, for the operator's status.
    pub(crate) fn update_status(&self) -> Option<Json> {
        self.updates
            .get()
            .and_then(|w| serde_json::to_value(w.status()).ok())
    }

    fn base_path(&self) -> String {
        let settings = self
            .settings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let url = settings.base_url.as_str();
        let rest = url.split_once("://").map_or(url, |(_, r)| r);
        rest.find('/').map_or(String::new(), |i| {
            rest[i..].trim_end_matches('/').to_owned()
        })
    }

    /// Answers one of the operator's commands (`radiopost-po code`, `deliver`, `revoke`,
    /// `gateways`, `unregister`, `outgoing`, `status`).
    #[must_use]
    #[allow(
        clippy::too_many_lines,
        reason = "the table of the operator's commands, one each"
    )]
    pub fn admin(&self, method: &str, path: &str, body: &[u8]) -> (u16, String) {
        let body: Json = serde_json::from_slice(body).unwrap_or(Json::Null);
        let text = |k: &str| body.get(k).and_then(Json::as_str).unwrap_or("").to_owned();
        if (method, path) == ("POST", "/applications") {
            let take = text("applications") == "open";
            return match self.take_applications(take) {
                Ok(()) => (200, self.desk_settings().to_string()),
                Err(e) => (400, json!({"error": e}).to_string()),
            };
        }
        if let Some(answer) = self.admin_desk(method, path, &body) {
            return answer;
        }
        let mut decided = Vec::new();
        let mut begins = None;
        let mut office = self.office();
        office.at(now());
        let mut answer = match (method, path) {
            ("POST", "/code") => {
                // the account in lowercase (RP-0 §6.2), however the operator typed it
                let address = radiopost::address::lowercase(&text("address"));
                if !office.state.accounts.contains(&address) {
                    begins = Some(address.clone()); // its mailbox is made once the lock is let go
                }
                let f = flags_named(&body);
                let code = office.new_code();
                office.add_code(&code, &address, &text("operator"), f);
                if !text("name").is_empty() {
                    office.state.names.insert(address.clone(), text("name"));
                }
                let mut made = json!({"code": code, "address": address,
                                      "operator": text("operator"), "flags": f});
                if office.operates(&text("operator")) {
                    made["operates"] = json!(true); // RP-5 §7.5: whoever enrolls is an operator
                }
                Ok(made)
            }
            ("POST", "/deliver") => {
                let air = base64url::decode(&text("air")).unwrap_or_default();
                let to = radiopost::address::lowercase(&text("to"));
                let serial = office.next_mailbox_serial(&to);
                let priority = body.get("priority").and_then(Json::as_u64).unwrap_or(0);
                let third = body
                    .get("third_party")
                    .and_then(Json::as_bool)
                    .unwrap_or(true);
                office.deliver(
                    &to,
                    serial,
                    &air,
                    &text("from"),
                    &text("subject"),
                    priority,
                    third,
                );
                Ok(json!({"serial": serial}))
            }
            ("POST", "/revoke") => {
                let device =
                    base64url::decode(&text("device")).and_then(|d| <[u8; 32]>::try_from(d).ok());
                match device {
                    Some(d) => Ok(json!({"revoked": office.revoke_device(&d)})),
                    None => Err("a device key, base64url".to_owned()),
                }
            }
            (_, p) if p.starts_with("/gateways") => admin_gateways(&mut office, method, path),
            (_, "/routes") => admin_routes(&mut office, method, &body),
            (_, "/backup-mx") => admin_backup_mx(&mut office, method, &body),
            ("POST", "/name") => {
                let address = radiopost::address::lowercase(&text("address"));
                office.state.names.insert(address.clone(), text("name"));
                Ok(json!({"address": address, "name": text("name")}))
            }
            ("GET", "/outgoing") => Ok(json!({"outgoing": office.state.outgoing})),
            ("GET", "/applications") | ("POST", "/decide") => {
                admin_applications(&mut office, path, &body, &mut decided)
            }
            ("GET", "/status") => Ok(json!({
                "version": env!("CARGO_PKG_VERSION"),
                "update": self.updates.get().map(|w| w.status()),
                "accounts": office.state.accounts.len(),
                "gateways": office.state.gateways.keys().collect::<Vec<_>>(),
                "codes": office.state.codes.len(),
                "outgoing": office.state.outgoing.len(),
                "sessions_open": office.state.sessions.len(),
                "next_serial": office.state.next_serial,
            })),
            _ => Err(format!("no command {method} {path}")),
        };
        self.save(&office);
        drop(office);
        for id in decided {
            self.after_decision(&id);
        }
        if let (Some(address), Ok(made)) = (begins, answer.as_mut()) {
            made["mailbox"] = self.mailbox_for(&address).unwrap_or(Json::Null);
        }
        match answer {
            Ok(j) => (200, j.to_string()),
            Err(e) => (400, json!({"error": e}).to_string()),
        }
    }
}

/// The credential flags a request names, by name (RP-1 §4.2).
fn flags_named(body: &Json) -> u64 {
    body.get("flags")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .fold(0, |f, name| {
            f | match name.as_str() {
                Some("send") => flags::SEND,
                Some("receive") => flags::RECEIVE,
                Some("role") => flags::ROLE,
                _ => 0,
            }
        })
}

/// The operator's port's gateways: `GET /gateways` lists those registered, for `radiopost-po
/// gateways`; `DELETE /gateways/{callsign}` withdraws one (RP-5 §7.7), for `radiopost-po
/// unregister`.
fn admin_gateways(office: &mut PostOffice, method: &str, path: &str) -> Result<Json, String> {
    match (method, path.strip_prefix("/gateways/")) {
        ("GET", None) => Ok(json!({"gateways": office.registrations()})),
        ("DELETE", Some(callsign)) => match office.withdraw_gateway(callsign) {
            Some((callsign, revoked)) => {
                eprintln!(
                    "radiopost-po: the operator's port withdrew gateway {callsign}'s                      registration, revoking {revoked:?}"
                );
                Ok(json!({"callsign": callsign, "revoked": revoked}))
            }
            None => Err(format!("no gateway {callsign} is registered here")),
        },
        _ => Err(format!("no command {method} {path}")),
    }
}

/// The operator's port's radio routes (RP-7 §6.1), for `radiopost-po route` and `routes`: a route
/// is set whole by its `via`, and one with no `users_at` is removed.
fn admin_routes(office: &mut PostOffice, method: &str, body: &Json) -> Result<Json, String> {
    if method == "POST" {
        let via = body["via"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_ascii_uppercase();
        if via.is_empty() {
            return Err("a route names its via gateway".to_owned());
        }
        office.state.routes.retain(|r| r.via != via);
        let users_at: Vec<String> = body["users_at"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
            .map(|c| c.trim().to_ascii_uppercase())
            .filter(|c| !c.is_empty() && *c != via)
            .collect();
        if !users_at.is_empty() {
            office
                .state
                .routes
                .push(crate::office::RadioRoute { users_at, via });
        }
    }
    Ok(json!({"routes": office.state.routes}))
}

/// The backup mail servers (wishlist W38), for `radiopost-po backup-mx`: `add` one by name (its
/// token, shown once), `remove` one, or list them. A name added again gets a new token.
fn admin_backup_mx(office: &mut PostOffice, method: &str, body: &Json) -> Result<Json, String> {
    let mut token = None;
    if method == "POST" {
        if let Some(name) = body["add"].as_str().map(str::trim) {
            if name.is_empty() {
                return Err("a backup mail server needs a name".to_owned());
            }
            let random = office.fresh("backup mx token", 32);
            let (record, t) = crate::backup_mx::make(name, &random, office.now());
            office.state.backup_mx.retain(|b| b.name != name);
            office.state.backup_mx.push(record);
            token = Some(t);
        } else if let Some(name) = body["remove"].as_str() {
            let before = office.state.backup_mx.len();
            office.state.backup_mx.retain(|b| b.name != name.trim());
            if office.state.backup_mx.len() == before {
                return Err(format!("no backup mail server called {name}"));
            }
        }
    }
    let list: Vec<Json> = office
        .state
        .backup_mx
        .iter()
        .map(|b| json!({"name": b.name, "added": b.added}))
        .collect();
    Ok(json!({"backup_mx": list, "token": token}))
}

/// The operator's port's view of applications (RP-5 §7.3), for `radiopost-po applications`,
/// `approve`, `decline` and `postcard`.
fn admin_applications(
    office: &mut PostOffice,
    path: &str,
    body: &Json,
    decided: &mut Vec<String>,
) -> Result<Json, String> {
    if path == "/applications" {
        let r = office.office("GET", &["applications"], &Json::Null, None);
        return Ok(r.body.unwrap_or_default());
    }
    let id = body.get("application").and_then(Json::as_str).unwrap_or("");
    let r = office.office("POST", &["applications", id], body, None);
    let b = r.body.unwrap_or_default();
    if r.status == 200 {
        decided.push(id.to_owned());
        Ok(b)
    } else if let Some(call) = b["operates"].as_str() {
        Err(format!(
            "{call} operates this post office: an application naming it can only be declined"
        ))
    } else {
        Err(b["name"].as_str().unwrap_or("refused").to_owned())
    }
}

/// How long a `GET /copies` may wait for items: its `wait`, at most 60 seconds (RP-5 §5.8).
fn long_poll(method: &str, path: &str) -> u64 {
    let Some(query) = path.strip_prefix("/copies?").filter(|_| method == "GET") else {
        return 0;
    };
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix("wait="))
        .and_then(|w| w.parse::<u64>().ok())
        .unwrap_or(0)
        .min(60)
}

fn problem_text(status: u16) -> String {
    json!({"type": "about:blank", "title": "Not found", "status": status,
           "code": 15, "name": "unknown-message"})
    .to_string()
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("a valid header")
}

fn method_of(m: &Method) -> &'static str {
    match m {
        Method::Get => "GET",
        Method::Post => "POST",
        Method::Put => "PUT",
        Method::Delete => "DELETE",
        _ => "OTHER",
    }
}

/// Serves the API on `listen` and the operator's commands on `admin`, until the process ends.
/// Returns the addresses taken.
///
/// # Errors
///
/// An address could not be taken, or the admin address is not on the loopback interface.
pub fn serve(running: Arc<Running>, listen: &str, admin: &str) -> Result<(String, String), String> {
    let api = std::net::TcpListener::bind(listen).map_err(|e| format!("{listen}: {e}"))?;
    serve_on(running, api, admin)
}

/// [`serve`], with the API on a port already taken. A caller that must name the port before the
/// post office runs (its base URL, a proxy in front of it) binds it first and hands it over, so
/// nothing else can take the port in between, as a test running alongside did when the port was
/// let go and bound again.
///
/// # Errors
///
/// The admin address could not be taken or is not on the loopback interface, or `api` has no
/// address.
pub fn serve_on(
    running: Arc<Running>,
    api: std::net::TcpListener,
    admin: &str,
) -> Result<(String, String), String> {
    crate::lookups::start(&running);
    crate::health::start(&running);
    crate::known::start(&running);
    crate::seen::start(&running);
    let api = Server::from_listener(api, None).map_err(|e| format!("the API's port: {e}"))?;
    let ops = Server::http(admin).map_err(|e| format!("{admin}: {e}"))?;
    let api_at = api.server_addr().to_string();
    let ops_at = ops.server_addr().to_string();
    if !ops
        .server_addr()
        .to_ip()
        .is_some_and(|a| a.ip().is_loopback())
    {
        return Err(format!(
            "the operator's port {admin} must be on the loopback interface"
        ));
    }
    let r = running.clone();
    std::thread::spawn(move || {
        // each request on a thread of its own, so a slow one (a lookup in QRZ, the health check
        // asking this very API for its info document) holds up no other
        for mut request in api.incoming_requests() {
            let r = r.clone();
            std::thread::spawn(move || {
                let mut body = Vec::new();
                let _ = request.as_reader().take(32 << 20).read_to_end(&mut body);
                let token = request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Authorization"))
                    .and_then(|h| h.value.as_str().strip_prefix("Bearer ").map(str::to_owned));
                let (status, kind, text) = r.api(
                    method_of(request.method()),
                    request.url(),
                    token.as_deref(),
                    &body,
                );
                let mut answer = Http::from_string(text).with_status_code(status);
                if status != 204 {
                    answer = answer.with_header(header("Content-Type", kind));
                }
                let _ = request.respond(answer);
            });
        }
    });
    std::thread::spawn(move || {
        for mut request in ops.incoming_requests() {
            let mut body = Vec::new();
            let _ = request.as_reader().take(32 << 20).read_to_end(&mut body);
            let (status, text) = running.admin(method_of(request.method()), request.url(), &body);
            let answer = Http::from_string(text)
                .with_status_code(status)
                .with_header(header("Content-Type", "application/json"));
            let _ = request.respond(answer);
        }
    });
    Ok((api_at, ops_at))
}

/// Serves the door to the mail server's MTA hooks on `addr` (RP-6 §2.1). Returns the address
/// taken.
///
/// # Errors
///
/// The address could not be taken, or it is off the loopback interface with no `hook_token`.
pub fn serve_hooks(running: Arc<Running>, addr: &str) -> Result<String, String> {
    let token = running.settings().hook_token;
    let server = Server::http(addr).map_err(|e| format!("{addr}: {e}"))?;
    let at = server.server_addr().to_string();
    let loopback = server
        .server_addr()
        .to_ip()
        .is_some_and(|a| a.ip().is_loopback());
    if !loopback && token.is_none() {
        return Err(format!(
            "the door's port {addr} is off the loopback interface: set `hook_token`"
        ));
    }
    std::thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let given = request
                .headers()
                .iter()
                .find(|h| h.field.equiv("Authorization"))
                .and_then(|h| h.value.as_str().strip_prefix("Bearer ").map(str::to_owned));
            if token.is_some() && given != token {
                let _ = request.respond(Http::from_string("").with_status_code(401));
                continue;
            }
            let mut body = Vec::new();
            let _ = request.as_reader().take(64 << 20).read_to_end(&mut body);
            let (status, text) = running.hook(&body);
            let mut answer = Http::from_string(text).with_status_code(status);
            if status != 204 {
                answer = answer.with_header(header("Content-Type", "application/json"));
            }
            let _ = request.respond(answer);
        }
    });
    Ok(at)
}

/// Sends the mail waiting to go on by email through the mail server at `url` (RP-6 §3), every
/// 20 seconds, until the process ends; says what happened with `say`.
///
/// # Errors
///
/// The URL is not an SMTP URL.
pub fn send_mail(
    running: Arc<Running>,
    url: &str,
    say: impl Fn(&str) + Send + 'static,
) -> Result<(), String> {
    use lettre::Transport as _;
    let transport = lettre::SmtpTransport::from_url(url)
        .map_err(|e| format!("{url}: {e}"))?
        .build();
    std::thread::spawn(move || {
        loop {
            let send = |from: &str, to: &[String], email: &[u8]| -> Result<(), (bool, String)> {
                let parse = |a: &str| {
                    a.parse::<lettre::Address>()
                        .map_err(|e| (true, e.to_string()))
                };
                let to: Vec<lettre::Address> =
                    to.iter().map(|a| parse(a)).collect::<Result<_, _>>()?;
                let envelope = lettre::address::Envelope::new(Some(parse(from)?), to)
                    .map_err(|e| (true, e.to_string()))?;
                transport
                    .send_raw(&envelope, email)
                    .map(|_| ())
                    .map_err(|e| (e.is_permanent(), e.to_string()))
            };
            for line in running.send_waiting(&send) {
                say(&line);
            }
            std::thread::sleep(std::time::Duration::from_secs(20));
        }
    });
    Ok(())
}
