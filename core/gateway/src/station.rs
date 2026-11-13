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
use radiopost::session::gateway::{Gateway, GatewayConfig, Note, Upstream, Upstreams};
use radiopost::session::{Caps, Event as SessionEvent};
use rpmi::host::{Carrier, Direction, Engine, Event, Host};
use rpmi::tcp::Connection;
use serde::{Deserialize, Serialize};

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
                Note::PostOffice(_) => {}
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
}

impl Modem {
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
                Some(e @ (Event::Ok { .. } | Event::Hello { .. })) => return Ok(e),
                _ => {}
            }
        }
        Err("the modem's adapter did not answer".into())
    }
}

/// Why the modem could not be opened.
enum Unopened {
    /// It may open later: it is not running yet, or it refused for now.
    Later(String),
    /// It never will as the gateway is set: the operator must change something.
    Never(String),
}

/// Opens the modem: `hello`, the gateway's callsign, and `listen` on (RP-4 §3, §4).
fn open(station: &Station) -> Result<Modem, Unopened> {
    let at = adapter_for(&station.settings.modem).map_err(Unopened::Never)?;
    let conn = Connection::open(at)
        .map_err(|e| Unopened::Later(format!("cannot reach the modem at {at}: {e}")))?;
    conn.set_timeout(Some(Duration::from_millis(250)))
        .map_err(|e| Unopened::Later(e.to_string()))?;
    let mut modem = Modem {
        conn,
        host: Host::new(),
    };
    let hello = modem
        .host
        .hello(concat!("radiopost-gateway ", env!("CARGO_PKG_VERSION")));
    let Event::Hello { caps, .. } = modem.ask(&hello).map_err(Unopened::Later)? else {
        return Err(Unopened::Later("the modem did not say hello".into()));
    };
    if !caps.regulatory && !station.settings.operator_checks_rules {
        return Err(Unopened::Never(
            "this modem does not check the band rules (RP-4 §8): the gateway's operator must, \
             and says so in the gateway's settings (`operator_checks_rules` in gateway.json)"
                .into(),
        ));
    }
    let calls = modem.host.callsigns(&[station.settings.callsign.as_str()]);
    modem.ask(&calls).map_err(Unopened::Later)?;
    let listen = modem.host.listen(true);
    modem.ask(&listen).map_err(Unopened::Later)?;
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
        match open(station) {
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
    let listening = format!(
        "listening as {} on {}",
        station.settings.callsign, station.settings.modem
    );
    tell(&listening);
    monitor.set_state("listening", None);
    let mut renewal = Renewal::new(dir, RENEW_EVERY, radiopost::credential::RENEW_WITHIN);
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
            return stopped(monitor, &mut tell);
        }
        let event = match modem.next() {
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
                tell(&listening);
                monitor.set_state("listening", None);
                continue;
            }
        };
        if let Event::Connected {
            direction: Direction::In,
            remote,
            ..
        } = &event
        {
            tell(&format!("{remote} called"));
            if station.reload_credential() {
                tell("this session uses the gateway's renewed credential");
            }
            monitor.begin(remote);
            let result = carry(&mut station, &mut modem, &event, monitor);
            match &result {
                Ok(()) => tell(&format!("the session with {remote} is over")),
                Err(e) => tell(&format!("the session with {remote} ended: {e}")),
            }
            monitor.end(&result);
        }
    }
}

/// The loop stopped as the operator asked.
#[allow(clippy::unnecessary_wraps, reason = "what `run` returns, in one place")]
fn stopped(monitor: &Monitor, tell: &mut dyn FnMut(&str)) -> Result<(), String> {
    tell("stopped answering");
    monitor.set_state("stopped", None);
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
    );
    let mut carrier = Carrier::new(Answering(engine));
    // each event is handed to the engine once: a quiet read hands it nothing
    let mut event = Some(connected.clone());
    let mut quiet_since = Instant::now();
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
                quiet_since = Instant::now();
                event = Some(e);
            }
            Ok(None) if quiet_since.elapsed() < QUIET => {}
            Ok(None) => {
                // RP-3 §8: the station called asks the modem to disconnect
                hang_up(&mut carrier, modem);
                break Err("nothing heard for 300 seconds".to_owned());
            }
            Err(e) => {
                carrier.engine_mut().link_lost();
                break Err(format!("the modem's adapter: {e}"));
            }
        }
    };
    let notes = carrier.engine_mut().0.take_notes();
    for note in &notes {
        if let Note::PostOffice(code) = note {
            monitor.post_office_failed(*code);
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
