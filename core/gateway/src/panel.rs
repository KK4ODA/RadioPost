// SPDX-License-Identifier: MIT OR Apache-2.0
//! The gateway's panel: the page in `app/gateway/ui/` and a JSON API under `/api/`, served on the
//! loopback interface. The gateway app (Tauri) shows it in its window; `radiopost-gateway panel`
//! serves it for a browser on the same machine, as on a gateway with no screen.
//!
//! It sets the gateway up (its callsign and modem, its post office, its registration), starts and
//! stops it, and shows what it does: its state, the session under way, the sessions it carried,
//! its log, and how its post offices answer. The settings change only while it is stopped.
//!
//! Only this machine's own pages reach it: a request must name the panel's own host (so a page
//! that rebinds a name to 127.0.0.1 is refused), and a change must carry `X-RadioPost: 1`, which
//! another site's page cannot send without a preflight the panel never grants.

use std::collections::BTreeMap;
use std::io::Read;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use radiopost::base64url;
use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, Server};

use radiopost_update::{Product, Watch};

use crate::operator;
use crate::runner::Runner;
use crate::station::{self, Settings, Station};

/// The panel's files, built into the program; a directory given serves them from disk instead.
const FILES: &[(&str, &str, &str)] = &[
    (
        "/icon.svg",
        "image/svg+xml",
        include_str!("../../../app/gateway/ui/icon.svg"),
    ),
    (
        "/",
        "text/html; charset=utf-8",
        include_str!("../../../app/gateway/ui/index.html"),
    ),
    (
        "/app.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../app/gateway/ui/app.js"),
    ),
    (
        "/style.css",
        "text/css; charset=utf-8",
        include_str!("../../../app/gateway/ui/style.css"),
    ),
];

/// Where Aether's control API is, as Aether runs it.
pub const AETHER: &str = "ws://127.0.0.1:8515/v1";

/// The gateway's panel.
#[derive(Clone)]
pub struct Panel {
    runner: Arc<Runner>,
    ui: Option<PathBuf>,
    addr: SocketAddr,
    updates: Arc<Watch>,
}

/// A callsign as RP-1 writes one: letters and digits, a `/` part or a `-` SSID allowed.
fn callsign_ok(c: &str) -> bool {
    c.len() >= 3
        && c.len() <= 16
        && c.chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '/' || ch == '-')
        && c.chars().any(|ch| ch.is_ascii_digit())
}

