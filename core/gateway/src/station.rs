// SPDX-License-Identifier: MIT OR Apache-2.0
//! The gateway on the air: its directory (settings, key, credential, what it keeps), its modem
//! (any RPMI adapter, or Aether's control API through the Aether adapter in this process), and
//! the loop that answers each call and carries its session to the user's post office.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use ed25519_dalek::SigningKey;
use radiopost::ErrorCode;
use radiopost::base64url;
use radiopost::crypto::ContentId;
use radiopost::session::client::{Client, ClientConfig, Forwarding};
use radiopost::session::gateway::{Gateway, GatewayConfig, Note, Partners, Upstream, Upstreams};
use radiopost::session::{Caps, Event as SessionEvent, WaitingEntry};
use rpmi::host::{Carrier, Direction, Engine, Event, Host, Silence};
use rpmi::tcp::Connection;
use serde::{Deserialize, Serialize};

use crate::forward::{Outcome, Partner};
use crate::monitor::{Check, Monitor};
use crate::offline::{Fallback, Offline};
use crate::store::Shared;
use crate::upstream::{self, Account};

/// RP-3 §8: nothing heard for this long ends the session.
pub const QUIET: Duration = Duration::from_secs(300);
/// Pieces of a broken upload are kept at least 24 hours (RP-3 §7); these are kept two days.
pub const UPLOAD_KEPT_S: u64 = 2 * 86_400;

/// A gateway's settings, as `gateway.json` holds them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    /// The callsign it answers to (RP-4 §1.1: on a shared modem, its own claim).
    pub callsign: String,
    /// Its modem: an RPMI adapter's `host:port`, or Aether's control API `ws://…`.
    pub modem: String,
    /// More modems it answers on, given as `modem` is (wishlist W26): Aether and Mercury on one
    /// radio, or HF and VHF packet on two. It carries one session at a time: while it carries one
    /// on a modem, the others do not answer. Calls it places (RP-7 §2) go on `modem` alone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub more_modems: Vec<String>,
    /// The country it operates in, for third-party mail (RP-6 §5.1).
    #[serde(default = "us")]
    pub country: String,
    /// Post offices' API base URLs by domain, when DNS is not to be asked.
    #[serde(default)]
    pub post_offices: BTreeMap<String, String>,
    /// Where DNS is asked for `_radiopost.<domain>` (RP-1 §7.1): a DNS-over-HTTPS JSON resolver.
    #[serde(default = "resolver")]
    pub resolver: String,
    /// Post offices its operator will not carry sessions to (RP-1 §7.3).
    #[serde(default)]
    pub blocked: Vec<String>,
    /// When set, the only post offices it carries sessions to (RP-1 §7.3).
    #[serde(default)]
    pub only: Option<Vec<String>>,
    /// The modem does not check the band rules, and the operator does (RP-4 §8).
    #[serde(default)]
    pub operator_checks_rules: bool,
    /// The gateway app answers calls as soon as it starts (the command line's `serve` always does).
    #[serde(default = "yes")]
    pub answer_at_start: bool,
    /// Whether it holds copies placed ahead, and its limits (RP-5 §4.4 `copies`, §5.8); none, it
    /// holds none and polls no post office.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copies: Option<CopiesSettings>,
    /// Its partners (RP-7 §1): the gateways it forwards its queue to when it is cut off, and the
    /// only gateways whose forwarding sessions it answers (RP-7 §2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub partners: Vec<PartnerSettings>,
    /// Its windows (RP-7 §5): when it listens for its partners' calls, published in its record.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<WindowSettings>,
    /// The channels its first modem scans (W50, RP-4 §4.2), dials in hertz, the first the home:
    /// *Scan these channels*. Empty, the radio stays where it is. Its channels in the record then
    /// carry the `scan_s` the modem answers (RP-5 §4.4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scan: Vec<u64>,
}

/// A window, as the operator sets it (RP-7 §5): a channel, and when it is open.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowSettings {
    /// The dial frequency, hertz.
    pub dial_hz: u64,
    /// The radio's mode: `USB`, or `FM`.
    pub mode: String,
    /// The modem, by its registry name (RP-0 §9.4).
    pub modem: String,
    /// The modem's bandwidth, or an FM channel's width, hertz.
    pub bandwidth_hz: u64,
    /// How often it opens, seconds.
    pub every_s: u64,
    /// When in each period it opens, seconds.
    pub at_s: u64,
    /// How long it stays open, seconds.
    pub length_s: u64,
}

impl WindowSettings {
    /// Its signal's lowest and highest radio frequency (RP-7 §5), when known.
    #[must_use]
    pub fn occupied(&self) -> Option<(u64, u64)> {
        radiopost::forwarding::occupied(self.dial_hz, &self.mode, &self.modem, self.bandwidth_hz)
    }

    /// Why it may not be published from `country` (RP-7 §5, §5.1), or `None` when it may.
    #[must_use]
    pub fn refused(&self, country: &str) -> Option<String> {
        if self.every_s == 0 || self.length_s == 0 || self.length_s > self.every_s {
            return Some("a window opens every so often, for a time shorter than that".into());
        }
        let Some(occupied) = self.occupied() else {
            return Some(format!(
                "the band a {} signal at {} Hz occupies on {} is not known here",
                self.modem, self.bandwidth_hz, self.mode
            ));
        };
        if !radiopost::forwarding::window_allowed(occupied, country) {
            return Some(format!(
                "its signal, {} to {} Hz, is not inside one segment where {country}'s rules let \
                 an unattended station start a data transmission (RP-7 §5.1)",
                occupied.0, occupied.1
            ));
        }
        None
    }

    /// Its entry in the record's `forwarding` (RP-7 §5).
    #[must_use]
    pub fn record(&self) -> serde_json::Value {
        let (low, high) = self.occupied().unwrap_or_default();
        serde_json::json!({
            "channel": {"dial_hz": self.dial_hz, "mode": self.mode, "modem": self.modem,
                        "bandwidth_hz": self.bandwidth_hz, "occupied_hz": [low, high]},
            "every_s": self.every_s, "at_s": self.at_s, "length_s": self.length_s,
        })
    }

    /// The window's times.
    #[must_use]
    pub fn window(&self) -> radiopost::forwarding::Window {
        radiopost::forwarding::Window {
            every_s: self.every_s,
            at_s: self.at_s,
            length_s: self.length_s,
        }
    }
}

/// A partner, as the operator names it (RP-7 §1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartnerSettings {
    /// Its callsign, as its credential names it.
    pub callsign: String,
}

impl Settings {
    /// Whether it polls the post offices for items placed at it (RP-5 §5.8): when its record
    /// holds copies, or lists windows.
    #[must_use]
    pub fn polls(&self) -> bool {
        self.copies.is_some() || !self.windows.is_empty()
    }

    /// Whether `callsign` is one of its partners.
    #[must_use]
    pub fn is_partner(&self, callsign: &str) -> bool {
        self.partners
            .iter()
            .any(|p| p.callsign.eq_ignore_ascii_case(callsign))
    }
}

/// What a gateway that holds copies placed ahead says in its record (RP-5 §4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopiesSettings {
    /// How many users' copies it holds at most.
    pub users: u64,
    /// How many days a copy is kept.
    pub days: u64,
    /// The largest air form it holds; a larger message is listed with `held`.
    pub max_air_bytes: u64,
}

impl CopiesSettings {
    /// The record's `copies` member (RP-5 §4.4).
    #[must_use]
    pub fn record(&self) -> serde_json::Value {
        serde_json::json!({"users": self.users, "days": self.days,
                           "max_air_bytes": self.max_air_bytes})
    }
}

impl Settings {
    /// `gateway.json` in `dir`.
    ///
    /// # Errors
    ///
    /// It is missing or not settings.
    pub fn load(dir: &Path) -> Result<Settings, String> {
        serde_json::from_str(&read(&dir.join("gateway.json"))?)
            .map_err(|e| format!("gateway.json: {e}"))
    }

    /// Writes `gateway.json` in `dir`.
    ///
    /// # Errors
    ///
    /// It could not be written.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        write(
            dir,
            "gateway.json",
            &serde_json::to_string_pretty(self).map_err(|e| e.to_string())?,
        )
    }
}

/// Keeps a credential a post office issued for the gateway's key (raw bytes, or hexadecimal
/// text), as `credential.hex`, and returns its body.
///
/// # Errors
///
/// It is not a credential, it is for another key, or it cannot be written.
pub fn keep_credential(dir: &Path, bytes: &[u8]) -> Result<radiopost::credential::Body, String> {
    let bytes = std::str::from_utf8(bytes)
        .ok()
        .and_then(unhex)
        .unwrap_or_else(|| bytes.to_vec());
    let body = radiopost::credential::Credential::decode(&bytes)
        .ok_or("that is not a credential (RP-1 §4)")?
        .body;
    if body.device != key(dir)?.verifying_key().to_bytes() {
        return Err("that credential is for another key than the gateway's".into());
    }
    write(dir, "credential.hex", &radiopost::cbor::hex(&bytes))?;
    Ok(body)
}

