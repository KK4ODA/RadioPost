// SPDX-License-Identifier: MIT OR Apache-2.0
//! The gateway on the air: its directory (settings, key, credential, what it keeps), its modem
//! (any RPMI adapter, or Aether's control API through the Aether adapter in this process), and
//! the loop that answers each call and carries its session to the user's post office.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use radiopost::ErrorCode;
use radiopost::base64url;
use radiopost::crypto::ContentId;
use radiopost::session::gateway::{Gateway, GatewayConfig, Note, Upstream, Upstreams};
use radiopost::session::{Caps, Event as SessionEvent};
use rpmi::host::{Carrier, Direction, Engine, Event, Host};
use rpmi::tcp::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

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
        })
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

    fn upstreams(&mut self) -> Upstreams {
        let account = self.account.clone();
        let station: Vec<(String, String)> =
            self.settings.post_offices.clone().into_iter().collect();
        let blocked = self.settings.blocked.clone();
        let only = self.settings.only.clone();
        let resolver = self.settings.resolver.clone();
        let tokens = Arc::new(Mutex::new(std::mem::take(&mut self.tokens)));
        Box::new(
            move |domain: &str| -> Result<Box<dyn Upstream + Send>, ErrorCode> {
                if blocked.iter().any(|b| b == domain)
                    || only
                        .as_ref()
                        .is_some_and(|o| !o.iter().any(|d| d == domain))
                {
                    return Err(ErrorCode::UnknownPostOffice);
                }
                let url = station
                    .iter()
                    .find(|(d, _)| d == domain)
                    .map(|(_, u)| u.clone())
                    .or_else(|| lookup(&resolver, domain))
                    .ok_or(ErrorCode::UnknownPostOffice)?;
                let token = tokens
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .entry(domain.to_owned())
                    .or_default()
                    .clone();
                Ok(upstream::http(&url, account.clone(), token))
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
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    let mut response = agent
        .get(resolver)
        .query("name", format!("_radiopost.{domain}"))
        .query("type", "TXT")
        .header("Accept", "application/dns-json")
        .call()
        .ok()?;
    let answer: Json = serde_json::from_str(&response.body_mut().read_to_string().ok()?).ok()?;
    answer
        .get("Answer")?
        .as_array()?
        .iter()
        .filter_map(|a| a.get("data")?.as_str())
        .map(|d| {
            d.split('"')
                .filter(|s| !s.trim().is_empty())
                .collect::<String>()
        })
        .filter_map(|txt| radiopost::dns::parse(&txt))
        .find(|t| t.get("t").map(String::as_str) == Some("api"))
        .and_then(|t| t.get("url").cloned())
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
        radiopost_aether::serve::listen("127.0.0.1:0", modem)
            .map_err(|e| format!("cannot start the Aether adapter: {e}"))
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

/// Opens the modem: `hello`, the gateway's callsign, and `listen` on (RP-4 §3, §4).
fn open(station: &Station) -> Result<Modem, String> {
    let at = adapter_for(&station.settings.modem)?;
    let conn = Connection::open(at).map_err(|e| format!("cannot reach the modem at {at}: {e}"))?;
    conn.set_timeout(Some(Duration::from_millis(250)))
        .map_err(|e| e.to_string())?;
    let mut modem = Modem {
        conn,
        host: Host::new(),
    };
    let hello = modem
        .host
        .hello(concat!("radiopost-gateway ", env!("CARGO_PKG_VERSION")));
    let Event::Hello { caps, .. } = modem.ask(&hello)? else {
        return Err("the modem did not say hello".into());
    };
    if !caps.regulatory && !station.settings.operator_checks_rules {
        return Err(
            "this modem does not check the band rules (RP-4 §8): the gateway's operator must, \
                    and says so with `operator_checks_rules` in gateway.json"
                .into(),
        );
    }
    let calls = modem.host.callsigns(&[station.settings.callsign.as_str()]);
    modem.ask(&calls)?;
    let listen = modem.host.listen(true);
    modem.ask(&listen)?;
    Ok(modem)
}

/// Answers calls until the process ends, carrying each session (RP-3 §9).
///
/// # Errors
///
/// The modem cannot be opened when the gateway starts.
pub fn serve(dir: &Path, say: &mut dyn FnMut(&str)) -> Result<(), String> {
    let mut station = Station::load(dir)?;
    let mut modem = open(&station)?;
    say(&format!(
        "listening as {} on {}",
        station.settings.callsign, station.settings.modem
    ));
    loop {
        let event = match modem.next() {
            Ok(Some(e)) => e,
            Ok(None) => continue,
            Err(e) => {
                say(&format!(
                    "the modem's adapter went away ({e}); opening it again"
                ));
                std::thread::sleep(Duration::from_secs(5));
                match open(&station) {
                    Ok(m) => modem = m,
                    Err(e) => say(&format!("cannot open the modem: {e}")),
                }
                continue;
            }
        };
        if let Event::Connected {
            direction: Direction::In,
            remote,
            ..
        } = &event
        {
            say(&format!("{remote} called"));
            match carry(&mut station, &mut modem, &event) {
                Ok(()) => say(&format!("the session with {remote} is over")),
                Err(e) => say(&format!("the session with {remote} ended: {e}")),
            }
        }
    }
}

/// One session, from the link coming up until the caller hangs up or the link is quiet too long.
fn carry(station: &mut Station, modem: &mut Modem, connected: &Event) -> Result<(), String> {
    let country = station.settings.country.clone();
    let engine = Gateway::new(
        station.config(),
        station.upstreams(),
        Box::new(move |call| radiopost::thirdparty::may_hand(call, &country)),
    );
    let mut carrier = Carrier::new(Answering(engine));
    let mut event = connected.clone();
    let mut quiet_since = std::time::Instant::now();
    let result = loop {
        let (requests, _) = carrier.on_event(&mut modem.host, &event);
        if let Some(e) = requests.iter().find_map(|r| modem.conn.send(r).err()) {
            carrier.engine_mut().link_lost();
            break Err(format!("the modem's adapter: {e}"));
        }
        if carrier.is_ended() {
            break Ok(());
        }
        match modem.next() {
            Ok(Some(e)) => {
                quiet_since = std::time::Instant::now();
                event = e;
            }
            Ok(None) if quiet_since.elapsed() < QUIET => {}
            Ok(None) => {
                // RP-3 §8: the station called asks the modem to disconnect
                if let Some(link) = carrier.link() {
                    let bye = modem.host.disconnect(link);
                    let _ = modem.conn.send(&bye);
                }
                carrier.engine_mut().link_lost();
                break Err("nothing heard for 300 seconds".to_owned());
            }
            Err(e) => {
                carrier.engine_mut().link_lost();
                break Err(format!("the modem's adapter: {e}"));
            }
        }
    };
    let notes = carrier.engine_mut().0.take_notes();
    station.keep(notes);
    result
}
