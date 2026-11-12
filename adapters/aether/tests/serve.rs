// SPDX-License-Identifier: MIT OR Apache-2.0
//! The adapter's connections, against a stand-in for Aether's control API on a real socket:
//! Aether going away closes the host's connection (RP-4 §2.1).
//!
//! Found on the air (2026-11-12): two Setup saves restarted the gateway's Aether, the adapter
//! kept RadioPost Gateway's connection open and answered nothing, and the gateway listened for
//! calls for an hour and a half that Aether, its claim gone, answered for nobody.

use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use rpmi::tcp::Connection;
use serde_json::{Value, json};
use tungstenite::Message;

/// Aether's answer to what the adapter asks while a host opens, as its control API gives it.
fn answer(request: &Value) -> Value {
    let id = request["id"].clone();
    let result = match request["method"].as_str().unwrap_or("") {
        "status" => json!({"version": "0.2.0-beta.146", "state": "idle", "owner": null,
                           "channel_busy": false, "claims": []}),
        "capabilities" => json!({"bandwidth_hz": 2300, "bandwidths_hz": [2300, 500]}),
        "claim" => json!({"id": 7, "callsigns": request["params"]["callsigns"],
                          "refused": [], "listening": false}),
        _ => json!({}),
    };
    json!({"id": id, "ok": true, "result": result})
}

/// A stand-in Aether that answers each connection until the host turns `listen` on, and then
/// stops as a daemon restarting does: the socket dropped, no closing handshake. Each method asked
/// comes out of the receiver.
fn aether_that_restarts() -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let url = format!("ws://{}/v1", listener.local_addr().expect("bound"));
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            thread::spawn(move || serve_until_listen(stream, &tx));
        }
    });
    (url, rx)
}

fn serve_until_listen(stream: TcpStream, asked: &mpsc::Sender<String>) {
    let Ok(mut ws) = tungstenite::accept(stream) else {
        return;
    };
    while let Ok(message) = ws.read() {
        let Message::Text(text) = message else {
            continue;
        };
        let request: Value = serde_json::from_str(&text).expect("JSON");
        let method = request["method"].as_str().unwrap_or("").to_owned();
        if ws
            .send(Message::text(answer(&request).to_string()))
            .is_err()
        {
            return;
        }
        let _ = asked.send(method.clone());
        if method == "listen" {
            return;
        }
    }
}

/// Reads the adapter's lines until it closes the connection: true when it did within `wait`.
fn closed_within(conn: &mut Connection, wait: Duration) -> bool {
    let until = Instant::now() + wait;
    conn.set_timeout(Some(Duration::from_millis(200)))
        .expect("a timeout");
    while Instant::now() < until {
        match conn.read_message() {
            Ok(Some(_)) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Ok(None) | Err(_) => return true,
        }
    }
    false
}

/// Sends one request and reads lines until its answer.
fn ask(conn: &mut Connection, line: &str) -> Value {
    let request: Value = serde_json::from_str(line).expect("JSON");
    conn.send_line(line).expect("sent");
    conn.set_timeout(Some(Duration::from_secs(10)))
        .expect("a timeout");
    loop {
        let m = conn
            .read_message()
            .expect("an answer")
            .expect("the connection open");
        if m.get("id") == request.get("id") {
            return Value::Object(m);
        }
    }
}

/// A [`radiopost_aether::serve::Note`] that keeps what it is told.
fn notes() -> (radiopost_aether::serve::Note, Arc<Mutex<Vec<String>>>) {
    let kept = Arc::new(Mutex::new(Vec::new()));
    let into = Arc::clone(&kept);
    let note: radiopost_aether::serve::Note = Arc::new(move |line: &str| {
        into.lock().expect("notes").push(line.to_owned());
    });
    (note, kept)
}

