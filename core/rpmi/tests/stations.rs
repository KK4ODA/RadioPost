// SPDX-License-Identifier: MIT OR Apache-2.0
//! Two RadioPost stations through the simulated channel: session-03 (RP-3 §10), Facundo calling
//! Ana's station, carried by RPMI between their engines, in memory and over TCP. The
//! transmissions must be session-03.json's, byte for byte, whatever pieces `received` cut them
//! into.

mod common;

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use radiopost::session::Event as SessionEvent;
use rpmi::base64;
use rpmi::channel::{Channel, Out, StationConfig};
use rpmi::host::{Carrier, Engine, Event, Host};
use rpmi::tcp::{Connection, Server};
use rpmi::wire::{Caps, Message};
use serde_json::Value as Json;

use common::{Answering, Calling, transmissions, vectors, world};

fn caps(shared: bool) -> Caps {
    Caps {
        bandwidths: vec![500, 2300],
        max_links: 1,
        delivery: true,
        broadcast: true,
        quality: false,
        busy: true,
        regulatory: false,
        max_queue: 65536,
        shared,
    }
}

fn station(name: &str, prefix: &str, frame_bytes: usize) -> StationConfig {
    StationConfig {
        name: name.into(),
        modem: "a simulated modem".into(),
        caps: caps(false),
        prefix: prefix.into(),
        frame_bytes,
    }
}

/// A RadioPost station's program: hello, its callsign, then a call or `listen`, and the session.
struct Program<E: Engine<Event = SessionEvent>> {
    host: Host,
    callsign: &'static str,
    calls: Option<&'static str>,
    carrier: Carrier<E>,
    events: Vec<SessionEvent>,
    /// Each `send`: its `ref` and its bytes.
    sent: Vec<(String, Vec<u8>)>,
    failed: Option<String>,
}

impl<E: Engine<Event = SessionEvent>> Program<E> {
    fn new(engine: E, callsign: &'static str, calls: Option<&'static str>) -> Self {
        Program {
            host: Host::new(),
            callsign,
            calls,
            carrier: Carrier::new(engine),
            events: Vec::new(),
            sent: Vec::new(),
            failed: None,
        }
    }

    fn start(&mut self) -> Vec<Message> {
        vec![self.host.hello("RadioPost test station")]
    }

    fn on_line(&mut self, m: &Message) -> Vec<Message> {
        let event = Event::read(m).expect("an event");
        let out = match &event {
            Event::Hello { .. } => vec![self.host.callsigns(&[self.callsign])],
            Event::Ok { id } if *id == 2 => match self.calls {
                Some(remote) => vec![self.host.connect(remote, None, None)],
                None => vec![self.host.listen(true)],
            },
            Event::Failed { code, .. } => {
                self.failed = Some(code.clone());
                vec![]
            }
            _ => {
                let (out, said) = self.carrier.on_event(&mut self.host, &event);
                self.events.extend(said);
                out
            }
        };
        for m in &out {
            if m.get("op") == Some(&"send".into()) {
                let data = base64::decode(m["data"].as_str().expect("data")).expect("base64");
                let reference = m.get("ref").and_then(Json::as_str).expect("a ref");
                self.sent.push((reference.into(), data));
            }
        }
        out
    }

    fn done(&self) -> bool {
        self.carrier.is_ended() || self.failed.is_some()
    }
}

fn check_session_03(
    facundo: &Program<Calling>,
    ana: &Program<Answering>,
    w: &common::World,
    file: &Json,
) {
    let mut ours: Vec<(String, Vec<u8>)> = Vec::new();
    let (mut c, mut a) = (facundo.sent.iter(), ana.sent.iter());
    for (from, _) in transmissions(file) {
        let next = if from == "caller" { c.next() } else { a.next() };
        ours.push(next.expect("a transmission").clone());
    }
    // Each transmission is one `send`, its `ref` its number in the session.
    let theirs: Vec<(String, Vec<u8>)> = transmissions(file)
        .into_iter()
        .enumerate()
        .map(|(i, t)| (format!("t{}", i + 1), t.1))
        .collect();
    assert_eq!(ours, theirs, "session-03's transmissions");
    assert!(c.next().is_none() && a.next().is_none(), "nothing more");
    assert!(facundo.carrier.is_ended() && ana.carrier.is_ended());
    assert_eq!(
        common::delivered(&facundo.events),
        vec![w.messages["facundo-direct"].content_id()]
    );
    assert_eq!(
        common::received(&facundo.events),
        vec![w.messages["ana-to-facundo"].content_id()]
    );
    assert_eq!(
        common::received(&ana.events),
        vec![w.messages["facundo-direct"].content_id()]
    );
    assert!(
        facundo
            .events
            .iter()
            .any(|e| matches!(e, SessionEvent::StationProved(b) if b.address == "ana@hampost.org"))
    );
}

