// SPDX-License-Identifier: MIT OR Apache-2.0
//! The client's panel: the page in `app/ui/` and a JSON API under `/api/`, served on the loopback
//! interface, as the Aether app serves its own. A browser can open it; the desktop application
//! (Tauri) shows the same page in its window.
//!
//! Each request opens the home afresh, so the panel always shows what is on disk: the home of the
//! callsign chosen ([`crate::callsigns`]), which the header's list changes between sessions. A
//! session runs on a thread of its own with a store of its own; while it runs, the panel changes
//! nothing in the home, and the callsign chosen stays, so the two never write over each other.
//! Between sessions, idle or listening for P2P, the panel writes, edits, deletes and marks
//! messages under the activity's lock (`Panel::between_sessions`), which a call that comes takes
//! before its session opens the home: what was written is offered, and nothing written is lost to
//! the session's own save.
//!
//! While it is open, the panel renews every callsign's credential over the Internet in its last
//! month ([`crate::renewal`]): when it starts and every hour, between sessions. Idle, the station
//! is taken as the activity `renewing`; listening, it is not, so a station calling never waits for
//! the post office, and only the keeping is between sessions (`Panel::renew`).
//!
//! While it runs and its modem is Aether, it holds a connection that says who it is and nothing
//! more ([`crate::presence`]), so Aether names the app between calls too. That connection, and a
//! station listening, carry alerts (RP-8, [`crate::alerts`]): the panel lends them its alerts and
//! its activity, which is `alerting` while one goes out, so no call starts meanwhile. The page says which
//! modem (`POST /api/modem`, from its Modem box), as each call and listening does.
//!
//! Only this machine's own pages reach it: a request must name the panel's own host (so a page
//! that rebinds a name to 127.0.0.1 is refused), and a change must carry `X-RadioPost: 1`, which
//! another site's page cannot send without a preflight the panel never grants.

use std::collections::HashMap;
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use radiopost::ContentId;
use radiopost::cbor::Value as Cbor;
use radiopost::credential::PostOfficeKey;
use radiopost::message::{Message, key};
use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, Server};

use radiopost_update::{Product, Watch};

use crate::addresses;
use crate::alerts::{self, Alerts, Level};
use crate::callsigns;
use crate::compose::{self, Answer, Draft, File, media_type};
use crate::online;
use crate::presence::{Carrier, Presence};
use crate::selection::hf::Hf;
use crate::selection::learning::{self, Forget, Forgotten, Learning, Seen};
use crate::selection::position::Position;
use crate::selection::terrain::{self, Terrain};
use crate::selection::{self, attempts, directory};
use crate::session::{self, Fetch, Kind, Options};
use crate::store::{Error, Folder, Store, unhex};

/// The panel's files, built into the program; `--ui DIR` serves them from disk instead.
const FILES: &[(&str, &str, &str)] = &[
    (
        "/icon.svg",
        "image/svg+xml",
        include_str!("../../ui/icon.svg"),
    ),
    (
        "/",
        "text/html; charset=utf-8",
        include_str!("../../ui/index.html"),
    ),
    (
        "/app.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/app.js"),
    ),
    (
        "/style.css",
        "text/css; charset=utf-8",
        include_str!("../../ui/style.css"),
    ),
    (
        "/map.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/map.js"),
    ),
    (
        "/world.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/world.js"),
    ),
    (
        "/terrain.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/terrain.js"),
    ),
];

/// How many gateways, and how many stations, the call box offers under ▾.
const CALLED_KEPT: usize = 10;

/// The panel's requests that reach the post office over the Internet (RP-5 §6).
const ONLINE: [&str; 9] = [
    "/api/enroll",
    "/api/settings",
    "/api/post-office",
    "/api/renew",
    "/api/report",
    "/api/accept",
    "/api/apply",
    "/api/application",
    "/api/application-forget",
];

/// What the station is doing, for the panel.
#[derive(Debug, Default)]
struct Activity {
    /// `idle`, `calling`, `listening`, `session`, `renewing` the credential, or `alerting`: an
    /// alert going out (RP-8), the station taken between calls for it.
    state: &'static str,
    remote: Option<String>,
    /// How the last session ended, when it failed.
    failed: Option<String>,
    /// Listen is on: pressed once, it stays on, and the station goes back to listening after each
    /// session, until it is pressed again ([`session::keep_listening`]). Off while a session runs
    /// means listening stops when it ends.
    listen: bool,
    /// Why the station cannot listen now, while Listen is on: the modem away, tried again.
    held: Option<String>,
    /// Where the modem says the station listens, while Listen is on: its dial, the dials a
    /// gateway on the station scans, and the bandwidths a call is answered in.
    listening_at: Option<session::Where>,
    /// The operator asked the call or session under way to end (**Stop calling**,
    /// **Disconnect**, **Abort**): `disconnect` or `abort`, until it has.
    stopping: Option<&'static str>,
    /// The last call or session ended because the operator said so: `failed` says how, and the
    /// page shows it as stopped, not failed.
    stopped: bool,
    /// What the sessions said, in order.
    log: Vec<String>,
}

/// The panel's server.
#[derive(Clone)]
pub struct Panel {
    /// The first callsign's home, which holds the others' and what is the computer's.
    base: PathBuf,
    ui: Option<PathBuf>,
    activity: Arc<Mutex<Activity>>,
    stop: Arc<AtomicBool>,
    /// The operator's word to end the call or the session under way, handed to each.
    hang_up: session::HangUp,
    addr: SocketAddr,
    updates: Arc<Watch>,
    renewal: Renewal,
    /// The running session's progress, or the last one's.
    live: Arc<Mutex<session::Live>>,
    /// The post office's token for the operator's requests (RP-5 §7), kept between them.
    office_token: Arc<Mutex<Option<String>>>,
    /// The app's connection to Aether between calls.
    presence: Presence,
    /// Alerts between stations that share a keyword (RP-8).
    alerts: Alerts,
    /// Set, the thread that fetches the post office's backups looks now, not at its hour.
    backups_now: Arc<AtomicBool>,
    /// How often that thread looks: [`crate::backups::EVERY`], shorter in tests.
    backups_every: std::time::Duration,
    /// Where a received attachment is saved: [`crate::downloads::folder`], elsewhere in tests.
    downloads: PathBuf,
    /// The places, the GPS and the position in use (choosing a gateway, §6).
    position: Arc<Position>,
    /// Terrain tiles for VHF paths, kept in the cache folder (choosing a gateway, §§8, 9).
    terrain: Arc<Terrain>,
    /// HF channels predicted both ways by VOACAP, the runs kept (choosing a gateway, §10).
    hf: Arc<Hf>,
}

/// When the panel renews the credential: first after `first`, then every `every`, once it has
/// `within` seconds or less to run.
#[derive(Clone, Copy, Debug)]
struct Renewal {
    first: std::time::Duration,
    every: std::time::Duration,
    within: u64,
}

fn lock(a: &Mutex<Activity>) -> MutexGuard<'_, Activity> {
    a.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Why a request that needs the station is refused while it is in `state`.
