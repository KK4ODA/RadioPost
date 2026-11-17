// SPDX-License-Identifier: MIT OR Apache-2.0
//! One gateway, several modems (wishlist W26): KK4ODA-1 answers on two modems, each its own
//! loopback channel with a user on it. A call on either is carried; while one is, the other modem
//! does not answer; and a call the other modem answered before it heard so is ended, not left up.
//!
//! Each modem is reached through a tap, which passes the lines both ways, keeps what the gateway
//! asked of the modem, and can hand the gateway a line the modem never wrote.

mod common;

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use radiopost_client::session::{self, Kind, Options};
use radiopost_gateway::{operator, station};
use rpmi::channel::{Channel, StationConfig};
use rpmi::tcp::Server;
use rpmi::wire::Caps;
use serde_json::{Value, json};

use common::{client, key, post_office, scratch};

/// A TCP proxy to a modem's adapter: the gateway's requests kept, and lines put in its way.
#[derive(Clone)]
struct Tap {
    at: SocketAddr,
    asked: Arc<Mutex<Vec<Value>>>,
    to_gateway: Arc<Mutex<Option<TcpStream>>>,
}

impl Tap {
    fn new(target: SocketAddr) -> Tap {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let tap = Tap {
            at: listener.local_addr().expect("bound"),
            asked: Arc::default(),
            to_gateway: Arc::default(),
        };
        let t = tap.clone();
        thread::spawn(move || {
            for gateway in listener.incoming().flatten() {
                let Ok(modem) = TcpStream::connect(target) else {
                    let _ = gateway.shutdown(Shutdown::Both);
                    continue;
                };
                *t.lock_out() = gateway.try_clone().ok();
                // the modem's lines go back as they are, through the same lock as injected ones
                let back = t.clone();
                let from_modem = modem.try_clone().expect("a clone");
                thread::spawn(move || {
                    for line in BufReader::new(from_modem).lines() {
                        let Ok(line) = line else { break };
                        back.say(&line);
                    }
                });
                let mut to_modem = modem;
                for line in BufReader::new(gateway).lines() {
                    let Ok(line) = line else { break };
                    if let Ok(v) = serde_json::from_str(&line) {
                        t.asked
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push(v);
                    }
                    if writeln!(to_modem, "{line}").is_err() {
                        break;
                    }
                }
                let _ = to_modem.shutdown(Shutdown::Both);
            }
        });
        tap
    }

    fn lock_out(&self) -> std::sync::MutexGuard<'_, Option<TcpStream>> {
        self.to_gateway
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// A line to the gateway, as from the modem.
    fn say(&self, line: &str) {
        if let Some(s) = self.lock_out().as_mut() {
            let _ = writeln!(s, "{line}");
        }
    }