#[test]
fn session_03_through_the_channel() {
    let w = world();
    let file = vectors("session-03.json");
    for frame_bytes in [1, 7, 128, 100_000] {
        let mut ch = Channel::new();
        let f = ch.add_station(station("KK4ODA", "c", frame_bytes));
        let a = ch.add_station(station("N0CALL", "a", frame_bytes));
        let mut facundo = Program::new(
            Calling(common::session_03_caller(&w)),
            "KK4ODA",
            Some("N0CALL"),
        );
        let mut ana = Program::new(Answering(common::session_03_called(&w)), "N0CALL", None);
        ch.open(a, "ana");
        for m in ana.start() {
            ch.request(a, "ana", &m);
        }
        ch.open(f, "facundo");
        for m in facundo.start() {
            ch.request(f, "facundo", &m);
        }
        for _ in 0..10_000 {
            let outs = ch.take();
            if outs.is_empty() {
                break;
            }
            for out in outs {
                let Out::Line {
                    station, message, ..
                } = out
                else {
                    panic!("a connection closed");
                };
                if station == f {
                    for m in facundo.on_line(&message) {
                        ch.request(f, "facundo", &m);
                    }
                } else {
                    for m in ana.on_line(&message) {
                        ch.request(a, "ana", &m);
                    }
                }
            }
        }
        check_session_03(&facundo, &ana, &w, &file);
    }
}

/// Runs a program on its own connection until its session is over or its call failed.
fn run<E: Engine<Event = SessionEvent>>(
    mut program: Program<E>,
    addr: std::net::SocketAddr,
    ready: Option<mpsc::Sender<()>>,
) -> Program<E> {
    let mut conn = Connection::open(addr).expect("the adapter");
    conn.set_timeout(Some(Duration::from_secs(20)))
        .expect("a timeout");
    for m in program.start() {
        conn.send(&m).expect("sent");
    }
    let mut ready = ready;
    while !program.done() {
        let m = conn
            .read_message()
            .expect("a line")
            .expect("the adapter stays");
        let listening = m.get("op") == Some(&"ok".into()) && m.get("id") == Some(&3.into());
        for out in program.on_line(&m) {
            conn.send(&out).expect("sent");
        }
        if listening && let Some(r) = ready.take() {
            r.send(()).expect("told");
        }
    }
    conn.close();
    program
}

#[test]
fn session_03_over_tcp() {
    let w = world();
    let file = vectors("session-03.json");
    let mut ch = Channel::new();
    let f = ch.add_station(station("KK4ODA", "c", 128));
    let a = ch.add_station(station("N0CALL", "a", 128));
    let server = Server::new(ch, Duration::from_secs(5));
    let at_f = server.listen(f, "127.0.0.1:0").expect("a port");
    let at_a = server.listen(a, "127.0.0.1:0").expect("a port");
    let ana = Program::new(Answering(common::session_03_called(&w)), "N0CALL", None);
    let facundo = Program::new(
        Calling(common::session_03_caller(&w)),
        "KK4ODA",
        Some("N0CALL"),
    );
    let (tx, rx) = mpsc::channel();
    let ana_runs = thread::spawn(move || run(ana, at_a, Some(tx)));
    rx.recv_timeout(Duration::from_secs(20))
        .expect("Ana's station listens");
    let facundo_runs = thread::spawn(move || run(facundo, at_f, None));
    let facundo = facundo_runs.join().expect("the caller's program");
    let ana = ana_runs.join().expect("the station's program");
    check_session_03(&facundo, &ana, &w, &file);
}

#[test]
fn a_call_nobody_answers_fails_on_time_over_tcp() {
    let w = world();
    let mut ch = Channel::new();
    let f = ch.add_station(station("KK4ODA", "c", 128));
    let server = Server::new(ch, Duration::from_millis(100));
    let at = server.listen(f, "127.0.0.1:0").expect("a port");
    let facundo = Program::new(
        Calling(common::session_03_caller(&w)),
        "KK4ODA",
        Some("N0CALL"),
    );
    let facundo = run(facundo, at, None);
    assert_eq!(facundo.failed.as_deref(), Some("timeout"));
    assert!(facundo.sent.is_empty());
}

/// RP-4 §1.1: an adapter that is not shared serves one host; a second is turned away `busy`.
#[test]
fn an_unshared_modem_turns_a_second_host_away() {
    let mut ch = Channel::new();
    let s = ch.add_station(station("KK4ODA", "c", 128));
    ch.open(s, "first");
    ch.open(s, "second");
    let hello = |id: i64| {
        let mut h = Host::new();
        let mut m = h.hello("RadioPost test station");
        m.insert("id".into(), id.into());
        m
    };
    ch.request(s, "first", &hello(1));
    ch.request(s, "second", &hello(1));
    let outs = ch.take();
    let lines: Vec<(String, String)> = outs
        .iter()
        .map(|o| match o {
            Out::Line { host, message, .. } => (
                host.clone(),
                message
                    .get("code")
                    .or(message.get("op"))
                    .and_then(Json::as_str)
                    .expect("a code or an op")
                    .into(),
            ),
            Out::Close { host, .. } => (host.clone(), "closed".into()),
        })
        .collect();
    assert_eq!(
        lines,
        [
            ("first".into(), "hello".into()),
            ("second".into(), "busy".into()),
            ("second".into(), "closed".into())
        ]
    );
    // Once the first host has gone, another may come.
    ch.close(s, "first");
    ch.open(s, "third");
    ch.request(s, "third", &hello(1));
    assert!(matches!(&ch.take()[..], [Out::Line { message, .. }] if message["op"] == "hello"));
}
