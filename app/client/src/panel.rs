// SPDX-License-Identifier: MIT OR Apache-2.0
//! The client's panel: the page in `app/ui/` and a JSON API under `/api/`, served on the loopback
//! interface, as the Aether app serves its own. A browser can open it; the desktop application
//! (Tauri) shows the same page in its window.
//!
//! Each request opens the home afresh, so the panel always shows what is on disk. A session runs
//! on a thread of its own with a store of its own; while it runs, the panel changes nothing in the
//! home, so the two never write over each other.
//!
//! Only this machine's own pages reach it: a request must name the panel's own host (so a page
//! that rebinds a name to 127.0.0.1 is refused), and a change must carry `X-RadioPost: 1`, which
//! another site's page cannot send without a preflight the panel never grants.

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

use crate::compose::{Draft, File, media_type};
use crate::online;
use crate::session::{self, Kind, Options};
use crate::store::{Error, Store, unhex};

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

/// What the station is doing, for the panel.
#[derive(Debug, Default)]
struct Activity {
    /// `idle`, `calling`, `listening` or `session`.
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
    home: PathBuf,
    ui: Option<PathBuf>,
    activity: Arc<Mutex<Activity>>,
    stop: Arc<AtomicBool>,
    addr: SocketAddr,
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

fn addresses(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => s
            .split([',', ';', ' '])
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(str::to_owned)
            .collect(),
        Value::Array(a) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

impl Panel {
    /// A panel for the home at `home`, serving its page from `ui` when given.
    #[must_use]
    pub fn new(home: &Path, ui: Option<PathBuf>) -> Panel {
        Panel {
            home: home.to_owned(),
            ui,
            activity: Arc::new(Mutex::new(Activity {
                state: "idle",
                ..Activity::default()
            })),
            stop: Arc::new(AtomicBool::new(false)),
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
        }
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
        thread::spawn(move || {
            for request in server.incoming_requests() {
                self.handle(request);
            }
        });
        Ok(local)
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
            _ => Err("a session is under way: try again when it ends".into()),
        }
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
            (Method::Get, "/api/folder") => self.folder(&param("name")),
            (Method::Get, "/api/message") => self.message(&param("id")),
            (Method::Get, "/api/log") => {
                let since: usize = param("since").parse().unwrap_or(0);
                let a = lock(&self.activity);
                Ok(json!({"lines": a.log.get(since..).unwrap_or(&[]), "next": a.log.len()}))
            }
            (Method::Post, "/api/compose") => self.compose(body),
            (Method::Post, "/api/held") => self.ask_for_held(body),
            (Method::Post, "/api/key") => {
                self.busy()?;
                let store = Store::open(&self.home)?;
                if store.device().is_err() {
                    let mut seed = [0u8; 32];
                    getrandom::fill(&mut seed).map_err(|e| e.to_string())?;
                    store.set_device(&ed25519_dalek::SigningKey::from_bytes(&seed))?;
                }
                self.state()
            }
            (Method::Post, "/api/credential") => {
                self.busy()?;
                let store = Store::open(&self.home)?;
                let bytes = unhex(&text("hex")).ok_or("paste the credential in hex")?;
                store.device()?;
                store.set_credential(&bytes)?;
                self.state()
            }
            (Method::Post, "/api/trust") => {
                self.busy()?;
                let store = Store::open(&self.home)?;
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
                self.run(Some(callsign), kind, &text("adapter"))
            }
            (Method::Post, "/api/listen") => self.run(None, Kind::Station, &text("adapter")),
            (
                Method::Post,
                "/api/enroll" | "/api/post-office" | "/api/renew" | "/api/report" | "/api/accept",
            ) => self.online(path, body),
            (Method::Post, "/api/stop") => {
                self.stop.store(true, Ordering::Relaxed);
                Ok(json!({}))
            }
            _ => Err(format!("no {method} {path}")),
        }
    }

    /// The post office over the Internet (RP-5 §6): enrollment, the post office's address,
    /// renewal, reports and the accept list.
    fn online(&self, path: &str, body: &Value) -> Result<Value, Error> {
        let text = |name: &str| body[name].as_str().unwrap_or("").trim().to_owned();
        match path {
            "/api/enroll" => {
                self.busy()?;
                let mut store = Store::open(&self.home)?;
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
                let mut store = Store::open(&self.home)?;
                let url = text("url");
                store.post_office = (!url.is_empty()).then(|| url.trim_end_matches('/').to_owned());
                store.save()?;
                self.state()
            }
            "/api/renew" => {
                self.busy()?;
                let store = Store::open(&self.home)?;
                Ok(json!({"said": online::renew(&store)?}))
            }
            "/api/report" => {
                self.busy()?;
                let mut store = Store::open(&self.home)?;
                Ok(json!({"lines": online::send_reports(&mut store)?}))
            }
            "/api/accept" => {
                let store = Store::open(&self.home)?;
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

    fn message(&self, prefix: &str) -> Result<Value, Error> {
        let store = Store::open(&self.home)?;
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
        let mut store = Store::open(&self.home)?;
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

    fn compose(&self, body: &Value) -> Result<Value, Error> {
        let text = |name: &str| body[name].as_str().unwrap_or("").trim().to_owned();
        self.busy()?;
        let mut store = Store::open(&self.home)?;
        let me = store.me()?;
        let files = body["files"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|f| {
                let name = f["name"].as_str().unwrap_or("file").to_owned();
                let data = f["data"]
                    .as_str()
                    .and_then(rpmi::base64::decode)
                    .ok_or("an attachment's data is base64")?;
                Ok(File {
                    media_type: media_type(&name).into(),
                    name,
                    data,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let draft = Draft {
            from: me.address,
            to: addresses(&body["to"]),
            cc: addresses(&body["cc"]),
            date: now(),
            subject: text("subject"),
            body: body["body"].as_str().unwrap_or("").to_owned(),
            files,
            priority: body["priority"].as_u64().unwrap_or(0),
            reply_to: None,
        };
        let id = store.queue(&draft.message()?)?;
        Ok(json!({"id": id.text()}))
    }

    fn state(&self) -> Result<Value, Error> {
        let store = Store::open(&self.home)?;
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
            "counts": {"outbox": store.outbox.len(), "inbox": store.inbox.len(),
                       "sent": store.sent.len(), "reports": store.reports.len(),
                       "files": store.files.len()},
            "activity": {"state": a.state, "remote": a.remote, "failed": a.failed},
            "log": a.log.len(),
        }))
    }

    fn folder(&self, name: &str) -> Result<Value, Error> {
        let store = Store::open(&self.home)?;
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
        for (id, station) in entries {
            let mut v = summary(&store.message(&id)?);
            if let Some(s) = station {
                v["station"] = s.into();
            }
            out.push(v);
        }
        Ok(Value::Array(out))
    }

    /// Starts a call (`remote`) or listening (`None`) on a thread of its own.
    fn run(&self, remote: Option<String>, kind: Kind, adapter: &str) -> Result<Value, Error> {
        {
            let mut a = lock(&self.activity);
            if a.state != "idle" {
                return Err("a session is under way".into());
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
        let home = self.home.clone();
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
                a.log.push(line);
            }
            a.state = "idle";
        });
        Ok(json!({}))
    }
}