/// RP-5 §6.2: in its credential's last month ([`radiopost::credential::RENEW_WITHIN`], or `within`
/// seconds), the gateway asks its own account's post office for a new one, as any account may,
/// and keeps it; the answering loop takes it up before its next session. `None`: not due.
///
/// # Errors
///
/// Not registered, the post office not found or not reached, or it refused.
pub fn renew_if_due(
    dir: &Path,
    now: u64,
    within: u64,
) -> Result<Option<radiopost::credential::Body>, String> {
    let station = Station::load(dir)?;
    let body = station
        .credential()
        .ok_or("credential.hex is not a credential")?;
    if !radiopost::credential::renewal_due(body.expires, now, within) {
        return Ok(None);
    }
    let domain = body
        .address
        .rsplit_once('@')
        .map(|(_, d)| d.to_lowercase())
        .ok_or("the credential names no account")?;
    let url = station
        .post_office(&domain)
        .map_err(|code| format!("{domain}'s post office: {}", code.name()))?;
    let mut http = upstream::Http::new(&url, station.account.clone(), Arc::new(Mutex::new(None)));
    let issued = match http.call("POST", "/credentials", Some(&serde_json::json!({}))) {
        Ok((200, Some(answer))) => answer
            .get("credential")
            .and_then(serde_json::Value::as_str)
            .and_then(radiopost::base64url::decode)
            .ok_or("the post office's answer held no credential")?,
        Ok((status, answer)) => {
            return Err(format!(
                "{domain}'s post office refused ({status} {})",
                radiopost_api::code_of(answer.as_ref()).name()
            ));
        }
        Err(code) => return Err(format!("{domain}'s post office: {}", code.name())),
    };
    keep_credential(dir, &issued).map(Some)
}

/// How often the answering loop asks whether the gateway's credential is due ([`Renewal`]).
pub const RENEW_EVERY: Duration = Duration::from_secs(600);

/// What a try at renewing comes to, from its thread.
type Renewed = Result<Option<radiopost::credential::Body>, String>;

/// Renewing the gateway's own credential while it answers (RP-5 §6.2): a try every `every`, on a
/// thread of its own, so a post office slow to answer holds up no call; the answering loop says
/// what came of it, and a failure once until it changes. The app and a gateway with no screen
/// (`radiopost-gateway serve`) both renew this way.
pub struct Renewal {
    dir: PathBuf,
    every: Duration,
    within: u64,
    last: Option<Instant>,
    pending: Option<std::sync::mpsc::Receiver<Renewed>>,
    said: Option<String>,
}

impl Renewal {
    /// Renewing the gateway in `dir`, a try every `every` once it has `within` seconds or less to
    /// run.
    #[must_use]
    pub fn new(dir: &Path, every: Duration, within: u64) -> Renewal {
        Renewal {
            dir: dir.to_owned(),
            every,
            within,
            last: None,
            pending: None,
            said: None,
        }
    }

    /// Says what a finished try came to, and starts the next when it is time and none is under
    /// way.
    pub fn tick(&mut self, tell: &mut dyn FnMut(&str)) {
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(result) => {
                    self.pending = None;
                    self.say(result, tell);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.pending = None,
            }
        }
        if self.last.is_some_and(|t| t.elapsed() < self.every) {
            return;
        }
        self.last = Some(Instant::now());
        let (tx, rx) = std::sync::mpsc::channel();
        let (dir, within) = (self.dir.clone(), self.within);
        std::thread::spawn(move || {
            let _ = tx.send(renew_if_due(&dir, now(), within));
        });
        self.pending = Some(rx);
    }

    fn say(&mut self, result: Renewed, tell: &mut dyn FnMut(&str)) {
        match result {
            Ok(None) => {}
            Ok(Some(body)) => {
                self.said = None;
                tell(&format!(
                    "renewed the gateway's credential: serial {}, good for {} days",
                    body.serial,
                    body.expires.saturating_sub(now()) / 86_400
                ));
            }
            Err(why) if self.said.as_deref() == Some(why.as_str()) => {}
            Err(why) => {
                tell(&format!(
                    "could not renew the gateway's credential: {why}; trying again later"
                ));
                self.said = Some(why);
            }
        }
    }
}

/// Makes a gateway in `dir`: its key and its settings.
///
/// # Errors
///
/// The directory already holds a gateway, or cannot be written.
pub fn make(dir: &Path, settings: &Settings) -> Result<(), String> {
    if dir.join("gateway.json").exists() {
        return Err("this directory already holds a gateway".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let seed: [u8; 32] = random();
    write(dir, "key.hex", &radiopost::cbor::hex(&seed))?;
    settings.save(dir)
}

/// The gateway's own key, `key.hex` in `dir`.
///
/// # Errors
///
/// It is missing or not a key.
pub fn key(dir: &Path) -> Result<SigningKey, String> {
    let seed: [u8; 32] = unhex(&read(&dir.join("key.hex"))?)
        .and_then(|b| b.try_into().ok())
        .ok_or("key.hex holds no 32-byte key")?;
    Ok(SigningKey::from_bytes(&seed))
}

fn us() -> String {
    "US".into()
}

fn yes() -> bool {
    true
}

/// The gateway's directory when none is named: `$RADIOPOST_GATEWAY`, else `.radiopost-gateway` in
/// the user's home directory.
#[must_use]
pub fn default_dir() -> PathBuf {
    if let Ok(d) = std::env::var("RADIOPOST_GATEWAY") {
        return PathBuf::from(d);
    }
    let base = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join(".radiopost-gateway")
}

fn resolver() -> String {
    "https://dns.google/resolve".into()
}

/// What a gateway keeps between sessions.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Held {
    /// Credentials seen, base64url, so callers may name theirs by hash (RP-3 §5.1).
    pub credentials: Vec<String>,
    /// Pieces of uploads broken off: (account, content ID, bytes base64url, total, when).
    pub uploads: Vec<(String, String, String, u64, u64)>,
}

/// A gateway's directory.
pub struct Station {
    dir: PathBuf,
    /// Its settings.
    pub settings: Settings,
    account: Arc<Account>,
    tokens: HashMap<String, Arc<Mutex<Option<String>>>>,
    held: Held,
    store: Shared,
    /// Whether it last reached its own post office, and when it asked (RP-7 §1).
    reached: Option<(Instant, bool)>,
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Hexadecimal text as bytes.
#[must_use]
pub fn unhex(text: &str) -> Option<Vec<u8>> {
    let t = text.trim();
    (t.len() % 2 == 0)
        .then(|| {
            (0..t.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&t[i..i + 2], 16).ok())
                .collect()
        })
        .flatten()
}

/// The clock, Unix seconds.
#[must_use]
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("the system's random numbers");
    b
}

/// Writes a file whole, so a crash leaves the old one or the new one.
///
/// # Errors
///
/// It could not be written.
pub fn write(dir: &Path, name: &str, text: &str) -> Result<(), String> {
    let fresh = dir.join(format!(".{name}.new"));
    std::fs::write(&fresh, text).map_err(|e| format!("{}: {e}", fresh.display()))?;
    std::fs::rename(&fresh, dir.join(name)).map_err(|e| format!("{name}: {e}"))
}

/// The gateway's settings, its credential and key, as they stand on disk, without what it holds:
/// for the threads beside the answering loop, which read them often.
///
/// # Errors
///
/// A file is missing or unreadable.
pub fn identity(dir: &Path) -> Result<(Settings, Arc<Account>), String> {
    let settings = Settings::load(dir)?;
    let key = key(dir)?;
    let credential =
        unhex(&read(&dir.join("credential.hex"))?).ok_or("credential.hex is not hexadecimal")?;
    Ok((settings, Arc::new(Account { credential, key })))
}

impl Station {
    /// The gateway in `dir`: `gateway.json`, `key.hex`, `credential.hex`, and `held.json`.
    ///
    /// # Errors
    ///
    /// A file is missing or unreadable.
    pub fn load(dir: &Path) -> Result<Station, String> {
        let settings = Settings::load(dir)?;
        let key = key(dir)?;
        let credential = unhex(&read(&dir.join("credential.hex")).map_err(|_| {
            "no credential.hex: register the gateway first (`radiopost-gateway register`)"
                .to_owned()
        })?)
        .ok_or("credential.hex is not hexadecimal")?;
        let held = std::fs::read_to_string(dir.join("held.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Ok(Station {
            dir: dir.to_path_buf(),
            settings,
            account: Arc::new(Account { credential, key }),
            tokens: HashMap::new(),
            held,
            store: Shared::open(dir),
            reached: None,
        })
    }

    /// The gateway's credential and key, to log in with.
    #[must_use]
    pub fn account(&self) -> Arc<Account> {
        self.account.clone()
    }

    /// What it holds for when the Internet is down.
    #[must_use]
    pub fn store(&self) -> &Shared {
        &self.store
    }

    fn save(&self) {
        let text = serde_json::to_string_pretty(&self.held).unwrap_or_default();
        if let Err(e) = write(&self.dir, "held.json", &text) {
            eprintln!("radiopost-gateway: cannot save what it holds: {e}");
        }
    }

    /// The API base URL of a domain's post office: from the settings, or from DNS (RP-1 §7.1),
    /// if the operator's choices let the gateway carry sessions there (RP-1 §7.3).
    ///
    /// # Errors
    ///
    /// `unknown-post-office`: none found, or one the operator blocked or left out.
    pub fn post_office(&self, domain: &str) -> Result<String, ErrorCode> {
        let s = &self.settings;
        if s.blocked.iter().any(|b| b == domain)
            || s.only
                .as_ref()
                .is_some_and(|o| !o.iter().any(|d| d == domain))
        {
            return Err(ErrorCode::UnknownPostOffice);
        }
        if let Some(url) = s.post_offices.get(domain) {
            return Ok(url.clone());
        }
        lookup(&s.resolver, domain).ok_or(ErrorCode::UnknownPostOffice)
    }

