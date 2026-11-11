// SPDX-License-Identifier: MIT OR Apache-2.0
//! The gateway's panel, as its operator uses it: the guards that keep other pages out; the
//! gateway set up, registered with its post office and started from the panel; a user's call
//! carried and shown among the sessions; the post offices checked; the gateway stopped; and a
//! modem that cannot be reached waited for, and the wait stopped.

mod common;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ed25519_dalek::SigningKey;
use radiopost_client::session::{self, Kind, Options};
use radiopost_gateway::panel::Panel;
use radiopost_gateway::runner::Runner;
use radiopost_postoffice::serve::Running;
use serde_json::{Value, json};

use common::{client, draft, loopback, post_office, scratch};

/// One HTTP exchange: the status and the body.
fn http(
    at: SocketAddr,
    method: &str,
    path: &str,
    host: &str,
    marked: bool,
    body: &Value,
) -> (u16, String) {
    let mut s = TcpStream::connect(at).expect("the panel");
    let body = if method == "POST" {
        body.to_string()
    } else {
        String::new()
    };
    let mark = if marked { "X-RadioPost: 1\r\n" } else { "" };
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\n{mark}Content-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .expect("sent");
    let mut text = String::new();
    s.read_to_string(&mut text).expect("an answer");
    let status = text[9..12].parse().expect("a status");
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_owned())
        .unwrap_or_default();
    (status, body)
}

struct Page {
    at: SocketAddr,
}

impl Page {
    fn host(&self) -> String {
        format!("127.0.0.1:{}", self.at.port())
    }

    fn get(&self, path: &str) -> Value {
        let (status, body) = http(self.at, "GET", path, &self.host(), false, &Value::Null);
        assert_eq!(status, 200, "{path}: {body}");
        serde_json::from_str(&body).expect("JSON")
    }

    fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        let (status, text) = http(self.at, "POST", path, &self.host(), true, body);
        let v: Value = serde_json::from_str(&text).map_err(|_| text.clone())?;
        if status == 200 {
            Ok(v)
        } else {
            Err(v["error"].as_str().unwrap_or("").to_owned())
        }
    }

    /// Waits until the gateway's state is `state`.
    fn wait_for(&self, state: &str) -> Value {
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            let s = self.get("/api/state");
            if s["status"]["state"] == state {
                return s;
            }
            assert!(Instant::now() < until, "never {state}: {s}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn log(&self) -> Vec<String> {
        self.get("/api/log?since=0")["lines"]
            .as_array()
            .expect("lines")
            .iter()
            .map(|l| l[1].as_str().expect("a line").to_owned())
            .collect()
    }
}

