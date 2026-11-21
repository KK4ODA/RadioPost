// SPDX-License-Identifier: MIT OR Apache-2.0
//! Alerts (RP-8) between two clients' connections between calls (`presence`), through the
//! loopback adapter shared as Aether is (RP-4 §1.1): Facundo alerts Ana; her station says heard by
//! itself; she acknowledges and replies; each answer reaches him. The alert goes on a dial, the
//! radio tuned there, and her answers name the dial she heard it on (RP-8 0.2.0, RP-4 1.5.0). A
//! modem that serves one program at a time is let go at once.

mod common;

use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use radiopost_client::alerts::{Alerts, Level, Sending};
use radiopost_client::presence::{Carrier, Presence, unix_now};
use rpmi::channel::{Channel, StationConfig};
use rpmi::tcp::Server;
use rpmi::wire::Caps;
use serde_json::Value;

use common::{scratch, store_for};

const KEYWORD: &str = "kimo dali sune pato";

/// The dial both stations' radios are on.
const DIAL: u64 = 7_101_000;

/// Two stations, KK4ODA and N0CALL, on a simulated modem that carries broadcasts, has a dial and
/// is shared.
fn shared_loopback() -> (Server, String, String) {
    let caps = Caps {
        bandwidths: vec![2300],
        max_links: 1,
        delivery: true,
        broadcast: true,
        quality: false,
        busy: true,
        regulatory: false,
        max_queue: 65536,
        shared: true,
        dial: true,
        scan: false,
        call_s: false,
        tone: false,
    };
    let station = |name: &str, prefix: &str| StationConfig {
        name: name.into(),
        modem: "a shared loopback".into(),
        caps: caps.clone(),
        prefix: prefix.into(),
        frame_bytes: 128,
        dial_hz: Some(DIAL),
    };
    let mut ch = Channel::new();
    let f = ch.add_station(station("KK4ODA", "c"));
    let a = ch.add_station(station("N0CALL", "a"));
    let server = Server::new(ch, Duration::from_secs(10));
    let at_f = server.listen(f, "127.0.0.1:0").expect("a port").to_string();
    let at_a = server.listen(a, "127.0.0.1:0").expect("a port").to_string();
    (server, at_f, at_a)
}

/// The times the panel lent its station to the alerts, and the times it was given back.
#[derive(Clone, Default)]
struct Lends {
    taken: Arc<AtomicUsize>,
    back: Arc<AtomicUsize>,
}