    /// The gateway's own credential's body: its operator, address and validity.
    #[must_use]
    pub fn credential(&self) -> Option<radiopost::credential::Body> {
        radiopost::credential::Credential::decode(&self.account.credential).map(|c| c.body)
    }

    /// Takes up the credential kept on disk when it is another than the one in use, as after a
    /// renewal ([`renew_if_due`]) or a registration while the gateway answers: whether it did. A
    /// gateway left answering for months logs in with the newest.
    pub fn reload_credential(&mut self) -> bool {
        let Some(bytes) = read(&self.dir.join("credential.hex"))
            .ok()
            .and_then(|t| unhex(&t))
        else {
            return false;
        };
        if bytes == self.account.credential
            || radiopost::credential::Credential::decode(&bytes).is_none()
        {
            return false;
        }
        self.account = Arc::new(Account {
            credential: bytes,
            key: self.account.key.clone(),
        });
        self.tokens.clear();
        true
    }

    /// Logs in at each post office the gateway knows of, as the gateway (RP-5 §3): its own
    /// account's, and those named in its settings. A post office that lets it in will open its
    /// users' sessions.
    #[must_use]
    pub fn check(&self) -> Vec<Check> {
        let mut domains: Vec<String> = self.settings.post_offices.keys().cloned().collect();
        if let Some(own) = self
            .credential()
            .and_then(|b| b.address.rsplit_once('@').map(|(_, d)| d.to_lowercase()))
            && !domains.contains(&own)
        {
            domains.insert(0, own);
        }
        domains
            .into_iter()
            .map(|domain| {
                let at = now();
                let Ok(url) = self.post_office(&domain) else {
                    return Check {
                        domain,
                        url: None,
                        ok: false,
                        said: "not found: no URL in the settings, and none in DNS".into(),
                        at,
                    };
                };
                let http =
                    upstream::Http::new(&url, self.account.clone(), Arc::new(Mutex::new(None)));
                let (ok, said) = match http.login() {
                    Ok(_) => (true, "the gateway logged in".to_owned()),
                    Err(ErrorCode::PostOfficeUnreachable) => (false, "no answer".to_owned()),
                    Err(code) => (false, format!("refused the gateway: {}", code.name())),
                };
                Check {
                    domain,
                    url: Some(url),
                    ok,
                    said,
                    at,
                }
            })
            .collect()
    }

    /// Each domain's post office for a session: reached over HTTPS when it can be, and
    /// otherwise the gateway's own answer from what it holds (RP-3 §9), with the keys it keeps
    /// (RP-1 §7.3). A post office it neither finds nor holds the keys of is unknown.
    fn upstreams(&mut self, at: u64) -> Upstreams {
        let account = self.account.clone();
        let settings = self.settings.clone();
        let store = self.store.clone();
        let tokens = Arc::new(Mutex::new(std::mem::take(&mut self.tokens)));
        Box::new(
            move |domain: &str| -> Result<Box<dyn Upstream + Send>, ErrorCode> {
                // the operator's choices (RP-1 §7.3): `api_of` finds no post office it blocked or
                // left out, and the keys it holds are trusted only as they allow
                let url = crate::upkeep::api_of(&settings, &store, domain);
                let known = store
                    .lock()
                    .trust(&settings.blocked, settings.only.as_deref())
                    .keys
                    .contains_key(domain);
                if url.is_none() && !known {
                    return Err(ErrorCode::UnknownPostOffice);
                }
                let online = url.map(|url| {
                    let token = tokens
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .entry(domain.to_owned())
                        .or_default()
                        .clone();
                    upstream::http(&url, account.clone(), token)
                });
                let offline = Offline::new(
                    store.clone(),
                    &settings.callsign,
                    settings.blocked.clone(),
                    settings.only.clone(),
                    at,
                );
                Ok(Box::new(Fallback::new(online, offline)))
            },
        )
    }

    /// Whether it is cut off (RP-7 §1): its own post office does not let it log in now. Asked at
    /// most once a minute.
    fn cut_off(&mut self) -> bool {
        if let Some((at, reached)) = self.reached
            && at.elapsed() < Duration::from_secs(60)
        {
            return !reached;
        }
        let reached = self
            .credential()
            .map(|b| b.domain())
            .and_then(|d| crate::upkeep::api_of(&self.settings, &self.store, &d))
            .is_some_and(|url| {
                upstream::Http::new(&url, self.account.clone(), Arc::new(Mutex::new(None)))
                    .login()
                    .is_ok()
            });
        self.reached = Some((Instant::now(), reached));
        !reached
    }

    /// The dial of `partner`'s window open at `at`, from its record (RP-7 §5): where the radio goes
    /// for the call.
    fn window_dial(&self, partner: &str, at: u64) -> Option<u64> {
        let own = self.credential().map(|b| b.domain()).unwrap_or_default();
        let record = self.store.lock().record_of(&own, partner)?;
        crate::forward::open_window_dial(&record, &self.settings.country, at)
    }

    /// A partner to call by itself now, if one is due (RP-7 §5, §6.2): cut off, in the partner's
    /// window, its record's window inside this gateway's country's segments.
    fn due_call(
        &mut self,
        caller: &mut crate::forward::Caller,
        busy: bool,
    ) -> Option<crate::forward::Due> {
        let partners: Vec<String> = self
            .settings
            .partners
            .iter()
            .map(|p| p.callsign.clone())
            .collect();
        if partners.is_empty() {
            return None;
        }
        let own = self.credential().map(|b| b.domain()).unwrap_or_default();
        let (windows, online): (HashMap<String, _>, HashMap<String, Option<bool>>) = {
            let s = self.store.lock();
            partners
                .iter()
                .map(|p| {
                    let ws = s
                        .record_of(&own, p)
                        .map(|r| crate::forward::windows_of(&r, &self.settings.country))
                        .unwrap_or_default();
                    let on = s.partners.get(p).and_then(|state| state.online);
                    ((p.clone(), ws), (p.clone(), on))
                })
                .unzip()
        };
        caller.due(
            &partners,
            |p| windows.get(p).cloned().unwrap_or_default(),
            |p| online.get(p).copied().flatten(),
            now(),
            busy,
            || self.cut_off(),
        )
    }

    /// The forwarding sessions it answers (RP-7 §2): its partners' alone, each from its own store.
    /// It reaches the Internet (`WELCOME` `online`) when it can log in at its own post office now.
    fn partners(&self, at: u64) -> Partners {
        let settings = self.settings.clone();
        let store = self.store.clone();
        let account = self.account.clone();
        let own = self.credential().map(|b| b.domain());
        Box::new(
            move |caller: &str| -> Result<Box<dyn Upstream + Send>, ErrorCode> {
                if !settings.is_partner(caller) {
                    return Err(ErrorCode::RefusedByPolicy);
                }
                let online = own
                    .as_deref()
                    .and_then(|d| crate::upkeep::api_of(&settings, &store, d))
                    .is_some_and(|url| {
                        upstream::Http::new(&url, account.clone(), Arc::new(Mutex::new(None)))
                            .login()
                            .is_ok()
                    });
                Ok(Box::new(Partner::new(
                    store.clone(),
                    &settings.callsign,
                    caller,
                    (settings.blocked.clone(), settings.only.clone()),
                    at,
                    online,
                )))
            },
        )
    }

    fn config(&self) -> GatewayConfig {
        let at = now();
        let uploads = self
            .held
            .uploads
            .iter()
            .filter(|(_, _, _, _, when)| at.saturating_sub(*when) < UPLOAD_KEPT_S)
            .filter_map(|(a, id, bytes, total, _)| {
                Some((
                    (a.clone(), ContentId::from_text(id)?),
                    (base64url::decode(bytes)?, *total),
                ))
            })
            .collect();
        GatewayConfig {
            callsign: self.settings.callsign.clone(),
            time: at,
            session_id: random(),
            challenge: random(),
            caps: Caps {
                compression: vec![0, 1],
                max_pdu: 4096,
                renew: false,
            },
            piece: 960,
            credentials: self
                .held
                .credentials
                .iter()
                .filter_map(|c| base64url::decode(c))
                .collect(),
            uploads,
            // read at each call, so a caller refused while the gateway answers is refused next
            refused: crate::refused::callers(&self.dir),
        }
    }

    /// What a session left to keep.
    fn keep(&mut self, notes: Vec<Note>) {
        let at = now();
        for note in notes {
            match note {
                Note::Credential(c) => {
                    let c = base64url::encode(&c);
                    if !self.held.credentials.contains(&c) {
                        self.held.credentials.push(c);
                    }
                }
                Note::Upload {
                    address,
                    id,
                    held,
                    total,
                } => {
                    self.held
                        .uploads
                        .retain(|(a, i, ..)| !(a == &address && i == &id.text()));
                    if !held.is_empty() {
                        self.held.uploads.push((
                            address,
                            id.text(),
                            base64url::encode(&held),
                            total,
                            at,
                        ));
                    }
                }
                Note::PostOffice(_) | Note::Refused(_) | Note::RecordRefused { .. } => {}
            }
        }
        self.held
            .uploads
            .retain(|(.., when)| at.saturating_sub(*when) < UPLOAD_KEPT_S);
        self.save();
    }
}

/// RP-1 §7.1: a domain's API record, by DNS over HTTPS.
#[must_use]
pub fn lookup(resolver: &str, domain: &str) -> Option<String> {
    radiopost_api::lookup(resolver, domain)
}