    fn asked(&self) -> Vec<Value> {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The `listen` values the gateway sent, in order.
    fn listens(&self) -> Vec<bool> {
        self.asked()
            .iter()
            .filter(|m| m["op"] == "listen")
            .filter_map(|m| m["on"].as_bool())
            .collect()
    }

    /// Waits until the gateway has sent `n` `listen`s.
    fn wait_listens(&self, n: usize) -> Vec<bool> {
        let until = Instant::now() + Duration::from_secs(20);
        while self.listens().len() < n && Instant::now() < until {
            thread::sleep(Duration::from_millis(20));
        }
        self.listens()
    }
}

/// A loopback channel with a user, `user`, and the gateway's station: the adapter's two ports.
fn channel(user: &str) -> (Server, SocketAddr, SocketAddr) {
    let caps = Caps {
        bandwidths: vec![2300],
        max_links: 1,
        delivery: true,
        broadcast: false,
        quality: false,
        busy: true,
        regulatory: false,
        max_queue: 65_536,
        shared: false,
        dial: false,
        scan: false,
        call_s: false,
    };
    let station = |name: &str, prefix: &str| StationConfig {
        name: name.into(),
        modem: "the loopback adapter".into(),
        caps: caps.clone(),
        prefix: prefix.into(),
        frame_bytes: 128,
        dial_hz: None,
    };
    let mut ch = Channel::new();
    let u = ch.add_station(station(user, "u"));
    let g = ch.add_station(station("KK4ODA-1", "g"));
    let server = Server::new(ch, Duration::from_secs(10));
    let at_user = server.listen(u, "127.0.0.1:0").expect("a port");
    let at_gateway = server.listen(g, "127.0.0.1:0").expect("a port");
    (server, at_user, at_gateway)
}

/// Lines the gateway says until one matches `what`.
fn until(rx: &Receiver<String>, what: &str) -> Vec<String> {
    let mut lines = Vec::new();
    while let Ok(line) = rx.recv_timeout(Duration::from_secs(20)) {
        let found = line.contains(what);
        lines.push(line);
        if found {
            return lines;
        }
    }
    panic!("never said {what:?}: {lines:#?}");
}

/// A user calls KK4ODA-1 on the modem at `at`; the session must go through.
fn call(store: &mut radiopost_client::store::Store, at: SocketAddr) {
    let mut said = Vec::new();
    let report = session::call(
        store,
        "KK4ODA-1",
        Kind::Gateway,
        &Options::new(&at.to_string()),
        &mut |s| said.push(s.to_owned()),
    )
    .expect("called");
    assert_eq!(report.failed, None, "{said:?}");
}

#[test]
#[allow(clippy::too_many_lines, reason = "one gateway's day, told in order")]
fn a_gateway_answers_on_two_modems_one_session_at_a_time() {
    let dir = scratch("several-modems");
    let (running, base, po_key) = post_office(&dir.join("po"));
    let (_one, user_one, gateway_one) = channel("WD5EMA");
    let (_two, user_two, gateway_two) = channel("N1CALL");
    let (a, b) = (Tap::new(gateway_one), Tap::new(gateway_two));

    // KK4ODA-1 on both modems, registered by its operator
    let _operator = client(
        &dir.join("kk4oda"),
        &running,
        &po_key,
        "facundo@test.example",
        "KK4ODA",
    );
    let gw = dir.join("gateway");
    std::fs::create_dir_all(&gw).expect("a directory");
    station::write(&gw, "key.hex", &radiopost::cbor::hex(&key().to_bytes())).expect("written");
    station::Settings {
        callsign: "KK4ODA-1".into(),
        modem: a.at.to_string(),
        more_modems: vec![b.at.to_string()],
        country: "US".into(),
        post_offices: BTreeMap::from([("test.example".into(), base.clone())]),
        resolver: String::new(),
        blocked: Vec::new(),
        only: None,
        operator_checks_rules: true,
        answer_at_start: true,
        copies: None,
        partners: Vec::new(),
        windows: Vec::new(),
        scan: Vec::new(),
    }
    .save(&gw)
    .expect("saved");
    operator::register(
        &gw,
        &dir.join("kk4oda"),
        json!({"channels": [{"dial_hz": 7_101_000, "mode": "USB", "modem": "tcp", "bandwidth_hz": 2300},
                            {"dial_hz": 7_102_000, "mode": "USB", "modem": "mercury", "bandwidth_hz": 2300}],
               "hours": "always", "internet": true}),
    )
    .expect("registered");
    let (tx, rx) = std::sync::mpsc::channel();
    let gw_dir = gw.clone();
    thread::spawn(move || {
        let _ = station::serve(&gw_dir, &mut |line| {
            let _ = tx.send(line.to_owned());
        });
    });
    let first = until(&rx, "listening");
    assert_eq!(
        first.last().map(String::as_str),
        Some(format!("listening as KK4ODA-1 on {} and {}", a.at, b.at).as_str())
    );
    until(&rx, &format!("also listening on {}", b.at));
    assert_eq!(a.wait_listens(1), [true]);
    assert_eq!(b.wait_listens(1), [true]);

    // the second modem answered a call just as the first's session began: once it hears it is to
    // hold, the tap says so, and that call is ended after the session rather than left up
    let hold = b.clone();
    thread::spawn(move || {
        if hold.wait_listens(2) == [true, false] {
            hold.say(
                &json!({"op": "connected", "link": "x1", "remote": "W1AW", "local": "KK4ODA-1",
                        "direction": "in"})
                .to_string(),
            );
        }
    });
    let mut one = client(
        &dir.join("wd5ema"),
        &running,
        &po_key,
        "club@test.example",
        "WD5EMA",
    );
    call(&mut one, user_one);
    let lines = until(&rx, "the call was ended");
    assert!(
        lines
            .iter()
            .any(|l| *l == format!("WD5EMA called on {}", a.at)),
        "{lines:#?}"
    );
    assert!(
        lines.iter().any(|l| *l
            == format!(
                "W1AW called on {} while another session was carried: the call was ended",
                b.at
            )),
        "{lines:#?}"
    );
    assert_eq!(
        b.wait_listens(3),
        [true, false, true],
        "held, then answering"
    );
    assert!(
        b.asked()
            .iter()
            .any(|m| m["op"] == "abort" && m["link"] == "x1"),
        "{:#?}",
        b.asked()
    );
    assert_eq!(a.listens(), [true], "the first modem carried it");

    // a call on the second modem: the first holds while it is carried
    let mut two = client(
        &dir.join("n1call"),
        &running,
        &po_key,
        "n1call@test.example",
        "N1CALL",
    );
    call(&mut two, user_two);
    let lines = until(&rx, "the session with N1CALL is over");
    assert!(
        lines
            .iter()
            .any(|l| *l == format!("N1CALL called on {}", b.at)),
        "{lines:#?}"
    );
    assert_eq!(a.wait_listens(3), [true, false, true]);
    let _ = std::fs::remove_dir_all(dir.as_path() as &Path);
}
