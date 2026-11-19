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

use common::{loopback, loopback_dialled, scratch, store_for};

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
    // what the session carried, as the progress rings show it: the one message, whole
    let sending = &state["progress"]["sending"];
    assert_eq!(
        (&sending["messages"], &sending["whole"]),
        (&json!(1), &json!(1)),
        "{state}"
    );
    assert!(
        sending["bytes"].as_u64().is_some_and(|b| b > 100),
        "{state}"
    );
    assert_eq!(sending["crossed"], sending["bytes"], "{state}");
    assert_eq!(state["progress"]["receiving"]["messages"], 0, "{state}");
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

/// RP-2 §8 (the 40 m sessions of 2026-11-18): an ICS-213 with a file, sent P2P from the panel,
/// reaches the station called as a form. Its panel gives the form's name and each field filled in,
/// label and value, in the template's order, the subject, date and time the message's own; the
/// sender's note stays the message's text. The station called had shown the form's text in the
/// message's text box, the labels typed out like any other words.
#[test]
fn a_form_sent_p2p_reads_as_the_form() {
    let dir = scratch("panel-form-p2p");
    store_for(&dir.join("facundo"), "device:facundo", "facundo");
    let mut ana = store_for(&dir.join("ana"), "device:ana", "ana");
    let c = Client {
        at: Panel::new(&dir.join("facundo"), None)
            .start("127.0.0.1:0")
            .expect("a port"),
    };
    let ics213 = c.get("/api/forms")[0].clone();
    assert_eq!(ics213["name"], "ICS-213 General Message");
    let id = c
        .post(
            "/api/compose",
            &json!({"to": "ana@hampost.org", "subject": "Cots and water", "priority": 1,
                    "body": "Sent from the mobile.",
                    "files": [{"name": "grid.txt", "data": "RU03Mw=="}],
                    "form": {"template": ics213["template"],
                             "values": {"1": "Shelter 3", "2": "Ana Lopez, EOC",
                                        "3": "Facundo, net control",
                                        "7": "Forty cots by 18:00.\nWater for 120.",
                                        "8": "J. Ortega"}}}),
        )
        .expect("composed")["id"]
        .as_str()
        .expect("an ID")
        .to_owned();

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
    let deadline = Instant::now() + Duration::from_secs(20);
    while c.get("/api/state")["activity"]["state"] != "idle" {
        assert!(Instant::now() < deadline, "the session never ended");
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(c.get("/api/folder?name=sent")[0]["id"], id.as_str());
    let ana = ana_runs.join().expect("Ana's side");
    assert_eq!(ana.inbox.len(), 1);
    drop(ana);

    // Ana's panel, after the session: the message received P2P is the one sent, form and all
    let a = Client {
        at: Panel::new(&dir.join("ana"), None)
            .start("127.0.0.1:0")
            .expect("a port"),
    };
    assert_eq!(a.get("/api/folder?name=inbox")[0]["id"], id.as_str());
    let m = a.get(&format!("/api/message?id={}", &id[..6]));
    assert_eq!(m["subject"], "Cots and water", "{m}");
    assert_eq!(
        m["files"],
        json!([{"name": "grid.txt", "type": "text/plain", "size": 4}])
    );
    let form = &m["form"];
    assert_eq!(form["template"], ics213["template"], "{m}");
    assert_eq!(form["name"], "ICS-213 General Message", "{m}");
    assert_eq!(form["title"], "ICS-213 General Message", "{m}");
    assert_eq!(form["filled"], true, "{m}");
    // the date and time are the message's own, as the sender's panel stamped them, written as the
    // form's text writes them (RP-2 §8.4)
    let minutes = m["date"].as_u64().expect("a date") % 86_400 / 60;
    let time = format!("{:02}:{:02} UTC", minutes / 60, minutes % 60);
    let text = form["text"].as_str().expect("the form's text");
    let date = text
        .lines()
        .find_map(|l| l.strip_prefix("Date: "))
        .unwrap_or_else(|| panic!("no date in {text}"));
    assert_eq!(date.len(), "2026-11-18".len(), "{text}");
    assert!(text.contains(&format!("\nTime: {time}\n")), "{text}");
    assert_eq!(
        form["rows"],
        json!([
            {"n": 1, "label": "Incident name", "value": "Shelter 3", "lines": false},
            {"n": 2, "label": "To (name and position)", "value": "Ana Lopez, EOC", "lines": false},
            {"n": 3, "label": "From (name and position)", "value": "Facundo, net control", "lines": false},
            {"n": 4, "label": "Subject", "value": "Cots and water", "lines": false},
            {"n": 5, "label": "Date", "value": date, "lines": false},
            {"n": 6, "label": "Time", "value": time, "lines": false},
            {"n": 7, "label": "Message", "value": "Forty cots by 18:00.\nWater for 120.", "lines": true},
            {"n": 8, "label": "Approved by (name and position)", "value": "J. Ortega", "lines": false},
        ]),
        "{m}"
    );
    // the note the sender added below the form is the message's text, apart from the form
    assert_eq!(m["body"], "Sent from the mobile.", "{m}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The reader's **Save** (the author's ask of 2026-11-18): a message's attachment saved as a file,
/// under a safe name whatever the other station called it, never over a file already there; and
/// a form's print layout, the paper form it comes from, given with its template.
#[test]
fn an_attachment_is_saved_and_a_form_prints_as_its_paper_form() {
    let dir = scratch("panel-save");
    store_for(&dir.join("facundo"), "device:facundo", "facundo");
    let saved = dir.join("Downloads");
    let c = Client {
        at: Panel::new(&dir.join("facundo"), None)
            .saving_in(&saved)
            .start("127.0.0.1:0")
            .expect("a port"),
    };
    let id = c
        .post(
            "/api/compose",
            &json!({"to": "ana@hampost.org", "subject": "Two files", "body": "",
                    "files": [{"name": "grid.txt", "data": "RU03Mw=="},
                              {"name": "../../evil.txt", "data": "bm8="}]}),
        )
        .expect("composed")["id"]
        .as_str()
        .expect("an ID")
        .to_owned();
    let save = |n: u64| c.post("/api/save", &json!({"id": &id[..6], "n": n}));
    let first = save(0).expect("saved");
    assert_eq!(first["name"], "grid.txt", "{first}");
    assert_eq!(
        std::fs::read(saved.join("grid.txt")).expect("the file"),
        b"EM73"
    );
    // saved again, it goes beside the first, which stays as it was
    let second = save(0).expect("saved again");
    assert_eq!(second["name"], "grid (2).txt", "{second}");
    assert!(saved.join("grid (2).txt").is_file());
    // a name that would climb out of the folder stays in it
    let climbing = save(1).expect("saved");
    assert_eq!(climbing["name"], "evil.txt", "{climbing}");
    assert_eq!(
        std::fs::read(saved.join("evil.txt")).expect("the file"),
        b"no"
    );
    assert!(!dir.join("evil.txt").exists());
    assert!(
        save(2)
            .expect_err("no third")
            .contains("no such attachment")
    );

    // the published ICS-213 prints as FEMA's ICS 213, its layout named by the template's fingerprint
    let ics213 = &c.get("/api/forms")[0];
    let print = ics213["print"].as_str().expect("a print layout");
    assert!(
        print.contains(&format!(
            "data-template=\"{}\"",
            ics213["template"].as_str().expect("a fingerprint")
        )),
        "{print}"
    );
    assert!(print.contains("GENERAL MESSAGE (ICS 213)"), "{print}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// W14, RP-4 §4.1, §4.3 (the author's notes of 2026-11-17): a channel picked is tuned at once, so
/// the radio and the antenna tuner can be set before the call; the tuner tone keys and stops; a
/// station call carries its dial and bandwidth, finds the radio there and leaves it there. A modem
/// with no dial says plainly which dial to set by hand.
#[test]
#[allow(clippy::too_many_lines, reason = "one story, told in order")]
fn a_channel_picked_is_tuned_at_once_and_a_station_call_keeps_it() {
    let dir = scratch("panel-tune");
    store_for(&dir.join("facundo"), "device:facundo", "facundo");
    let mut ana = store_for(&dir.join("ana"), "device:ana", "ana");
    let c = Client {
        at: Panel::new(&dir.join("facundo"), None)
            .start("127.0.0.1:0")
            .expect("a port"),
    };
    let (server, at_f, at_a) = loopback_dialled(Some(7_101_000));
    let modem = at_f.to_string();
    let picked = c
        .post(
            "/api/tune",
            &json!({"adapter": modem, "dial_hz": 3_591_500}),
        )
        .expect("tuned");
    assert_eq!(
        picked,
        json!({"tuned": "moved", "dial_hz": 3_591_500,
               "said": "tuned the radio to 3.5915 MHz", "tone": true})
    );
    assert_eq!(server.with_channel(|ch| ch.dial(0)), Some(3_591_500));
    let again = c
        .post(
            "/api/tune",
            &json!({"adapter": modem, "dial_hz": 3_591_500}),
        )
        .expect("there");
    assert_eq!(again["tuned"], "there");
    assert_eq!(
        c.post("/api/tone", &json!({"adapter": modem, "on": true})),
        Ok(json!({"on": true, "seconds": 10}))
    );
    assert_eq!(
        c.post("/api/tone", &json!({"adapter": modem, "on": false})),
        Ok(json!({"on": false, "seconds": 0}))
    );

    // a station call on that dial, at 500 Hz: the radio is there already, and stays
    let (tx, rx) = std::sync::mpsc::channel();
    let ana_runs = thread::spawn(move || {
        let mut say = |s: &str| {
            if s.starts_with("listening") {
                let _ = tx.send(());
            }
        };
        session::answer(&mut ana, &Options::new(&at_a.to_string()), &mut say).expect("answered");
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("Ana listens");
    let from = c.get("/api/log?since=0")["next"].as_u64().expect("next");
    assert_eq!(
        c.post(
            "/api/call",
            &json!({"callsign": "n0call", "station": true, "adapter": modem,
                    "dial_hz": 3_591_500, "bandwidth": 500}),
        ),
        Ok(json!({}))
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    while c.get("/api/state")["activity"]["state"] != "idle" {
        assert!(Instant::now() < deadline, "the session never ended");
        thread::sleep(Duration::from_millis(50));
    }
    ana_runs.join().expect("Ana's side");
    let state = c.get("/api/state");
    assert_eq!(state["activity"]["failed"], Value::Null, "{state}");
    let log: Vec<String> = c.get(&format!("/api/log?since={from}"))["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .filter_map(|l| l.as_str().map(str::to_owned))
        .collect();
    assert!(
        log.iter().any(|l| l == "calling N0CALL at 500 Hz"),
        "{log:?}"
    );
    assert!(
        log.iter().any(|l| l == "connected to N0CALL at 500 Hz"),
        "{log:?}"
    );
    assert!(
        !log.iter().any(|l| l.starts_with("tuned")),
        "not tuned again, nor back: {log:?}"
    );
    assert_eq!(server.with_channel(|ch| ch.dial(0)), Some(3_591_500));
    assert!(
        c.post(
            "/api/call",
            &json!({"callsign": "n0call", "station": true, "adapter": modem, "bandwidth": 1800}),
        )
        .is_err_and(|e| e.contains("500 or 2300")),
        "a bandwidth no one runs"
    );

    // a modem with no dial: the operator is told which dial to set, by hand
    let (_plain, at_plain, _) = loopback();
    let picked = c
        .post(
            "/api/tune",
            &json!({"adapter": at_plain.to_string(), "dial_hz": 7_101_000}),
        )
        .expect("said");
    assert_eq!(
        picked,
        json!({"tuned": "by_hand", "dial_hz": 7_101_000,
               "said": "set the radio's dial to 7.101 MHz (USB) yourself: the modem cannot set it",
               "tone": false})
    );
    let refused = c
        .post(
            "/api/tone",
            &json!({"adapter": at_plain.to_string(), "on": true}),
        )
        .expect_err("no tone");
    assert!(refused.contains("cannot key a tuner tone"), "{refused}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Listen is a switch that stays on (the 40 m sessions of 2026-11-18): after a session the
/// station is listening again, the switch still on, until it is pressed again.
#[test]
fn listening_from_the_panel_stays_on_until_pressed_again() {
    let dir = scratch("panel-stop");
    store_for(&dir.join("ana"), "device:ana", "ana");
    let mut facundo = store_for(&dir.join("facundo"), "device:facundo", "facundo");
    facundo
        .queue(&common::message("facundo-direct"))
        .expect("queued");
    let c = Client {
        at: Panel::new(&dir.join("ana"), None)
            .start("127.0.0.1:0")
            .expect("a port"),
    };
    let (_server, at_f, at_a) = loopback();
    c.post("/api/listen", &json!({"adapter": at_a.to_string()}))
        .expect("listening");
    let deadline = Instant::now() + Duration::from_secs(10);
    let activity = || c.get("/api/state")["activity"].clone();
    while !c
        .get("/api/log?since=0")
        .to_string()
        .contains("listening as N0CALL")
    {
        assert!(Instant::now() < deadline, "never listened");
        thread::sleep(Duration::from_millis(20));
    }
    let a = activity();
    assert_eq!(
        (&a["state"], &a["listen"]),
        (&json!("listening"), &json!(true))
    );
    assert!(c.post("/api/listen", &json!({})).is_err(), "one at a time");
    let called = session::call(
        &mut facundo,
        "N0CALL",
        session::Kind::Station,
        &Options::new(&at_f.to_string()),
        &mut |_| {},
    )
    .expect("called");
    assert_eq!(called.failed, None);
    while !c
        .get("/api/log?since=0")
        .to_string()
        .contains("listening again")
    {
        assert!(Instant::now() < deadline, "never listened again");
        thread::sleep(Duration::from_millis(20));
    }
    let a = activity();
    assert_eq!(
        (&a["state"], &a["listen"], &a["failed"]),
        (&json!("listening"), &json!(true), &Value::Null),
        "{a}"
    );
    assert_eq!(c.get("/api/state")["counts"]["inbox"], 1);
    c.post("/api/stop", &json!({})).expect("stopped");
    assert_eq!(activity()["listen"], false, "off at once");
    while activity()["state"] != "idle" {
        assert!(Instant::now() < deadline, "never stopped");
        thread::sleep(Duration::from_millis(20));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The content ID the panel gave a message written.
fn written(v: &Value) -> String {
    v["id"].as_str().expect("an id").to_owned()
}

/// A house station may listen for P2P for days (the 40 m sessions of 2026-11-18), so mail is
/// written, edited and deleted while it waits; the modem stays the station's. The next station to
/// call takes what was written: the message written and the one edited go, the one deleted stays
/// in the trash. Messages written as the call comes in are carried or stay in the outbox, never
/// lost to the session's own save; during the session they are refused, saying why.
#[test]
#[allow(clippy::too_many_lines, reason = "one story, told in order")]
fn mail_written_while_listening_goes_to_the_next_caller() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let dir = scratch("panel-listen-write");
    store_for(&dir.join("ana"), "device:ana", "ana");
    let mut facundo = store_for(&dir.join("facundo"), "device:facundo", "facundo");
    let c = Client {
        at: Panel::new(&dir.join("ana"), None)
            .start("127.0.0.1:0")
            .expect("a port"),
    };
    let (_server, at_f, at_a) = loopback();
    c.post("/api/listen", &json!({"adapter": at_a.to_string()}))
        .expect("listening");
    let deadline = Instant::now() + Duration::from_secs(10);
    let said = |line: &str| c.get("/api/log?since=0").to_string().contains(line);
    while !said("listening as N0CALL") {
        assert!(Instant::now() < deadline, "never listened");
        thread::sleep(Duration::from_millis(20));
    }

    // written, deleted and edited while the station waits for a call
    let write = |subject: &str| {
        written(
            &c.post(
                "/api/compose",
                &json!({"to": "facundo@radiopost.org", "subject": subject, "body": "73"}),
            )
            .expect("written while listening"),
        )
    };
    let kept = write("kept");
    let gone = write("gone");
    let draft = write("draft");
    c.post("/api/delete", &json!({"folder": "outbox", "id": gone}))
        .expect("deleted while listening");
    assert_eq!(c.get(&format!("/api/edit?id={draft}"))["subject"], "draft");
    let edited = written(
        &c.post(
            "/api/compose",
            &json!({"edit": draft, "to": "facundo@radiopost.org", "subject": "edited",
                    "body": "73 de N0CALL"}),
        )
        .expect("edited while listening"),
    );
    let a = c.get("/api/state")["activity"].clone();
    assert_eq!(
        (&a["state"], &a["listen"]),
        (&json!("listening"), &json!(true))
    );

    // the modem stays the station's: no call, no tuning, no tuner tone while it listens
    let modem = at_a.to_string();
    for (path, body) in [
        ("/api/call", json!({"callsign": "kk4oda", "adapter": modem})),
        ("/api/tune", json!({"adapter": modem, "dial_hz": 7_103_000})),
        ("/api/tone", json!({"adapter": modem, "on": true})),
    ] {
        let why = c.post(path, &body).expect_err("the modem is listening");
        assert!(why.contains("listening for P2P"), "{path}: {why}");
    }

    // a station calls while more is written
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let (stop, at) = (Arc::clone(&stop), c.at);
        thread::spawn(move || {
            let c = Client { at };
            let (mut taken, mut refused) = (Vec::new(), Vec::new());
            for n in 0.. {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let body = json!({"to": "facundo@radiopost.org", "subject": format!("as it calls {n}"),
                                  "body": "73"});
                match c.post("/api/compose", &body) {
                    Ok(v) => taken.push(written(&v)),
                    Err(why) => refused.push(why),
                }
                thread::sleep(Duration::from_millis(10));
            }
            (taken, refused)
        })
    };
    thread::sleep(Duration::from_millis(100));
    let called = session::call(
        &mut facundo,
        "N0CALL",
        session::Kind::Station,
        &Options::new(&at_f.to_string()),
        &mut |_| {},
    )
    .expect("called");
    assert_eq!(called.failed, None);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !said("listening again") {
        assert!(Instant::now() < deadline, "never listened again");
        thread::sleep(Duration::from_millis(20));
    }
    thread::sleep(Duration::from_millis(100));
    stop.store(true, Ordering::Relaxed);
    let (taken, refused) = writer.join().expect("the writer");
    for why in &refused {
        assert!(why.contains("a session is under way"), "{why}");
    }

    // nothing written is lost: each message is carried to Facundo or waits in the outbox
    let ana = radiopost_client::store::Store::open(&dir.join("ana")).expect("Ana's home");
    let sent: Vec<String> = ana.sent.iter().map(radiopost::ContentId::text).collect();
    let outbox: Vec<String> = ana.outbox.iter().map(|w| w.id.text()).collect();
    for id in [&kept, &edited] {
        assert!(
            sent.contains(id),
            "{id} was written before the call: {sent:?}"
        );
    }
    for id in &taken {
        assert!(
            sent.contains(id) || outbox.contains(id),
            "{id} is lost: sent {sent:?}, outbox {outbox:?}"
        );
        ana.message(&ana.find(id).expect("kept")).expect("its file");
    }
    let mut carried: Vec<String> = facundo.inbox.iter().map(|m| m.id.text()).collect();
    let mut went = sent.clone();
    carried.sort();
    went.sort();
    assert_eq!(carried, went, "what Ana sent is what Facundo has");
    assert!(!sent.contains(&gone) && !outbox.contains(&gone));
    assert_eq!(ana.trash.len(), 1, "the one deleted");
    assert!(
        !sent.contains(&draft) && !outbox.contains(&draft),
        "the edit took its place"
    );

    // and listening goes on, the switch still on, mail still written
    let a = c.get("/api/state")["activity"].clone();
    assert_eq!(
        (&a["state"], &a["listen"], &a["failed"]),
        (&json!("listening"), &json!(true), &Value::Null),
        "{a}"
    );
    write("after the call");
    c.post("/api/stop", &json!({})).expect("stopped");
    while c.get("/api/state")["activity"]["state"] != "idle" {
        assert!(Instant::now() < deadline, "never stopped");
        thread::sleep(Duration::from_millis(20));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The address list: whom a message was written to is kept with the names typed, one is added
/// and renamed by hand, one is taken off and put back as it was, and the list changes while a
/// session runs (it is a file of its own, which the session never writes).
#[test]
fn the_address_list_learns_and_forgets() {
    let dir = scratch("panel-address-list");
    store_for(&dir.join("ana"), "device:ana", "ana");
    let c = Client {
        at: Panel::new(&dir.join("ana"), None)
            .start("127.0.0.1:0")
            .expect("a port"),
    };
    assert_eq!(c.get("/api/addresses")["entries"], json!([]));
    c.post(
        "/api/compose",
        &json!({"to": "Maria Perez <maria@example.com>, eoc@radiopost.org",
                "cc": "\"Lopez, Ana\" <ana@hampost.org>", "subject": "x", "body": "y"}),
    )
    .expect("composed");
    let book = c.get("/api/addresses")["entries"].clone();
    let names: Vec<(String, String)> = book
        .as_array()
        .expect("a list")
        .iter()
        .map(|e| {
            (
                e["name"].as_str().unwrap_or("").to_owned(),
                e["address"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect();
    assert_eq!(
        names,
        [
            (String::new(), "eoc@radiopost.org".to_owned()),
            ("Lopez, Ana".to_owned(), "ana@hampost.org".to_owned()),
            ("Maria Perez".to_owned(), "maria@example.com".to_owned()),
        ],
        "{book}"
    );
    assert_eq!(book[2]["uses"], 1);
    assert!(
        c.post("/api/addresses", &json!({"add": "bob at example"}))
            .is_err(),
        "not an address"
    );
    let after = c
        .post("/api/addresses", &json!({"add": "EOC <eoc@radiopost.org>"}))
        .expect("renamed");
    assert_eq!(after["entries"][0]["name"], "EOC", "{after}");

    // during a session the list still changes
    let (_server, _, at_a) = loopback();
    c.post("/api/listen", &json!({"adapter": at_a.to_string()}))
        .expect("listening");
    let deadline = Instant::now() + Duration::from_secs(10);
    while c.get("/api/state")["activity"]["state"] != "listening" {
        assert!(Instant::now() < deadline, "never listened");
        thread::sleep(Duration::from_millis(20));
    }
    let gone = c
        .post("/api/addresses", &json!({"remove": "maria@Example.COM"}))
        .expect("removed");
    assert_eq!(gone["entries"].as_array().map(Vec::len), Some(2));
    assert_eq!(gone["removed"]["name"], "Maria Perez");
    let back = c
        .post("/api/addresses", &json!({"put_back": gone["removed"]}))
        .expect("put back");
    assert_eq!(back["entries"][2], book[2], "as it was, its count with it");
    c.post("/api/stop", &json!({})).expect("stopped");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The trash: a message deleted from the outbox is listed there with the folder it came from,
/// goes back, and goes for good; the trash is emptied; listening for P2P, the folders change
/// between calls; and nothing changes during a call, which keeps the folders until it ends.
#[test]
#[allow(clippy::too_many_lines, reason = "one story, told in order")]
fn messages_go_to_the_trash_and_back() {
    let dir = scratch("panel-trash");
    store_for(&dir.join("ana"), "device:ana", "ana");
    let c = Client {
        at: Panel::new(&dir.join("ana"), None)
            .start("127.0.0.1:0")
            .expect("a port"),
    };
    let write = |subject: &str| -> String {
        c.post(
            "/api/compose",
            &json!({"to": "maria@example.com", "subject": subject, "body": "73"}),
        )
        .expect("composed")["id"]
            .as_str()
            .expect("an id")
            .to_owned()
    };
    let (one, two) = (write("one"), write("two"));
    let count = |name: &str| {
        c.get("/api/state")["counts"][name]
            .as_u64()
            .expect("a count")
    };
    c.post("/api/delete", &json!({"folder": "outbox", "id": one}))
        .expect("deleted");
    assert_eq!((count("outbox"), count("trash")), (1, 1));
    let trash = c.get("/api/folder?name=trash");
    assert_eq!(trash[0]["id"], one.as_str(), "{trash}");
    assert_eq!(trash[0]["was"], "outbox");
    assert!(trash[0]["deleted"].as_u64().is_some_and(|t| t > 0));
    assert!(
        c.post("/api/delete", &json!({"folder": "inbox", "id": two}))
            .is_err(),
        "not in the inbox"
    );
    assert!(
        c.post("/api/purge", &json!({"id": two, "was": "outbox"}))
            .is_err(),
        "not in the trash"
    );
    c.post("/api/restore", &json!({"id": one, "was": "outbox"}))
        .expect("restored");
    let outbox: Vec<Value> = c
        .get("/api/folder?name=outbox")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(outbox.len(), 2);
    assert_eq!(outbox[0]["id"], one.as_str(), "back first, where it was");
    c.post("/api/delete", &json!({"folder": "outbox", "id": one}))
        .expect("deleted");
    c.post("/api/purge", &json!({"id": one, "was": "outbox"}))
        .expect("deleted for good");
    let (status, said) = http(
        c.at,
        "GET",
        &format!("/api/message?id={one}"),
        &c.host(),
        false,
        &Value::Null,
    );
    assert_eq!(status, 400, "its file is gone with it: {said}");
    assert_eq!(count("trash"), 0);
    c.post("/api/delete", &json!({"folder": "outbox", "id": two}))
        .expect("deleted");
    assert_eq!(
        c.post("/api/empty-trash", &json!({})).expect("emptied")["deleted"],
        1
    );
    assert_eq!((count("outbox"), count("trash")), (0, 0));

    // listening for P2P, the folders change between calls
    let three = write("three");
    let (_server, _, at_a) = loopback();
    c.post("/api/listen", &json!({"adapter": at_a.to_string()}))
        .expect("listening");
    let deadline = Instant::now() + Duration::from_secs(10);
    while c.get("/api/state")["activity"]["state"] != "listening" {
        assert!(Instant::now() < deadline, "never listened");
        thread::sleep(Duration::from_millis(20));
    }
    c.post("/api/delete", &json!({"folder": "outbox", "id": three}))
        .expect("deleted while listening");
    assert_eq!((count("outbox"), count("trash")), (0, 1));
    c.post("/api/restore", &json!({"id": three, "was": "outbox"}))
        .expect("put back while listening");
    assert_eq!((count("outbox"), count("trash")), (1, 0));
    c.post("/api/stop", &json!({})).expect("stopped");
    while c.get("/api/state")["activity"]["state"] != "idle" {
        assert!(Instant::now() < deadline, "never stopped");
        thread::sleep(Duration::from_millis(20));
    }

    // a call keeps the folders until it ends: here one nobody answers, given up after 10 s
    c.post(
        "/api/call",
        &json!({"callsign": "kk4oda", "station": true, "adapter": at_a.to_string()}),
    )
    .expect("calling");
    let refused = c
        .post("/api/delete", &json!({"folder": "outbox", "id": three}))
        .expect_err("refused during a call");
    assert!(refused.contains("a session is under way"), "{refused}");
    assert_eq!(count("outbox"), 1);
    let deadline = Instant::now() + Duration::from_secs(30);
    while c.get("/api/state")["activity"]["state"] != "idle" {
        assert!(Instant::now() < deadline, "the call was never given up");
        thread::sleep(Duration::from_millis(50));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// An outbox message edited from the panel (the author's notes of 2026-11-18): the composer is
/// filled in with everything it holds, the ICS-213's values with it; the message saved takes the
/// old one's place, and the old one is gone. Listening for P2P, it is saved between calls. An
/// edit is refused, saying why, while a call may be carrying the message, and once it is queued
/// at a gateway or sent.
#[test]
#[allow(clippy::too_many_lines, reason = "one story, told in order")]
fn an_outbox_message_is_edited_in_its_place() {
    let dir = scratch("panel-edit");
    store_for(&dir.join("facundo"), "device:facundo", "facundo");
    let mut ana = store_for(&dir.join("ana"), "device:ana", "ana");
    let c = Client {
        at: Panel::new(&dir.join("facundo"), None)
            .start("127.0.0.1:0")
            .expect("a port"),
    };
    let ics = c.get("/api/forms")[0]["template"]
        .as_str()
        .expect("ICS-213")
        .to_owned();
    let id_of = |v: Value| v["id"].as_str().expect("an id").to_owned();
    let first = id_of(
        c.post(
            "/api/compose",
            &json!({"to": "maria@example.com", "subject": "first", "body": "1"}),
        )
        .expect("composed"),
    );
    let old = id_of(
        c.post(
            "/api/compose",
            &json!({"to": "Ana <ana@hampost.org>", "cc": "jim@hampost.org",
                    "subject": "Shelter count", "priority": 1, "body": "",
                    "files": [{"name": "a.txt", "data": "QQ=="}, {"name": "b.txt", "data": "Qg=="}],
                    "form": {"template": ics, "values": {"2": "EOC", "3": "Shelter 4",
                                                          "7": "40 cots."}}}),
        )
        .expect("composed"),
    );
    let last = id_of(
        c.post(
            "/api/compose",
            &json!({"to": "maria@example.com", "subject": "last", "body": "3"}),
        )
        .expect("composed"),
    );

    // the composer is filled in with what the message holds
    let e = c.get(&format!("/api/edit?id={old}"));
    assert_eq!(e["to"], json!(["ana@hampost.org"]), "{e}");
    assert_eq!(e["cc"], json!(["jim@hampost.org"]));
    assert_eq!(
        (&e["subject"], &e["priority"]),
        (&json!("Shelter count"), &json!(1))
    );
    assert_eq!(
        e["files"],
        json!([{"name": "a.txt", "type": "text/plain", "size": 1},
               {"name": "b.txt", "type": "text/plain", "size": 1}])
    );
    assert_eq!(e["form"]["template"], ics.as_str());
    assert_eq!(
        e["form"]["values"],
        json!({"2": "EOC", "3": "Shelter 4", "7": "40 cots."})
    );
    assert_eq!(e["reply_to"], Value::Null);

    // changed everywhere: addressees, subject, text, a file left out and one added, the form
    let saved = c
        .post(
            "/api/compose",
            &json!({"edit": old, "to": "eoc@radiopost.org", "cc": "", "subject": "Shelter count, corrected",
                    "priority": 0, "body": "Updated at noon.", "carry": [1],
                    "files": [{"name": "c.txt", "data": "Qw=="}],
                    "form": {"template": ics, "values": {"2": "EOC", "3": "Shelter 4",
                                                          "7": "42 cots."}}}),
        )
        .expect("saved");
    let mut new = id_of(saved.clone());
    assert_eq!(saved["replaced"], old.as_str());
    assert_ne!(new, old);
    let outbox: Vec<String> = c
        .get("/api/folder?name=outbox")
        .as_array()
        .expect("a list")
        .iter()
        .map(|m| m["id"].as_str().unwrap_or("").to_owned())
        .collect();
    assert_eq!(
        outbox,
        [first.clone(), new.clone(), last.clone()],
        "in its place, once"
    );
    assert_eq!(c.get("/api/state")["counts"]["outbox"], 3);
    let (status, said) = http(
        c.at,
        "GET",
        &format!("/api/message?id={old}"),
        &c.host(),
        false,
        &Value::Null,
    );
    assert_eq!(status, 400, "the old message is gone: {said}");
    let m = c.get(&format!("/api/message?id={new}"));
    assert_eq!(m["to"], json!(["eoc@radiopost.org"]), "{m}");
    assert_eq!((&m["cc"], &m["priority"]), (&json!([]), &json!(0)));
    assert_eq!(m["subject"], "Shelter count, corrected");
    assert_eq!(m["body"], "Updated at noon.");
    let files: Vec<&str> = m["files"]
        .as_array()
        .expect("files")
        .iter()
        .filter_map(|f| f["name"].as_str())
        .collect();
    assert_eq!(files, ["b.txt", "c.txt"]);
    assert!(
        m["form"]["text"]
            .as_str()
            .is_some_and(|t| t.contains("42 cots.")),
        "{m}"
    );
    let e = c.get(&format!("/api/edit?id={new}"));
    assert_eq!(e["form"]["values"]["7"], "42 cots.");
    assert!(
        c.post("/api/compose", &json!({"edit": old, "to": "x@example.com"}))
            .expect_err("gone")
            .contains("no message"),
        "the old one cannot be saved again"
    );

    // queued at a cut-off gateway, it is that gateway's to pass on
    let mut store = radiopost_client::store::Store::open(&dir.join("facundo")).expect("the home");
    let first_id = radiopost::ContentId::from_text(&first).expect("an id");
    store.queued_at(&first_id, "N1CALL-10", 1);
    store.save().expect("saved");
    let why = c
        .post(
            "/api/compose",
            &json!({"edit": first, "to": "x@example.com"}),
        )
        .expect_err("queued");
    assert!(why.contains("queued at N1CALL-10"), "{why}");
    let (status, why) = http(
        c.at,
        "GET",
        &format!("/api/edit?id={first}"),
        &c.host(),
        false,
        &Value::Null,
    );
    assert_eq!(status, 400);
    assert!(why.contains("queued at N1CALL-10"), "{why}");

    // listening for P2P, it is changed between calls
    let (_quiet, at_l, _) = loopback();
    c.post("/api/listen", &json!({"adapter": at_l.to_string()}))
        .expect("listening");
    let deadline = Instant::now() + Duration::from_secs(10);
    while c.get("/api/state")["activity"]["state"] != "listening" {
        assert!(Instant::now() < deadline, "never listened");
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(c.get(&format!("/api/edit?id={last}"))["subject"], "last");
    let saved = c
        .post(
            "/api/compose",
            &json!({"edit": last, "to": "maria@example.com", "subject": "last, between calls"}),
        )
        .expect("saved while listening");
    assert_eq!(saved["replaced"], last.as_str());
    c.post("/api/stop", &json!({})).expect("stopped");
    while c.get("/api/state")["activity"]["state"] != "idle" {
        assert!(Instant::now() < deadline, "never stopped");
        thread::sleep(Duration::from_millis(20));
    }

    // a call carries the edited message to Ana: meanwhile it may be going out, and after, it is
    // sent; either way it is not changed
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
    new = id_of(
        c.post(
            "/api/compose",
            &json!({"edit": new, "to": "ana@hampost.org", "subject": "for Ana", "body": "73"}),
        )
        .expect("saved"),
    );
    c.post(
        "/api/call",
        &json!({"callsign": "n0call", "station": true, "adapter": at_f.to_string()}),
    )
    .expect("calling");
    let why = c
        .post("/api/compose", &json!({"edit": new, "to": "x@example.com"}))
        .expect_err("not while it may be going out");
    assert!(
        why.contains("while a session is under way") || why.contains("has been sent"),
        "{why}"
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    while c.get("/api/state")["activity"]["state"] != "idle" {
        assert!(Instant::now() < deadline, "the session never ended");
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(c.get("/api/folder?name=sent")[0]["id"], new.as_str());
    let why = c
        .post("/api/compose", &json!({"edit": new, "to": "x@example.com"}))
        .expect_err("sent");
    assert!(why.contains("has been sent"), "{why}");
    let ana = ana_runs.join().expect("Ana's side");
    assert_eq!(ana.inbox.len(), 1);
    assert_eq!(ana.inbox[0].id.text(), new);
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

/// W1: a reply to a message received, a reply to all, and a forward, from the reader's buttons as
/// the page makes them: what each starts with, then the message the outbox gets.
#[test]
#[allow(clippy::too_many_lines, reason = "the three ways, told in order")]
fn a_reply_a_reply_to_all_and_a_forward_from_the_panel() {
    use radiopost::cbor::Value as Cbor;
    use radiopost::message::{Message, key};
    let dir = scratch("panel-answer");
    let mut store = store_for(&dir.join("facundo"), "device:facundo", "facundo");
    // Maria's question to Facundo alone; the EOC's roster to Facundo, copied to shelter 3
    let maria = common::message("maria-419");
    let roster = common::message("eoc-420");
    // a message with a file carried and a file held at the post office, to Facundo and Ana, with
    // Facundo's own address written with capitals, as a sender may
    let both = Message::from_map(&Cbor::int_map(vec![
        (key::V, Cbor::Uint(1)),
        (key::FROM, Cbor::text("maria@example.com")),
        (
            key::TO,
            Cbor::Array(vec![
                Cbor::text("Facundo@radiopost.org"),
                Cbor::text("ana@hampost.org"),
            ]),
        ),
        (key::DATE, Cbor::Uint(1_794_240_000)),
        (key::SUBJECT, Cbor::text("RE: The house")),
        (key::BODY, Cbor::text("The roof held.\n")),
        (
            key::FILES,
            Cbor::Array(vec![Cbor::int_map(vec![
                (1, Cbor::text("roof.txt")),
                (2, Cbor::text("text/plain")),
                (3, Cbor::bytes(b"Tiles: 3 missing")),
            ])]),
        ),
        (key::PRIORITY, Cbor::Uint(2)),
        (key::FLAGS, Cbor::Uint(2)),
        (
            key::HELD,
            Cbor::Array(vec![Cbor::int_map(vec![
                (1, Cbor::text("roof.mp4")),
                (2, Cbor::text("video/mp4")),
                (3, Cbor::Uint(900_000)),
                (4, Cbor::bytes(&[7; 8])),
            ])]),
        ),
    ]))
    .expect("a message");
    for (m, serial) in [(&maria, 419), (&roster, 420), (&both, 423)] {
        store
            .received(m, "N1CALL-10", 1_794_240_100, Some(serial))
            .expect("kept");
    }
    let c = Client {
        at: Panel::new(&dir.join("facundo"), None)
            .start("127.0.0.1:0")
            .expect("a port"),
    };
    let short = |m: &Message| m.content_id().text()[..6].to_owned();
    let answer = |m: &Message, how: &str| c.get(&format!("/api/answer?id={}&how={how}", short(m)));
    let composed = |body: Value| -> Message {
        let id = c.post("/api/compose", &body).expect("composed")["id"]
            .as_str()
            .expect("an ID")
            .to_owned();
        common::kept(&dir.join("facundo"), &id)
    };

    // a reply: to the sender, Re:, in the thread, nothing quoted, routine
    let a = answer(&maria, "reply");
    assert_eq!(a["to"], json!(["maria@example.com"]));
    assert_eq!(a["cc"], json!([]));
    assert_eq!(a["subject"], "Re: Are you safe?");
    assert_eq!(a["body"], "");
    assert_eq!(a["reply_to"], maria.content_id().text().as_str());
    assert_eq!(a["original"]["subject"], "Are you safe?");
    let reply = composed(
        json!({"to": a["to"], "cc": a["cc"], "subject": a["subject"],
                                "body": "All safe here.\n", "reply_to": a["reply_to"]}),
    );
    let map = reply.map();
    assert_eq!(
        map.get(key::REPLY_TO).and_then(Cbor::as_bytes),
        Some(maria.content_id().0.as_slice())
    );
    assert_eq!(reply.recipients(), ["maria@example.com"]);
    assert_eq!(
        map.get(key::SUBJECT).and_then(Cbor::as_text),
        Some("Re: Are you safe?")
    );
    assert_eq!(map.get(key::PRIORITY), None, "routine");
    // a subject that starts with Re: already, in any case, is kept as it is
    assert_eq!(answer(&both, "reply")["subject"], "RE: The house");

    // a reply to all: the others in Cc, less this account's own address and the sender
    let a = answer(&roster, "reply-all");
    assert_eq!(a["to"], json!(["eoc@radiopost.org"]));
    assert_eq!(a["cc"], json!(["shelter3@radiopost.org"]));
    assert_eq!(a["subject"], "Re: Shelter roster");
    let a = answer(&both, "reply-all");
    assert_eq!(a["to"], json!(["maria@example.com"]));
    assert_eq!(a["cc"], json!(["ana@hampost.org"]), "not Facundo@");
    let all = composed(
        json!({"to": a["to"], "cc": a["cc"], "subject": a["subject"],
                              "body": "", "reply_to": a["reply_to"]}),
    );
    assert_eq!(all.recipients(), ["maria@example.com", "ana@hampost.org"]);
    assert_eq!(
        all.map().get(key::REPLY_TO).and_then(Cbor::as_bytes),
        Some(both.content_id().0.as_slice())
    );

    // a forward: Fwd:, the original's text under a short header, its file carried, the held one
    // listed as not included, answering nothing, routine though the original was an emergency
    let a = answer(&both, "forward");
    assert_eq!(a["to"], json!([]));
    assert_eq!(a["subject"], "Fwd: RE: The house");
    assert_eq!(a["reply_to"], Value::Null);
    assert_eq!(
        a["body"],
        "Forwarded message\nFrom: maria@example.com\nDate: 2026-11-09 16:00Z\n\
         Subject: RE: The house\n\nThe roof held.\n"
    );
    assert_eq!(
        a["files"],
        json!([{"name": "roof.txt", "type": "text/plain", "size": 16}])
    );
    assert_eq!(
        a["held"],
        json!([{"name": "roof.mp4", "type": "video/mp4", "size": 900_000, "asked": false}])
    );
    assert_eq!(a["can_ask"], true);
    let forward = composed(json!({"to": "jose@example.net", "subject": a["subject"],
                                  "body": format!("For you.\n\n{}", a["body"].as_str().unwrap_or("")),
                                  "forward": a["original"]["id"]}));
    let map = forward.map();
    assert_eq!(map.get(key::REPLY_TO), None, "a forward answers nothing");
    assert_eq!(map.get(key::PRIORITY), None, "routine");
    assert_eq!(map.get(key::HELD), None, "the held attachment is left out");
    assert_eq!(map.get(key::FLAGS), None);
    let files = map.get(key::FILES).and_then(Cbor::as_array).expect("files");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].get(1).and_then(Cbor::as_text), Some("roof.txt"));
    assert_eq!(
        files[0].get(3).and_then(Cbor::as_bytes),
        Some(b"Tiles: 3 missing".as_slice())
    );
    assert!(
        map.get(key::BODY)
            .and_then(Cbor::as_text)
            .is_some_and(|b| b.starts_with("For you.\n\nForwarded message\n")),
        "{map:?}"
    );
    // a file unticked stays behind
    let lighter = composed(json!({"to": "jose@example.net", "subject": "Fwd: light",
                                  "body": "", "forward": a["original"]["id"], "carry": []}));
    assert_eq!(lighter.map().get(key::FILES), None);

    // only a message received is replied to; a message sent is forwarded
    let sent = reply.content_id().text();
    let (status, body) = http(
        c.at,
        "GET",
        &format!("/api/answer?id={}&how=reply", &sent[..6]),
        &c.host(),
        false,
        &Value::Null,
    );
    assert_ne!(status, 200, "{body}");
    assert_eq!(
        answer(&reply, "forward")["subject"],
        "Fwd: Re: Are you safe?"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// W1, RP-2 §8 (found 2026-11-18, while a received form came to show as the form): a message that
/// carries a form is forwarded with the form's text (RP-2 §8.4), the sender's note below it, and a
/// reply shows that text under the composer. Both had taken the note alone, so the form's values
/// were lost. The forward is not the form again: its Subject, Date and Time are bound to the
/// message's own, and would have become the forward's.
#[test]
fn a_form_forwarded_or_answered_keeps_the_form() {
    use radiopost::cbor::Value as Cbor;
    use radiopost::message::key;
    use radiopost_client::compose::{Draft, form_from_args};
    let dir = scratch("panel-answer-form");
    let mut store = store_for(&dir.join("facundo"), "device:facundo", "facundo");
    let fields = [
        "1=Shelter 3".to_owned(),
        "2=Facundo, net control".to_owned(),
        "3=J. Park, Shelter 3 manager".to_owned(),
        "7=Forty cots by 18:00.\\nWater for 120.".to_owned(),
    ];
    let ics213 = Draft {
        from: "shelter3@radiopost.org".into(),
        to: vec!["facundo@radiopost.org".into()],
        date: 1_794_233_400,
        subject: "Cots".into(),
        body: "Sent from the mobile.".into(),
        priority: 1,
        form: Some(form_from_args("ics-213", &fields).expect("a form")),
        ..Draft::default()
    }
    .message()
    .expect("a message");
    store
        .received(&ics213, "N1CALL-10", 1_794_233_500, Some(431))
        .expect("kept");
    let c = Client {
        at: Panel::new(&dir.join("facundo"), None)
            .start("127.0.0.1:0")
            .expect("a port"),
    };
    let short = &ics213.content_id().text()[..6];
    let shown = "ICS-213 General Message\n\nIncident name: Shelter 3\n\
                 To (name and position): Facundo, net control\n\
                 From (name and position): J. Park, Shelter 3 manager\n\
                 Subject: Cots\nDate: 2026-11-09\nTime: 14:10 UTC\n\
                 Message:\nForty cots by 18:00.\nWater for 120.\n\n\
                 Sent from the mobile.";

    // a reply shows the form under the composer, for reference, the note below it
    let a = c.get(&format!("/api/answer?id={short}&how=reply"));
    assert_eq!(a["body"], "", "a reply quotes nothing");
    assert_eq!(a["original"]["text"], shown, "{a}");

    // a forward carries the form's text, the original's subject, date and time in it
    let a = c.get(&format!("/api/answer?id={short}&how=forward"));
    assert_eq!(a["subject"], "Fwd: Cots");
    assert_eq!(
        a["body"].as_str(),
        Some(
            format!(
                "Forwarded message\nFrom: shelter3@radiopost.org\nDate: 2026-11-09 14:10Z\n\
                 Subject: Cots\n\n{shown}"
            )
            .as_str()
        ),
        "{a}"
    );
    let id = c
        .post(
            "/api/compose",
            &json!({"to": "eoc@radiopost.org", "subject": a["subject"],
                    "body": format!("For the EOC.\n\n{}", a["body"].as_str().unwrap_or("")),
                    "forward": a["original"]["id"]}),
        )
        .expect("composed")["id"]
        .as_str()
        .expect("an ID")
        .to_owned();
    let forward = common::kept(&dir.join("facundo"), &id);
    let map = forward.map();
    assert_eq!(map.get(key::FORM), None, "the form goes as its text");
    let body = map.get(key::BODY).and_then(Cbor::as_text).unwrap_or("");
    assert!(
        body.starts_with("For the EOC.\n\nForwarded message\n") && body.ends_with(shown),
        "{body}"
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

/// A release out since the client last asked is found when the user presses Check for updates:
/// the channel is asked on a thread of its own (this panel answers one request at a time), and
/// the offer shows at the next look, without waiting six hours.
#[test]
fn check_for_updates_finds_a_release_out_since_the_last_check() {
    use radiopost_update::{Product, Watch};
    use std::sync::{Arc, Mutex};
    let server = tiny_http::Server::http("127.0.0.1:0").expect("a port");
    let base = format!(
        "http://{}",
        server.server_addr().to_ip().expect("an address")
    );
    let offered = Arc::new(Mutex::new("0.1.0-beta.27".to_owned()));
    let version = Arc::clone(&offered);
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let v = version.lock().expect("lock").clone();
            let manifest = json!({"version": v, "notes": "",
                "platforms": {radiopost_update::platform(): {"url": "http://x/y", "signature": "s"}}});
            let _ = request.respond(tiny_http::Response::from_string(manifest.to_string()));
        }
    });
    let dir = scratch("panel-update-check");
    store_for(&dir.join("facundo"), "device:facundo", "facundo");
    let watch = Watch::new(Product::Client, "0.1.0-beta.27", &base);
    watch.check();
    let c = Client {
        at: Panel::new(&dir.join("facundo"), None)
            .with_updates(Arc::clone(&watch))
            .start("127.0.0.1:0")
            .expect("a port"),
    };
    assert_eq!(c.get("/api/state")["update"]["offer"], Value::Null);
    *offered.lock().expect("lock") = "0.1.0-beta.29".to_owned();
    c.post("/api/update/check", &json!({})).expect("asked");
    let deadline = Instant::now() + Duration::from_secs(5);
    while c.get("/api/state")["update"]["offer"]["version"] != "0.1.0-beta.29" {
        assert!(
            Instant::now() < deadline,
            "the new release is never offered"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