#[test]
fn the_panel_keeps_other_pages_out() {
    let dir = scratch("panel-guards");
    let at = Panel::new(Arc::new(Runner::new(&dir.join("gateway"))), None)
        .start("127.0.0.1:0")
        .expect("a port");
    let host = format!("127.0.0.1:{}", at.port());
    let (status, page) = http(at, "GET", "/", &host, false, &Value::Null);
    assert_eq!(status, 200);
    assert!(page.contains("<title>RadioPost Gateway</title>"), "{page}");
    for file in ["/app.js", "/style.css", "/icon.svg"] {
        assert_eq!(
            http(at, "GET", file, &host, false, &Value::Null).0,
            200,
            "{file}"
        );
    }
    // a page that rebinds its own name to this address is refused
    let (status, _) = http(
        at,
        "GET",
        "/api/state",
        "evil.example:80",
        false,
        &Value::Null,
    );
    assert_eq!(status, 403, "another host");
    // a change without the panel's header is refused, so another site's form cannot make one
    let setup = json!({"callsign": "KK4ODA-1"});
    assert_eq!(http(at, "POST", "/api/setup", &host, false, &setup).0, 403);
    let page = Page { at };
    assert_eq!(
        page.get("/api/state")["settings"],
        Value::Null,
        "nothing made"
    );
    assert!(!dir.join("gateway").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Sets the gateway up through its panel, from nothing, as its operator does: its callsign and
/// `modem`, its post office at `base` (before DNS has it), and its registration as KK4ODA with a
/// client home that post office enrolled. Each step refused until the one before is done.
fn set_up(
    page: &Page,
    dir: &Path,
    modem: &str,
    running: &Running,
    base: &str,
    po_key: &SigningKey,
) {
    let state = page.get("/api/state");
    assert_eq!(state["settings"], Value::Null);
    assert_eq!(state["running"], false);
    assert_eq!(state["status"]["state"], "stopped");
    assert!(page.post("/api/start", &json!({})).is_err(), "not set up");

    // 1. the gateway: a callsign and its modem
    assert!(
        page.post("/api/setup", &json!({"callsign": "radio"}))
            .is_err(),
        "not a callsign"
    );
    let state = page
        .post(
            "/api/setup",
            &json!({"callsign": "kk4oda-1", "modem": modem, "country": "us",
                    "operator_checks_rules": true}),
        )
        .expect("made");
    assert_eq!(state["settings"]["callsign"], "KK4ODA-1");
    assert_eq!(state["settings"]["country"], "US");
    assert_eq!(state["settings"]["answer_at_start"], true);
    assert!(
        state["key"].as_str().is_some_and(|k| k.len() == 43),
        "{state}"
    );
    assert_eq!(state["credential"], Value::Null);
    assert!(
        page.post("/api/start", &json!({})).is_err(),
        "not registered"
    );

    // 3. its post office, before DNS has it
    page.post(
        "/api/post-office",
        &json!({"domain": "test.example", "url": format!("{base}/")}),
    )
    .expect("named");

    // 2. registered, as its operator with their client's home
    let home = dir.join("kk4oda");
    let _operator = client(&home, running, po_key, "facundo@test.example", "KK4ODA");
    let no_channel = json!({"home": home.display().to_string(), "channels": []});
    assert!(
        page.post("/api/register", &no_channel).is_err(),
        "no channel"
    );
    let registered = page
        .post(
            "/api/register",
            &json!({"home": home.display().to_string(), "grid": "EM73", "place": "Atlanta, GA",
                    "channels": [{"dial_hz": 7_101_000, "bandwidth_hz": 2300}]}),
        )
        .expect("registered");
    assert!(
        registered["said"]
            .as_str()
            .is_some_and(|s| s.starts_with("KK4ODA-1 registered")),
        "{registered}"
    );
    assert_eq!(
        registered["state"]["credential"]["address"],
        "facundo@test.example"
    );
    // RP-5 §6.4: the gateway's own credential names the gateway as its operator
    assert_eq!(registered["state"]["credential"]["operator"], "KK4ODA-1");
    let record = running.office().state.gateways["KK4ODA-1"].clone();
    assert_eq!(record["grid"], "EM73");
    assert_eq!(record["channels"][0]["dial_hz"], 7_101_000);
}

/// The operator's whole path: set up, registered, started, a call carried, checked, stopped.
#[test]
fn a_gateway_set_up_registered_and_run_from_its_panel() {
    let dir = scratch("panel-run");
    let (running, base, po_key) = post_office(&dir.join("po"));
    let (_server, at_user, at_gateway) = loopback();
    let gw = dir.join("gateway");
    let page = Page {
        at: Panel::new(Arc::new(Runner::new(&gw)), None)
            .start("127.0.0.1:0")
            .expect("a port"),
    };
    set_up(
        &page,
        &dir,
        &at_gateway.to_string(),
        &running,
        &base,
        &po_key,
    );

    // started: it listens, and holds its settings while it does
    page.post("/api/start", &json!({})).expect("started");
    let state = page.wait_for("listening");
    assert_eq!(state["running"], true);
    assert!(page.post("/api/start", &json!({})).is_err(), "once");
    let change = json!({"callsign": "KK4ODA-2", "modem": at_gateway.to_string()});
    assert!(
        page.post("/api/setup", &change).is_err(),
        "held while answering"
    );

    // a user calls with a message
    let mut user = client(
        &dir.join("wd5ema"),
        &running,
        &po_key,
        "club@test.example",
        "WD5EMA",
    );
    let out = draft(
        "club@test.example",
        "maria@example.com",
        "Net",
        "Tonight.\n",
    );
    user.queue(&out).expect("queued");
    let report = session::call(
        &mut user,
        "KK4ODA-1",
        Kind::Gateway,
        &Options::new(&at_user.to_string()),
        &mut |_| {},
    )
    .expect("called");
    assert_eq!(report.failed, None);
    let until = Instant::now() + Duration::from_secs(20);
    let sessions = loop {
        let s = page.get("/api/sessions");
        if s.as_array().is_some_and(|a| !a.is_empty()) {
            break s;
        }
        assert!(Instant::now() < until, "the session is never recorded");
        std::thread::sleep(Duration::from_millis(50));
    };
    let s = &sessions[0];
    assert_eq!(s["remote"], "WD5EMA");
    assert_eq!(s["account"], "club@test.example");
    assert_eq!(
        (s["handed_in"].as_u64(), s["picked_up"].as_u64()),
        (Some(1), Some(0))
    );
    assert_eq!(s["result"], "over");
    page.wait_for("listening");
    let log = page.log();
    assert!(log.iter().any(|l| l == "WD5EMA called"), "{log:?}");
    assert!(
        log.iter()
            .any(|l| *l == format!("listening as KK4ODA-1 on {at_gateway}")),
        "{log:?}"
    );

    // the post offices, checked now
    let state = page.post("/api/check", &json!({})).expect("checked");
    let checks = &state["status"]["checks"];
    assert_eq!(checks[0]["domain"], "test.example", "{checks}");
    assert_eq!(checks[0]["ok"], true, "{checks}");

    // stopped: it lets the modem go, and its settings may change again
    let state = page.post("/api/stop", &json!({})).expect("stopped");
    assert_eq!(state["running"], false);
    assert_eq!(state["status"]["state"], "stopped");
    assert!(page.log().iter().any(|l| l == "stopped answering"));
    page.post("/api/setup", &change)
        .expect("changed while stopped");

    // the sessions are kept for the next run
    let again = Runner::new(&gw);
    assert_eq!(again.monitor().sessions().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A modem that cannot be reached is waited for, and said; stopping ends the wait at once.
#[test]
fn a_modem_away_is_waited_for_and_the_wait_stopped() {
    let dir = scratch("panel-away");
    let (running, base, po_key) = post_office(&dir.join("po"));
    let gw = dir.join("gateway");
    let nobody = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("a port");
    let page = Page {
        at: Panel::new(Arc::new(Runner::new(&gw)), None)
            .start("127.0.0.1:0")
            .expect("a port"),
    };
    set_up(&page, &dir, &nobody.to_string(), &running, &base, &po_key);
    page.post("/api/start", &json!({})).expect("started");
    let until = Instant::now() + Duration::from_secs(20);
    let state = loop {
        let s = page.get("/api/state");
        if s["status"]["problem"].is_string() {
            break s;
        }
        assert!(Instant::now() < until, "no problem said: {s}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(state["status"]["state"], "opening");
    assert!(
        state["status"]["problem"]
            .as_str()
            .is_some_and(|p| p.contains("cannot reach the modem")),
        "{state}"
    );
    assert!(
        page.log()
            .iter()
            .any(|l| l.starts_with("waiting for the modem"))
    );
    let asked = Instant::now();
    let state = page.post("/api/stop", &json!({})).expect("stopped");
    assert!(
        asked.elapsed() < Duration::from_secs(3),
        "{:?}",
        asked.elapsed()
    );
    assert_eq!(state["status"]["state"], "stopped");
    assert_eq!(state["running"], false);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A stand-in update channel offering `version` of every program, with a file for this platform.
fn update_channel(version: &str) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("a port");
    let base = format!(
        "http://{}",
        server.server_addr().to_ip().expect("an address")
    );
    let manifest = json!({"version": version, "notes": "Better.",
        "platforms": {radiopost_update::platform(): {"url": format!("{base}/x"), "signature": "s"}}})
    .to_string();
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let _ = request.respond(tiny_http::Response::from_string(manifest.clone()));
        }
    });
    base
}

/// D34: the panel says when a newer release is out, and its update runs the application's own
/// installer (here a stand-in that only says it ran).
#[test]
fn the_panel_says_a_newer_release_is_out_and_installs_it() {
    use radiopost_update::{Product, Watch};
    let dir = scratch("panel-update");
    let watch = Watch::new(
        Product::Gateway,
        "0.1.0-beta.4",
        &update_channel("0.1.0-beta.9"),
    );
    watch.check();
    let page = Page {
        at: Panel::new(Arc::new(Runner::new(&dir.join("gateway"))), None)
            .with_updates(Arc::clone(&watch))
            .start("127.0.0.1:0")
            .expect("a port"),
    };
    let update = &page.get("/api/state")["update"];
    assert_eq!(update["current"], "0.1.0-beta.4");
    assert_eq!(update["offer"]["version"], "0.1.0-beta.9");
    assert_eq!(
        update["can_install"], false,
        "no installer yet: a download by hand"
    );
    let (tx, rx) = std::sync::mpsc::channel();
    watch.set_install(Arc::new(move || {
        let _ = tx.send(());
        Err("a stand-in installer".into())
    }));
    assert_eq!(page.get("/api/state")["update"]["can_install"], true);
    page.post("/api/update", &json!({})).expect("started");
    rx.recv_timeout(Duration::from_secs(5))
        .expect("the application's installer ran");
    let until = Instant::now() + Duration::from_secs(5);
    while page.get("/api/state")["update"]["failed"] != "a stand-in installer" {
        assert!(Instant::now() < until, "the failure is never said");
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
