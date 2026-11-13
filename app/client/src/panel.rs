// SPDX-License-Identifier: MIT OR Apache-2.0
//! The client's panel: the page in `app/ui/` and a JSON API under `/api/`, served on the loopback
//! interface, as the Aether app serves its own. A browser can open it; the desktop application
//! (Tauri) shows the same page in its window.
//!
//! Each request opens the home afresh, so the panel always shows what is on disk: the home of the
//! callsign chosen ([`crate::callsigns`]), which the header's list changes between sessions. A
//! session runs on a thread of its own with a store of its own; while it runs, the panel changes
//! nothing in the home, and the callsign chosen stays, so the two never write over each other.
//!
//! While it is open, the panel renews every callsign's credential over the Internet in its last
//! month ([`crate::renewal`]): when it starts and every hour, between sessions, as the activity
//! `renewing`.
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
use crate::callsigns;
use crate::compose::{self, Answer, Draft, File, media_type};
use crate::online;
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
];

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
    /// `idle`, `calling`, `listening`, `session`, or `renewing` the credential.
    state: &'static str,
    remote: Option<String>,
    /// How the last session ended, when it failed.
    failed: Option<String>,
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
    addr: SocketAddr,
    updates: Arc<Watch>,
    renewal: Renewal,
    /// The running session's progress, or the last one's.
    live: Arc<Mutex<session::Live>>,
    /// The post office's token for the operator's requests (RP-5 §7), kept between them.
    office_token: Arc<Mutex<Option<String>>>,
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

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
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
    let id = m.content_id().text();
    json!({
        "id": id, "short": &id[..6], "from": text(key::FROM), "to": list(key::TO),
        "cc": list(key::CC), "date": map.get(key::DATE).and_then(Cbor::as_u64),
        "subject": text(key::SUBJECT),
        "priority": map.get(key::PRIORITY).and_then(Cbor::as_u64).unwrap_or(0),
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
        }
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
        let renewing = self.clone();
        thread::spawn(move || renewing.keep_renewed());
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
            thread::sleep(self.renewal.every);
        }
    }

    /// One try for the credential at `home`: what to say of it, if anything (`said` is what was
    /// said of its last failure). Only between sessions, as the activity `renewing`, so a session
    /// and a renewal never write a home together.
    fn renew_now(&self, home: &Path, said: &mut String) -> Option<String> {
        let Ok(me) = Store::open(home).and_then(|s| s.me()) else {
            return None;
        };
        let now = now();
        if !crate::renewal::due(me.expires, now, self.renewal.within) {
            return None;
        }
        {
            let mut a = lock(&self.activity);
            if a.state != "idle" {
                return None;
            }
            a.state = "renewing";
        }
        let result = Store::open(home).and_then(|store| online::renew(&store));
        lock(&self.activity).state = "idle";
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

    fn busy(&self) -> Result<(), Error> {
        match lock(&self.activity).state {
            "idle" => Ok(()),
            "renewing" => Err("renewing the credential: try again in a moment".into()),
            _ => Err("a session is under way: try again when it ends".into()),
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
            (Method::Get, "/api/settings") => {
                let store = Store::open(&self.home())?;
                online::settings(&store, None)
            }
            (Method::Get, "/api/folder") => self.folder(&param("name")),
            (Method::Get, "/api/message") => self.message(&param("id")),
            (Method::Get, "/api/answer") => self.answer(&param("id"), &param("how")),
            (Method::Get, "/api/log") => {
                let since: usize = param("since").parse().unwrap_or(0);
                let a = lock(&self.activity);
                Ok(json!({"lines": a.log.get(since..).unwrap_or(&[]), "next": a.log.len()}))
            }
            (Method::Post, "/api/callsigns") => self.callsigns(body),
            (Method::Post, "/api/compose") => self.compose(body),
            (Method::Post, "/api/held") => self.ask_for_held(body),
            (_, "/api/addresses") => self.addresses(method, body),
            (Method::Post, "/api/delete" | "/api/restore" | "/api/purge" | "/api/empty-trash") => {
                self.trash(path, body)
            }
            (Method::Post, "/api/key" | "/api/credential" | "/api/trust") => {
                self.identity(path, body)
            }
            (Method::Post, "/api/call") => {
                let callsign = text("callsign").to_ascii_uppercase();
                if callsign.is_empty() {
                    return Err("whom to call".into());
                }
                let kind = if body["station"].as_bool().unwrap_or(true) {
                    Kind::Station
                } else {
                    Kind::Gateway
                };
                let fetch = match &body["fetch"] {
                    Value::Null => Fetch::All,
                    Value::String(f) if f == "all" => Fetch::All,
                    Value::String(f) if f == "none" => Fetch::Nothing,
                    Value::Number(n) => Fetch::UpTo(n.as_u64().ok_or("fetch: a size in bytes")?),
                    _ => return Err("fetch: all, none or a size in bytes".into()),
                };
                self.run(Some(callsign), kind, &text("adapter"), fetch)
            }
            (Method::Post, "/api/listen") => {
                self.run(None, Kind::Station, &text("adapter"), Fetch::All)
            }
            (_, "/api/gateways" | "/api/grid" | "/api/waiting" | "/api/fetch") => {
                self.gateway_side(method, path, body)
            }
            (Method::Post, p) if ONLINE.contains(&p) => self.online(path, body),
            (Method::Post, "/api/office") => self.office(body),
            (Method::Post, "/api/office-backup") => self.office_backup(body),
            (Method::Post, "/api/terminal") => {
                let target = body["target"].as_str().unwrap_or("").trim();
                Ok(json!({"opened": crate::terminal::open(target)?}))
            }
            (Method::Post, "/api/update") => self.update(),
            (Method::Post, "/api/stop") => {
                self.stop.store(true, Ordering::Relaxed);
                Ok(json!({}))
            }
            _ => Err(format!("no {method} {path}")),
        }
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
            "/api/renew" => {
                self.busy()?;
                let store = Store::open(&self.home())?;
                Ok(json!({"said": online::renew(&store)?}))
            }
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

    /// A backup of the post office sealed with the operator's passphrase, fetched over the desk
    /// and kept on this computer, in `RadioPost backups` in the user's home folder.
    fn office_backup(&self, body: &Value) -> Result<Value, Error> {
        let store = Store::open(&self.home())?;
        let sent = json!({"passphrase": body["passphrase"].as_str().unwrap_or("")});
        let (status, answer) = online::office(
            &store,
            &self.office_token,
            "POST",
            "/office/backup",
            Some(&sent),
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
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map_or_else(|| self.base.clone(), PathBuf::from);
        let folder = home.join("RadioPost backups");
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
        self.busy()?;
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
        let started = compose::answer(&original, &me.address, how);
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

    /// A message into the outbox. Besides what the user wrote, `reply_to` names the message it
    /// answers (its content ID or the start of it), and `forward` the message it passes on, whose
    /// files it carries: those whose places `carry` lists, or all of them (W1).
    fn compose(&self, body: &Value) -> Result<Value, Error> {
        let text = |name: &str| body[name].as_str().unwrap_or("").trim().to_owned();
        self.busy()?;
        let mut store = Store::open(&self.home())?;
        let me = store.me()?;
        let mut files = Vec::new();
        if let Some(prefix) = body["forward"].as_str().filter(|p| !p.is_empty()) {
            let carried = compose::files_of(&store.message(&store.find(prefix)?)?);
            let carry: Option<Vec<u64>> = body["carry"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_u64).collect());
            files.extend(carried.into_iter().enumerate().filter_map(|(i, f)| {
                let i = u64::try_from(i).ok()?;
                carry.as_ref().is_none_or(|c| c.contains(&i)).then_some(f)
            }));
        }
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
        let reply_to = match body["reply_to"].as_str().filter(|p| !p.is_empty()) {
            Some(prefix) => Some(store.find(prefix)?.0),
            None => None,
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
        };
        let id = store.queue(&draft.message()?)?;
        // the address list learns whom the message is for; a failure there loses no message
        if let Err(why) = addresses::remember(&self.base, &[to, cc].concat(), now()) {
            radiopost::logfile::line(&format!("the address list was not updated: {why}"));
        }
        Ok(json!({"id": id.text()}))
    }

    fn state(&self) -> Result<Value, Error> {
        let store = Store::open(&self.home())?;
        let me = store
            .me()
            .ok()
            .map(|b| json!({"address": b.address, "operator": b.operator, "expires": b.expires}));
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
            "activity": {"state": a.state, "remote": a.remote, "failed": a.failed},
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

    /// The address list, the computer's for every callsign (it is a file of its own, so it
    /// changes during a session too): `add`
    /// typed text, `remove` an address, or `put_back` an entry removed.
    fn addresses(&self, method: &Method, body: &Value) -> Result<Value, Error> {
        let entries = |list: Vec<addresses::Entry>| -> Value {
            list.iter().map(addresses::Entry::json).collect()
        };
        if *method == Method::Get {
            return Ok(json!({"entries": entries(addresses::list(&self.base))}));
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
    /// the trash emptied. Not while a session runs: it keeps the folders until it ends.
    fn trash(&self, path: &str, body: &Value) -> Result<Value, Error> {
        self.busy()?;
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
    fn gateway_side(&self, method: &Method, path: &str, body: &Value) -> Result<Value, Error> {
        let text = |name: &str| body[name].as_str().unwrap_or("").trim().to_owned();
        match (method, path) {
            (Method::Get, "/api/gateways") => self.gateways(),
            (Method::Post, "/api/gateways") => {
                self.busy()?;
                let mut store = Store::open(&self.home())?;
                online::gateways(&mut store)?;
                self.gateways()
            }
            (Method::Post, "/api/grid") => {
                self.busy()?;
                let mut store = Store::open(&self.home())?;
                store.set_grid(Some(&text("grid")))?;
                self.gateways()
            }
            (Method::Get, "/api/waiting") => {
                let store = Store::open(&self.home())?;
                let w = store.waiting.clone().unwrap_or(Value::Null);
                Ok(json!({"via": w["via"], "at": w["at"], "entries": store.waiting_entries()}))
            }
            (Method::Post, "/api/fetch") => {
                self.busy()?;
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
    /// were taken, which could not be, and the callsigns listed twice.
    fn gateways(&self) -> Result<Value, Error> {
        let store = Store::open(&self.home())?;
        let list = store.gateway_list.clone().unwrap_or(Value::Null);
        let others: Vec<&String> = list["lists"]
            .as_object()
            .map(|l| l.keys().collect())
            .unwrap_or_default();
        Ok(json!({
            "domain": list["domain"], "fetched": list["fetched"], "grid": store.grid,
            "gateways": store.gateways_ranked(), "others": others,
            "errors": list["errors"], "ambiguous": list["ambiguous"],
        }))
    }

    fn run(
        &self,
        remote: Option<String>,
        kind: Kind,
        adapter: &str,
        fetch: Fetch,
    ) -> Result<Value, Error> {
        {
            let mut a = lock(&self.activity);
            match a.state {
                "idle" => {}
                "renewing" => return Err("renewing the credential: try again in a moment".into()),
                _ => return Err("a session is under way".into()),
            }
            a.state = if remote.is_some() {
                "calling"
            } else {
                "listening"
            };
            a.remote.clone_from(&remote);
            a.failed = None;
        }
        self.stop.store(false, Ordering::Relaxed);
        let mut opts = Options::new(if adapter.is_empty() {
            "127.0.0.1:8600"
        } else {
            adapter
        });
        opts.stop = Some(Arc::clone(&self.stop));
        opts.fetch = fetch;
        *self.live.lock().unwrap_or_else(PoisonError::into_inner) = session::Live::default();
        opts.live = Some(Arc::clone(&self.live));
        // read once the activity is set, so the callsign chosen cannot change before it is opened
        let home = self.home();
        let activity = Arc::clone(&self.activity);
        thread::spawn(move || {
            let mut say = |line: &str| {
                let mut a = lock(&activity);
                if line.starts_with("connected to ") || line.ends_with(" called") {
                    a.state = "session";
                }
                if let Some(who) = line.strip_suffix(" called") {
                    a.remote = Some(who.to_owned());
                }
                a.log.push(line.to_owned());
            };
            let result = Store::open(&home).and_then(|mut store| match &remote {
                Some(r) => session::call(&mut store, r, kind, &opts, &mut say),
                None => session::answer(&mut store, &opts, &mut say),
            });
            let mut a = lock(&activity);
            a.failed = match result {
                Ok(report) => report.failed,
                Err(e) => Some(e),
            };
            if let Some(why) = &a.failed {
                let line = format!("ended: {why}");
                radiopost::logfile::line(&line);
                a.log.push(line);
            }
            a.state = "idle";
        });
        Ok(json!({}))
    }
}