impl Lends {
    /// Waits until every lend has been given back. While a lend lasts, `send_alerts` sends all
    /// that falls due on the one connection, so a step taken before the last lend ends has what
    /// it queues go on that lend, not a new one (CI's runners did, now and then). An operator
    /// acknowledges or replies seconds later, long after it ended.
    fn settled(&self) {
        let end = Instant::now() + Duration::from_secs(10);
        while self.taken.load(Ordering::SeqCst) != self.back.load(Ordering::SeqCst) {
            assert!(
                Instant::now() < end,
                "the station was not given back in time"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

/// A client's connection between calls, carrying its alerts; the station is lent to it each time
/// it asks, as an idle panel would, and the times it was taken and given back are counted.
fn carrying(home: &Path, modem: &str, lends: &Lends) -> (Presence, Alerts) {
    let alerts = Alerts::open(home);
    let presence = Presence::new(home).retrying_every(Duration::from_millis(200));
    let counted = lends.clone();
    presence.carry(Carrier {
        alerts: alerts.clone(),
        base: home.to_owned(),
        gate: Arc::new(move |take| {
            let count = if take { &counted.taken } else { &counted.back };
            count.fetch_add(1, Ordering::SeqCst);
            true
        }),
        say: Arc::new(|_line: &str| {}),
    });
    presence.start();
    presence.set_modem(modem);
    (presence, alerts)
}

/// Waits until the log's newest entry says what `wanted` takes.
fn until(alerts: &Alerts, within: Duration, wanted: impl Fn(&Value) -> bool) -> Value {
    let end = Instant::now() + within;
    loop {
        let log = alerts.json();
        if let Some(newest) = log["log"].get(0)
            && wanted(newest)
        {
            return newest.clone();
        }
        assert!(
            Instant::now() < end,
            "not in time: {}",
            serde_json::to_string_pretty(&log).unwrap_or_default()
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn an_alert_crosses_between_two_panels_and_is_answered() {
    let dir = scratch("alerts-two");
    let (f_home, a_home) = (dir.join("facundo"), dir.join("ana"));
    store_for(&f_home, "device:facundo", "facundo");
    store_for(&a_home, "device:ana", "ana");
    let (_server, at_f, at_a) = shared_loopback();
    let lends = Lends::default();
    let (f_presence, facundo) = carrying(&f_home, &at_f, &lends);
    let (a_presence, ana) = carrying(&a_home, &at_a, &lends);
    facundo
        .add_circle("Family", KEYWORD, Level::TimeSensitive)
        .expect("kept");
    ana.add_circle("Family", "Kimo  DALI sune pato", Level::TimeSensitive)
        .expect("kept");
    // both connections hold the modem before anything is sent
    thread::sleep(Duration::from_millis(600));

    // the connections between calls know where the radio is
    assert_eq!(a_presence.dial(), Some(DIAL));
    let tag = facundo
        .send_on(
            "Family",
            Some("N0CALL"),
            "Please call me on 146.52 now",
            "KK4ODA",
            unix_now(),
            Sending {
                dial_hz: Some(DIAL),
                outlast_s: None,
            },
        )
        .expect("written");
    let heard_there = until(&ana, Duration::from_secs(10), |e| e["tag"] == tag);
    assert_eq!(heard_there["from"], "KK4ODA");
    assert_eq!(heard_there["state"], "new");
    assert_eq!(heard_there["dial_hz"], DIAL, "the dial it was heard on");
    assert!(ana.alarming(), "it sounds until acknowledged");
    until(&facundo, Duration::from_secs(10), |e| e["state"] == "heard");
    lends.settled();

    ana.acknowledge(&tag, unix_now()).expect("acknowledged");
    assert!(!ana.alarming());
    until(&facundo, Duration::from_secs(10), |e| e["state"] == "seen");
    lends.settled();
    ana.reply(&tag, "On my way, 5 min", unix_now())
        .expect("replied");
    let answered = until(&facundo, Duration::from_secs(10), |e| {
        e["state"] == "replied"
    });
    let last = answered["answers"]
        .as_array()
        .and_then(|a| a.last())
        .cloned()
        .unwrap_or_default();
    assert_eq!(last["from"], "N0CALL");
    assert_eq!(last["text"], "On my way, 5 min");
    assert!(
        lends.taken.load(Ordering::SeqCst) >= 4,
        "each send took the station from the panel"
    );
    lends.settled();
    assert_eq!(
        lends.taken.load(Ordering::SeqCst),
        lends.back.load(Ordering::SeqCst),
        "every lend was given back"
    );
    f_presence.stop();
    a_presence.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_modem_that_serves_one_program_is_let_go() {
    let dir = scratch("alerts-unshared");
    let home = dir.join("facundo");
    store_for(&home, "device:facundo", "facundo");
    // an RPMI adapter that answers nobody: the connection is opened and, with no hello answered,
    // given up; one that says it is not shared is never held (the loopback test of `presence`
    // covers the answered case through Aether's stand-in)
    let adapter = TcpListener::bind("127.0.0.1:0").expect("a port");
    let (presence, alerts) = carrying(
        &home,
        &adapter.local_addr().expect("bound").to_string(),
        &Lends::default(),
    );
    // no circle: no connection to a modem that is not Aether
    adapter.set_nonblocking(true).expect("non-blocking");
    thread::sleep(Duration::from_millis(600));
    assert!(adapter.accept().is_err(), "held with no circle kept");
    alerts
        .add_circle("Family", KEYWORD, Level::TimeSensitive)
        .expect("kept");
    let end = Instant::now() + Duration::from_secs(3);
    while adapter.accept().is_err() {
        assert!(Instant::now() < end, "a circle kept: the adapter is tried");
        thread::sleep(Duration::from_millis(50));
    }
    presence.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
