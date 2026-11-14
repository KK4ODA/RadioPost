// SPDX-License-Identifier: MIT OR Apache-2.0
//! The app's connection to Aether between calls (`presence`), against a stand-in for Aether's
//! control API on a real socket: the panel names the app there once the page says its modem is
//! Aether, claims nothing, comes back after Aether restarts, finds an Aether started late, leaves
//! other modems alone, and closes when the program leaves.
//!
//! Asked for on 2026-11-13: with the app open and idle, Aether's header said "calls to KK4ODA-1 not
//! answered", since the app held no connection between calls.

mod common;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use radiopost_client::panel::Panel;
use radiopost_client::presence::Presence;
use radiopost_client::session;
use serde_json::{Value, json};
use tungstenite::Message;

use common::scratch;

/// What the stand-in Aether saw: a request on a connection, or the connection's end.
#[derive(Debug)]
enum Seen {
    Asked {
        connection: usize,
        method: String,
        params: Value,
    },
    Closed(usize),
}

/// A stand-in Aether: it answers what the adapter asks as Aether's control API does, and its
/// connections can be cut, as a restart cuts them.
struct Aether {
    url: String,
    seen: Receiver<Seen>,
    open: Arc<Mutex<Vec<TcpStream>>>,
}

fn answer(request: &Value) -> Value {
    let id = request["id"].clone();
    let result = match request["method"].as_str().unwrap_or("") {
        "status" => json!({"version": "0.2.0-beta.159", "state": "idle", "owner": null,
                           "channel_busy": false, "claims": [], "programs": []}),
        "capabilities" => json!({"bandwidth_hz": 2300, "bandwidths_hz": [2300, 500]}),
        "hello" => json!({"id": 1, "app": request["params"]["app"]}),
        _ => json!({}),
    };
    json!({"id": id, "ok": true, "result": result})
}

fn serve(stream: TcpStream, connection: usize, seen: &Sender<Seen>) {
    let Ok(mut ws) = tungstenite::accept(stream) else {
        return;
    };
    loop {
        match ws.read() {
            Ok(Message::Text(text)) => {
                let request: Value = serde_json::from_str(&text).expect("JSON");
                let _ = seen.send(Seen::Asked {
                    connection,
                    method: request["method"].as_str().unwrap_or("").to_owned(),
                    params: request["params"].clone(),
                });
                if ws
                    .send(Message::text(answer(&request).to_string()))
                    .is_err()
                {
                    break;
                }
            }
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => {}
        }
    }
    let _ = seen.send(Seen::Closed(connection));
}

impl Aether {
    fn on(listener: TcpListener) -> Aether {
        let url = format!("ws://{}/v1", listener.local_addr().expect("bound"));
        let (tx, seen) = mpsc::channel();
        let open = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&open);
        thread::spawn(move || {
            for (n, stream) in listener.incoming().flatten().enumerate() {
                if let Ok(copy) = stream.try_clone() {
                    kept.lock().expect("open").push(copy);
                }
                let tx = tx.clone();
                thread::spawn(move || serve(stream, n + 1, &tx));
            }
        });
        Aether { url, seen, open }
    }

    fn start() -> Aether {
        Aether::on(TcpListener::bind("127.0.0.1:0").expect("a port"))
    }

    /// Cuts every connection, as Aether does when it restarts (its Setup saved).
    fn restart(&self) {
        for s in self.open.lock().expect("open").drain(..) {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }

    /// The next thing seen that `wanted` picks, within `wait`; what was seen before it is dropped
    /// into `before`.
    fn until(
        &self,
        wait: Duration,
        before: &mut Vec<Seen>,
        wanted: impl Fn(&Seen) -> bool,
    ) -> Option<Seen> {
        let deadline = Instant::now() + wait;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match self.seen.recv_timeout(left) {
                Ok(s) if wanted(&s) => return Some(s),
                Ok(s) => before.push(s),
                Err(_) => break,
            }
        }
        None
    }

    /// The next `hello`: its connection and the name it gave.
    fn hello(&self, wait: Duration, before: &mut Vec<Seen>) -> Option<(usize, String)> {
        match self.until(
            wait,
            before,
            |s| matches!(s, Seen::Asked { method, .. } if method == "hello"),
        )? {
            Seen::Asked {
                connection, params, ..
            } => Some((connection, params["app"].as_str().unwrap_or("").to_owned())),
            Seen::Closed(_) => None,
        }
    }
}

fn claims(seen: &[Seen]) -> usize {
    seen.iter()
        .filter(|s| matches!(s, Seen::Asked { method, .. } if method == "claim"))
        .count()
}

