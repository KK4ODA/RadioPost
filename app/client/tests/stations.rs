// SPDX-License-Identifier: MIT OR Apache-2.0
//! The stations the modem heard reach the client (RP-4 §4.4, wishlist W59): one heard before the
//! panel's connection opened comes in its `stations` answer, one heard after as `heard`, each with
//! the dial and the position the beacon carried, into the store **Call**'s list is made from.

mod common;

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use radiopost_client::alerts::{Alerts, Level};
use radiopost_client::presence::{Carrier, Presence, unix_now};
use radiopost_client::stations::Stations;
use rpmi::channel::{Channel, StationConfig};
use rpmi::host::Host;
use rpmi::position::{Position, Precision};
use rpmi::tcp::{Connection, Server};
use rpmi::wire::Caps;

use common::{scratch, store_for};

const DIAL: u64 = 7_101_000;

/// A beacon from `callsign`, on a host of its own at `at`, with its position.
fn beacon_from(at: &str, callsign: &str, position: &Position) {
    let mut conn = Connection::open(at).expect("the adapter");
    conn.set_timeout(Some(Duration::from_secs(5)))
        .expect("a timeout");
    let mut host = Host::new();
    for request in [
        host.hello("another program 1.0"),
        host.callsigns(&[callsign]),
        host.beacon(None, Some("b1"), None, None, Some(position)),
    ] {
        conn.send(&request).expect("sent");
    }
    // the hello, the ok and the beacon's ok, then what became of it
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        if let Ok(Some(m)) = conn.read_message()
            && m.get("op").and_then(|o| o.as_str()) == Some("beacon-sent")
        {
            return;
        }
    }
    panic!("the beacon was not said to have gone");
}

fn waits_for(stations: &Stations, callsign: &str) -> radiopost_client::stations::Seen {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(seen) = stations.all().into_iter().find(|s| s.callsign == callsign) {
            return seen;
        }
        assert!(Instant::now() < end, "{callsign} never reached the store");
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn stations_heard_before_and_after_the_panel_connects_reach_its_list() {
    let dir = scratch("stations-heard");
    let home = dir.join("facundo");
    store_for(&home, "device:facundo", "facundo");
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
        heard: true,
        beacon: true,
        position: true,
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
    ch.set_now(unix_now());
    let mine = ch.add_station(station("KK4ODA", "c"));
    let first = ch.add_station(station("N2CALL", "a"));
    let second = ch.add_station(station("N3CALL", "b"));
    let server = Server::new(ch, Duration::from_secs(10));
    let at_mine = server.listen(mine, "127.0.0.1:0").expect("a port").to_string();
    let at_first = server.listen(first, "127.0.0.1:0").expect("a port").to_string();
    let at_second = server.listen(second, "127.0.0.1:0").expect("a port").to_string();

    // heard while the client was away: its station's modem keeps it
    let exact = Position {
        lat: 34.4621,
        lon: -83.7514,
        precision: Precision::Exact,
        grid: None,
    };
    beacon_from(&at_first, "N2CALL", &exact);

    // the panel's connection between calls holds a shared adapter while circles are kept
    let alerts = Alerts::open(&home);
    alerts
        .add_circle("Family", "kimo dali sune pato", Level::Normal)
        .expect("kept");
    let stations = Stations::in_memory();
    let presence = Presence::new(&home).retrying_every(Duration::from_millis(200));
    presence.carry(Carrier {
        alerts,
        base: home.clone(),
        gate: Arc::new(|_| true),
        say: Arc::new(|_line: &str| {}),
        stations: stations.clone(),
    });
    presence.start();
    presence.set_modem(&at_mine);

    let n2 = waits_for(&stations, "N2CALL");
    assert_eq!(n2.dial_hz, Some(DIAL));
    assert_eq!(n2.band.as_deref(), Some("40m"));
    assert_eq!(n2.kind.as_deref(), Some("beacon"));
    assert_eq!(n2.answers, Some(false), "nobody listens at N2CALL");
    assert_eq!(n2.position.as_ref().and_then(|p| p["precision"].as_str()), Some("exact"));

    // heard while the client is here: a `heard`, its square at the centre
    let square = Position {
        precision: Precision::Grid6,
        ..exact
    };
    beacon_from(&at_second, "N3CALL", &square);
    let n3 = waits_for(&stations, "N3CALL");
    assert_eq!(n3.position.as_ref().and_then(|p| p["grid"].as_str()), Some("EM84cl"));
    presence.stop();
}