/// The gateway's engine on a link: the caller speaks first, and hangs up (RP-3 §8).
struct Answering(Gateway);

impl Engine for Answering {
    type Event = SessionEvent;
    fn opening(&mut self) -> Option<Vec<u8>> {
        None
    }
    fn receive(&mut self, bytes: &[u8]) -> Vec<SessionEvent> {
        self.0.receive(bytes)
    }
    fn transmit(&mut self) -> Option<Vec<u8>> {
        self.0.transmit()
    }
    fn is_over(&self) -> bool {
        false
    }
    fn link_lost(&mut self) -> Vec<SessionEvent> {
        self.0.link_closed()
    }
}

/// A cut-off gateway's engine on a link to its partner: it speaks first, and hangs up (RP-7 §2).
struct Calling(Client);

impl Engine for Calling {
    type Event = SessionEvent;
    fn opening(&mut self) -> Option<Vec<u8>> {
        Some(self.0.hello())
    }
    fn receive(&mut self, bytes: &[u8]) -> Vec<SessionEvent> {
        self.0.receive(bytes)
    }
    fn transmit(&mut self) -> Option<Vec<u8>> {
        self.0.transmit()
    }
    fn is_over(&self) -> bool {
        self.0.is_over()
    }
    fn link_lost(&mut self) -> Vec<SessionEvent> {
        self.0.link_broken()
    }
}

/// The address of the modem's RPMI adapter: as given, or for Aether's control API the Aether
/// adapter started in this process (RP-4 §9.1).
fn adapter_for(modem: &str) -> Result<SocketAddr, String> {
    if modem.starts_with("ws://") {
        // one adapter for each Aether this process reaches: it serves each host in turn
        static STARTED: OnceLock<Mutex<HashMap<String, SocketAddr>>> = OnceLock::new();
        let mut started = STARTED
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(at) = started.get(modem) {
            return Ok(*at);
        }
        let note: radiopost_aether::serve::Note = std::sync::Arc::new(|line: &str| {
            radiopost::logfile::line(&format!("Aether adapter: {line}"));
        });
        let at = radiopost_aether::serve::listen_noting(
            "127.0.0.1:0",
            modem,
            note,
            radiopost::logfile::path(),
        )
        .map_err(|e| format!("cannot start the Aether adapter: {e}"))?;
        started.insert(modem.to_owned(), at);
        Ok(at)
    } else {
        modem
            .parse()
            .or_else(|_| {
                std::net::ToSocketAddrs::to_socket_addrs(modem)
                    .ok()
                    .and_then(|mut a| a.next())
                    .ok_or(())
            })
            .map_err(|()| format!("{modem} is not a modem's address"))
    }
}

struct Modem {
    conn: Connection,
    host: Host,
    /// The modem reads and sets the radio's dial (RP-4 §4.1).
    dial: bool,
    /// The gateway judges the dial itself (RP-4 §8, wishlist W40): the modem checks no band rules
    /// and the operator has not said they do.
    judges: bool,
    /// Why it does not answer with the radio where it is, when it judges the dial.
    outside: Option<String>,
    /// The modem scans the radio over a list of dials (RP-4 §4.2).
    scans: bool,
    /// The dials it is scanning, the first the home; empty when it is not.
    scanning: Vec<u64>,
    /// The radio's dial, as the modem last said (RP-4 §4.1): where it stays when a scan ends.
    at_hz: Option<u64>,
    /// A scan ended and the dial was asked, with this request `id`: the answer, or the dial's
    /// next change, says where the radio stays (RP-4 §4.2: the end is said before the dial).
    stay_asked: Option<serde_json::Value>,
}

impl Modem {
    /// Stops answering while the gateway carries a session on another of its modems (W26).
    fn hold(&mut self) {
        let off = self.host.listen(false);
        let _ = self.conn.send(&off);
    }

    /// Answers again once the gateway's session on another modem is over (W26). A call this
    /// modem answered meanwhile, before it heard `listen` off, was never carried: it is aborted.
    fn release(&mut self, name: &str, tell: &mut dyn FnMut(&str)) -> io::Result<()> {
        while let Some(e) = self.next()? {
            if let Event::Connected {
                link,
                remote,
                direction: Direction::In,
                ..
            } = e
            {
                let abort = self.host.abort(Some(&link));
                self.conn.send(&abort)?;
                tell(&format!(
                    "{remote} called on {name} while another session was carried: the call was ended"
                ));
            }
        }
        if self.outside.is_none() {
            let on = self.host.listen(true);
            self.conn.send(&on)?;
        }
        Ok(())
    }

    fn next(&mut self) -> io::Result<Option<Event>> {
        match self.conn.read_message() {
            Ok(Some(m)) => Event::read(&m)
                .map(Some)
                .map_err(|why| io::Error::new(io::ErrorKind::InvalidData, why)),
            Ok(None) => Err(io::ErrorKind::ConnectionAborted.into()),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    fn ask(&mut self, request: &rpmi::Message) -> Result<Event, String> {
        self.conn.send(request).map_err(|e| e.to_string())?;
        let id = request.get("id").cloned();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            match self
                .next()
                .map_err(|e| format!("the modem's adapter: {e}"))?
            {
                Some(Event::Error {
                    id: Some(i),
                    code,
                    text,
                }) if Some(&i) == id.as_ref() => {
                    return Err(format!(
                        "the modem refused: {code} {}",
                        text.unwrap_or_default()
                    ));
                }
                // a request's answer, not another's: the answering loop sends `listen` unawaited
                Some(e @ Event::Ok { .. }) if matches!(&e, Event::Ok { id: i } if Some(i) == id.as_ref()) =>
                {
                    return Ok(e);
                }
                Some(e @ Event::Hello { .. }) => return Ok(e),
                Some(e @ Event::Dial { id: Some(_), .. }) if e_dial_id(&e) == id => {
                    if let Event::Dial { dial_hz, .. } = e {
                        self.at_hz = Some(dial_hz);
                    }
                    return Ok(e);
                }
                Some(e @ Event::Scan { id: Some(_), .. }) if e_scan_id(&e) == id => return Ok(e),
                // the radio moved meanwhile: a scan's move, or another program's tune
                Some(Event::Dial { id: None, dial_hz }) => self.at_hz = Some(dial_hz),
                _ => {}
            }
        }
        Err("the modem's adapter did not answer".into())
    }

    /// The radio's dial, asked of the modem (RP-4 §4.1).
    fn read_dial(&mut self) -> Result<u64, String> {
        let read = self.host.dial();
        match self.ask(&read)? {
            Event::Dial { dial_hz, .. } => Ok(dial_hz),
            _ => Err("the modem did not say where its dial is".into()),
        }
    }

    /// Answering turned on or off as the dial allows (W40), when the gateway judges it: `None`
    /// when nothing changed, else what to say. `ask`s only while opening; in the answering loop
    /// the modem's `ok` is one more event.
    fn judge(
        &mut self,
        dial_hz: u64,
        country: &str,
        opening: bool,
    ) -> Result<Option<String>, String> {
        if !self.judges {
            return Ok(None);
        }
        let outside = crate::rules::verdict(dial_hz, country);
        if outside == self.outside && !opening {
            return Ok(None);
        }
        let listen = self.host.listen(outside.is_none());
        if opening {
            self.ask(&listen)?;
        } else {
            self.conn.send(&listen).map_err(|e| e.to_string())?;
        }
        let said = outside.clone().unwrap_or_else(|| {
            format!(
                "the radio is at {} MHz, inside a segment where the gateway may answer",
                crate::rules::mhz(dial_hz)
            )
        });
        self.outside = outside;
        Ok(Some(said))
    }

    /// Starts the scan of `dials` (RP-4 §4.2): the dials and `scan_s` as the modem answers. When
    /// the gateway judges the dial itself (W40) it judges every dial first and does not ask for a
    /// list naming one outside the segments; the scan's moves then leave answering on.
    fn start_scan(
        &mut self,
        dials: &[u64],
        country: &str,
    ) -> Result<(Vec<u64>, Option<u64>), String> {
        if !self.scans {
            return Err(
                "the modem does not scan (RP-4 §4.2): the gateway answers on the radio's dial"
                    .into(),
            );
        }
        if self.judges
            && let Some(outside) = dials
                .iter()
                .find_map(|&hz| crate::rules::verdict(hz, country))
        {
            return Err(format!("not scanning: {outside}"));
        }
        if self.dial {
            // where the radio is as the scan starts: it may stay there, and the modem then says
            // nothing of the dial until its first move
            self.at_hz = self.read_dial().ok();
        }
        let scan = self.host.scan(dials);
        let Event::Scan { dials, scan_s, .. } = self.ask(&scan)? else {
            return Err("the modem did not answer the scan".into());
        };
        self.scanning.clone_from(&dials);
        if self.judges && self.outside.is_some() {
            let listen = self.host.listen(true);
            self.ask(&listen)?;
            self.outside = None;
        }
        Ok((dials, scan_s))
    }

    /// Stops the scan (RP-4 §4.2): the dials it scanned, for starting it again; none when it was
    /// not scanning.
    fn stop_scan(&mut self) -> Result<Vec<u64>, String> {
        if self.scanning.is_empty() {
            return Ok(Vec::new());
        }
        let stop = self.host.scan(&[]);
        self.ask(&stop)?;
        Ok(std::mem::take(&mut self.scanning))
    }

    /// Tunes the radio to `dial_hz` for a call (RP-4 §4.1), and gives the dial it left, to go
    /// back to; `None` when it is there already.
    fn tune_away(&mut self, dial_hz: u64) -> Result<Option<u64>, String> {
        let was = self.read_dial()?;
        if was == dial_hz {
            return Ok(None);
        }
        let tune = self.host.tune(dial_hz);
        self.ask(&tune).map_err(|e| {
            format!(
                "cannot tune the radio to {} MHz: {e}",
                crate::rules::mhz(dial_hz)
            )
        })?;
        Ok(Some(was))
    }

    /// Tunes the radio back after a call; the adapter may not have put the link down yet, so
    /// `busy` is tried again for a while.
    fn tune_back(&mut self, was: u64) -> Result<(), String> {
        let patience = Instant::now() + Duration::from_secs(30);
        loop {
            let tune = self.host.tune(was);
            match self.ask(&tune) {
                Ok(_) => return Ok(()),
                Err(e) if e.starts_with("the modem refused: busy") && Instant::now() < patience => {
                    std::thread::sleep(Duration::from_millis(250));
                }
                Err(e) => return Err(e),
            }
        }
    }
}

/// A `dial` answer's `id`.
fn e_dial_id(e: &Event) -> Option<serde_json::Value> {
    match e {
        Event::Dial { id, .. } => id.clone(),
        _ => None,
    }
}

/// A `scan` answer's `id`.
fn e_scan_id(e: &Event) -> Option<serde_json::Value> {
    match e {
        Event::Scan { id, .. } => id.clone(),
        _ => None,
    }
}

/// Why a scan ended (RP-4 §4.2), in the operator's words.
fn scan_ended(why: Option<&str>) -> &'static str {
    match why {
        Some("left") => "the modem lost the gateway's connection",
        Some("tuned") => "another program tuned the radio",
        Some("knob") => "the radio's dial was moved at the radio",
        Some("operator") => "the modem's operator stopped it",
        Some("radio") => "fewer than two of its channels are left that the radio will take",
        _ => "as the gateway asked",
    }
}