impl Panel {
    /// A panel for the gateway `runner` runs, serving its page from `ui` when given.
    #[must_use]
    pub fn new(runner: Arc<Runner>, ui: Option<PathBuf>) -> Panel {
        Panel {
            runner,
            ui,
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            updates: Watch::new(
                Product::Tools,
                env!("CARGO_PKG_VERSION"),
                &radiopost_update::channel(),
            ),
        }
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
    /// The address cannot be taken, or is not a loopback address.
    pub fn start(mut self, addr: &str) -> Result<SocketAddr, String> {
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
                let panel = self.clone();
                // a request that waits on a post office holds up no other
                thread::spawn(move || panel.handle(request));
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
        let _ = request.as_reader().take(1 << 20).read_to_string(&mut body);
        let body: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        let (code, value) = match self.api(&method, path, query, &body) {
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

    /// Settings change only while the gateway is stopped: it holds them while it answers.
    fn stopped(&self) -> Result<(), String> {
        if self.runner.running() {
            Err("stop the gateway first: it holds its settings while it answers".into())
        } else {
            Ok(())
        }
    }

    fn api(&self, method: &Method, path: &str, query: &str, body: &Value) -> Result<Value, String> {
        let param = |name: &str| {
            query
                .split('&')
                .find_map(|kv| kv.strip_prefix(name)?.strip_prefix('='))
                .unwrap_or("")
                .to_owned()
        };
        let dir = self.runner.dir();
        let monitor = self.runner.monitor();
        match (method, path) {
            (Method::Get, "/api/state") => Ok(self.state()),
            (Method::Get, "/api/log") => {
                let (lines, next) = monitor.log_since(param("since").parse().unwrap_or(0));
                Ok(json!({"lines": lines, "next": next}))
            }
            (Method::Get, "/api/sessions") => Ok(json!(monitor.sessions())),
            (Method::Get, "/api/held") => Ok(held(dir)),
            (Method::Post, "/api/setup") => {
                self.stopped()?;
                self.setup(body)?;
                Ok(self.state())
            }
            (Method::Post, "/api/post-office") => {
                self.stopped()?;
                let mut s = Settings::load(dir)?;
                let domain = text(body, "domain").to_lowercase();
                if !domain.contains('.') {
                    return Err("a post office's domain, such as radiopost.org".into());
                }
                match text(body, "url").trim_end_matches('/') {
                    "" => {
                        s.post_offices.remove(&domain);
                    }
                    url if url.starts_with("https://") || url.starts_with("http://") => {
                        s.post_offices.insert(domain, url.to_owned());
                    }
                    _ => return Err("the post office's API address starts with https://".into()),
                }
                s.save(dir)?;
                Ok(self.state())
            }
            (Method::Post, "/api/register") => {
                self.stopped()?;
                let said = self.register(body)?;
                Ok(json!({"said": said, "state": self.state()}))
            }
            (Method::Post, "/api/credential") => {
                self.stopped()?;
                station::keep_credential(dir, text(body, "hex").as_bytes())?;
                Ok(self.state())
            }
            (Method::Post, "/api/start") => {
                self.runner.start()?;
                Ok(self.state())
            }
            (Method::Post, "/api/stop") => {
                self.runner.stop();
                Ok(self.state())
            }
            (Method::Post, "/api/check") => {
                self.runner.check()?;
                Ok(self.state())
            }
            (Method::Post, "/api/update") => {
                if self.updates.status()["can_install"] == true {
                    self.updates.install()?;
                } else {
                    self.updates.download()?;
                }
                Ok(self.state())
            }
            _ => Err(format!("no {method} {path}")),
        }
    }

    /// The gateway made, or its settings changed.
    fn setup(&self, body: &Value) -> Result<(), String> {
        let dir = self.runner.dir();
        let callsign = text(body, "callsign").to_uppercase();
        if !callsign_ok(&callsign) {
            return Err("the gateway's callsign, such as KK4ODA-1".into());
        }
        let modem = match text(body, "modem") {
            "" => AETHER.to_owned(),
            m => m.to_owned(),
        };
        let country = match text(body, "country").to_uppercase() {
            c if c.is_empty() => "US".to_owned(),
            c if c.len() == 2 && c.chars().all(|ch| ch.is_ascii_uppercase()) => c,
            _ => return Err("the country as two letters, such as US".into()),
        };
        let checks_rules = body["operator_checks_rules"].as_bool().unwrap_or(false);
        let at_start = body["answer_at_start"].as_bool().unwrap_or(true);
        let copies = copies_of(&body["copies"])?;
        match Settings::load(dir) {
            Ok(mut s) => {
                s.callsign = callsign;
                s.modem = modem;
                s.country = country;
                s.operator_checks_rules = checks_rules;
                s.answer_at_start = at_start;
                s.copies = copies;
                s.save(dir)
            }
            Err(_) => station::make(
                dir,
                &Settings {
                    callsign,
                    modem,
                    country,
                    post_offices: BTreeMap::new(),
                    resolver: "https://dns.google/resolve".into(),
                    blocked: Vec::new(),
                    only: None,
                    operator_checks_rules: checks_rules,
                    answer_at_start: at_start,
                    copies,
                },
            ),
        }
    }

    /// RP-5 §6.4: the gateway registered with its operator's post office, logged in as the
    /// operator with the RadioPost app's enrollment on this computer.
    fn register(&self, body: &Value) -> Result<String, String> {
        let home = match text(body, "home") {
            "" => operator::client_home(),
            h => PathBuf::from(h),
        };
        let channels: Vec<Value> = body["channels"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| {
                let hz = c["dial_hz"].as_u64().filter(|hz| *hz >= 1_800_000);
                let bw = c["bandwidth_hz"]
                    .as_u64()
                    .filter(|bw| [500, 2300].contains(bw));
                match (hz, bw) {
                    (Some(hz), Some(bw)) => Ok(json!({
                        "dial_hz": hz,
                        "mode": c["mode"].as_str().filter(|m| !m.is_empty()).unwrap_or("USB"),
                        "modem": c["modem"].as_str().filter(|m| !m.is_empty()).unwrap_or("aether"),
                        "bandwidth_hz": bw,
                    })),
                    _ => Err(
                        "each channel has its dial frequency in hertz and a bandwidth of \
                              500 or 2300 Hz"
                            .to_owned(),
                    ),
                }
            })
            .collect::<Result<_, _>>()?;
        if channels.is_empty() {
            return Err("at least one channel: where callers find the gateway".into());
        }
        let hours = match text(body, "hours") {
            "" => "always",
            h => h,
        };
        let mut record = json!({"channels": channels, "hours": hours, "internet": true});
        for key in ["grid", "place"] {
            if !text(body, key).is_empty() {
                record[key] = text(body, key).into();
            }
        }
        operator::register(self.runner.dir(), &home, record)
    }

    /// What the page shows.
    fn state(&self) -> Value {
        let dir = self.runner.dir();
        let settings = Settings::load(dir).ok();
        let key = station::key(dir)
            .ok()
            .map(|k| base64url::encode(k.verifying_key().as_bytes()));
        let credential = Station::load(dir)
            .ok()
            .and_then(|s| s.credential())
            .map(|b| {
                json!({"operator": b.operator, "address": b.address, "serial": b.serial,
                       "expires": b.expires})
            });
        let home = operator::client_home();
        let enrolled = operator::enrolled_as(&home)
            .ok()
            .map(|b| json!({"address": b.address, "operator": b.operator}));
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "dir": dir.display().to_string(),
            "settings": settings,
            "key": key,
            "credential": credential,
            "running": self.runner.running(),
            "status": self.runner.monitor().status(),
            "client": {"home": home.display().to_string(), "enrolled": enrolled},
            "held": held_summary(dir),
            "aether": AETHER,
            "update": self.updates.status(),
        })
    }
}

/// When `store.json` was last read for the page's header, and what it said.
type Summary = Option<(
    std::path::PathBuf,
    Option<std::time::SystemTime>,
    u64,
    Value,
)>;

/// What the gateway holds, in numbers, for the page's header (W27, W28): read again only when
/// `store.json` changed, since the page asks every second and the copies' text makes it large.
fn held_summary(dir: &std::path::Path) -> Value {
    static LAST: std::sync::Mutex<Summary> = std::sync::Mutex::new(None);
    let meta = std::fs::metadata(dir.join("store.json")).ok();
    let stamp = (
        meta.as_ref().and_then(|m| m.modified().ok()),
        meta.as_ref().map_or(0, std::fs::Metadata::len),
    );
    let mut last = LAST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((d, when, len, v)) = last.as_ref()
        && d == dir
        && (*when, *len) == stamp
    {
        return v.clone();
    }
    let s = crate::store::Store::load(dir);
    let v = json!({
        "copies": s.copies.iter().filter(|c| !c.picked).count(),
        "users": s.users().len(),
        "queued": s.records.iter().map(|r| r.messages.len()).sum::<usize>(),
        "records": s.records.len(),
        "forwards": s.forwards.len(),
    });
    *last = Some((dir.to_owned(), stamp.0, stamp.1, v.clone()));
    v
}

/// What the gateway holds, for the page's Held tab: the copies placed ahead by user (never their
/// text), the queue waiting to be reported, the last reports, the post offices it polls and the
/// keys it keeps.
fn held(dir: &std::path::Path) -> Value {
    let s = crate::store::Store::load(dir);
    let copies: Vec<Value> = s
        .copies
        .iter()
        .map(|c| {
            json!({"address": c.address, "serial": c.serial, "from": c.from,
                   "subject": c.subject, "size": c.size, "held": c.air.is_none(),
                   "expires": c.expires, "by": c.by, "picked": c.picked})
        })
        .collect();
    let queue: Vec<Value> = s
        .records
        .iter()
        .map(|r| {
            json!({"operator": r.operator, "address": r.address, "started": r.started,
                   "messages": r.messages.len(), "pickups": r.pickups.len(), "domain": r.domain,
                   "done": r.done, "tries": r.tries, "error": r.error})
        })
        .collect();
    let keys: Value = s
        .keys
        .iter()
        .map(|(d, k)| {
            (
                d.clone(),
                json!({"how": k.how, "fetched": k.fetched, "error": k.error,
                       "keys": k.found.keys.len(), "revoked": k.found.serials.len()}),
            )
        })
        .collect::<serde_json::Map<_, _>>()
        .into();
    json!({"copies": copies, "queue": queue, "reported": s.reported, "polls": s.polls,
           "keys": keys, "forwards": s.forwards.len()})
}

/// RP-5 §4.4 `copies`: whether the gateway holds copies placed ahead, and its limits.
fn copies_of(v: &Value) -> Result<Option<station::CopiesSettings>, String> {
    if v.is_null() || v == &Value::Bool(false) || v["on"] == Value::Bool(false) {
        return Ok(None);
    }
    let n = |k: &str| v[k].as_u64().filter(|n| *n > 0);
    match (n("users"), n("days"), n("max_air_bytes")) {
        (Some(users), Some(days), Some(max_air_bytes)) if days <= 60 => {
            Ok(Some(station::CopiesSettings {
                users,
                days,
                max_air_bytes,
            }))
        }
        _ => Err(
            "copies placed ahead: how many users, how many days (at most 60) and the largest \
             message in bytes"
                .into(),
        ),
    }
}

fn text<'a>(body: &'a Value, name: &str) -> &'a str {
    body[name].as_str().unwrap_or("").trim()
}

#[cfg(test)]
mod tests {
    use super::callsign_ok;

    #[test]
    fn callsigns() {
        for ok in ["KK4ODA-1", "N0CALL", "W4/G4ABC", "KK4ODA"] {
            assert!(callsign_ok(ok), "{ok}");
        }
        for bad in ["", "KK", "kk4oda", "RADIO", "KK4ODA 1"] {
            assert!(!callsign_ok(bad), "{bad}");
        }
    }
}