/// The panel's `POST /api/modem`, as the page sends it from its Modem box.
fn tell_modem(at: SocketAddr, adapter: &str) {
    let body = json!({"adapter": adapter}).to_string();
    let mut s = TcpStream::connect(at).expect("the panel");
    write!(
        s,
        "POST /api/modem HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-RadioPost: 1\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        at.port(),
        body.len()
    )
    .expect("sent");
    let mut text = String::new();
    s.read_to_string(&mut text).expect("an answer");
    assert!(text.starts_with("HTTP/1.1 200"), "{text}");
}

#[test]
fn the_app_is_named_on_aether_between_calls_and_claims_nothing() {
    let dir = scratch("presence-named");
    let aether = Aether::start();
    let panel =
        Panel::new(&dir.join("facundo"), None).retrying_aether_every(Duration::from_millis(200));
    let at = panel.clone().start("127.0.0.1:0").expect("a port");
    let mut seen = Vec::new();
    // nothing until the page says which modem
    assert_eq!(aether.hello(Duration::from_millis(600), &mut seen), None);
    tell_modem(at, &aether.url);
    let (first, app) = aether
        .hello(Duration::from_secs(5), &mut seen)
        .expect("the app says hello");
    assert_eq!(app, session::app_name(), "the name its calls claim under");

    // held, with nothing claimed
    assert!(
        aether
            .until(Duration::from_millis(800), &mut seen, |s| matches!(
                s,
                Seen::Closed(_)
            ))
            .is_none(),
        "the connection is held: {seen:?}"
    );
    assert_eq!(claims(&seen), 0, "{seen:?}");

    // Aether restarts: the app says hello again on a new connection
    aether.restart();
    let (second, app) = aether
        .hello(Duration::from_secs(5), &mut seen)
        .expect("again after a restart");
    assert!(second > first);
    assert_eq!(app, session::app_name());

    // the program leaves: the connection closes, and Aether hears it
    panel.leave();
    assert!(
        aether
            .until(
                Duration::from_secs(5),
                &mut seen,
                |s| matches!(s, Seen::Closed(c) if *c == second)
            )
            .is_some(),
        "the connection closed when the app left: {seen:?}"
    );
    assert_eq!(claims(&seen), 0, "{seen:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_aether_started_after_the_app_is_found() {
    let dir = scratch("presence-late");
    let free = TcpListener::bind("127.0.0.1:0").expect("a port");
    let port = free.local_addr().expect("bound");
    drop(free);
    let presence = Presence::new(&dir.join("facundo")).retrying_every(Duration::from_millis(200));
    presence.start();
    presence.set_modem(&format!("ws://{port}/v1"));
    thread::sleep(Duration::from_millis(700));
    let aether = Aether::on(TcpListener::bind(port).expect("the same port again"));
    let mut seen = Vec::new();
    assert!(
        aether.hello(Duration::from_secs(5), &mut seen).is_some(),
        "found once it runs"
    );
    presence.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn another_modem_or_conformance_mode_holds_no_connection() {
    let dir = scratch("presence-other");
    let home = dir.join("facundo");
    std::fs::create_dir_all(&home).expect("a home");
    // an RPMI adapter that is not Aether: it may serve one host at a time (RP-4 §3)
    let adapter = TcpListener::bind("127.0.0.1:0").expect("a port");
    adapter.set_nonblocking(true).expect("non-blocking");
    let presence = Presence::new(&home).retrying_every(Duration::from_millis(200));
    presence.start();
    presence.set_modem(&adapter.local_addr().expect("bound").to_string());
    thread::sleep(Duration::from_millis(800));
    assert!(
        adapter.accept().is_err(),
        "a connection to a modem that is not Aether"
    );

    // Aether, in conformance mode: the client reaches only the test adapter
    let aether = Aether::start();
    std::fs::write(home.join("conformance"), "127.0.0.1:9").expect("conformance mode");
    presence.set_modem(&aether.url);
    let mut seen = Vec::new();
    assert_eq!(aether.hello(Duration::from_millis(800), &mut seen), None);
    // and out of it, Aether names the app
    std::fs::remove_file(home.join("conformance")).expect("out of it");
    assert!(aether.hello(Duration::from_secs(5), &mut seen).is_some());
    // a call to another modem moves the app off Aether
    presence.set_modem("127.0.0.1:9");
    assert!(
        aether
            .until(Duration::from_secs(3), &mut seen, |s| matches!(
                s,
                Seen::Closed(_)
            ))
            .is_some(),
        "left Aether: {seen:?}"
    );
    presence.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