fn why_busy(state: &str) -> Error {
    match state {
        "renewing" => "renewing the credential: try again in a moment",
        "alerting" => "an alert is going out: try again in a moment",
        "listening" => {
            "the station is listening for P2P calls: click Listening for P2P to stop it first"
        }
        _ => "a session is under way: try again when it ends",
    }
    .into()
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// When each gateway's post office last saw it (RP-5 §4.4 `seen`), by callsign: availability's prior
/// (§12.4).
fn seen_of(gateways: &[Value]) -> Seen {
    gateways
        .iter()
        .filter_map(|g| {
            Some((
                g["callsign"].as_str()?.to_ascii_uppercase(),
                g["seen"].as_u64()?,
            ))
        })
        .collect()
}

/// The recent calls, each marked whether the operator's rules forget its record (§12.9).
fn marked(mut recent: Vec<Value>, records: &[Value], forgotten: &Forgotten) -> Vec<Value> {
    for r in &mut recent {
        let gone = records
            .iter()
            .find(|v| v["id"] == r["id"])
            .is_some_and(|v| forgotten.forgets(v));
        r["forgotten"] = json!(gone);
    }
    recent
}

/// A message's fields as the panel shows them.
fn summary(m: &Message) -> Value {
    let map = m.map();
    let text = |k| map.get(k).and_then(Cbor::as_text).unwrap_or("").to_owned();
    let list = |k| {
        map.get(k)
            .and_then(Cbor::as_array)
            .map(|a| a.iter().filter_map(Cbor::as_text).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    // what the lists mark with a clip: the files it carries, and those its post office holds
    // (RP-6 §2.5), each with their bytes, counted as the reader lists them
    let entries = |k| map.get(k).and_then(Cbor::as_array).unwrap_or_default();
    let carried: Vec<usize> = entries(key::FILES)
        .iter()
        .filter_map(|f| {
            f.get(1)?.as_text()?;
            Some(f.get(3)?.as_bytes()?.len())
        })
        .collect();
    let held: Vec<u64> = entries(key::HELD)
        .iter()
        .filter_map(|f| {
            f.get(1)?.as_text()?;
            f.get(4)?.as_bytes()?;
            Some(f.get(3).and_then(Cbor::as_u64).unwrap_or(0))
        })
        .collect();
    let id = m.content_id().text();
    json!({
        "id": id, "short": &id[..6], "from": text(key::FROM), "to": list(key::TO),
        "cc": list(key::CC), "date": map.get(key::DATE).and_then(Cbor::as_u64),
        "subject": text(key::SUBJECT),
        "priority": map.get(key::PRIORITY).and_then(Cbor::as_u64).unwrap_or(0),
        "attachments": {"carried": carried.len(), "bytes": carried.iter().sum::<usize>(),
                        "held": held.len(), "held_bytes": held.iter().sum::<u64>()},
    })
}

/// The addresses in To or Cc, each with the name typed before it: text as a user types it, or a
/// list of addresses.
fn named(v: &Value) -> Result<Vec<(Option<String>, String)>, Error> {
    match v {
        Value::String(s) => crate::compose::parse_named(s),
        Value::Array(a) => a
            .iter()
            .filter_map(Value::as_str)
            .map(|x| Ok((None, crate::compose::checked_address(x)?)))
            .collect(),
        _ => Ok(Vec::new()),
    }
}

fn plain(named: &[(Option<String>, String)]) -> Vec<String> {
    named.iter().map(|(_, a)| a.clone()).collect()
}

/// The modem the page names, or RP-4 §2's adapter port when it names none.
fn modem_or_default(adapter: &str) -> String {
    if adapter.is_empty() {
        format!("127.0.0.1:{}", rpmi::DEFAULT_PORT)
    } else {
        adapter.to_owned()
    }
}

/// The places kept under the first home `home`; the grid square set there before places existed
/// becomes the first (choosing a gateway, §6.1).
fn position_of(home: &Path) -> Position {
    let position = Position::open(home);
    if let Ok(store) = Store::open(home) {
        position.adopt_grid(store.grid.as_deref());
    }
    position
}

impl Panel {
    /// A panel for the home at `home` and the callsigns it holds, serving its page from `ui` when
    /// given.
    #[must_use]
    pub fn new(home: &Path, ui: Option<PathBuf>) -> Panel {
        Panel {
            base: home.to_owned(),
            ui,
            activity: Arc::new(Mutex::new(Activity {
                state: "idle",
                ..Activity::default()
            })),
            stop: Arc::new(AtomicBool::new(false)),
            hang_up: session::HangUp::new(),
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            updates: Watch::new(
                Product::Tools,
                env!("CARGO_PKG_VERSION"),
                &radiopost_update::channel(),
            ),
            renewal: Renewal {
                first: std::time::Duration::from_secs(5),
                every: crate::renewal::EVERY,
                within: crate::renewal::WITHIN,
            },
            live: Arc::default(),
            office_token: Arc::default(),
            presence: Presence::new(home),
            alerts: Alerts::open(home),
            backups_now: Arc::new(AtomicBool::new(true)),
            backups_every: crate::backups::EVERY,
            downloads: crate::downloads::folder(home),
            position: Arc::new(position_of(home)),
            terrain: Arc::new(Terrain::new(&terrain::cache_dir())),
            hf: Arc::new(Hf::new()),
        }
    }

    /// The same panel with its HF predictions made by `hf`: for tests, whose engine makes them up.
    #[must_use]
    pub fn with_hf(mut self, hf: Hf) -> Panel {
        self.hf = Arc::new(hf);
        self
    }

    /// The same panel with its terrain tiles kept as `terrain` says: for tests, which leave the
    /// computer's cache alone and fetch from a server of their own.
    #[must_use]
    pub fn with_terrain(mut self, terrain: Terrain) -> Panel {
        self.terrain = Arc::new(terrain);
        self
    }

    /// The same panel, saving received attachments in `folder`: for tests, which leave the
    /// user's Downloads folder alone.
    #[must_use]
    pub fn saving_in(mut self, folder: &Path) -> Panel {
        folder.clone_into(&mut self.downloads);
        self
    }

    /// The same panel, looking for a new backup of the post office every `every`: for tests.
    #[must_use]
    pub fn with_backups_every(mut self, every: std::time::Duration) -> Panel {
        self.backups_every = every;
        self
    }

    /// The same panel, renewing first after `first`, then every `every`, once the credential has
    /// `within` seconds or less to run: for tests, which cannot wait a month.
    #[must_use]
    pub fn with_renewal(
        mut self,
        first: std::time::Duration,
        every: std::time::Duration,
        within: u64,
    ) -> Panel {
        self.renewal = Renewal {
            first,
            every,
            within,
        };
        self
    }

    /// The same panel, showing `updates` (the desktop application's own, which installs).
    #[must_use]
    pub fn with_updates(mut self, updates: Arc<Watch>) -> Panel {
        self.updates = updates;
        self
    }

    /// What it shows of updates; [`Watch::watch`] starts asking the channel.
    #[must_use]
    pub fn updates(&self) -> &Arc<Watch> {
        &self.updates
    }

    /// The same panel, trying Aether again after `retry` when it holds no connection between
    /// calls ([`crate::presence`]): for tests, which cannot wait five seconds.
    #[must_use]
    pub fn retrying_aether_every(mut self, retry: std::time::Duration) -> Panel {
        self.presence = self.presence.retrying_every(retry);
        self
    }

    /// A heard alert sounds and waits to be acknowledged (RP-8): the window comes forward. A clone
    /// of the panel kept before [`Panel::start`] reads the running panel's alerts.
    #[must_use]
    pub fn alarming(&self) -> bool {
        self.alerts.alarming()
    }

    /// What the connections holding the modem need to carry alerts: the alerts, the station's
    /// callsigns, and the activity, which they take as `alerting` while one goes out.
    fn carrier(&self) -> Carrier {
        let activity = Arc::clone(&self.activity);
        let gate = move |take: bool| {
            let mut a = lock(&activity);
            match (take, a.state) {
                (true, "idle") => {
                    a.state = "alerting";
                    true
                }
                (true, _) => false,
                (false, "alerting") => {
                    a.state = "idle";
                    true
                }
                (false, _) => true,
            }
        };
        let activity = Arc::clone(&self.activity);
        let say = move |line: &str| {
            radiopost::logfile::line(line);
            lock(&activity).log.push(line.to_owned());
        };
        Carrier {
            alerts: self.alerts.clone(),
            base: self.base.clone(),
            gate: Arc::new(gate),
            say: Arc::new(say),
        }
    }

    /// The alerts for the page, with what its dials are checked against (RP-8 §7): the radio's
    /// dial as the modem last said it (`radio_hz`), and the dials the station's own listed
    /// gateway scans (`scanned_hz`), where the circles' watch dials must be at such a station.
    fn alerts_view(&self) -> Value {
        let mut view = self.alerts.json();
        let mine = crate::alerts::callsigns_of(&self.base);
        let scanned = Store::open(&self.home())
            .map(|store| store.scanned_dials_of(&mine))
            .unwrap_or_default();
        view["radio_hz"] = json!(self.presence.dial());
        view["scanned_hz"] = json!(scanned);
        view
    }

    /// `/api/alerts`: send an alert, acknowledge one heard (or all), or reply to one (RP-8).
    fn alert_request(&self, body: &Value) -> Result<Value, Error> {
        let now = now();
        if let Some(send) = body.get("send") {
            let store = Store::open(&self.home())?;
            let me = store
                .me()
                .map_err(|_| "no credential yet: enroll in Setup first")?;
            let circle = send["circle"].as_str().unwrap_or("");
            let to = send["to"].as_str().filter(|t| !t.trim().is_empty());
            let text = send["text"].as_str().unwrap_or("");
            // the dial it goes on (RP-8 §7), and how long its copies go on: the scan of the
            // station alerted, or of any member's gateway for the whole circle (§5)
            let dial_hz = dial_member(&send["dial_hz"], "dial_hz")?;
            let outlast_s = dial_hz.and_then(|hz| store.outlast_s(to, hz));
            let how = crate::alerts::Sending { dial_hz, outlast_s };
            self.alerts
                .send_on(circle, to, text, &me.operator, now, how)?;
        } else if let Some(report) = body.get("report") {
            self.send_report(report, now)?;
        } else if body["stop_repeat"] == true {
            self.alerts.stop_repeat();
        } else if let Some(tag) = body["ack"].as_str() {
            self.alerts.acknowledge(tag, now)?;
        } else if body["ack_all"] == true {
            self.alerts.acknowledge_all(now);
        } else if let Some(reply) = body.get("reply") {
            let tag = reply["tag"].as_str().unwrap_or("");
            self.alerts
                .reply(tag, reply["text"].as_str().unwrap_or(""), now)?;
        } else {
            return Err("send, report, stop_repeat, ack, ack_all or reply".into());
        }
        Ok(self.alerts_view())
    }

    /// A form sent as a report to a circle (RP-8 §3, §7): `{circle, to?, dial_hz?, form, every_s?,
    /// here}`, `here` the fields whose position is the station's own, which a repeat brings up to
    /// date. A report carries a position: a form with none is refused.
    fn send_report(&self, report: &Value, now: u64) -> Result<(), Error> {
        let store = Store::open(&self.home())?;
        let me = store
            .me()
            .map_err(|_| "no credential yet: enroll in Setup first")?;
        let held = crate::templates::held(store.home());
        let form = form_of_with(&report["form"], &held)?.ok_or("choose a form to report")?;
        let t = held
            .iter()
            .find(|t| t.fingerprint() == form.template)
            .ok_or("this computer does not hold that form")?;
        if radiopost::forms::places(t, &form).is_empty() {
            return Err("a report carries a position: fill in where".into());
        }
        let to = report["to"].as_str().filter(|t| !t.trim().is_empty());
        let dial_hz = dial_member(&report["dial_hz"], "dial_hz")?;
        let here: Vec<u64> = report["here"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default();
        let how = crate::alerts::Sending {
            dial_hz,
            outlast_s: None,
        };
        self.alerts.report(
            report["circle"].as_str().unwrap_or(""),
            to,
            form,
            &me.operator,
            now,
            how,
            report["every_s"].as_u64(),
            here,
        )?;
        Ok(())
    }

    /// Sends the report being repeated again whenever it is due (RP-8 §7), with the station's
    /// position as it is then, for as long as the panel runs: repeats stop with the window.
    fn keep_reporting(&self) {
        loop {
            thread::sleep(std::time::Duration::from_secs(5));
            let now = now();
            if self.alerts.repeat_due(now).is_none() {
                continue;
            }
            let here = self.position.for_form()["text"]
                .as_str()
                .filter(|t| !t.is_empty())
                .and_then(|t| crate::compose::position_from_text(t).ok());
            if let Err(why) = self.alerts.repeat_now(now, here.as_ref()) {
                let line = format!("reports: not sent again: {why}");
                radiopost::logfile::line(&line);
                lock(&self.activity).log.push(line);
            }
        }
    }

    /// `/api/circles`: add a circle from its keyword, change how it arrives, or forget it.
    fn circle_request(&self, body: &Value) -> Result<Value, Error> {
        let level = |v: &Value| match v.as_str() {
            None => Ok(None),
            Some(name) => Level::named(name).map(Some).ok_or_else(|| {
                format!("level: silent, normal, important or time-sensitive, not {name}")
            }),
        };
        if let Some(add) = body.get("add") {
            let name = add["name"].as_str().unwrap_or("");
            let keyword = add["keyword"].as_str().unwrap_or("");
            let level = level(&add["level"])?.unwrap_or(Level::TimeSensitive);
            let watch = dial_member(&add["watch_hz"], "watch_hz")?;
            self.alerts.add_circle(name, keyword, level)?;
            if watch.is_some() {
                self.alerts
                    .set_circle(name.trim(), None, None, Some(watch))?;
            }
        } else if let Some(set) = body.get("set") {
            let name = set["name"].as_str().unwrap_or("");
            // the watch dial: a number sets it, null clears it, absent leaves it
            let watch = match set.get("watch_hz") {
                None => None,
                Some(v) => Some(dial_member(v, "watch_hz")?),
            };
            self.alerts
                .set_circle(name, level(&set["level"])?, set["answer"].as_bool(), watch)?;
        } else if let Some(name) = body["remove"].as_str() {
            self.alerts.remove_circle(name)?;
        } else {
            return Err("add, set or remove a circle".into());
        }
        Ok(self.alerts_view())
    }

    /// The program is ending: the connection between calls closes, so Aether says the app has
    /// gone, rather than learning it from a connection cut at the exit. A clone of the panel kept
    /// before [`Panel::start`] ends the running panel's.
    pub fn leave(&self) {
        self.presence.stop();
    }

    /// Serves on `addr` (loopback; port 0 picks one) on a thread of its own; returns the address.
    ///
    /// # Errors
    ///
    /// The address cannot be taken.
    pub fn start(mut self, addr: &str) -> Result<SocketAddr, Error> {
        let server = Server::http(addr).map_err(|e| format!("cannot listen on {addr}: {e}"))?;
        let local = server
            .server_addr()
            .to_ip()
            .ok_or("the panel listens on an IP address")?;
        if !local.ip().is_loopback() {
            return Err("the panel listens only on the loopback interface".into());
        }
        self.addr = local;
        self.presence.carry(self.carrier());
        self.presence.start();
        let renewing = self.clone();
        thread::spawn(move || renewing.keep_renewed());
        let reporting = self.clone();
        thread::spawn(move || reporting.keep_reporting());
        let backing_up = self.clone();
        thread::spawn(move || backing_up.keep_backed_up());
        thread::spawn(move || {
            for request in server.incoming_requests() {
                self.handle(request);
            }
        });
        Ok(local)
    }

    /// The home of the callsign chosen.
    fn home(&self) -> PathBuf {
        callsigns::chosen(&self.base)
    }

    /// Renews each callsign's credential over the Internet whenever it is due, for as long as the
    /// panel runs: a callsign not chosen for months still keeps its credential. A failure is said
    /// once until it changes: with no Internet, the hourly try is quiet.
    fn keep_renewed(&self) {
        let mut said: HashMap<u32, String> = HashMap::new();
        thread::sleep(self.renewal.first);
        loop {
            for c in callsigns::list(&self.base) {
                if let Some(line) = self.renew_now(&c.home, said.entry(c.id).or_default()) {
                    radiopost::logfile::line(&line);
                    lock(&self.activity).log.push(line);
                }
            }
            self.hand_in_templates();
            thread::sleep(self.renewal.every);
        }
    }

    /// Fetches the post office's new backups onto this computer whenever it is turned on, for as
    /// long as the panel runs: at start, every [`crate::backups::EVERY`], and when asked. A
    /// failure is said once until it changes. It reads the first home's credential and writes
    /// nothing a session writes, so it goes on during a session too.
    fn keep_backed_up(&self) {
        let mut said = String::new();
        let step = std::time::Duration::from_millis(200);
        loop {
            let mut waited = std::time::Duration::ZERO;
            while waited < self.backups_every && !self.backups_now.swap(false, Ordering::Relaxed) {
                thread::sleep(step);
                waited += step;
            }
            let result = Store::open(&self.base).and_then(|store| {
                crate::backups::fetch_if_new(&store, &self.office_token, &self.base, now())
            });
            let line = match result {
                Ok(Some(line)) => {
                    said.clear();
                    line
                }
                Ok(None) => continue,
                Err(why) if why == said => continue,
                Err(why) => {
                    said.clone_from(&why);
                    format!(
                        "could not fetch the post office's backup: {why}; trying again every hour \
                         while this window is open"
                    )
                }
            };
            radiopost::logfile::line(&line);
            lock(&self.activity).log.push(line);
        }
    }

    /// The post office's backups kept on this computer: turned on or off (`on`, and `computer`,
    /// the name the desk shows), or what is known of them.
    fn backups(&self, method: &Method, path: &str, body: &Value) -> Result<Value, Error> {
        if *method == Method::Get {
            return Ok(crate::backups::status(&self.base));
        }
        if path == "/api/backups/fetch" {
            self.backups_now.store(true, Ordering::Relaxed);
            return Ok(crate::backups::status(&self.base));
        }
        let store = Store::open(&self.base)?;
        let answer = if body["on"].as_bool().unwrap_or(false) {
            let computer = body["computer"]
                .as_str()
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map_or_else(crate::backups::computer_name, str::to_owned);
            let into = body["folder"]
                .as_str()
                .map(str::trim)
                .filter(|f| !f.is_empty())
                .map(PathBuf::from);
            let on = crate::backups::turn_on(
                &store,
                &self.office_token,
                &self.base,
                &computer,
                into.as_deref(),
            )?;
            self.backups_now.store(true, Ordering::Relaxed);
            on
        } else {
            crate::backups::turn_off(&store, &self.office_token, &self.base)?
        };
        Ok(answer)
    }

    /// One try for the credential at `home`: what to say of it, if anything (`said` is what was
    /// said of its last failure). Only while the station is idle or listening ([`Panel::renew`]):
    /// during a call or a session it waits for the next hour.
    fn renew_now(&self, home: &Path, said: &mut String) -> Option<String> {
        let Ok(me) = Store::open(home).and_then(|s| s.me()) else {
            return None;
        };
        let now = now();
        if !crate::renewal::due(me.expires, now, self.renewal.within) {
            return None;
        }
        let Ok(result) = self.renew(home) else {
            return None;
        };
        match result {
            Ok(issued) => {
                said.clear();
                Some(format!(
                    "renewed the credential over the Internet: {issued}"
                ))
            }
            Err(why) if *said == why => None,
            Err(why) => {
                let line = format!(
                    "could not renew the credential, which runs out in {} days, for {} ({}): \
                     {why}; trying again every hour, and on the next gateway call",
                    crate::renewal::days_left(me.expires, now),
                    me.address,
                    me.operator
                );
                *said = why;
                Some(line)
            }
        }
    }

    /// Renews the credential at `home` over the Internet: refused (the outer error, saying why)
    /// while a call, a session, an alert or another renewal holds the station; otherwise the
    /// try's line, or why it failed.
    ///
    /// The post office may take seconds to answer, so no lock is held while it does. Idle, the
    /// station is taken as `renewing`, as before, so that nothing starts meanwhile. Listening (a
    /// house station may listen for days), it is not taken: a station that calls is answered at
    /// once. Only the keeping, one file renamed into place, waits for no session to hold the home
    /// ([`Panel::keep_credential`]), so a session and a renewal never write a home together.
    fn renew(&self, home: &Path) -> Result<Result<String, Error>, Error> {
        let took = {
            let mut a = lock(&self.activity);
            match a.state {
                "idle" => {
                    a.state = "renewing";
                    true
                }
                "listening" => false,
                other => return Err(why_busy(other)),
            }
        };
        let result = Store::open(home)
            .and_then(|store| online::renewal(&store))
            .and_then(|bytes| self.keep_credential(home, bytes));
        if took {
            lock(&self.activity).state = "idle";
        }
        Ok(result)
    }

    /// Keeps a renewed credential at `home`: now, under the activity's lock, when no call or
    /// session holds the home; otherwise when it ends, by a thread of its own, so that neither the
    /// renewal nor the page waits for a session. What to say of it.
    fn keep_credential(&self, home: &Path, bytes: Vec<u8>) -> Result<String, Error> {
        let holds = |state: &str| matches!(state, "calling" | "session");
        {
            let a = lock(&self.activity);
            if !holds(a.state) {
                let kept = Store::open(home)?.set_credential(&bytes)?;
                drop(a);
                return Ok(online::described(&kept));
            }
        }
        let body = radiopost::credential::Credential::decode(&bytes)
            .ok_or("the post office's answer is not a credential")?
            .body;
        let (activity, home) = (Arc::clone(&self.activity), home.to_owned());
        thread::spawn(move || {
            loop {
                thread::sleep(std::time::Duration::from_millis(200));
                let mut a = lock(&activity);
                if holds(a.state) {
                    continue;
                }
                if let Err(why) = Store::open(&home).and_then(|s| s.set_credential(&bytes)) {
                    let line = format!("could not keep the renewed credential: {why}");
                    radiopost::logfile::line(&line);
                    a.log.push(line);
                }
                return;
            }
        });
        Ok(format!(
            "{}, kept when the session under way ends",
            online::described(&body)
        ))
    }

    fn handle(&self, mut request: Request) {
        let host = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Host"))
            .map(|h| h.value.as_str().to_owned())
            .unwrap_or_default();
        let ours = [
            format!("127.0.0.1:{}", self.addr.port()),
            format!("localhost:{}", self.addr.port()),
        ];
        if !ours.contains(&host) {
            let _ = request
                .respond(Response::from_string("not this panel's host").with_status_code(403));
            return;
        }
        let url = request.url().to_owned();
        let (path, query) = url.split_once('?').unwrap_or((&url, ""));
        let method = request.method().clone();
        if method == Method::Get && !path.starts_with("/api/") {
            let _ = request.respond(self.file(path));
            return;
        }
        let marked = request
            .headers()
            .iter()
            .any(|h| h.field.equiv("X-RadioPost") && h.value.as_str() == "1");
        if method == Method::Post && !marked {
            let _ = request.respond(
                Response::from_string("a change needs X-RadioPost: 1").with_status_code(403),
            );
            return;
        }
        if method == Method::Get && path == "/api/terrain/hillshade" {
            let _ = request.respond(self.hillshade(query));
            return;
        }
        let mut body = String::new();
        let _ = request.as_reader().take(32 << 20).read_to_string(&mut body);
        let body: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        let answer = self.api(&method, path, query, &body);
        let (code, value) = match answer {
            Ok(v) => (200, v),
            Err(e) => (400, json!({"error": e})),
        };
        let response = Response::from_string(value.to_string())
            .with_status_code(code)
            .with_header(Header::from_bytes("Content-Type", "application/json").expect("a header"))
            .with_header(Header::from_bytes("Cache-Control", "no-store").expect("a header"));
        let _ = request.respond(response);
    }

    /// The terrain map's background (`GET /api/terrain/hillshade?south=&west=&north=&east=&width=`),
    /// a PNG drawn from the tiles held: the one answer of the API that is not JSON.
    fn hillshade(&self, query: &str) -> Response<std::io::Cursor<Vec<u8>>> {
        let number = |name: &str| {
            query
                .split('&')
                .find_map(|kv| kv.strip_prefix(name)?.strip_prefix('='))
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(f64::NAN)
        };
        let area = selection::hillshade::Area::new(
            number("south"),
            number("west"),
            number("north"),
            number("east"),
        );
        let width = number("width");
        match area {
            Ok(area) if (1.0..=f64::from(selection::hillshade::MAX_PIXELS)).contains(&width) => {
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "1 to MAX_PIXELS, checked above"
                )]
                let png = self.terrain.hillshade(area, width as u32);
                Response::from_data(png.as_slice())
                    .with_header(Header::from_bytes("Content-Type", "image/png").expect("a header"))
                    .with_header(Header::from_bytes("Cache-Control", "no-store").expect("a header"))
            }
            Ok(_) => Response::from_string("width: 1 to 1024 pixels").with_status_code(400),
            Err(why) => Response::from_string(why).with_status_code(400),
        }
    }

    fn file(&self, path: &str) -> Response<std::io::Cursor<Vec<u8>>> {
        let Some((name, kind, built_in)) = FILES.iter().find(|(p, _, _)| *p == path) else {
            return Response::from_string("not found").with_status_code(404);
        };
        let text = self
            .ui
            .as_ref()
            .and_then(|dir| {
                let file = if *name == "/" {
                    "index.html"
                } else {
                    &name[1..]
                };
                std::fs::read_to_string(dir.join(file)).ok()
            })
            .unwrap_or_else(|| (*built_in).to_owned());
        Response::from_string(text)
            .with_header(Header::from_bytes("Content-Type", *kind).expect("a header"))
            .with_header(Header::from_bytes("Cache-Control", "no-store").expect("a header"))
    }

    /// What the station is doing: `idle`, `calling`, `listening` or `session`. A clone of the
    /// panel kept before [`Panel::start`] reads the running panel's state.
    #[must_use]
    pub fn activity(&self) -> &'static str {
        lock(&self.activity).state
    }

    /// Refused unless the station is idle: what uses the modem (a call, listening, the dial, the
    /// tuner tone), what changes who the station is (Setup), what reaches the post office for
    /// long, and an update, which restarts the program.
    fn busy(&self) -> Result<(), Error> {
        match lock(&self.activity).state {
            "idle" => Ok(()),
            other => Err(why_busy(other)),
        }
    }

    /// The home held for a quick change of the messages (written, edited, deleted, marked to
    /// fetch or ask for) while no session can begin: the activity's lock, taken while the station
    /// is idle or listening between sessions, and held until the change is on disk. A call that
    /// comes meanwhile waits for it: [`Panel::listen`]'s `Called` takes the same lock and sets
    /// `session` before [`session::keep_listening`] opens the home for the call, and a call from
    /// the panel takes the station under it too. So what was written is offered to the caller,
    /// and the session's own save, which writes the folders whole, keeps it. Refused, saying why,
    /// while a call, a session, a renewal or an alert holds the station.
    fn between_sessions(&self) -> Result<MutexGuard<'_, Activity>, Error> {
        let a = lock(&self.activity);
        match a.state {
            "idle" | "listening" => Ok(a),
            other => Err(why_busy(other)),
        }
    }

    /// [`Panel::between_sessions`] for an edit of an outbox message, saying why the message may
    /// be going out now: a session holds the outbox until it ends, and sends from it.
    fn editing_between_sessions(&self) -> Result<MutexGuard<'_, Activity>, Error> {
        let a = lock(&self.activity);
        match a.state {
            "idle" | "listening" => Ok(a),
            "calling" | "session" => Err(
                "this message cannot be changed while a session is under way: it may be going \
                 out now. Edit it when the session ends"
                    .into(),
            ),
            other => Err(why_busy(other)),
        }
    }

    /// The header's list of callsigns: `choose` one by its `id`, `add` a home for another (chosen,
    /// to be enrolled from Setup), or `remove` one that holds no mail. Only while the station is
    /// idle, decided under the activity's lock, so no call or listening starts on one callsign
    /// while the choice moves to another. The post office's token for the operator's desk was the
    /// last credential's: it goes.
    fn callsigns(&self, body: &Value) -> Result<Value, Error> {
        let id = |k: &str| {
            body[k]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| format!("{k}: which callsign, by its id"))
        };
        {
            let a = lock(&self.activity);
            match a.state {
                "idle" => {}
                "renewing" => return Err("renewing a credential: try again in a moment".into()),
                "alerting" => return Err("an alert is going out: try again in a moment".into()),
                _ => return Err("a session is under way: change callsign when it ends".into()),
            }
            if !body["choose"].is_null() {
                callsigns::choose(&self.base, id("choose")?)?;
            } else if body["add"] == true {
                callsigns::add(&self.base)?;
            } else if !body["remove"].is_null() {
                callsigns::remove(&self.base, id("remove")?)?;
            } else {
                return Err("choose, add or remove a callsign".into());
            }
        }
        *self
            .office_token
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        self.state()
    }

    #[allow(clippy::too_many_lines, reason = "the panel's routes, one arm a route")]
    fn api(&self, method: &Method, path: &str, query: &str, body: &Value) -> Result<Value, Error> {
        let param = |name: &str| {
            query
                .split('&')
                .find_map(|kv| kv.strip_prefix(name)?.strip_prefix('='))
                .unwrap_or("")
                .to_owned()
        };
        let text = |name: &str| body[name].as_str().unwrap_or("").trim().to_owned();
        match (method, path) {
            (Method::Get, "/api/state") => self.state(),
            (Method::Get, "/api/forms") => Ok(forms_json(&self.home())),
            (Method::Post, "/api/forms/import") => self.import_template(body),
            (Method::Post, "/api/forms/export") => self.export_template(&text("template")),
            (Method::Post, "/api/forms/remove") => {
                let gone = crate::templates::remove(&self.home(), &text("template"))?;
                Ok(json!({"removed": gone.name}))
            }
            (Method::Post, "/api/forms/fetch") => {
                let store = Store::open(&self.home())?;
                let from = text("from");
                let t = crate::templates::fetch(
                    &store,
                    &text("template"),
                    Some(from.as_str()).filter(|f| !f.is_empty()),
                )?;
                Ok(template_json(&t))
            }
            (Method::Get, "/api/form-position") => Ok(self.position.for_form()),
            (Method::Get, "/api/reports") => self.reports_view(),
            (Method::Post, "/api/reports/export") => self.export_reports(body),
            (Method::Post, "/api/reports/map") => {
                // the desktop app's window opens no other site: the system's browser does
                let link = crate::reports::osm_link(
                    body["lat"].as_f64().ok_or("lat: a latitude")?,
                    body["lon"].as_f64().ok_or("lon: a longitude")?,
                    body["within"].as_u64(),
                )?;
                crate::reports::open_in_browser(&link)?;
                Ok(json!({"opened": link}))
            }
            (Method::Post, "/api/form-position") => form_position(&text("text")),
            (Method::Get, "/api/settings") => {
                let store = Store::open(&self.home())?;
                online::settings(&store, None)
            }
            (Method::Get, "/api/folder") => self.folder(&param("name")),
            (Method::Get, "/api/message") => self.message(&param("id")),
            (Method::Get, "/api/answer") => self.answer(&param("id"), &param("how")),
            (Method::Get, "/api/edit") => self.edit(&param("id")),
            (Method::Get, "/api/log") => {
                let since: usize = param("since").parse().unwrap_or(0);
                let a = lock(&self.activity);
                Ok(json!({"lines": a.log.get(since..).unwrap_or(&[]), "next": a.log.len()}))
            }
            (Method::Post, "/api/callsigns") => self.callsigns(body),
            (Method::Post, "/api/compose") => self.compose(body),
            (Method::Post, "/api/held") => self.ask_for_held(body),
            (Method::Post, "/api/save") => self.save_file(body),
            (_, "/api/addresses") => self.addresses(method, body),
            (Method::Post, "/api/delete" | "/api/restore" | "/api/purge" | "/api/empty-trash") => {
                self.trash(path, body)
            }
            (Method::Post, "/api/key" | "/api/credential" | "/api/trust") => {
                self.identity(path, body)
            }
            (Method::Post, "/api/call") => self.call(body),
            (Method::Post, "/api/listen") => {
                self.listen(&text("adapter"), dial_of(&body["dial_hz"])?)
            }
            (Method::Post, "/api/tune") => {
                let dial = dial_of(&body["dial_hz"])?.ok_or("dial_hz: which dial")?;
                self.tune(&text("adapter"), dial)
            }
            (Method::Post, "/api/tone") => {
                let on = body["on"].as_bool().ok_or("on: true or false")?;
                self.tone(&text("adapter"), on)
            }
            (Method::Post, "/api/modem") => {
                // the page's Modem box, when it loads and when it changes: the app's connection
                // between calls follows it (only to Aether)
                self.presence.set_modem(&modem_or_default(&text("adapter")));
                Ok(json!({}))
            }
            (_, "/api/gateways" | "/api/grid" | "/api/waiting" | "/api/fetch") => {
                self.gateway_side(method, path, body, param("at").parse().ok())
            }
            (Method::Post, "/api/gateway-marks") => {
                let store = Store::open(&self.home())?;
                let listed: Vec<String> = store
                    .gateway_list
                    .as_ref()
                    .and_then(|l| l["gateways"].as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|g| g["callsign"].as_str().map(str::to_owned))
                    .collect();
                directory::request(&self.base, body, &listed)?;
                self.gateways(param("at").parse().ok())
            }
            (Method::Get, "/api/attempts") => Ok(self.attempts(param("limit").parse().ok())),
            (Method::Get, "/api/called") => Ok(self.called()),
            (Method::Get, "/api/learned") => self.learned(param("callsign").trim()),
            (Method::Get, "/api/p2p-chance") => self.p2p_chance(
                param("callsign").trim(),
                param("dial_hz").parse().ok(),
                param("bandwidth").parse().unwrap_or(2300),
            ),
            (_, "/api/forget") => self.forget(method, body),
            (Method::Post, p) if ONLINE.contains(&p) => self.online(path, body),
            (Method::Post, "/api/office") => self.office(body),
            (Method::Post, "/api/office-backup") => self.office_backup(body),
            (_, "/api/backups" | "/api/backups/fetch") => self.backups(method, path, body),
            (Method::Post, "/api/terminal") => {
                let target = body["target"].as_str().unwrap_or("").trim();
                Ok(json!({"opened": crate::terminal::open(target)?}))
            }
            (Method::Post, "/api/update") => self.update(),
            (Method::Post, "/api/update/check") => {
                // on a thread of its own: this panel answers one request at a time
                self.updates.check_now();
                self.state()
            }
            (Method::Get, "/api/alerts") => Ok(self.alerts_view()),
            (Method::Post, "/api/alerts") => self.alert_request(body),
            (Method::Post, "/api/circles") => self.circle_request(body),
            (_, p) if p.starts_with("/api/terrain") => self.terrain_api(method, p, &param, body),
            (Method::Get, "/api/places") => Ok(self.position.json(param("ports") == "1")),
            (Method::Post, "/api/places") => self.position.request(body),
            (Method::Get, "/api/notify") => Ok(alerts::notify_settings(&self.base)),
            (Method::Post, "/api/notify") => {
                alerts::set_notify_settings(&self.base, body)?;
                Ok(alerts::notify_settings(&self.base))
            }
            (Method::Post, "/api/stop") => Ok(self.stop_listening()),
            (Method::Post, "/api/hang-up") => self.hang_up_asked(body),
            _ => Err(format!("no {method} {path}")),
        }
    }

    /// A call from the page: whom, a gateway or a station, what to fetch, and the channel, how
    /// long its tries must last and the bandwidth.
    fn call(&self, body: &Value) -> Result<Value, Error> {
        let text = |name: &str| body[name].as_str().unwrap_or("").trim().to_owned();
        let callsign = text("callsign").to_ascii_uppercase();
        if callsign.is_empty() {
            return Err("whom to call".into());
        }
        let kind = if body["station"].as_bool().unwrap_or(true) {
            Kind::Station
        } else {
            Kind::Gateway
        };
        // a P2P call to a listed gateway's callsign goes nowhere useful: the page is told whom to
        // call instead, and offers it
        if kind == Kind::Station {
            let store = Store::open(&self.home())?;
            if let (Some((gateway, instead)), Some(why)) = (
                store.gateway_named(&callsign),
                store.not_a_station(&callsign),
            ) {
                return Ok(
                    json!({"not_a_station": {"gateway": gateway, "call": instead,
                                                   "why": why}}),
                );
            }
        }
        let fetch = match &body["fetch"] {
            Value::Null => Fetch::All,
            Value::String(f) if f == "all" => Fetch::All,
            Value::String(f) if f == "none" => Fetch::Nothing,
            Value::Number(n) => Fetch::UpTo(n.as_u64().ok_or("fetch: a size in bytes")?),
            _ => return Err("fetch: all, none or a size in bytes".into()),
        };
        // a gateway's channel, chosen from its record, or a station call's dial (W14): the
        // radio goes there if it is not there already
        let dial = dial_of(&body["dial_hz"])?;
        // on a channel a gateway scans, how long the call must keep trying (RP-5 §4.4)
        let call_s = match &body["call_s"] {
            Value::Null => None,
            v => Some(v.as_u64().ok_or("call_s: seconds, 0 or above")?),
        };
        // the call's bandwidth (RP-4 §4 `bandwidth`): a station call's, as the two operators
        // agreed, or the gateway channel's, as its record lists it (RP-5 §4.4), which the call
        // leaves out if the modem cannot be asked for it
        let bandwidth = match (&body["bandwidth"], kind) {
            (Value::Null, _) => None,
            (v, Kind::Station) => Some(
                v.as_i64()
                    .filter(|b| [500, 2300].contains(b))
                    .ok_or("bandwidth: 500 or 2300 Hz")?,
            ),
            (v, Kind::Gateway) => Some(
                v.as_i64()
                    .filter(|b| *b > 0)
                    .ok_or("bandwidth: the channel's, in Hz")?,
            ),
        };
        self.run(
            callsign,
            kind,
            &text("adapter"),
            fetch,
            (dial, call_s, bandwidth),
        )
    }

    /// Setup's device key (made once), a credential imported by hand, and a post office trusted:
    /// the callsign chosen's.
    fn identity(&self, path: &str, body: &Value) -> Result<Value, Error> {
        let text = |name: &str| body[name].as_str().unwrap_or("").trim().to_owned();
        self.busy()?;
        let store = Store::open(&self.home())?;
        match path {
            "/api/key" => {
                if store.device().is_err() {
                    let mut seed = [0u8; 32];
                    getrandom::fill(&mut seed).map_err(|e| e.to_string())?;
                    store.set_device(&ed25519_dalek::SigningKey::from_bytes(&seed))?;
                }
                self.state()
            }
            "/api/credential" => {
                let bytes = unhex(&text("hex")).ok_or("paste the credential in hex")?;
                store.device()?;
                store.set_credential(&bytes)?;
                self.state()
            }
            "/api/trust" => {
                let public: [u8; 32] = unhex(&text("public"))
                    .and_then(|b| b.try_into().ok())
                    .ok_or("a post office key is 32 bytes in hex")?;
                let domain = text("domain");
                if domain.is_empty() {
                    return Err("the post office's domain".into());
                }
                store.add_trust(
                    &domain,
                    &PostOfficeKey {
                        public,
                        not_before: body["not_before"].as_u64().unwrap_or(0),
                        not_after: body["not_after"].as_u64().unwrap_or(u64::MAX),
                    },
                )?;
                self.state()
            }
            _ => Err(format!("no POST {path}")),
        }
    }

    /// The post office over the Internet (RP-5 §6): enrollment, the post office's address,
    /// renewal, reports and the accept list.
    fn online(&self, path: &str, body: &Value) -> Result<Value, Error> {
        let text = |name: &str| body[name].as_str().unwrap_or("").trim().to_owned();
        match path {
            "/api/enroll" => {
                self.busy()?;
                let mut store = Store::open(&self.home())?;
                let url = text("url");
                let said = online::enroll(
                    &mut store,
                    (!url.is_empty()).then_some(url.as_str()),
                    &text("address"),
                    &text("operator"),
                    &text("code"),
                )?;
                Ok(json!({"said": said}))
            }
            "/api/post-office" => {
                self.busy()?;
                let mut store = Store::open(&self.home())?;
                let url = text("url");
                store.post_office = (!url.is_empty()).then(|| url.trim_end_matches('/').to_owned());
                store.save()?;
                self.state()
            }
            "/api/renew" => Ok(json!({"said": self.renew(&self.home())??})),
            "/api/report" => {
                self.busy()?;
                let mut store = Store::open(&self.home())?;
                Ok(json!({"lines": online::send_reports(&mut store)?}))
            }
            "/api/apply" => {
                self.busy()?;
                let mut store = Store::open(&self.home())?;
                let url = text("url");
                let said = online::apply(
                    &mut store,
                    (!url.is_empty()).then_some(url.as_str()),
                    &text("address"),
                    &text("operator"),
                    &text("name"),
                    &text("contact"),
                )?;
                Ok(json!({"said": said}))
            }
            "/api/application" => {
                self.busy()?;
                let mut store = Store::open(&self.home())?;
                online::check_application(&mut store)
            }
            "/api/application-forget" => {
                self.busy()?;
                let mut store = Store::open(&self.home())?;
                store.application = None;
                store.save()?;
                Ok(json!({}))
            }
            "/api/settings" => {
                self.busy()?;
                let store = Store::open(&self.home())?;
                // RP-5 §6.3: only what Setup changes, merged into the rest
                let mut change = serde_json::Map::new();
                if let Some(list) = body["usual_gateways"].as_array() {
                    let calls: Vec<Value> = list
                        .iter()
                        .filter_map(|c| c.as_str().map(|c| c.trim().to_uppercase()))
                        .filter(|c| !c.is_empty())
                        .map(Value::from)
                        .collect();
                    change.insert("usual_gateways".into(), Value::Array(calls));
                }
                if let Some(skip) = body["radio_skips_read"].as_bool() {
                    change.insert("radio_skips_read".into(), skip.into());
                }
                online::settings(&store, Some(&Value::Object(change)))
            }
            "/api/accept" => {
                let store = Store::open(&self.home())?;
                let list = |k: &str| -> Vec<String> {
                    body[k]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|e| e.as_str().map(|e| e.trim().to_owned()))
                        .collect()
                };
                Ok(json!({"entries": online::accept_list(&store, &list("add"), &list("remove"))?}))
            }
            _ => Err(format!("no POST {path}")),
        }
    }

    /// The operator's desk (RP-5 §7): one request to the post office under `/office/`, as this
    /// device's credential, answered with the post office's status and body. The post office
    /// decides who may: an operator's credential, issued by it, not a gateway's.
    fn office(&self, body: &Value) -> Result<Value, Error> {
        let store = Store::open(&self.home())?;
        let method = body["method"].as_str().unwrap_or("GET").to_uppercase();
        let path = body["path"].as_str().unwrap_or("");
        let sent = body.get("body").filter(|b| !b.is_null());
        let (status, answer) = online::office(&store, &self.office_token, &method, path, sent)?;
        Ok(json!({"status": status, "body": answer}))
    }

    /// A received message's attachment saved as a file in the Downloads folder, under a safe
    /// name and never over another file ([`crate::downloads`]): `{id, n}`, `n` counted from 0 as
    /// the message lists its files. Answers where it went.
    fn save_file(&self, body: &Value) -> Result<Value, Error> {
        let store = Store::open(&self.home())?;
        let id = store.find(body["id"].as_str().unwrap_or(""))?;
        let n = body["n"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or("n: which attachment, counted from 0")?;
        let path = crate::downloads::save(&store.message(&id)?, n, &self.downloads)?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(json!({"path": path.display().to_string(), "name": name}))
    }

    /// A template from a file the page read (`{data}`, its bytes in base64url): its JSON or its
    /// CBOR, kept for every callsign of this computer and handed in to each one's post office
    /// by a thread of its own (RP-5 §6.8), since the post office may be slow or out of reach.
    fn import_template(&self, body: &Value) -> Result<Value, Error> {
        let data = body["data"]
            .as_str()
            .and_then(radiopost::base64url::decode)
            .ok_or("data: the file's bytes in base64url")?;
        let t = crate::templates::import(&self.home(), &data)?;
        let panel = self.clone();
        thread::spawn(move || panel.hand_in_templates());
        Ok(template_json(&t))
    }

    /// A template written to the Downloads folder as its JSON, to give to someone, under a new
    /// name: `{template}`, its fingerprint.
    fn export_template(&self, fingerprint: &str) -> Result<Value, Error> {
        let t = crate::templates::held(&self.home())
            .into_iter()
            .find(|t| crate::templates::fingerprint(t) == fingerprint)
            .ok_or("no such form here")?;
        let name = format!("{}.json", t.name);
        let text = crate::templates::exported(&t);
        let path = crate::downloads::write_new(&self.downloads, &name, text.as_bytes())?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(json!({"path": path.display().to_string(), "name": name}))
    }

    /// Hands every template kept in to each callsign's post office that has not had it (RP-5
    /// §6.8), saying what went. A post office out of reach is tried again with the next hour's
    /// renewal, quietly.
    fn hand_in_templates(&self) {
        for c in callsigns::list(&self.base) {
            let Ok(store) = Store::open(&c.home) else {
                continue;
            };
            if let Ok(done) = crate::templates::hand_in(&store)
                && !done.is_empty()
            {
                let me = store.me().map(|m| m.address).unwrap_or_default();
                let line = format!(
                    "handed in {} form(s) to the post office of {me}",
                    done.len()
                );
                radiopost::logfile::line(&line);
                lock(&self.activity).log.push(line);
            }
        }
    }

    /// The reports (the forms design, §4.3): every form received or sent that carries a position,
    /// newest first ([`crate::reports`]).
    fn reports_view(&self) -> Result<Value, Error> {
        let store = Store::open(&self.home())?;
        let held = crate::templates::held(store.home());
        let mut all = crate::reports::gather(&store, &held);
        all.extend(
            self.alerts
                .reports()
                .iter()
                .filter_map(|r| crate::reports::of_circle(r, &held)),
        );
        all.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| a.id.cmp(&b.id)));
        Ok(Value::Array(
            all.iter().map(crate::reports::Report::json).collect(),
        ))
    }

    /// The reports named, written to the Downloads folder as CSV or `GeoJSON`, under a new name:
    /// `{format: "csv" | "geojson", ids}`, each a message's content ID as the reports give it.
    fn export_reports(&self, body: &Value) -> Result<Value, Error> {
        let store = Store::open(&self.home())?;
        let held = crate::templates::held(store.home());
        let mut all = crate::reports::gather(&store, &held);
        all.extend(
            self.alerts
                .reports()
                .iter()
                .filter_map(|r| crate::reports::of_circle(r, &held)),
        );
        let ids: Vec<&str> = body["ids"]
            .as_array()
            .ok_or("ids: the reports to write")?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let chosen: Vec<&crate::reports::Report> = all
            .iter()
            .filter(|r| ids.contains(&r.id.as_str()))
            .collect();
        if chosen.is_empty() {
            return Err("none of those reports is here".into());
        }
        let day = radiopost::forms::ymd(now() / 86_400);
        let (name, data) = match body["format"].as_str() {
            Some("csv") => (
                format!("RadioPost reports {day}.csv"),
                crate::reports::csv(&chosen).into_bytes(),
            ),
            Some("geojson") => (
                format!("RadioPost reports {day}.geojson"),
                serde_json::to_vec_pretty(&crate::reports::geojson(&chosen))
                    .map_err(|e| e.to_string())?,
            ),
            _ => return Err("format: csv or geojson".into()),
        };
        let path = crate::downloads::write_new(&self.downloads, &name, &data)?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(json!({"path": path.display().to_string(), "name": name, "count": chosen.len()}))
    }

    /// A backup of the post office sealed with the operator's passphrase, fetched over the desk
    /// and kept on this computer, in `RadioPost backups` in the user's home folder.
    fn office_backup(&self, body: &Value) -> Result<Value, Error> {
        let store = Store::open(&self.home())?;
        let sent = json!({"passphrase": body["passphrase"].as_str().unwrap_or("")});
        // the mailboxes go with it: past ten megabytes, a smaller limit read no backup at all
        let (status, answer) = online::office_reading(
            &store,
            &self.office_token,
            "POST",
            "/office/backup",
            Some(&sent),
            1 << 30,
        )?;
        if status != 200 {
            return Err(answer["title"]
                .as_str()
                .unwrap_or("the post office refused")
                .to_owned());
        }
        let data = answer["data"]
            .as_str()
            .and_then(radiopost::base64url::decode)
            .ok_or("the post office's answer held no backup")?;
        let name: String = answer["name"]
            .as_str()
            .unwrap_or("post-office.rpbackup")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            .collect();
        let folder = crate::backups::folder(&self.base);
        std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        let path = folder.join(name);
        std::fs::write(&path, &data).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(json!({"path": path.display().to_string(), "bytes": data.len()}))
    }

    fn message(&self, prefix: &str) -> Result<Value, Error> {
        let store = Store::open(&self.home())?;
        let id = store.find(prefix)?;
        let m = store.message(&id)?;
        let map = m.map();
        let mut v = summary(&m);
        v["body"] = map
            .get(key::BODY)
            .and_then(Cbor::as_text)
            .unwrap_or("")
            .into();
        // a form shows as the form: its rows, label and value, made from the template this client
        // holds (RP-2 §8.4), which the page lays out; its text too, as an email would have it
        if let Some(form) = radiopost::forms::Form::of(&m) {
            let templates = crate::templates::held(store.home());
            let name = templates
                .iter()
                .find(|t| t.fingerprint() == form.template)
                .map(|t| t.name.clone());
            let shown = radiopost::forms::shown_as(&m, &templates);
            let rows: Vec<Value> = shown
                .iter()
                .flat_map(|s| &s.rows)
                .map(|r| {
                    let table = r
                        .table
                        .as_ref()
                        .map(|t| json!({"columns": t.columns, "rows": t.rows}));
                    json!({"n": r.n, "label": r.label, "value": r.value, "lines": r.lines,
                           "heading": r.heading, "table": table})
                })
                .collect();
            v["form"] = json!({
                "template": radiopost::cbor::hex(&form.template),
                "name": name,
                "title": shown.as_ref().map(|s| s.title.clone()),
                "filled": shown.as_ref().is_some_and(|s| s.filled),
                "rows": rows,
                "text": shown.map(|s| s.text(&m)),
            });
        }
        v["files"] = map
            .get(key::FILES)
            .and_then(Cbor::as_array)
            .into_iter()
            .flatten()
            .map(|f| {
                json!({"name": f.get(1).and_then(Cbor::as_text),
                       "type": f.get(2).and_then(Cbor::as_text),
                       "size": f.get(3).and_then(Cbor::as_bytes).map_or(0, <[u8]>::len)})
            })
            .collect();
        let serial = store
            .inbox
            .iter()
            .find(|a| a.id == id)
            .and_then(|a| a.serial);
        v["held"] = Store::held(&m)
            .into_iter()
            .map(|(name, kind, size, r)| {
                let asked = serial.is_some_and(|s| store.files.contains(&(s, r)));
                json!({"name": name, "type": kind, "size": size, "asked": asked})
            })
            .collect();
        Ok(v)
    }

    /// RP-3 §5.6: asks, on the next call to a gateway, for a received message's held attachments,
    /// those named or all of them.
    fn ask_for_held(&self, body: &Value) -> Result<Value, Error> {
        let _held = self.between_sessions()?;
        let mut store = Store::open(&self.home())?;
        let id = store.find(body["id"].as_str().unwrap_or(""))?;
        let names: Vec<String> = body["names"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|n| n.as_str().map(str::to_owned))
            .collect();
        let asked = store.ask_for_held(&id, &names)?;
        Ok(json!({"asked": asked}))
    }

    /// W1: what replying to the message `prefix` names, replying to all, or forwarding it starts
    /// with, for the composer to fill in; the original comes too, for the composer to show.
    fn answer(&self, prefix: &str, how: &str) -> Result<Value, Error> {
        let how = Answer::named(how)?;
        let store = Store::open(&self.home())?;
        let me = store.me()?;
        let id = store.find(prefix)?;
        if how != Answer::Forward && !store.was_received(&id) {
            return Err(
                "only a message received is replied to; forward passes any message on".into(),
            );
        }
        let original = store.message(&id)?;
        let held = crate::templates::held(store.home());
        let started = compose::answer(&original, &me.address, how, &held);
        let serial = store
            .inbox
            .iter()
            .find(|a| a.id == id)
            .and_then(|a| a.serial);
        let asked: Vec<String> = Store::held(&original)
            .into_iter()
            .filter(|(.., r)| serial.is_some_and(|s| store.files.contains(&(s, r.clone()))))
            .map(|(name, ..)| name)
            .collect();
        let mut v = summary(&original);
        v["body"] = original
            .map()
            .get(key::BODY)
            .and_then(Cbor::as_text)
            .unwrap_or("")
            .into();
        // what the composer shows of it, for reference: a form's text, the note below it
        v["text"] = compose::text_of(&original, &held).into();
        Ok(json!({
            "how": how.name(), "to": started.to, "cc": started.cc, "subject": started.subject,
            "body": started.body,
            "reply_to": started.reply_to.map(|_| id.text()),
            "files": started.files.iter()
                .map(|f| json!({"name": f.name, "type": f.media_type, "size": f.data.len()}))
                .collect::<Vec<_>>(),
            "held": started.held.iter()
                .map(|h| json!({"name": h.name, "type": h.media_type, "size": h.size,
                                "asked": asked.contains(&h.name)}))
                .collect::<Vec<_>>(),
            // held attachments are asked for only of a message the post office brought
            "can_ask": serial.is_some(),
            "original": v,
        }))
    }

    /// What editing the outbox message `prefix` names starts with, for the composer: its fields
    /// and text, its files, the message it answers, and its form's template and values as the
    /// composer gives them. Refused, saying why, for a message that cannot be changed now: one a
    /// session may be sending, or one handed over ([`Store::edit_refusal`]).
    fn edit(&self, prefix: &str) -> Result<Value, Error> {
        drop(self.editing_between_sessions()?);
        let store = Store::open(&self.home())?;
        let id = store.find(prefix)?;
        if let Some(why) = store.edit_refusal(&id) {
            return Err(why);
        }
        let m = store.message(&id)?;
        let map = m.map();
        let mut v = summary(&m);
        v["body"] = map
            .get(key::BODY)
            .and_then(Cbor::as_text)
            .unwrap_or("")
            .into();
        v["files"] = compose::files_of(&m)
            .iter()
            .map(|f| json!({"name": f.name, "type": f.media_type, "size": f.data.len()}))
            .collect();
        v["reply_to"] = map
            .get(key::REPLY_TO)
            .and_then(Cbor::as_bytes)
            .and_then(|b| <[u8; 12]>::try_from(b).ok())
            .map(|b| ContentId(b).text())
            .into();
        v["form"] = radiopost::forms::Form::of(&m).map_or(Value::Null, |form| {
            let values: serde_json::Map<String, Value> = form
                .values
                .iter()
                .filter_map(|(n, x)| Some((n.to_string(), json_of(x)?)))
                .collect();
            json!({"template": radiopost::cbor::hex(&form.template), "values": values})
        });
        Ok(v)
    }

    /// A message into the outbox. Besides what the user wrote, `reply_to` names the message it
    /// answers (its content ID or the start of it), and `forward` the message it passes on, whose
    /// files it carries: those whose places `carry` lists, or all of them (W1).
    ///
    /// With `edit`, it is that outbox message changed, and takes its place ([`Store::replace`]):
    /// it keeps the files of it that `carry` lists (all of them when it lists none) and answers
    /// what it answered, and is refused, saying why, for one that cannot be changed now.
    ///
    /// Between sessions only, idle or listening, the home held until the message is on disk
    /// ([`Panel::between_sessions`]): a station that calls while Listen for P2P is on takes it.
    fn compose(&self, body: &Value) -> Result<Value, Error> {
        let text = |name: &str| body[name].as_str().unwrap_or("").trim().to_owned();
        let editing = body["edit"].as_str().filter(|p| !p.is_empty());
        let _held = if editing.is_some() {
            self.editing_between_sessions()?
        } else {
            self.between_sessions()?
        };
        let mut store = Store::open(&self.home())?;
        let me = store.me()?;
        let edited = match editing {
            Some(prefix) => {
                let id = store.find(prefix)?;
                if let Some(why) = store.edit_refusal(&id) {
                    return Err(why);
                }
                Some((id, store.message(&id)?))
            }
            None => None,
        };
        let forward = body["forward"].as_str().filter(|p| !p.is_empty());
        if edited.is_some() && forward.is_some() {
            return Err("an edit keeps the message's own files: it forwards nothing".into());
        }
        let carry: Option<Vec<u64>> = body["carry"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_u64).collect());
        let carried = match (&edited, forward) {
            (Some((_, old)), _) => compose::files_of(old),
            (None, Some(prefix)) => compose::files_of(&store.message(&store.find(prefix)?)?),
            (None, None) => Vec::new(),
        };
        let mut files: Vec<File> = carried
            .into_iter()
            .enumerate()
            .filter_map(|(i, f)| {
                let i = u64::try_from(i).ok()?;
                carry.as_ref().is_none_or(|c| c.contains(&i)).then_some(f)
            })
            .collect();
        for f in body["files"].as_array().into_iter().flatten() {
            let name = f["name"].as_str().unwrap_or("file").to_owned();
            let data = f["data"]
                .as_str()
                .and_then(rpmi::base64::decode)
                .ok_or("an attachment's data is base64")?;
            files.push(File {
                media_type: media_type(&name).into(),
                name,
                data,
            });
        }
        let reply_to = match (&edited, body["reply_to"].as_str().filter(|p| !p.is_empty())) {
            // an edit answers what the message answered, even once that is deleted for good
            (Some((_, old)), _) => old
                .map()
                .get(key::REPLY_TO)
                .and_then(Cbor::as_bytes)
                .and_then(|b| <[u8; 12]>::try_from(b).ok()),
            (None, Some(prefix)) => Some(store.find(prefix)?.0),
            (None, None) => None,
        };
        let (to, cc) = (named(&body["to"])?, named(&body["cc"])?);
        let draft = Draft {
            from: me.address,
            to: plain(&to),
            cc: plain(&cc),
            date: now(),
            subject: text("subject"),
            body: body["body"].as_str().unwrap_or("").to_owned(),
            files,
            priority: body["priority"].as_u64().unwrap_or(0),
            reply_to,
            form: form_of_with(&body["form"], &crate::templates::held(store.home()))?,
        };
        let message = draft.message()?;
        let (id, was) = match &edited {
            Some((old, _)) => (store.replace(old, &message)?, Some(old.text())),
            None => (store.queue(&message)?, None),
        };
        // the address list learns whom the message is for (an edit, only whom it adds); a
        // failure there loses no message
        let before: Vec<String> = edited.as_ref().map_or_else(Vec::new, |(_, old)| {
            [key::TO, key::CC]
                .iter()
                .filter_map(|k| old.map().get(*k).and_then(Cbor::as_array))
                .flatten()
                .filter_map(Cbor::as_text)
                .map(compose::address)
                .collect()
        });
        let learned: Vec<_> = [to, cc]
            .concat()
            .into_iter()
            .filter(|(_, a)| !before.contains(a))
            .collect();
        if let Err(why) = addresses::remember(&self.base, &learned, now()) {
            radiopost::logfile::line(&format!("the address list was not updated: {why}"));
        }
        Ok(json!({"id": id.text(), "replaced": was}))
    }

    fn state(&self) -> Result<Value, Error> {
        let store = Store::open(&self.home())?;
        let me = store
            .me()
            .ok()
            // shown as it goes in From (RP-0 §6.2), whatever case the credential was issued in
            .map(|b| {
                json!({"address": radiopost::address::lowercase(&b.address),
                       "operator": b.operator, "expires": b.expires})
            });
        let device = store
            .device()
            .ok()
            .map(|k| radiopost::cbor::hex(&k.verifying_key().to_bytes()));
        let mut trusted: Vec<String> = store.trust()?.keys.into_keys().collect();
        trusted.sort();
        let a = lock(&self.activity);
        Ok(json!({
            "me": me, "device": device, "trusted": trusted, "post_office": store.post_office,
            "application": store.application,
            "callsigns": callsigns::list(&self.base).iter().map(callsigns::Entry::json)
                .collect::<Vec<_>>(),
            "counts": {"outbox": store.outbox.len(), "inbox": store.inbox.len(),
                       "sent": store.sent.len(), "trash": store.trash.len(),
                       "reports": store.reports.len(),
                       "files": store.files.len(), "waiting": store.waiting_entries().len()},
            "activity": {"state": a.state, "remote": a.remote, "failed": a.failed,
                         "listen": a.listen, "held": a.held, "stopping": a.stopping,
                         "stopped": a.stopped,
                         "listening_at": a.listening_at.as_ref().map(session::Where::to_json)},
            "alerts": self.alerts.summary(),
            "progress": self.live.lock().unwrap_or_else(PoisonError::into_inner).summary(),
            "update": self.updates.status(),
            "log": a.log.len(),
        }))
    }

    fn folder(&self, name: &str) -> Result<Value, Error> {
        let store = Store::open(&self.home())?;
        if name == "trash" {
            let mut out = Vec::new();
            for t in store.trash.iter().rev() {
                let mut v = summary(&store.message(&t.was.id())?);
                v["was"] = t.was.folder().name().into();
                v["deleted"] = t.at.into();
                out.push(v);
            }
            return Ok(Value::Array(out));
        }
        let entries: Vec<(ContentId, Option<&str>)> = match name {
            "outbox" => store.outbox.iter().map(|w| (w.id, None)).collect(),
            "sent" => store.sent.iter().rev().map(|id| (*id, None)).collect(),
            "inbox" => store
                .inbox
                .iter()
                .rev()
                .map(|a| (a.id, Some(a.station.as_str())))
                .collect(),
            _ => return Err(format!("no folder {name}")),
        };
        let mut out = Vec::new();
        let at = now();
        for (id, station) in entries {
            let mut v = summary(&store.message(&id)?);
            if let Some(s) = station {
                v["station"] = s.into();
            }
            if name != "inbox" {
                // RP-7 §3.6: written, queued at a cut-off gateway, in doubt, or sent
                v["state"] = store.state_of(&id, at).into();
                if let Some(q) = store.queued_for(&id) {
                    v["queued"] = json!({"gateway": q.gateway, "at": q.at});
                }
            }
            out.push(v);
        }
        Ok(Value::Array(out))
    }

    /// The address list and its groups, each group with its members written as To takes them.
    fn address_book(&self) -> Value {
        let groups: Vec<Value> = addresses::groups(&self.base)
            .iter()
            .map(|g| {
                let written: Vec<Value> = addresses::members(&self.base, &g.name)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(name, address)| json!({"name": name, "address": address}))
                    .collect();
                json!({"name": g.name, "members": written})
            })
            .collect();
        let entries: Vec<Value> = addresses::list(&self.base)
            .iter()
            .map(addresses::Entry::json)
            .collect();
        json!({"entries": entries, "groups": groups})
    }

    /// The address list, the computer's for every callsign (it is a file of its own, so it
    /// changes during a session too): `add` typed text, `remove` an address, or `put_back` an
    /// entry removed; its groups (wishlist W2): `group_save` (`was`, `name`, `members` as typed),
    /// `group_remove` and `group_put_back`; and `import` a contact file's text.
    fn addresses(&self, method: &Method, body: &Value) -> Result<Value, Error> {
        let entries = |list: Vec<addresses::Entry>| -> Value {
            list.iter().map(addresses::Entry::json).collect()
        };
        if *method == Method::Get {
            return Ok(self.address_book());
        }
        if let Some(g) = body.get("group_save") {
            addresses::save_group(
                &self.base,
                g["was"].as_str(),
                g["name"].as_str().unwrap_or(""),
                g["members"].as_str().unwrap_or(""),
            )?;
            return Ok(self.address_book());
        }
        if let Some(name) = body["group_remove"].as_str() {
            let (gone, _) = addresses::remove_group(&self.base, name)?;
            let mut book = self.address_book();
            book["removed_group"] = gone.json();
            return Ok(book);
        }
        if let Some(g) = body.get("group_put_back") {
            let group = addresses::Group {
                name: g["name"].as_str().ok_or("a group's name")?.to_owned(),
                members: g["members"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|m| m.as_str().map(str::to_owned))
                    .collect(),
            };
            addresses::put_back_group(&self.base, group)?;
            return Ok(self.address_book());
        }
        if let Some(text) = body["import"].as_str() {
            let done = addresses::import(&self.base, text)?;
            let mut book = self.address_book();
            book["imported"] = done.json();
            return Ok(book);
        }
        if let Some(typed) = body["add"].as_str() {
            return Ok(json!({"entries": entries(addresses::add(&self.base, typed)?)}));
        }
        if let Some(address) = body["remove"].as_str() {
            let (gone, left) = addresses::remove(&self.base, address)?;
            return Ok(json!({"entries": entries(left), "removed": gone.json()}));
        }
        let e = &body["put_back"];
        let entry = addresses::Entry {
            address: e["address"]
                .as_str()
                .ok_or("add, remove or put_back")?
                .to_owned(),
            name: e["name"].as_str().map(str::to_owned),
            used: e["used"].as_u64().unwrap_or(0),
            uses: e["uses"].as_u64().unwrap_or(0),
        };
        Ok(json!({"entries": entries(addresses::put_back(&self.base, entry)?)}))
    }

    /// The trash: a message moved there from a folder (`delete`, `folder` and `id`), put back
    /// (`restore`) or deleted for good (`purge`, `id` and `was`, the folder it came from), and
    /// the trash emptied. Not while a session runs: it keeps the folders until it ends. Between
    /// sessions, listening too, the home is held until the change is on disk.
    fn trash(&self, path: &str, body: &Value) -> Result<Value, Error> {
        let _held = self.between_sessions()?;
        let mut store = Store::open(&self.home())?;
        let id = || {
            let text = body["id"].as_str().unwrap_or("");
            ContentId::from_text(text).ok_or_else(|| format!("{text:?} is not a content ID"))
        };
        let folder = |name: &str| Folder::named(body[name].as_str().unwrap_or(""));
        match path {
            "/api/delete" => store.delete(folder("folder")?, &id()?, now())?,
            "/api/restore" => store.restore(&id()?, folder("was")?)?,
            "/api/purge" => store.purge(&id()?, folder("was")?)?,
            _ => {
                let n = store.empty_trash()?;
                return Ok(json!({"deleted": n}));
            }
        }
        Ok(json!({}))
    }

    /// Starts a call (`remote`) or listening (`None`) on a thread of its own.
    /// The gateways the post office lists, the operator's grid, what waited at the last call to
    /// a gateway, and the messages marked to fetch on the next.
    fn gateway_side(
        &self,
        method: &Method,
        path: &str,
        body: &Value,
        at: Option<u64>,
    ) -> Result<Value, Error> {
        let text = |name: &str| body[name].as_str().unwrap_or("").trim().to_owned();
        match (method, path) {
            (Method::Get, "/api/gateways") => self.gateways(at),
            (Method::Post, "/api/gateways") => {
                self.busy()?;
                let mut store = Store::open(&self.home())?;
                online::gateways(&mut store)?;
                self.gateways(at)
            }
            (Method::Post, "/api/grid") => {
                let held = self.between_sessions()?;
                let mut store = Store::open(&self.home())?;
                store.set_grid(Some(&text("grid")))?;
                drop(held);
                self.gateways(at)
            }
            (Method::Get, "/api/waiting") => {
                let store = Store::open(&self.home())?;
                let w = store.waiting.clone().unwrap_or(Value::Null);
                Ok(json!({"via": w["via"], "at": w["at"], "entries": store.waiting_entries()}))
            }
            (Method::Post, "/api/fetch") => {
                let _held = self.between_sessions()?;
                let mut store = Store::open(&self.home())?;
                let serial = body["serial"].as_u64().ok_or("which message: its serial")?;
                store.fetch_on_next_call(serial, body["on"].as_bool().unwrap_or(true))?;
                Ok(json!({"entries": store.waiting_entries()}))
            }
            _ => Err(format!("no {method} {path}")),
        }
    }

    /// Installs the update on offer, through the desktop application; or opens its page for a
    /// download by hand. An update restarts the program: not while a session is under way.
    fn update(&self) -> Result<Value, Error> {
        self.busy()?;
        if self.updates.status()["can_install"] == true {
            self.updates.install()?;
        } else {
            self.updates.download()?;
        }
        self.state()
    }

    /// The gateways kept from the post office's list and those of the post offices it knows
    /// (RP-5 §4.4), nearest first, and the grid they are ranked from; which post offices' lists
    /// were taken, which could not be, and the callsigns listed twice. `at`, seconds since 1970:
    /// another time to judge them for (§§10.6, 14.7), as HF predictions and listening times go;
    /// `None`, now.
    fn gateways(&self, at: Option<u64>) -> Result<Value, Error> {
        self.gateways_judged(true, at)
    }

    /// The gateway list, each VHF gateway's path over the terrain judged and each HF channel
    /// predicted when `terrain` says, at `at` or now.
    fn gateways_judged(&self, terrain: bool, at: Option<u64>) -> Result<Value, Error> {
        let store = Store::open(&self.home())?;
        let list = store.gateway_list.clone().unwrap_or(Value::Null);
        let others: Vec<&String> = list["lists"]
            .as_object()
            .map(|l| l.keys().collect())
            .unwrap_or_default();
        let here = self.position.here();
        let from = here.here.as_ref().map(|h| (h.lat, h.lon));
        let (marks, marks_error) = match directory::Marks::load(&self.base) {
            Ok(m) => (m, None),
            Err(why) => (directory::Marks::default(), Some(why)),
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut gateways = directory::annotate(
            store.gateways_ranked_from(from),
            &list,
            &marks,
            &attempts::read(&self.base),
            from,
            now,
        );
        // what the call records taught (§ 12), worked out again only when they or the rules changed
        let learning = terrain.then(|| Learning::cached(&self.base, &seen_of(&gateways), now));
        // another time (§ 14.7): whether each listens then; VHF paths do not change with it
        let when = at.unwrap_or(now);
        // a VHF channel's path over the terrain, from the tiles already held (§ 8)
        let (spot, station) = self.position.for_terrain();
        for g in gateways.iter_mut().filter(|_| terrain) {
            if let Some(channel) = terrain::vhf_channel(g) {
                let mut t = self.terrain.judge(spot, &station, g, &channel, None, false);
                let learned = learning.as_ref().and_then(|l| {
                    let dial = channel["dial_hz"].as_u64()?;
                    l.for_line(g["callsign"].as_str()?, &learning::channel_of(dial)?, when)
                });
                if let Some(l) = learned {
                    terrain::apply_learned(&mut t, l);
                }
                t["dial_hz"] = channel["dial_hz"].clone();
                g["terrain"] = t;
            }
        }
        if at.is_some() {
            for g in &mut gateways {
                directory::listening_at(g, when);
            }
        }
        // each HF channel both ways at the hour, VOACAP's runs kept for the month (§ 10)
        if terrain {
            let (here, station, bands) = self.position.for_hf();
            let when = i64::try_from(when).unwrap_or(i64::MAX);
            self.hf.judge(
                here,
                &station,
                &bands,
                &mut gateways,
                (when, at.is_none()),
                learning.as_ref(),
            );
        }
        Ok(json!({
            "domain": list["domain"], "fetched": list["fetched"], "grid": store.grid,
            "gateways": gateways, "others": others,
            "errors": list["errors"], "ambiguous": list["ambiguous"],
            "here": here.here, "note": here.note, "marks_error": marks_error,
            "hf": if terrain { self.hf.status() } else { Value::Null }, "at": at,
            "learning": learning.map(|l| json!({
                "health": l.health, "records": l.records, "forgotten": l.forgotten,
                "direction": l.direction,
            })),
        }))
    }

    /// `GET /api/p2p-chance?callsign=&dial_hz=&bandwidth=`: a P2P call's chance on the dial
    /// picked, predicted as the gateway listed under the station's base callsign would be on it
    /// (its site is the station's, as KK4ODA-1's is KK4ODA's), with the best of that gateway's
    /// own channels on another band beside it when it does better. Null when no gateway of that
    /// base is listed. Three P2P calls on 40 m at about 95 km, an hour and a half after sunset,
    /// were heard by nobody (2026-11-23): 40 m had closed for so short a path, and nothing said so.
    fn p2p_chance(
        &self,
        callsign: &str,
        dial: Option<u64>,
        bandwidth: u64,
    ) -> Result<Value, Error> {
        let Some(dial) = dial.filter(|&d| d > 0) else {
            return Ok(Value::Null);
        };
        let store = Store::open(&self.home())?;
        let Some(gateway) = store
            .gateway_list
            .as_ref()
            .and_then(|l| l["gateways"].as_array())
            .and_then(|list| {
                list.iter().find(|g| {
                    g["callsign"]
                        .as_str()
                        .is_some_and(|c| radiopost::callsign::same_base(c, callsign))
                })
            })
            .cloned()
        else {
            return Ok(Value::Null);
        };
        let mut channels = vec![json!({"dial_hz": dial, "bandwidth_hz": bandwidth,
                                       "modem": "aether"})];
        channels.extend(gateway["channels"].as_array().cloned().unwrap_or_default());
        let mut judged = [gateway];
        judged[0]["channels"] = Value::Array(channels);
        let (here, station, bands) = self.position.for_hf();
        let when = i64::try_from(now()).unwrap_or(i64::MAX);
        self.hf
            .judge(here, &station, &bands, &mut judged, (when, true), None);
        let g = &judged[0];
        let chosen = g["channels"][0]["hf"].clone();
        let chance = |c: &Value| c["hf"]["chance"].as_f64();
        let better = g["channels"]
            .as_array()
            .into_iter()
            .flatten()
            .skip(1)
            .filter(|c| c["hf"]["band"] != chosen["band"])
            .filter(|c| chance(c) > chosen["chance"].as_f64())
            .max_by(|a, b| {
                chance(a)
                    .partial_cmp(&chance(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|c| json!({"dial_hz": c["dial_hz"], "hf": c["hf"]}));
        Ok(json!({"gateway": g["callsign"], "chosen": chosen, "better": better}))
    }

    /// `GET /api/learned?callsign=`: a gateway's details (§16.4): for each of its channels, what the
    /// model says now, what was learned and what was measured in the last 30 days; its
    /// availability through the day, its service, its last ten calls, and this station's health.
    fn learned(&self, callsign: &str) -> Result<Value, Error> {
        if callsign.is_empty() {
            return Err("which gateway: its callsign".into());
        }
        let judged = self.gateways_judged(true, None)?;
        let listed = judged["gateways"].as_array().cloned().unwrap_or_default();
        let gw = listed.iter().find(|g| {
            g["callsign"]
                .as_str()
                .is_some_and(|c| c.eq_ignore_ascii_case(callsign))
        });
        let now = now();
        let l = Learning::cached(&self.base, &seen_of(&listed), now);
        let records = attempts::read(&self.base);
        let forgotten = Forgotten::load(&self.base);
        // its channels: those listed, with what the model says of each now, then any learned of
        // that it no longer lists
        let mut channels: Vec<(String, Value, Value)> = Vec::new();
        for c in gw
            .and_then(|g| g["channels"].as_array())
            .into_iter()
            .flatten()
        {
            let Some(name) = c["dial_hz"].as_u64().and_then(learning::channel_of) else {
                continue;
            };
            let model = if c.get("hf").is_some() {
                json!({"hf": c["hf"]})
            } else if gw.is_some_and(|g| g["terrain"]["dial_hz"] == c["dial_hz"]) {
                json!({"terrain": gw.map(|g| g["terrain"].clone())})
            } else {
                Value::Null
            };
            if !channels.iter().any(|(n, ..)| *n == name) {
                channels.push((name, model, c.clone()));
            }
        }
        let prefix = learning::key(callsign, "");
        for k in l.channels.keys().filter(|k| k.starts_with(&prefix)) {
            let name = k[prefix.len()..].to_owned();
            if !channels.iter().any(|(n, ..)| *n == name) {
                channels.push((name, Value::Null, Value::Null));
            }
        }
        let upper = callsign.to_ascii_uppercase();
        let channels: Vec<Value> = channels
            .into_iter()
            .map(|(name, model, channel)| {
                json!({
                    "channel": name, "listed": channel, "model": model,
                    "learned": l.channel(callsign, &name),
                    "measured": learning::measured(&records, &forgotten, (callsign, &name), now),
                })
            })
            .collect();
        let mine: Vec<Value> = records
            .iter()
            .filter(|r| {
                r["remote"]
                    .as_str()
                    .is_some_and(|c| c.eq_ignore_ascii_case(callsign))
            })
            .cloned()
            .collect();
        Ok(json!({
            "callsign": gw.map_or_else(|| json!(upper), |g| g["callsign"].clone()),
            "gateway": gw, "channels": channels,
            "availability": l.availability.get(&upper), "service": l.service.get(&upper),
            "recent": marked(directory::recent(&mine, 10), &mine, &forgotten),
            "health": l.health, "direction": l.direction,
            "forgotten": forgotten.rules.iter().any(|r| matches!(r,
                Forget::Gateway { callsign: c, .. } if c.eq_ignore_ascii_case(callsign))),
        }))
    }

    /// `GET /api/attempts`: the last `limit` calls (100 unsaid), each marked whether it is
    /// forgotten.
    fn attempts(&self, limit: Option<usize>) -> Value {
        let records = attempts::read(&self.base);
        let forgotten = Forgotten::load(&self.base);
        let recent = directory::recent(&records, limit.unwrap_or(100));
        json!({"recent": marked(recent, &records, &forgotten)})
    }

    /// `GET /api/called`: whom the station called, by kind, once each and newest first, for the
    /// call box (`directory::called`). The gateway list is read with the operator's marks but not
    /// judged (no terrain, no predictions): the box asks for it whenever it opens its list.
    fn called(&self) -> Value {
        let listed = self
            .gateways_judged(false, None)
            .ok()
            .and_then(|v| v["gateways"].as_array().cloned())
            .unwrap_or_default();
        directory::called(&attempts::read(&self.base), &listed, CALLED_KEPT)
    }

    /// `/api/forget`: the operator's forgetting rules (§12.9), `GET` to list them with the counts,
    /// `POST` `{"what": "gateway", "callsign"}`, `{"what": "place", "km"}` (around the position in
    /// use), `{"what": "before", "before"}` or `{"what": "everything"}` to add one, or
    /// `{"undo": n}` to take the n-th back, its records counting again.
    fn forget(&self, method: &Method, body: &Value) -> Result<Value, Error> {
        let mut rules = Forgotten::load(&self.base);
        if *method == Method::Post {
            let now = now();
            if let Some(n) = body["undo"].as_u64() {
                let n = usize::try_from(n).map_err(|_| "no such rule")?;
                if n >= rules.rules.len() {
                    return Err("no such rule".into());
                }
                rules.rules.remove(n);
            } else {
                let rule = match body["what"].as_str().unwrap_or("") {
                    "gateway" => Forget::Gateway {
                        callsign: body["callsign"]
                            .as_str()
                            .filter(|c| !c.trim().is_empty())
                            .ok_or("which gateway: its callsign")?
                            .trim()
                            .to_ascii_uppercase(),
                        at: now,
                    },
                    "place" => {
                        let here = self.position.here();
                        let h = here.here.ok_or("where you are is not known")?;
                        Forget::Place {
                            lat: h.lat,
                            lon: h.lon,
                            km: body["km"].as_f64().filter(|k| *k > 0.0).unwrap_or(1.0),
                            at: now,
                        }
                    }
                    "before" => Forget::Before {
                        before: body["before"]
                            .as_u64()
                            .ok_or("before when: seconds since 1970")?,
                    },
                    "everything" => Forget::Everything { at: now },
                    other => return Err(format!("nothing to forget by {other:?}")),
                };
                rules.rules.push(rule);
            }
            rules.save(&self.base)?;
        }
        let records = attempts::read(&self.base);
        let forgotten = records.iter().filter(|r| rules.forgets(r)).count();
        Ok(json!({"rules": rules.rules, "records": records.len(), "forgotten": forgotten}))
    }

    /// `/api/terrain`: what is held and being fetched (`GET`), a request about it (`POST`), and one
    /// gateway's path (`GET /api/terrain/path`).
    fn terrain_api(
        &self,
        method: &Method,
        path: &str,
        param: &dyn Fn(&str) -> String,
        body: &Value,
    ) -> Result<Value, Error> {
        match (method, path) {
            (Method::Get, "/api/terrain") => Ok(self.terrain_status()),
            (Method::Post, "/api/terrain") => self.terrain_request(body),
            (Method::Get, "/api/terrain/path") => self.terrain_path(
                &param("callsign"),
                param("dial").parse().ok(),
                param("agl").parse().ok(),
            ),
            _ => Err(format!("no {method} {path}")),
        }
    }

    /// The terrain tiles held and being fetched, and what preparing the area around the position
    /// in use would fetch.
    fn terrain_status(&self) -> Value {
        let mut out = self.terrain.status();
        if let (Some((here, _)), _) = self.position.for_terrain() {
            out["around"] = terrain::RADII_KM
                .iter()
                .map(|&r| {
                    let tiles = Terrain::around(here, r);
                    json!({"radius_km": r, "tiles": tiles.len(),
                           "mb": {"glo90": Terrain::size_mb(&tiles, terrain::Source::Glo90),
                                  "glo30": Terrain::size_mb(&tiles, terrain::Source::Glo30)}})
                })
                .collect();
        }
        out
    }

    /// The panel's terrain requests: `fetch` (`tiles` by name, `source` `glo90` or `glo30`),
    /// `prepare` (`radius_km` around the position in use, `source`) and `cancel`.
    fn terrain_request(&self, body: &Value) -> Result<Value, Error> {
        let source = match body["source"].as_str().unwrap_or("glo90") {
            "glo30" => terrain::Source::Glo30,
            "glo90" => terrain::Source::Glo90,
            other => return Err(format!("source: glo90 or glo30, not {other}")),
        };
        match body["op"].as_str().unwrap_or("") {
            "fetch" => {
                let tiles = body["tiles"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|t| {
                        t.as_str()
                            .and_then(terrain::tile_named)
                            .ok_or("tiles: names such as N33W085")
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                self.terrain.fetch(&tiles, source);
            }
            "prepare" => {
                let radius = body["radius_km"]
                    .as_f64()
                    .filter(|r| (1.0..=250.0).contains(r));
                let radius = radius.ok_or("radius_km: 1 to 250")?;
                let (Some((here, _)), _) = self.position.for_terrain() else {
                    return Err("where you are is not known: save a place or set up a GPS".into());
                };
                self.terrain.fetch(&Terrain::around(here, radius), source);
            }
            "cancel" => self.terrain.cancel(),
            other => return Err(format!("no terrain request {other:?}")),
        }
        Ok(self.terrain_status())
    }

    /// One gateway's path over the terrain on its channel at `dial` (its first VHF channel by
    /// default), with the profile to draw; `agl` tries a height for the operator's antenna.
    fn terrain_path(
        &self,
        callsign: &str,
        dial: Option<u64>,
        agl: Option<f64>,
    ) -> Result<Value, Error> {
        let all = self.gateways_judged(false, None)?;
        let gw = all["gateways"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|g| g["callsign"] == callsign)
            .ok_or_else(|| format!("no gateway {callsign} in the list"))?;
        let channel = match dial {
            Some(hz) => gw["channels"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|c| c["dial_hz"].as_u64() == Some(hz))
                .cloned(),
            None => terrain::vhf_channel(gw),
        }
        .ok_or("no such VHF channel")?;
        let agl = agl.filter(|h| (0.5..=200.0).contains(h));
        let (at, station) = self.position.for_terrain();
        let mut out = self.terrain.judge(at, &station, gw, &channel, agl, true);
        out["callsign"] = json!(callsign);
        out["dial_hz"] = channel["dial_hz"].clone();
        Ok(out)
    }

    /// What the gateway list predicts for `callsign` on the channel at `dial` now, for the call's
    /// record (choosing a gateway, §12.1): its HF prediction, or its path over the terrain.
    fn prediction_for(&self, callsign: &str, dial: Option<u64>) -> Option<Value> {
        let dial = dial?;
        let all = self.gateways_judged(true, None).ok()?;
        let gw = all["gateways"]
            .as_array()?
            .iter()
            .find(|g| g["callsign"] == callsign)?;
        if gw["terrain"]["dial_hz"].as_u64() == Some(dial) {
            return Some(json!({"terrain": gw["terrain"]}));
        }
        let channel = gw["channels"]
            .as_array()?
            .iter()
            .find(|c| c["dial_hz"].as_u64() == Some(dial))?;
        channel.get("hf").map(|hf| json!({"hf": hf}))
    }

    /// The modem as the page names it, for a request outside a session: settings with the
    /// program's own clock, and the app's connection between calls following it.
    fn modem_options(&self, adapter: &str) -> Options {
        let adapter = modem_or_default(adapter);
        self.presence.set_modem(&adapter);
        Options::new(&adapter)
    }

    /// Says `line` in the panel's log, as a session's lines are.
    fn note(&self, line: &str) {
        lock(&self.activity).log.push(line.to_owned());
    }

    /// The operator picked a channel (W14): the radio is tuned there now, between sessions, so
    /// that it and the antenna tuner are set before the call (RP-4 §4.1). What the page shows:
    /// where the radio is (`there`, `moved`, or `by_hand` when the modem cannot set its dial), the
    /// line said, and whether the modem keys a tuner tone.
    fn tune(&self, adapter: &str, dial_hz: u64) -> Result<Value, Error> {
        self.busy()?;
        let opts = self.modem_options(adapter);
        let store = Store::open(&self.home())?;
        let result = session::tune(&store, &opts, dial_hz, &mut |line| self.note(line));
        let picked = result.inspect_err(|why| self.note(why))?;
        let tuned = match picked.tuned {
            session::Tuned::There => "there",
            session::Tuned::Moved(_) => "moved",
            session::Tuned::ByHand => "by_hand",
        };
        Ok(
            json!({"tuned": tuned, "dial_hz": dial_hz, "said": picked.said,
                  "tone": picked.tone}),
        )
    }

    /// The tuner tone (RP-4 §4.3): on for ten seconds, or off.
    fn tone(&self, adapter: &str, on: bool) -> Result<Value, Error> {
        if on {
            self.busy()?;
        }
        let opts = self.modem_options(adapter);
        let store = Store::open(&self.home())?;
        let seconds = if on { rpmi::wire::TONE_MAX_S } else { 0 };
        session::tone(&store, &opts, seconds, &mut |line| self.note(line))
            .inspect_err(|why| self.note(&format!("tuner tone: {why}")))?;
        Ok(json!({"on": on, "seconds": seconds}))
    }

    /// Takes the station for a call or for listening (`state`), when it is idle. The operator's
    /// word from the last call or session is forgotten here, under the lock [`Panel::hang_up`]
    /// takes, so a word for this one cannot be lost.
    fn take_station(&self, state: &'static str, remote: Option<String>) -> Result<(), Error> {
        let mut a = lock(&self.activity);
        if a.state != "idle" {
            return Err(why_busy(a.state));
        }
        self.hang_up.clear();
        a.state = state;
        a.remote = remote;
        a.failed = None;
        a.listen = state == "listening";
        a.held = None;
        a.stopping = None;
        a.stopped = false;
        Ok(())
    }

    /// Listen pressed again: off at once, and the station stops listening, now or when the
    /// session under way ends.
    fn stop_listening(&self) -> Value {
        self.stop.store(true, Ordering::Relaxed);
        lock(&self.activity).listen = false;
        json!({})
    }

    /// The page's **Stop calling**, **Disconnect** or **Abort**: `{how: "disconnect" | "abort"}`.
    fn hang_up_asked(&self, body: &Value) -> Result<Value, Error> {
        let how = match body["how"].as_str() {
            Some("disconnect") => session::Stop::Disconnect,
            Some("abort") => session::Stop::Abort,
            _ => return Err("how: disconnect or abort".into()),
        };
        self.hang_up(how)?;
        Ok(json!({}))
    }

    /// Ends the call or the session under way (**Stop calling**, **Disconnect**, **Abort**): the
    /// session's thread hears it within a quarter of a second ([`session::HangUp`]). A call is
    /// stopped by either word; an abort after a disconnect still under way ends the link at once.
    /// Listening goes on after a session ended so: **Listening for P2P** is its own switch.
    ///
    /// # Errors
    ///
    /// No call or session is under way.
    pub fn hang_up(&self, how: session::Stop) -> Result<(), Error> {
        let mut a = lock(&self.activity);
        if !matches!(a.state, "calling" | "session") {
            return Err("no call or session is under way".into());
        }
        self.hang_up.ask(how);
        a.stopping = Some(match self.hang_up.asked() {
            Some(session::Stop::Abort) => "abort",
            _ => "disconnect",
        });
        Ok(())
    }

    /// What a call or listening runs with: the modem, which becomes the app's modem between calls
    /// too, the panel's stop, its progress rings and its alerts (RP-8); listening answers calls to
    /// every callsign of this client (W46).
    fn station_options(&self, adapter: &str) -> Options {
        self.stop.store(false, Ordering::Relaxed);
        let adapter = modem_or_default(adapter);
        self.presence.set_modem(&adapter);
        let mut opts = Options::new(&adapter);
        opts.stop = Some(Arc::clone(&self.stop));
        opts.hang_up = Some(self.hang_up.clone());
        // the channel was tuned when the operator picked it: the radio stays where it was put
        opts.tune_back = false;
        *self.live.lock().unwrap_or_else(PoisonError::into_inner) = session::Live::default();
        opts.live = Some(Arc::clone(&self.live));
        opts.alerts = Some(self.carrier());
        opts.also = callsigns::list(&self.base)
            .into_iter()
            .map(|c| c.home)
            .collect();
        opts.place = self.position.for_record();
        opts.station = self.position.station_for_record();
        opts
    }

    /// A call, on a thread of its own.
    fn run(
        &self,
        remote: String,
        kind: Kind,
        adapter: &str,
        fetch: Fetch,
        (dial_hz, call_s, bandwidth): (Option<u64>, Option<u64>, Option<i64>),
    ) -> Result<Value, Error> {
        self.take_station("calling", Some(remote.clone()))?;
        let mut opts = self.station_options(adapter);
        opts.fetch = fetch;
        opts.dial_hz = dial_hz;
        opts.call_s = call_s;
        opts.bandwidth = bandwidth;
        if kind == Kind::Gateway {
            opts.predicted = self.prediction_for(&remote, dial_hz);
        }
        // read once the activity is set, so the callsign chosen cannot change before it is opened
        let home = self.home();
        let activity = Arc::clone(&self.activity);
        let hang_up = self.hang_up.clone();
        thread::spawn(move || {
            let mut say = |line: &str| {
                let mut a = lock(&activity);
                if line.starts_with("connected to ") {
                    a.state = "session";
                }
                a.log.push(line.to_owned());
            };
            let result = Store::open(&home)
                .and_then(|mut store| session::call(&mut store, &remote, kind, &opts, &mut say));
            let mut a = lock(&activity);
            (a.failed, a.stopped) = match result {
                Ok(report) => (report.failed, report.stopped),
                Err(e) => (Some(e), hang_up.asked().is_some()),
            };
            a.stopping = None;
            if let Some(why) = &a.failed {
                let line = format!("ended: {why}");
                radiopost::logfile::line(&line);
                a.log.push(line);
            }
            a.state = "idle";
        });
        Ok(json!({}))
    }

    /// Listen pressed: the station listens for P2P calls on a thread of its own, and goes back to
    /// listening after each session and whenever the modem comes back, until Listen is pressed
    /// again (`/api/stop`). Between sessions the activity is `listening`; in one, `session`. With
    /// `dial_hz` (the P2P dial typed), the radio is tuned there first, unless the station scans a
    /// gateway's channels.
    fn listen(&self, adapter: &str, dial_hz: Option<u64>) -> Result<Value, Error> {
        self.take_station("listening", None)?;
        let mut opts = self.station_options(adapter);
        opts.dial_hz = dial_hz;
        // read once the activity is set, so the callsign chosen cannot change before it is opened
        let home = self.home();
        let (activity, told_to) = (Arc::clone(&self.activity), Arc::clone(&self.activity));
        let rings = Arc::clone(&self.live);
        thread::spawn(move || {
            let mut say = |line: &str| lock(&activity).log.push(line.to_owned());
            let mut told = |what: session::Listening| {
                let mut a = lock(&told_to);
                match what {
                    session::Listening::Waiting(at) => {
                        a.state = "listening";
                        a.held = None;
                        a.listening_at = Some(at);
                    }
                    session::Listening::Called(who) => {
                        // told before the home is opened for the call, under the lock the panel
                        // holds while it changes the home (`between_sessions`): a change under
                        // way is on disk before the session reads the outbox, and none begins
                        // until the session is over
                        a.state = "session";
                        a.remote = Some(who);
                        a.failed = None;
                        a.stopping = None;
                        a.stopped = false;
                        *rings.lock().unwrap_or_else(PoisonError::into_inner) =
                            session::Live::default();
                    }
                    session::Listening::Ended(result) => {
                        (a.failed, a.stopped) = match result {
                            Ok(report) => (report.failed, report.stopped),
                            Err(e) => (Some(e), a.stopping.is_some()),
                        };
                        a.stopping = None;
                        if let Some(why) = &a.failed {
                            let line = format!("ended: {why}");
                            radiopost::logfile::line(&line);
                            a.log.push(line);
                        }
                        a.state = "listening";
                    }
                    session::Listening::Held(why) => {
                        a.state = "listening";
                        a.held = Some(why);
                    }
                }
            };
            session::keep_listening(&home, &opts, &mut say, &mut told);
            let mut a = lock(&activity);
            a.state = "idle";
            a.listen = false;
            a.held = None;
            a.listening_at = None;
        });
        Ok(json!({}))
    }
}

/// A dial from the page, in hertz above 0; `null` for none.
fn dial_of(v: &Value) -> Result<Option<u64>, Error> {
    match v {
        Value::Null => Ok(None),
        v => v
            .as_u64()
            .filter(|&d| d > 0)
            .map(Some)
            .ok_or_else(|| "dial_hz: hertz above 0".into()),
    }
}

/// The templates this client holds (RP-2 §8.3), for the composer: each with its fingerprint, and
/// whether it is one kept here (`kept`) rather than published.
fn forms_json(home: &Path) -> Value {
    crate::templates::held(home)
        .iter()
        .map(template_json)
        .collect()
}

/// A template for the page: its fingerprint, name, note and fields, how it prints, and whether it
/// is kept here.
fn template_json(t: &radiopost::forms::Template) -> Value {
    let fields: Vec<Value> = t.fields.iter().map(field_json).collect();
    // how it prints, as the paper form it comes from, when this program holds a layout
    let print = radiopost::forms::layout(&t.fingerprint());
    json!({"template": radiopost::cbor::hex(&t.fingerprint()), "name": t.name,
           "note": t.note, "fields": fields, "print": print,
           "kept": !crate::templates::published(&t.fingerprint())})
}

/// A field of a template (or a column of a `rows` field) for the composer.
fn field_json(f: &radiopost::forms::Field) -> Value {
    let columns: Vec<Value> = f.columns.iter().map(field_json).collect();
    json!({"n": f.n, "label": f.label, "type": f.kind.name(), "required": f.required,
           "choices": f.choices, "bound": f.bound.map(radiopost::forms::Bound::name),
           "max": f.max, "scale": f.scale, "unit": f.unit, "columns": columns})
}

/// A form's value as JSON, for the composer to put back in its fields: integers, text, yes and
/// no, a list (a position, rows) and a map (a row) by its numbers. `None` for what JSON cannot
/// hold exactly.
fn json_of(v: &Cbor) -> Option<Value> {
    Some(match v {
        Cbor::Text(t) => Value::from(t.as_str()),
        Cbor::Bool(b) => Value::from(*b),
        Cbor::Array(items) => Value::Array(items.iter().map(json_of).collect::<Option<_>>()?),
        Cbor::Map(entries) => Value::Object(
            entries
                .iter()
                .map(|(k, v)| Some((k.as_u64()?.to_string(), json_of(v)?)))
                .collect::<Option<_>>()?,
        ),
        other => Value::from(other.as_i64()?),
    })
}

/// A position typed in the composer (`POST /api/form-position`): its value, and the text the form
/// will show (RP-2 §8.4), so the operator sees exactly what goes.
fn form_position(text: &str) -> Result<Value, Error> {
    let value = crate::compose::position_from_text(text)?;
    let shown = radiopost::forms::Template::from_value(&json!({
        "name": "", "fields": [{"n": 1, "label": "", "type": "position"}]
    }))
    .ok()
    .and_then(|t| {
        let form = radiopost::forms::Form {
            template: t.fingerprint(),
            values: vec![(1, value.clone())],
        };
        (t.problem(&form.values).is_none()).then(|| form_row_value(&t, &form))
    })
    .ok_or("a latitude within 90° and a longitude within 180°")?;
    Ok(json!({"value": json_of(&value), "shown": shown}))
}

/// The text of a form's first row, as RP-2 §8.4 writes it.
fn form_row_value(t: &radiopost::forms::Template, form: &radiopost::forms::Form) -> String {
    let message = radiopost::message::Message::from_map(&Cbor::int_map(vec![
        (1, Cbor::Uint(1)),
        (2, Cbor::text("")),
        (3, Cbor::Array(vec![])),
        (5, Cbor::Uint(0)),
        (6, Cbor::text("")),
        (7, Cbor::text("")),
        (13, form.to_cbor()),
    ]));
    message
        .ok()
        .and_then(|m| t.shown(&m, form).rows.into_iter().next())
        .map(|r| r.value)
        .unwrap_or_default()
}

/// A form from the composer: `{template, values}`, the values by field number; `null` for none.
/// With a template this client holds (`templates`), each value is read as its field's type: text
/// as typed for a `line` or `text` field and through [`crate::compose::value_from_text`] for any
/// other (a position, a decimal), integers and yes and no as they are, rows as a list of maps by
/// column number. Empty fields, cells and rows are left out.
fn form_of_with(
    form: &Value,
    templates: &[radiopost::forms::Template],
) -> Result<Option<radiopost::forms::Form>, Error> {
    if form.is_null() {
        return Ok(None);
    }
    let template: [u8; 8] = form["template"]
        .as_str()
        .and_then(crate::store::unhex)
        .and_then(|b| b.try_into().ok())
        .ok_or("a form names its template's fingerprint, 16 hex digits")?;
    let held = templates.iter().find(|t| t.fingerprint() == template);
    let mut values = Vec::new();
    for (n, v) in form["values"].as_object().into_iter().flatten() {
        let n: u64 = n.parse().map_err(|_| "a form's fields are numbered")?;
        let value = match held.and_then(|t| t.field(n)) {
            Some(field) => value_of(field, v).map_err(|why| format!("{}: {why}", field.label))?,
            None => plain_value(v)?,
        };
        if let Some(value) = value {
            values.push((n, value));
        }
    }
    values.sort_by_key(|(n, _)| *n);
    Ok(Some(radiopost::forms::Form { template, values }))
}

/// A value from the composer for `field`, read as its type; `None` when it is empty.
fn value_of(field: &radiopost::forms::Field, v: &Value) -> Result<Option<Cbor>, Error> {
    use radiopost::forms::Kind;
    Ok(Some(match (field.kind, v) {
        (_, Value::Null) => return Ok(None),
        (_, Value::String(t)) if t.trim().is_empty() => return Ok(None),
        (Kind::Line | Kind::Text, Value::String(t)) => Cbor::text(t),
        (_, Value::String(t)) => crate::compose::value_from_text(field, t)?,
        (Kind::Rows, Value::Array(rows)) => {
            let mut out = Vec::new();
            for row in rows {
                let mut cells = Vec::new();
                for (k, cell) in row.as_object().ok_or("a row is a map of its columns")? {
                    let column = k
                        .parse::<u64>()
                        .ok()
                        .and_then(|c| field.columns.iter().find(|col| col.n == c))
                        .ok_or("a row's cells are its columns, by number")?;
                    if let Some(value) = value_of(column, cell)? {
                        cells.push((Cbor::Uint(column.n), value));
                    }
                }
                if !cells.is_empty() {
                    out.push(Cbor::Map(cells));
                }
            }
            if out.is_empty() {
                return Ok(None);
            }
            Cbor::Array(out)
        }
        (_, other) => match plain_value(other)? {
            Some(value) => value,
            None => return Ok(None),
        },
    }))
}

/// A value from the composer for a field this client does not know: text, an integer, yes or no.
fn plain_value(v: &Value) -> Result<Option<Cbor>, Error> {
    Ok(match v {
        Value::String(t) if t.is_empty() => None,
        Value::String(t) => Some(Cbor::text(t)),
        Value::Bool(b) => Some(Cbor::Bool(*b)),
        Value::Number(x) => Some(Cbor::int(
            x.as_i64().ok_or("a form's numbers are integers")?,
        )),
        Value::Null => None,
        _ => return Err("a form's values are text, integers or yes and no".into()),
    })
}

/// A dial member of a request: absent or null is none, else a whole number of hertz above 0.
fn dial_member(v: &Value, name: &str) -> Result<Option<u64>, Error> {
    match v {
        Value::Null => Ok(None),
        _ => v
            .as_u64()
            .filter(|&hz| hz > 0)
            .map(Some)
            .ok_or_else(|| format!("{name}: the dial in hertz, a whole number above 0")),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use super::{Panel, Store, form_of_with, json_of, lock, thread, unhex};

    /// What the composer sends for RP-2 §8's test template (`vectors/forms.json`, `every_type`):
    /// text as typed, a position and decimals as text, integers for dates, choices and bits,
    /// rows by column number with empty cells and rows; read as the template's types, it is the
    /// test message's form, and put back for Edit it is the same values.
    #[test]
    fn a_form_of_every_type_reads_from_the_composer() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vectors/forms.json");
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("forms.json"))
                .expect("JSON");
        let e = &v["every_type"];
        let t = radiopost::forms::Template::from_value(&e["template"]).expect("it reads");
        let sent = serde_json::json!({
            "template": e["fingerprint_hex"],
            "values": {
                "1": "Shelter 3", "3": "33.78123, -84.38765", "4": 29_903_885, "6": 5,
                "7": "1.25", "8": "45", "9": "1.5", "10": false, "12": "",
                "14": [
                    {"1": 29_903_885, "2": "KK4ODA", "3": "N0CALL",
                     "4": "Shelter 3 open, 40 cots", "5": 0},
                    {"1": 29_903_900, "2": "N0CALL", "3": "", "4": "Copied"},
                    {"2": ""}
                ],
                "15": "Water rising on Main St.\nRoad closed at the bridge."
            }
        });
        let form = form_of_with(&sent, std::slice::from_ref(&t))
            .expect("it reads")
            .expect("a form");
        let message = radiopost::message::Message::from_canonical(
            &unhex(e["messages"][0]["canonical_hex"].as_str().expect("hex")).expect("hex"),
        )
        .expect("canonical");
        assert_eq!(Some(form.clone()), radiopost::forms::Form::of(&message));
        // Edit puts the same values back: positions and rows as lists, rows by column number
        let back: serde_json::Map<String, serde_json::Value> = form
            .values
            .iter()
            .map(|(n, x)| (n.to_string(), json_of(x).expect("JSON holds it")))
            .collect();
        assert_eq!(back["3"], serde_json::json!([337_812_300, -843_876_500]));
        assert_eq!(
            back["14"][1],
            serde_json::json!({"1": 29_903_900, "2": "N0CALL", "4": "Copied"})
        );
        // what does not read as its type is refused, with the field's label
        let bad = serde_json::json!({"template": e["fingerprint_hex"], "values": {"3": "Atlanta"}});
        let why = form_of_with(&bad, std::slice::from_ref(&t)).expect_err("refused");
        assert!(why.starts_with("Location: "), "{why}");
    }

    /// A field of an entry of one of the vectors' lists, found by its name.
    fn vector(file: &str, list: &str, (key, name): (&str, &str), field: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../vectors")
            .join(file);
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("a vector file"))
                .expect("JSON");
        v[list]
            .as_array()
            .expect("a list")
            .iter()
            .find(|e| e[key] == name)
            .and_then(|e| e[field].as_str())
            .expect("the entry")
            .to_owned()
    }

    /// A renewal that comes back once a call has taken the station (it was listening when the
    /// renewal began) keeps the credential only when the session ends, never while the session
    /// holds the home; between sessions it is kept at once.
    #[test]
    fn a_credential_renewed_during_a_session_is_kept_when_it_ends() {
        let home =
            std::env::temp_dir().join(format!("radiopost-panel-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let store = Store::open(&home).expect("a home");
        let seed: [u8; 32] = unhex(&vector(
            "keys.json",
            "keys",
            ("label", "device:ana"),
            "seed_hex",
        ))
        .and_then(|b| b.try_into().ok())
        .expect("a seed");
        store
            .set_device(&ed25519_dalek::SigningKey::from_bytes(&seed))
            .expect("kept");
        let bytes = unhex(&vector(
            "credentials.json",
            "credentials",
            ("name", "ana"),
            "credential_hex",
        ))
        .expect("hex");
        let panel = Panel::new(&home, None);

        lock(&panel.activity).state = "session";
        let said = panel
            .keep_credential(&home, bytes.clone())
            .expect("a credential");
        assert!(
            said.ends_with(", kept when the session under way ends"),
            "{said}"
        );
        thread::sleep(Duration::from_millis(600));
        assert!(
            store.credential().is_err(),
            "not while a session holds the home"
        );
        lock(&panel.activity).state = "listening";
        let until = Instant::now() + Duration::from_secs(5);
        while store.credential().is_err() {
            assert!(Instant::now() < until, "never kept");
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(store.credential().expect("kept"), bytes);

        std::fs::remove_file(home.join("credential")).expect("removed");
        let said = panel.keep_credential(&home, bytes.clone()).expect("kept");
        assert!(said.starts_with("credential "), "{said}");
        assert!(!said.contains("kept when"), "at once: {said}");
        assert_eq!(store.credential().expect("kept"), bytes);
        let _ = std::fs::remove_dir_all(&home);
    }
}
