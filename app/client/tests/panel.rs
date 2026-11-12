// SPDX-License-Identifier: MIT OR Apache-2.0
//! The client's panel and its API: the guards that keep other pages out, composing, and a call
//! carried from the panel to a station listening on the loopback adapter.

mod common;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use radiopost_client::panel::Panel;
use radiopost_client::session::{self, Options};
use serde_json::{Value, json};

use common::{loopback, scratch, store_for};

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

struct Client {
    at: SocketAddr,
}

impl Client {
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
}

#[test]
fn the_panel_keeps_other_pages_out() {
    let dir = scratch("panel-guards");
    store_for(&dir.join("facundo"), "device:facundo", "facundo");
    let at = Panel::new(&dir.join("facundo"), None)
        .start("127.0.0.1:0")
        .expect("a port");
    let host = format!("127.0.0.1:{}", at.port());
    let (status, page) = http(at, "GET", "/", &host, false, &Value::Null);
    assert_eq!(status, 200);
    assert!(page.contains("<title>RadioPost</title>"));
    let (status, icon) = http(at, "GET", "/icon.svg", &host, false, &Value::Null);
    assert_eq!((status, icon.starts_with("<svg")), (200, true), "the icon");
    // what the page marks hidden stays hidden whatever display a class gives it: the update notice
    // (`.update`, a flex box) showed empty with a blank button until this rule
    let (status, style) = http(at, "GET", "/style.css", &host, false, &Value::Null);
    assert_eq!(status, 200, "the stylesheet");
    assert!(
        style.contains("[hidden] { display: none !important; }"),
        "the stylesheet lets a class show what is hidden"
    );
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
    let compose = json!({"to": "ana@hampost.org", "subject": "x", "body": "y"});
    let (status, _) = http(at, "POST", "/api/compose", &host, false, &compose);
    assert_eq!(status, 403, "no X-RadioPost");
    let c = Client { at };
    assert_eq!(
        c.get("/api/state")["counts"]["outbox"],
        0,
        "nothing was written"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// What a user types in To and Cc: a name with its address in angle brackets, a quoted name with
/// a comma in it, and semicolons between them; the message holds the addresses alone.
#[test]
fn addresses_typed_with_names_are_read_as_addresses() {
    let dir = scratch("panel-addresses");
    store_for(&dir.join("facundo"), "device:facundo", "facundo");
    let c = Client {
        at: Panel::new(&dir.join("facundo"), None)
            .start("127.0.0.1:0")
            .expect("a port"),
    };
    let named = c
        .post(
            "/api/compose",
            &json!({"to": r#"Maria Perez <maria@Example.com>; "Lopez, Ana" <ana@hampost.org>"#,
                    "cc": "eoc@radiopost.org", "subject": "x", "body": "y"}),
        )
        .expect("composed")["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let m = c.get(&format!("/api/message?id={named}"));
    assert_eq!(
        m["to"],
        json!(["maria@example.com", "ana@hampost.org"]),
        "{m}"
    );
    assert_eq!(m["cc"], json!(["eoc@radiopost.org"]), "{m}");
    assert_eq!(c.get("/api/state")["counts"]["outbox"], 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_message_written_and_carried_from_the_panel() {
    let dir = scratch("panel-call");
    store_for(&dir.join("facundo"), "device:facundo", "facundo");
    let mut ana = store_for(&dir.join("ana"), "device:ana", "ana");
    let c = Client {
        at: Panel::new(&dir.join("facundo"), None)
            .start("127.0.0.1:0")
            .expect("a port"),
    };
    let state = c.get("/api/state");
    assert_eq!(state["me"]["address"], "facundo@radiopost.org");
    assert_eq!(state["trusted"], json!(["hampost.org", "radiopost.org"]));

    assert!(
        c.post("/api/compose", &json!({"subject": "no one"}))
            .is_err(),
        "no recipient"
    );
    // a mistyped address is refused where it can still be put right, and nothing is queued
    let refused = c
        .post(
            "/api/compose",
            &json!({"to": "ana@hampost.org, maria@example", "subject": "x", "body": "y"}),
        )
        .expect_err("refused");
    assert!(refused.contains("maria@example"), "{refused}");
    assert_eq!(c.get("/api/state")["counts"]["outbox"], 0);
    let id = c
        .post(
            "/api/compose",
            &json!({"to": "ana@hampost.org", "subject": "From the panel", "priority": 1,
                    "body": "Written in the panel.\r\n",
                    "files": [{"name": "grid.txt", "data": "RU03Mw=="}]}),
        )
        .expect("composed")["id"]
        .as_str()
        .expect("an ID")
        .to_owned();
    let outbox = c.get("/api/folder?name=outbox");
    assert_eq!(outbox[0]["id"], id.as_str());
    let m = c.get(&format!("/api/message?id={}", &id[..6]));
    assert_eq!(m["body"], "Written in the panel.\n");
    assert_eq!(
        m["files"],
        json!([{"name": "grid.txt", "type": "text/plain", "size": 4}])
    );

    let (_server, at_f, at_a) = loopback();
    let (tx, rx) = std::sync::mpsc::channel();
    let ana_runs = thread::spawn(move || {
        let mut say = |s: &str| {
            if s.starts_with("listening") {
                let _ = tx.send(());
            }
        };
        session::answer(&mut ana, &Options::new(&at_a.to_string()), &mut say).expect("answered");
        ana
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("Ana listens");
    c.post(
        "/api/call",
        &json!({"callsign": "n0call", "station": true, "adapter": at_f.to_string()}),
    )
    .expect("calling");
    assert!(
        c.post("/api/compose", &json!({"to": "x@y.z"})).is_err()
            || c.get("/api/state")["activity"]["state"] == "idle",
        "nothing is written while a session runs"
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    while c.get("/api/state")["activity"]["state"] != "idle" {
        assert!(Instant::now() < deadline, "the session never ended");
        thread::sleep(Duration::from_millis(50));
    }
    let state = c.get("/api/state");
    assert_eq!(state["activity"]["failed"], Value::Null, "{state}");
    assert_eq!(c.get("/api/folder?name=sent")[0]["id"], id.as_str());
    let log = c.get("/api/log?since=0");
    assert!(
        log["lines"]
            .as_array()
            .expect("lines")
            .iter()
            .any(|l| l.as_str().is_some_and(|l| l.starts_with("delivered"))),
        "{log}"
    );
    let ana = ana_runs.join().expect("Ana's side");
    assert_eq!(ana.inbox.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn listening_from_the_panel_can_be_stopped() {
    let dir = scratch("panel-stop");
    store_for(&dir.join("ana"), "device:ana", "ana");
    let c = Client {
        at: Panel::new(&dir.join("ana"), None)
            .start("127.0.0.1:0")
            .expect("a port"),
    };
    let (_server, _, at_a) = loopback();
    c.post("/api/listen", &json!({"adapter": at_a.to_string()}))
        .expect("listening");
    let deadline = Instant::now() + Duration::from_secs(10);
    while c.get("/api/state")["activity"]["state"] != "listening" {
        assert!(Instant::now() < deadline, "never listened");
        thread::sleep(Duration::from_millis(20));
    }
    assert!(c.post("/api/listen", &json!({})).is_err(), "one at a time");
    c.post("/api/stop", &json!({})).expect("stopped");
    while c.get("/api/state")["activity"]["state"] != "idle" {
        assert!(Instant::now() < deadline, "never stopped");
        thread::sleep(Duration::from_millis(20));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// RP-3 §5.6: a received message's held attachments are shown, and one is asked for on the next
/// call to a gateway.
#[test]
fn a_held_attachment_is_asked_for_from_the_panel() {
    let dir = scratch("panel-held");
    let mut store = store_for(&dir.join("facundo"), "device:facundo", "facundo");
    let house = common::message("maria-house");
    store
        .received(&house, "N1CALL-10", 1_794_235_200, Some(422))
        .expect("kept");
    let c = Client {
        at: Panel::new(&dir.join("facundo"), None)
            .start("127.0.0.1:0")
            .expect("a port"),
    };
    let id = house.content_id().text();
    let m = c.get(&format!("/api/message?id={}", &id[..6]));
    let held: Vec<(&str, bool)> = m["held"]
        .as_array()
        .expect("held")
        .iter()
        .map(|f| (f["name"].as_str().expect("a name"), f["asked"] == true))
        .collect();
    assert_eq!(
        held,
        [
            ("house-1.jpg", false),
            ("house-2.jpg", false),
            ("house.mp4", false)
        ]
    );
    let asked = c
        .post(
            "/api/held",
            &json!({"id": &id[..6], "names": ["house-2.jpg"]}),
        )
        .expect("asked");
    assert_eq!(asked["asked"], json!(["house-2.jpg"]));
    let m = c.get(&format!("/api/message?id={}", &id[..6]));
    assert_eq!(m["held"][1]["asked"], true);
    assert_eq!(m["held"][0]["asked"], false);
    assert_eq!(c.get("/api/state")["counts"]["files"], 1);
    assert!(
        c.post(
            "/api/held",
            &json!({"id": &id[..6], "names": ["nothing.jpg"]})
        )
        .is_err(),
        "no such attachment"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// D34: the panel says when a newer release is out, and its update runs the application's own
/// installer (here a stand-in that only says it ran).
#[test]
fn the_panel_says_a_newer_release_is_out_and_installs_it() {
    use radiopost_update::{Product, Watch};
    let server = tiny_http::Server::http("127.0.0.1:0").expect("a port");
    let base = format!(
        "http://{}",
        server.server_addr().to_ip().expect("an address")
    );
    let manifest = json!({"version": "0.1.0-beta.9", "notes": "Better.",
        "platforms": {radiopost_update::platform(): {"url": format!("{base}/x"), "signature": "s"}}})
    .to_string();
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let _ = request.respond(tiny_http::Response::from_string(manifest.clone()));
        }
    });
    let dir = scratch("panel-update");
    store_for(&dir.join("facundo"), "device:facundo", "facundo");
    let watch = Watch::new(Product::Client, "0.1.0-beta.4", &base);
    watch.check();
    let (tx, rx) = std::sync::mpsc::channel();
    watch.set_install(std::sync::Arc::new(move || {
        let _ = tx.send(());
        Err("a stand-in installer".into())
    }));
    let c = Client {
        at: Panel::new(&dir.join("facundo"), None)
            .with_updates(std::sync::Arc::clone(&watch))
            .start("127.0.0.1:0")
            .expect("a port"),
    };
    let update = &c.get("/api/state")["update"];
    assert_eq!(update["offer"]["version"], "0.1.0-beta.9");
    assert_eq!(update["can_install"], true);
    c.post("/api/update", &json!({})).expect("started");
    rx.recv_timeout(Duration::from_secs(5))
        .expect("the application's installer ran");
    let _ = std::fs::remove_dir_all(&dir);
}