/// A list of dials, in megahertz.
fn dials_in_mhz(dials: &[u64]) -> String {
    dials
        .iter()
        .map(|&hz| crate::rules::mhz(hz))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Why the modem could not be opened.
enum Unopened {
    /// It may open later: it is not running yet, or it refused for now.
    Later(String),
    /// It never will as the gateway is set: the operator must change something.
    Never(String),
}

/// Opens the modem at `address`: `hello`, the gateway's callsign, and `listen` on (RP-4 §3, §4).
fn open(station: &Station, address: &str) -> Result<Modem, Unopened> {
    let at = adapter_for(address).map_err(Unopened::Never)?;
    let conn = Connection::open(at)
        .map_err(|e| Unopened::Later(format!("cannot reach the modem at {at}: {e}")))?;
    conn.set_timeout(Some(Duration::from_millis(250)))
        .map_err(|e| Unopened::Later(e.to_string()))?;
    let mut modem = Modem {
        conn,
        host: Host::new(),
        dial: false,
        judges: false,
        outside: None,
        scans: false,
        scanning: Vec::new(),
        at_hz: None,
        stay_asked: None,
    };
    let hello = modem
        .host
        .hello(concat!("radiopost-gateway ", env!("CARGO_PKG_VERSION")));
    let Event::Hello { caps, .. } = modem.ask(&hello).map_err(Unopened::Later)? else {
        return Err(Unopened::Later("the modem did not say hello".into()));
    };
    modem.dial = caps.dial;
    modem.scans = caps.scan;
    // RP-4 §8: a modem that checks no band rules leaves them to the gateway, which judges the
    // radio's dial when the modem says where it is (W40), or to its operator, who says so
    modem.judges = !caps.regulatory && caps.dial && !station.settings.operator_checks_rules;
    if !caps.regulatory && !caps.dial && !station.settings.operator_checks_rules {
        return Err(Unopened::Never(
            "this modem neither checks the band rules nor says where the radio's dial is (RP-4 \
             §8): the gateway's operator must check them, and says so in the gateway's settings \
             (`operator_checks_rules` in gateway.json)"
                .into(),
        ));
    }
    let calls = modem.host.callsigns(&[station.settings.callsign.as_str()]);
    modem.ask(&calls).map_err(Unopened::Later)?;
    if modem.judges {
        let dial = modem.read_dial().map_err(Unopened::Later)?;
        modem
            .judge(dial, &station.settings.country, true)
            .map_err(Unopened::Later)?;
    } else {
        let listen = modem.host.listen(true);
        modem.ask(&listen).map_err(Unopened::Later)?;
    }
    Ok(modem)
}

/// Waits up to `d`, or less when the gateway is asked to stop.
fn pause(monitor: &Monitor, d: Duration) {
    let until = Instant::now() + d;
    while Instant::now() < until && !monitor.stopping() {
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Opens the modem, trying again every 5 seconds while it cannot be reached: `None` when the
/// gateway was asked to stop first.
///
/// # Errors
///
/// The modem never will open as the gateway is set.
fn open_when_able(
    station: &Station,
    monitor: &Monitor,
    tell: &mut dyn FnMut(&str),
) -> Result<Option<Modem>, String> {
    let mut said = None;
    loop {
        if monitor.stopping() {
            return Ok(None);
        }
        match open(station, &station.settings.modem) {
            Ok(m) => return Ok(Some(m)),
            Err(Unopened::Never(e)) => return Err(e),
            Err(Unopened::Later(e)) => {
                if said.as_ref() != Some(&e) {
                    tell(&format!("waiting for the modem: {e}"));
                    said = Some(e.clone());
                }
                monitor.set_state("opening", Some(e));
                pause(monitor, Duration::from_secs(5));
            }
        }
    }
}

/// Answers calls until the process ends, carrying each session (RP-3 §9), and says what
/// happens on `say`.
///
/// # Errors
///
/// The gateway is not set up, or its modem never will open as it is set.
pub fn serve(dir: &Path, say: &mut dyn FnMut(&str)) -> Result<(), String> {
    run(dir, &Monitor::new(), say)
}

/// Answers calls until `monitor` is asked to stop, carrying each session (RP-3 §9): it waits for
/// the modem while it cannot be reached, and opens it again when it goes away. What happens goes
/// to `monitor` and to `say`.
///
/// # Errors
///
/// The gateway is not set up, or its modem never will open as it is set.
#[allow(
    clippy::too_many_lines,
    reason = "the answering loop, read in one place: each modem's events and the calls it places"
)]
pub fn run(dir: &Path, monitor: &Monitor, say: &mut dyn FnMut(&str)) -> Result<(), String> {
    let mut tell = |line: &str| {
        monitor.log(line);
        say(line);
    };
    let mut station = Station::load(dir)?;
    monitor.set_state("opening", None);
    let Some(mut modem) = open_when_able(&station, monitor, &mut tell)? else {
        return stopped(monitor, &mut tell);
    };
    let listening = listening_line(&station.settings);
    answering(&modem, monitor, &listening, &mut tell);
    scan_at_start(&mut station, &mut modem, monitor, &mut tell);
    let mut others = Others::new(&station.settings.more_modems);
    let mut renewal = Renewal::new(dir, RENEW_EVERY, radiopost::credential::RENEW_WITHIN);
    let mut caller = crate::forward::Caller::default();
    let mut busy = false;
    let mut checked = Instant::now();
    // copies, the key cache and reports, beside the answering loop while it runs
    let (said, upkeep_says) = std::sync::mpsc::channel();
    let _upkeep = crate::upkeep::start(dir, station.store().clone(), said);
    loop {
        renewal.tick(&mut tell);
        while let Ok(line) = upkeep_says.try_recv() {
            tell(&line);
        }
        if monitor.stopping() {
            let off = modem.host.listen(false);
            let _ = modem.conn.send(&off);
            modem.conn.close();
            others.close();
            return stopped(monitor, &mut tell);
        }
        let auto = checked.elapsed() >= Duration::from_secs(5);
        if auto {
            checked = Instant::now();
        }
        others.open_due(&station, &mut tell);
        let busy_now = busy || others.busy();
        calls_between_sessions(
            &mut station,
            (&mut modem, &mut others),
            monitor,
            (&mut caller, busy_now, auto),
            &mut tell,
        );
        if carried_on_another(&mut station, (&mut modem, &mut others), monitor, &mut tell) {
            answering(&modem, monitor, &listening, &mut |_| {});
            continue;
        }
        let event = match modem.next() {
            Ok(Some(Event::Busy(b))) => {
                busy = b; // RP-4 §5: no call on a busy channel
                continue;
            }
            // the radio moved (W40): answering follows where it may, but for a scan's own moves,
            // whose dials were judged before it started
            // where the radio stays once a scan has ended: the answer to the dial asked, or a
            // failure to say, which leaves the last dial known
            Ok(Some(Event::Dial {
                id: Some(id),
                dial_hz,
            })) if modem.stay_asked.as_ref() == Some(&id) => {
                modem.at_hz = Some(dial_hz);
                stays_at(&mut station, &mut modem, Some(dial_hz), &mut tell);
                continue;
            }
            Ok(Some(Event::Error { id: Some(id), .. }))
                if modem.stay_asked.as_ref() == Some(&id) =>
            {
                let at = modem.at_hz;
                stays_at(&mut station, &mut modem, at, &mut tell);
                continue;
            }
            Ok(Some(Event::Dial { id: None, dial_hz })) => {
                modem.at_hz = Some(dial_hz);
                monitor.scan_at(dial_hz);
                if modem.stay_asked.is_some() {
                    // the scan ended as the radio moved: it stays where it was put
                    stays_at(&mut station, &mut modem, Some(dial_hz), &mut tell);
                }
                if modem.scanning.contains(&dial_hz) {
                    continue;
                }
                match modem.judge(dial_hz, &station.settings.country, false) {
                    Ok(Some(said)) => {
                        tell(&said);
                        answering(&modem, monitor, &listening, &mut |_| {});
                    }
                    Ok(None) => {}
                    Err(e) => tell(&format!("could not change answering: {e}")),
                }
                continue;
            }
            Ok(Some(Event::Scan {
                id: None,
                dials,
                scan_s,
                why,
            })) => {
                scan_changed(
                    &mut station,
                    &mut modem,
                    (dials, scan_s, why),
                    monitor,
                    &mut tell,
                );
                continue;
            }
            Ok(Some(e)) => e,
            Ok(None) => continue,
            Err(e) => {
                tell(&format!(
                    "the modem's adapter went away ({e}); opening it again"
                ));
                monitor.set_state(
                    "opening",
                    Some(format!("the modem's adapter went away: {e}")),
                );
                pause(monitor, Duration::from_secs(5));
                match open_when_able(&station, monitor, &mut tell)? {
                    Some(m) => modem = m,
                    None => return stopped(monitor, &mut tell),
                }
                answering(&modem, monitor, &listening, &mut tell);
                scan_at_start(&mut station, &mut modem, monitor, &mut tell);
                continue;
            }
        };
        if let Event::Connected {
            direction: Direction::In,
            ..
        } = &event
        {
            others.hold(None);
            let address = station.settings.modem.clone();
            carry_call(
                &mut station,
                &mut modem,
                &event,
                &address,
                monitor,
                &mut tell,
            );
            others.release(None, &mut tell);
            if !others.0.is_empty() {
                answering(&modem, monitor, &listening, &mut |_| {});
            }
        }
    }
}

/// A call one of the other modems answered, carried while the first modem and the rest hold their
/// answering (W26): whether there was one.
fn carried_on_another(
    station: &mut Station,
    (modem, others): (&mut Modem, &mut Others),
    monitor: &Monitor,
    tell: &mut dyn FnMut(&str),
) -> bool {
    let country = station.settings.country.clone();
    let Some((i, event)) = others.next(&country, tell) else {
        return false;
    };
    modem.hold();
    others.hold(Some(i));
    let address = others.0[i].address.clone();
    if let Some(m) = others.0[i].modem.as_mut() {
        let _ = m.conn.set_timeout(Some(SESSION_WAIT));
        carry_call(station, m, &event, &address, monitor, tell);
        let _ = m.conn.set_timeout(Some(OTHER_WAIT));
    }
    others.release(None, tell);
    if let Err(e) = modem.release(&station.settings.modem, tell) {
        tell(&format!("the modem's adapter: {e}"));
    }
    true
}

/// A call answered on the modem at `address`, carried to its end (RP-3 §9).
fn carry_call(
    station: &mut Station,
    modem: &mut Modem,
    event: &Event,
    address: &str,
    monitor: &Monitor,
    tell: &mut dyn FnMut(&str),
) {
    let Event::Connected { remote, .. } = event else {
        return;
    };
    if station.settings.more_modems.is_empty() {
        tell(&format!("{remote} called"));
    } else {
        tell(&format!("{remote} called on {address}"));
    }
    if station.reload_credential() {
        tell("this session uses the gateway's renewed credential");
    }
    monitor.begin(remote);
    let result = carry(station, modem, event, monitor);
    match &result {
        Ok(()) => tell(&format!("the session with {remote} is over")),
        Err(e) => tell(&format!("the session with {remote} ended: {e}")),
    }
    monitor.end(&result);
}

/// How long a read from one of the gateway's other modems waits, between sessions: each is asked
/// in turn after the first, which waits longer.
const OTHER_WAIT: Duration = Duration::from_millis(20);
/// How long a read waits while a session is carried, as on the first modem.
const SESSION_WAIT: Duration = Duration::from_millis(250);

/// One of the gateway's other modems (W26).
struct Other {
    address: String,
    modem: Option<Modem>,
    /// When to try opening it again; `None` once it never will open as the gateway is set.
    retry: Option<Instant>,
    /// What was last said of why it is not open, so it is said once.
    said: Option<String>,
    busy: bool,
}

/// The gateway's other modems (W26): opened when they can be, read between sessions, and kept
/// from answering while a session runs on another.
struct Others(Vec<Other>);

impl Others {
    fn new(addresses: &[String]) -> Others {
        Others(
            addresses
                .iter()
                .map(|a| Other {
                    address: a.clone(),
                    modem: None,
                    retry: Some(Instant::now()),
                    said: None,
                    busy: false,
                })
                .collect(),
        )
    }

    /// Opens those whose turn to try has come.
    fn open_due(&mut self, station: &Station, tell: &mut dyn FnMut(&str)) {
        for o in &mut self.0 {
            if o.modem.is_some() || o.retry.is_none_or(|r| Instant::now() < r) {
                continue;
            }
            match open(station, &o.address) {
                Ok(m) => {
                    let _ = m.conn.set_timeout(Some(OTHER_WAIT));
                    tell(&match &m.outside {
                        Some(why) => format!("{}: {why}", o.address),
                        None => format!("also listening on {}", o.address),
                    });
                    o.modem = Some(m);
                    o.said = None;
                }
                Err(Unopened::Never(e)) => {
                    tell(&format!("{} will not be used: {e}", o.address));
                    o.retry = None;
                }
                Err(Unopened::Later(e)) => {
                    if o.said.as_ref() != Some(&e) {
                        tell(&format!("waiting for {}: {e}", o.address));
                        o.said = Some(e);
                    }
                    o.retry = Some(Instant::now() + Duration::from_secs(5));
                }
            }
        }
    }

    /// Whether any says the channel is busy (RP-4 §5).
    fn busy(&self) -> bool {
        self.0.iter().any(|o| o.modem.is_some() && o.busy)
    }

    /// Reads each open one's events: the first call answered, with its modem's place.
    fn next(&mut self, country: &str, tell: &mut dyn FnMut(&str)) -> Option<(usize, Event)> {
        for (i, o) in self.0.iter_mut().enumerate() {
            let Some(m) = o.modem.as_mut() else {
                continue;
            };
            loop {
                match m.next() {
                    Ok(Some(Event::Busy(b))) => o.busy = b,
                    Ok(Some(Event::Dial { id: None, dial_hz })) => {
                        if let Ok(Some(said)) = m.judge(dial_hz, country, false) {
                            tell(&format!("{}: {said}", o.address));
                        }
                    }
                    Ok(Some(
                        e @ Event::Connected {
                            direction: Direction::In,
                            ..
                        },
                    )) => {
                        return Some((i, e));
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(e) => {
                        tell(&format!(
                            "the adapter of {} went away ({e}); opening it again",
                            o.address
                        ));
                        o.modem = None;
                        o.busy = false;
                        o.retry = Some(Instant::now() + Duration::from_secs(5));
                        break;
                    }
                }
            }
        }
        None
    }

    /// Keeps all but `except` from answering.
    fn hold(&mut self, except: Option<usize>) {
        for (i, o) in self.0.iter_mut().enumerate() {
            if Some(i) != except
                && let Some(m) = o.modem.as_mut()
            {
                m.hold();
            }
        }
    }

    /// Lets all but `except` answer again.
    fn release(&mut self, except: Option<usize>, tell: &mut dyn FnMut(&str)) {
        for (i, o) in self.0.iter_mut().enumerate() {
            if Some(i) == except {
                continue;
            }
            if let Some(m) = o.modem.as_mut()
                && let Err(e) = m.release(&o.address, tell)
            {
                tell(&format!("the adapter of {}: {e}", o.address));
                o.modem = None;
                o.retry = Some(Instant::now() + Duration::from_secs(5));
            }
        }
    }

    /// Lets them all go, as the gateway stops.
    fn close(&mut self) {
        for o in &mut self.0 {
            if let Some(mut m) = o.modem.take() {
                m.hold();
                m.conn.close();
            }
        }
    }
}

/// The line that says where the gateway listens.
fn listening_line(settings: &Settings) -> String {
    let mut modems = vec![settings.modem.as_str()];
    modems.extend(settings.more_modems.iter().map(String::as_str));
    format!(
        "listening as {} on {}",
        settings.callsign,
        modems.join(" and ")
    )
}

/// Starts the first modem's scan when the gateway is set to scan (W50, RP-4 §4.2), and says so.
/// A modem that cannot scan, or a list naming a dial the gateway may not answer on (W40), leaves
/// the radio where it is, said once and shown.
fn scan_at_start(
    station: &mut Station,
    modem: &mut Modem,
    monitor: &Monitor,
    tell: &mut dyn FnMut(&str),
) {
    let dials = station.settings.scan.clone();
    if dials.is_empty() {
        monitor.set_scan(None);
        // set not to scan: the record's channels lose their `scan_s` (RP-5 §4.4)
        let store = station.store().lock();
        let scanned = !store.scan_dials.is_empty() || !store.scan_left.is_empty();
        drop(store);
        if scanned {
            keep_scan(station, (Vec::new(), None), None, tell);
        }
        return;
    }
    match modem.start_scan(&dials, &station.settings.country) {
        Ok((dials, scan_s)) => scanning(station, dials, scan_s, monitor, tell),
        Err(e) => {
            tell(&e);
            monitor.set_scan(Some(crate::monitor::Scanning {
                not: Some(e),
                ..crate::monitor::Scanning::default()
            }));
        }
    }
}

/// The modem's scan as it now runs (RP-4 §4.2): said, shown, its `scan_s` kept, and the gateway
/// registered again when what its record publishes changed (RP-5 §4.4).
fn scanning(
    station: &mut Station,
    dials: Vec<u64>,
    scan_s: Option<u64>,
    monitor: &Monitor,
    tell: &mut dyn FnMut(&str),
) {
    tell(&format!(
        "scanning {} channels, {} MHz: a call must keep trying {} s to find the gateway",
        dials.len(),
        dials_in_mhz(&dials),
        scan_s.map_or_else(|| "?".to_owned(), |s| s.to_string())
    ));
    monitor.set_scan(Some(crate::monitor::Scanning {
        dials: dials.clone(),
        scan_s,
        at_hz: None,
        not: None,
    }));
    keep_scan(station, (dials, scan_s), None, tell);
}

/// Keeps the scan the modem answered, and registers the gateway again when its record no longer
/// says it (RP-5 §4.4, §6.4), as its operator's enrollment on this computer lets it. The channels
/// it is set to scan and no longer listens on are left out of the record: dials the modem dropped,
/// or, with no scan (`dials` empty), all but the one the radio `stays` on.
fn keep_scan(
    station: &mut Station,
    (dials, scan_s): (Vec<u64>, Option<u64>),
    stays: Option<u64>,
    tell: &mut dyn FnMut(&str),
) {
    let set = &station.settings.scan;
    let left: Vec<u64> = if dials.is_empty() {
        stays.map_or_else(Vec::new, |at| {
            set.iter().copied().filter(|&hz| hz != at).collect()
        })
    } else {
        set.iter()
            .copied()
            .filter(|hz| !dials.contains(hz))
            .collect()
    };
    if let Err(e) = station.store().change(|s| {
        s.scan_dials = dials;
        s.scan_s = scan_s;
        s.scan_left = left;
    }) {
        tell(&format!("could not keep the scan: {e}"));
    }
    match crate::operator::register_again(&station.dir) {
        None => {}
        Some(Ok(said)) => {
            tell(&format!(
                "registered again, since what it listens on changed: {said}"
            ));
            station.reload_credential();
        }
        Some(Err(e)) => tell(&format!(
            "register the gateway again: its record does not say where it listens now ({e})"
        )),
    }
}

/// A `scan` event (RP-4 §4.2): a dial dropped, or the scan ended. The gateway does not start an
/// ended scan again by itself: the operator moved the radio, or the modem cannot go on.
fn scan_changed(
    station: &mut Station,
    modem: &mut Modem,
    (dials, scan_s, why): (Vec<u64>, Option<u64>, Option<String>),
    monitor: &Monitor,
    tell: &mut dyn FnMut(&str),
) {
    if dials.is_empty() {
        modem.scanning.clear();
        let said = format!(
            "the scan ended: {}; the radio stays where it is",
            scan_ended(why.as_deref())
        );
        tell(&said);
        monitor.set_scan(Some(crate::monitor::Scanning {
            not: Some(said),
            ..crate::monitor::Scanning::default()
        }));
        // RP-5 §4.4: its record names only what it listens on now, the dial the radio stays on,
        // with no `scan_s`. The modem says the end before the dial it leaves the radio on (RP-4
        // §4.2), so the dial is asked, and the answering loop goes on meanwhile.
        if modem.dial {
            let ask = modem.host.dial();
            modem.stay_asked = ask.get("id").cloned();
            if modem.conn.send(&ask).is_ok() {
                return;
            }
        }
        let at = modem.at_hz;
        stays_at(station, modem, at, tell);
        return;
    }
    modem.scanning.clone_from(&dials);
    scanning(station, dials, scan_s, monitor, tell);
}

/// A scan has ended and the radio stays at `at`, when the modem says: the gateway listens there
/// only, and registers again with that channel alone (RP-5 §4.4).
fn stays_at(station: &mut Station, modem: &mut Modem, at: Option<u64>, tell: &mut dyn FnMut(&str)) {
    modem.stay_asked = None;
    match at {
        Some(hz) => tell(&format!(
            "listening on {} MHz only, until the scan runs again",
            crate::rules::mhz(hz)
        )),
        None => tell("the modem has not said where the radio stays"),
    }
    keep_scan(station, (Vec::new(), None), at, tell);
}

/// Says whether the gateway answers: listening, or not where the radio is (W40).
fn answering(modem: &Modem, monitor: &Monitor, listening: &str, tell: &mut dyn FnMut(&str)) {
    if let Some(why) = &modem.outside {
        tell(why);
        monitor.set_state("outside", Some(why.clone()));
    } else {
        tell(listening);
        monitor.set_state("listening", None);
    }
}

/// What happens between sessions (RP-7 §2, §5): when `auto`, a call to a partner whose window is
/// open while this gateway is cut off; and a call the operator asked for.
fn calls_between_sessions(
    station: &mut Station,
    (modem, others): (&mut Modem, &mut Others),
    monitor: &Monitor,
    (caller, busy, auto): (&mut crate::forward::Caller, bool, bool),
    tell: &mut dyn FnMut(&str),
) {
    let due = if auto {
        station.due_call(caller, busy)
    } else {
        None
    };
    let asked = monitor.take_forward();
    let mut call = |partner: &str, records: bool, why: &str, tell: &mut dyn FnMut(&str)| {
        tell(&format!("calling {partner} {why}"));
        monitor.begin(partner);
        let dial = station.window_dial(partner, now());
        others.hold(None);
        let result = forward(station, modem, partner, monitor, (records, dial));
        others.release(None, tell);
        match &result {
            Ok(line) => tell(line),
            Err(e) => tell(&format!("the call to {partner} failed: {e}")),
        }
        let through = result.is_ok();
        monitor.end(&result.map(|_| ()));
        if modem.outside.is_none() {
            monitor.set_state("listening", None);
        } else {
            monitor.set_state("outside", modem.outside.clone());
        }
        through
    };
    // in a partner's window while cut off, it calls by itself (RP-7 §5, §6.2)
    if let Some(due) = due {
        let why = if due.records {
            "in its window to forward the queue"
        } else {
            "in its window to collect the mail: it is cut off too, and a partner that reaches \
             the Internet opens a window within the hour"
        };
        let through = call(&due.partner, due.records, why, tell);
        caller.called(&due.partner, through);
    }
    // the operator asked for its queue to go to a partner
    if let Some(partner) = asked {
        call(&partner, true, "to forward the queue", tell);
    }
}

/// The loop stopped as the operator asked.
#[allow(clippy::unnecessary_wraps, reason = "what `run` returns, in one place")]
fn stopped(monitor: &Monitor, tell: &mut dyn FnMut(&str)) -> Result<(), String> {
    tell("stopped answering");
    monitor.set_state("stopped", None);
    // the scan ended with the gateway's connection to its modem (RP-4 §1.1)
    monitor.set_scan(None);
    Ok(())
}

/// One session, from the link coming up until the caller hangs up, the link is quiet too long, or
/// the gateway is asked to stop.
fn carry(
    station: &mut Station,
    modem: &mut Modem,
    connected: &Event,
    monitor: &Monitor,
) -> Result<(), String> {
    let country = station.settings.country.clone();
    let config = station.config();
    let at = config.time;
    let engine = Gateway::new(
        config,
        station.upstreams(at),
        Box::new(move |call| radiopost::thirdparty::may_hand(call, &country)),
    )
    .with_partner(station.partners(at));
    let mut carrier = Carrier::new(Answering(engine));
    // each event is handed to the engine once: a quiet read hands it nothing
    let mut event = Some(connected.clone());
    // RP-3 §8: from the last bytes, stopped while the modem receives
    let mut silence = Silence::new();
    let hang_up = |carrier: &mut Carrier<Answering>, modem: &mut Modem| {
        if let Some(link) = carrier.link() {
            let bye = modem.host.disconnect(link);
            let _ = modem.conn.send(&bye);
        }
        carrier.engine_mut().link_lost()
    };
    let result = loop {
        if let Some(e) = event.take() {
            let (requests, events) = carrier.on_event(&mut modem.host, &e);
            for e in &events {
                monitor.event(e);
            }
            monitor.account(carrier.engine().0.account());
            if let Some(err) = requests.iter().find_map(|r| modem.conn.send(r).err()) {
                carrier.engine_mut().link_lost();
                break Err(format!("the modem's adapter: {err}"));
            }
        }
        if carrier.is_ended() {
            break Ok(());
        }
        if monitor.stopping() {
            hang_up(&mut carrier, modem);
            break Err("the operator stopped the gateway".to_owned());
        }
        match modem.next() {
            Ok(Some(e)) => {
                silence.heard(&e);
                event = Some(e);
            }
            Ok(None) => {}
            Err(e) => {
                carrier.engine_mut().link_lost();
                break Err(format!("the modem's adapter: {e}"));
            }
        }
        if silence.over(QUIET) {
            // RP-3 §8: the station called asks the modem to disconnect
            hang_up(&mut carrier, modem);
            break Err("nothing heard for 300 seconds".to_owned());
        }
    };
    let notes = carrier.engine_mut().0.take_notes();
    for note in &notes {
        match note {
            Note::PostOffice(code) => monitor.post_office_failed(*code),
            Note::Refused(entry) => monitor.refused(entry),
            _ => {}
        }
    }
    station.keep(notes);
    // the caller's post office: its documents kept, and asked whether it places copies here
    if let Some(domain) = carrier
        .engine()
        .0
        .account()
        .and_then(|a| a.rsplit_once('@'))
        .map(|(_, d)| d.to_lowercase())
    {
        let _ = station.store.change(|s| {
            if !s.learn.contains(&domain) {
                s.learn.push(domain);
            }
        });
    }
    result
}

/// What a forwarding session asks to come unasked: every forward item the partner holds for this
/// gateway, up to a megabyte each (RP-7 §2 `auto`, "as large as it can take").
pub const FORWARD_AUTO: u64 = 1 << 20;

/// How long a call to a partner may take to come up.
pub const CALL_WAIT: Duration = Duration::from_secs(180);

/// The client a cut-off gateway calls `partner` with (RP-7 §2): its own credential and key, the
/// records and messages its store holds for the partner, and the keys it checks placements with.
fn forwarding_client(
    station: &Station,
    partner: &str,
    (at, records): (u64, bool),
    own: &str,
) -> Result<Client, String> {
    let me = station.settings.callsign.clone();
    let (records, outbox, state, trust) = {
        let s = station.store.lock();
        // a call that only collects mail offers nothing (RP-7 §6.2)
        let (records, outbox) = if records {
            crate::forward::outbox(&s, &me, partner)
        } else {
            (Vec::new(), Vec::new())
        };
        let state = s.partners.get(partner).cloned().unwrap_or_default();
        let trust = s.trust(&station.settings.blocked, station.settings.only.as_deref());
        (records, outbox, state, trust)
    };
    let cfg = ClientConfig {
        credential: station.account.credential.clone(),
        device: station.account.key.clone(),
        called: partner.to_owned(),
        time: at,
        nonce: random(),
        outbox,
        cursor: state.cursor,
        auto: FORWARD_AUTO,
        partials: Vec::new(),
        caps: Caps {
            compression: vec![0, 1],
            max_pdu: 4096,
            renew: false,
        },
        credential_cached: state.holds.iter().any(|h| h == own),
        piece: 960,
        station: None,
    };
    let everything = Box::new(|entries: &[WaitingEntry]| entries.iter().map(|e| e.id).collect());
    Ok(Client::new(cfg, everything)
        .map_err(|e| format!("cannot start a forwarding session: {e}"))?
        .forwarding(Forwarding { records, trust }))
}

/// Calls `partner` on `modem`: its `connected`, once the link is up.
fn call_up(modem: &mut Modem, partner: &str, monitor: &Monitor) -> Result<Event, String> {
    let connect = modem.host.connect(partner, None, None);
    let id = connect.get("id").cloned();
    modem.ask(&connect)?;
    let since = Instant::now();
    loop {
        if monitor.stopping() {
            return Err("the operator stopped the gateway".into());
        }
        if since.elapsed() > CALL_WAIT {
            return Err(format!(
                "no link to {partner} in {} seconds",
                CALL_WAIT.as_secs()
            ));
        }
        match modem
            .next()
            .map_err(|e| format!("the modem's adapter: {e}"))?
        {
            Some(e @ Event::Connected { id: Some(_), .. }) if e_id(&e) == id => return Ok(e),
            Some(Event::Failed {
                id: i, code, text, ..
            }) if i == id => {
                return Err(format!(
                    "the call failed: {code}{}",
                    text.map(|t| format!(" ({t})")).unwrap_or_default()
                ));
            }
            _ => {}
        }
    }
}

/// Calls the partner `partner` on `modem` and carries a forwarding session (RP-7 §2): the
/// gateway's queue out, the forward items its partner holds for it in, each event kept in the
/// store as it comes. Returns what it came to.
///
/// # Errors
///
/// The call failed or did not come up, the link broke, or the operator stopped the gateway.
fn forward(
    station: &mut Station,
    modem: &mut Modem,
    partner: &str,
    monitor: &Monitor,
    (records, dial): (bool, Option<u64>),
) -> Result<String, String> {
    // a scan stops for the call and starts again after it, in place of tuning back (W50)
    let scanned = if dial.is_some() {
        modem.stop_scan()?
    } else {
        Vec::new()
    };
    // in the partner's window the radio goes to its channel, and back after (RP-4 §4.1)
    let back = match dial {
        Some(hz) if modem.dial => {
            let back = modem.tune_away(hz)?;
            if back.is_some() {
                monitor.log(&format!(
                    "tuned the radio to {} MHz, {partner}'s channel",
                    crate::rules::mhz(hz)
                ));
            }
            back
        }
        Some(hz) => {
            monitor.log(&format!(
                "calling on the radio's own dial: this modem cannot tune it to {partner}'s \
                 channel, {} MHz",
                crate::rules::mhz(hz)
            ));
            None
        }
        None => None,
    };
    let result = forward_here(station, modem, partner, monitor, records);
    if !scanned.is_empty() {
        let country = station.settings.country.clone();
        match modem.start_scan(&scanned, &country) {
            Ok((dials, _)) => monitor.log(&format!("scanning again: {} MHz", dials_in_mhz(&dials))),
            Err(e) => monitor.log(&format!("could not scan again after the call: {e}")),
        }
    } else if let Some(was) = back {
        match modem.tune_back(was) {
            Ok(()) => monitor.log(&format!(
                "tuned the radio back to {} MHz",
                crate::rules::mhz(was)
            )),
            Err(e) => monitor.log(&format!(
                "could not tune the radio back to {} MHz: {e}",
                crate::rules::mhz(was)
            )),
        }
    }
    result
}

/// [`forward`] with the radio where the call goes.
fn forward_here(
    station: &mut Station,
    modem: &mut Modem,
    partner: &str,
    monitor: &Monitor,
    records: bool,
) -> Result<String, String> {
    let at = now();
    let own = radiopost::cbor::hex(&radiopost::credential::hash(&station.account.credential));
    let client = forwarding_client(station, partner, (at, records), &own)?;
    let connected = call_up(modem, partner, monitor)?;
    let mut carrier = Carrier::new(Calling(client));
    let mut outcome = Outcome::default();
    let mut event = Some(connected);
    // RP-3 §8: from the last bytes, stopped while the modem receives
    let mut silence = Silence::new();
    let result = loop {
        if let Some(e) = event.take() {
            let (requests, events) = carrier.on_event(&mut modem.host, &e);
            let _ = station.store.change(|s| {
                for e in &events {
                    outcome.apply(s, partner, e, at);
                }
            });
            for e in &events {
                monitor.event(e);
            }
            if let Some(err) = requests.iter().find_map(|r| modem.conn.send(r).err()) {
                carrier.engine_mut().link_lost();
                break Err(format!("the modem's adapter: {err}"));
            }
        }
        if carrier.is_ended() {
            break Ok(());
        }
        if monitor.stopping() {
            if let Some(link) = carrier.link() {
                let bye = modem.host.disconnect(link);
                let _ = modem.conn.send(&bye);
            }
            carrier.engine_mut().link_lost();
            break Err("the operator stopped the gateway".to_owned());
        }
        match modem.next() {
            Ok(Some(e)) => {
                silence.heard(&e);
                event = Some(e);
            }
            Ok(None) => {}
            Err(e) => {
                carrier.engine_mut().link_lost();
                break Err(format!("the modem's adapter: {e}"));
            }
        }
        if silence.over(QUIET) {
            if let Some(link) = carrier.link() {
                let bye = modem.host.disconnect(link);
                let _ = modem.conn.send(&bye);
            }
            carrier.engine_mut().link_lost();
            break Err(format!("nothing heard for {} seconds", QUIET.as_secs()));
        }
    };
    let summary = match &result {
        Ok(()) => outcome.summary(partner),
        Err(e) => format!("{}; then {e}", outcome.summary(partner)),
    };
    let _ = station.store.change(|s| {
        let p = s.partners.entry(partner.to_owned()).or_default();
        p.called = at;
        p.said.clone_from(&summary);
        // a partner that welcomed this gateway keeps its credential (RP-3 §5.1)
        if outcome.online.is_some() && !p.holds.contains(&own) {
            p.holds.push(own.clone());
        }
    });
    for line in &outcome.problems {
        monitor.log(line);
    }
    result.map(|()| summary)
}

/// A `connected` event's `id`, when it has one.
fn e_id(e: &Event) -> Option<serde_json::Value> {
    match e {
        Event::Connected { id, .. } => id.clone(),
        _ => None,
    }
}

/// Opens the gateway's modem, calls `partner` once and forwards its queue (RP-7 §2), saying what
/// happens on `say`: the command line's `forward`, for a gateway that is not answering.
///
/// # Errors
///
/// The gateway is not set up, its modem cannot be opened, or the call or the session failed.
pub fn forward_once(dir: &Path, partner: &str, say: &mut dyn FnMut(&str)) -> Result<(), String> {
    let mut station = Station::load(dir)?;
    let monitor = Monitor::new();
    let address = station.settings.modem.clone();
    let mut modem = match open(&station, &address) {
        Ok(m) => m,
        Err(Unopened::Later(e) | Unopened::Never(e)) => return Err(e),
    };
    say(&format!("calling {partner} to forward the queue"));
    let dial = station.window_dial(partner, now());
    let result = forward(&mut station, &mut modem, partner, &monitor, (true, dial));
    for (_, line) in monitor.log_since(0).0 {
        say(&line);
    }
    let off = modem.host.listen(false);
    let _ = modem.conn.send(&off);
    modem.conn.close();
    say(&result?);
    Ok(())
}