#[test]
fn aether_going_away_closes_the_hosts_connection() {
    let (aether, asked) = aether_that_restarts();
    let (note, said) = notes();
    let at =
        radiopost_aether::serve::listen_noting("127.0.0.1:0", &aether, note).expect("an adapter");
    let mut host = Connection::open(at).expect("the adapter");
    let hello = ask(
        &mut host,
        r#"{"op":"hello","id":1,"version":[1,1],"app":"radiopost-gateway test"}"#,
    );
    assert_eq!(hello["op"], "hello", "{hello}");
    let claimed = ask(
        &mut host,
        r#"{"op":"callsigns","id":2,"callsigns":["KK4ODA-1"]}"#,
    );
    assert_eq!(claimed["op"], "ok", "{claimed}");
    let listening = ask(&mut host, r#"{"op":"listen","id":3,"on":true}"#);
    assert_eq!(listening["op"], "ok", "{listening}");
    let methods: Vec<String> = asked.try_iter().collect();
    assert_eq!(methods, ["status", "capabilities", "claim", "listen"]);
    // Aether stops after the listen: the host is let go, not left listening for nothing
    assert!(
        closed_within(&mut host, Duration::from_secs(5)),
        "the adapter kept the host's connection open with Aether gone"
    );
    // and it says why, for the program's log
    let said = said.lock().expect("notes").clone();
    assert!(
        said.iter().any(|l| l.starts_with(&format!(
            "the connection to Aether at {aether} ended: a read failed: "
        ))),
        "{said:?}"
    );
}

#[test]
fn an_aether_that_cannot_be_reached_is_said_so() {
    // a port nobody listens on
    let free = TcpListener::bind("127.0.0.1:0").expect("a port");
    let aether = format!("ws://{}/v1", free.local_addr().expect("bound"));
    drop(free);
    let (note, said) = notes();
    let at =
        radiopost_aether::serve::listen_noting("127.0.0.1:0", &aether, note).expect("an adapter");
    let mut host = Connection::open(at).expect("the adapter");
    let hello = ask(
        &mut host,
        r#"{"op":"hello","id":1,"version":[1,1],"app":"radiopost-gateway test"}"#,
    );
    assert_eq!(hello["code"], "no-modem", "{hello}");
    let said = said.lock().expect("notes").clone();
    assert!(
        said.iter()
            .any(|l| l.starts_with(&format!("Aether at {aether} could not be reached: "))),
        "{said:?}"
    );
}

#[test]
fn a_link_up_when_aether_goes_away_is_said_ended_before_the_close() {
    // Aether answers the opening and the call, says the link is up, then stops
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let aether = format!("ws://{}/v1", listener.local_addr().expect("bound"));
    thread::spawn(move || {
        let stream = listener.incoming().flatten().next().expect("the adapter");
        let mut ws = tungstenite::accept(stream).expect("a WebSocket");
        while let Ok(message) = ws.read() {
            let Message::Text(text) = message else {
                continue;
            };
            let request: Value = serde_json::from_str(&text).expect("JSON");
            let mut reply = answer(&request);
            if request["method"] == "connect" {
                reply["result"] = json!({"session": 42});
            }
            let _ = ws.send(Message::text(reply.to_string()));
            if request["method"] == "connect" {
                let up = json!({"event": "state",
                                "data": {"name": "connected", "detail": "KK4ODA-1 (master)",
                                         "state": "Connected", "remote": "KK4ODA-1",
                                         "callsign": "KK4ODA", "owner": 7}});
                let _ = ws.send(Message::text(up.to_string()));
                thread::sleep(Duration::from_millis(300));
                return;
            }
        }
    });
    let at = radiopost_aether::serve::listen("127.0.0.1:0", &aether).expect("an adapter");
    let mut host = Connection::open(at).expect("the adapter");
    ask(
        &mut host,
        r#"{"op":"hello","id":1,"version":[1,1],"app":"RadioPost client test"}"#,
    );
    ask(
        &mut host,
        r#"{"op":"callsigns","id":2,"callsigns":["KK4ODA"]}"#,
    );
    let calling = ask(&mut host, r#"{"op":"connect","id":3,"remote":"KK4ODA-1"}"#);
    assert_eq!(calling["op"], "ok", "{calling}");
    host.set_timeout(Some(Duration::from_secs(5)))
        .expect("a timeout");
    let mut said = Vec::new();
    let closed = loop {
        match host.read_message() {
            Ok(Some(m)) => said.push(m),
            Ok(None) => break true,
            Err(_) => break false,
        }
    };
    assert!(
        closed,
        "the adapter kept the host's connection open: {said:?}"
    );
    let up = said
        .iter()
        .find(|m| m.get("op") == Some(&json!("connected")))
        .unwrap_or_else(|| panic!("no connected: {said:?}"));
    let ended = said
        .iter()
        .find(|m| m.get("op") == Some(&json!("disconnected")))
        .unwrap_or_else(|| panic!("no disconnected before the close: {said:?}"));
    assert_eq!(ended["reason"], "failure");
    let text = ended["text"].as_str().unwrap_or("");
    assert!(
        text.starts_with("Aether went away: a read failed: "),
        "{text}"
    );
    assert_eq!(ended["link"], up["link"]);
}
