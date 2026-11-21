// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where the station is, through the panel (choosing a gateway, §6): the grid square set before
//! places becomes Home, places added by square and by latitude and longitude, a GPS sending NMEA
//! over UDP taking over, the gateways ranked from the position in use, and a fix saved as a place.

mod common;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::thread;
use std::time::{Duration, Instant};

use radiopost_client::panel::Panel;
use serde_json::{Value, json};

use common::{scratch, store_for};

fn http(at: SocketAddr, method: &str, path: &str, body: &Value) -> (u16, Value) {
    let mut s = TcpStream::connect(at).expect("the panel");
    let body = if method == "POST" {
        body.to_string()
    } else {
        String::new()
    };
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-RadioPost: 1\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        at.port(),
        body.len()
    )
    .expect("sent");
    let mut text = String::new();
    s.read_to_string(&mut text).expect("an answer");
    let status = text[9..12].parse().expect("a status");
    let body = text.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    (status, serde_json::from_str(body).expect("JSON"))
}

fn get(at: SocketAddr, path: &str) -> Value {
    let (status, v) = http(at, "GET", path, &Value::Null);
    assert_eq!(status, 200, "{path}: {v}");
    v
}

fn post(at: SocketAddr, body: &Value) -> Value {
    let (status, v) = http(at, "POST", "/api/places", body);
    assert_eq!(status, 200, "{body}: {v}");
    v
}

/// The gateways in the order the panel lists them.
fn order(at: SocketAddr) -> Vec<String> {
    get(at, "/api/gateways")["gateways"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|g| g["callsign"].as_str().expect("a callsign").to_owned())
        .collect()
}

/// An NMEA sentence with its checksum.
fn nmea(body: &str) -> String {
    let sum = body.bytes().fold(0u8, |x, b| x ^ b);
    format!("${body}*{sum:02X}\r\n")
}

#[test]
fn the_position_in_use_comes_from_places_or_the_gps_and_ranks_the_gateways() {
    let dir = scratch("places");
    let home = dir.join("facundo");
    let mut store = store_for(&home, "device:facundo", "facundo");
    store.set_grid(Some("EM73tr")).expect("a grid square");
    store.gateway_list = Some(
        json!({"domain": "radiopost.org", "fetched": 1, "gateways": [
            {"callsign": "W1AW-10", "grid": "FN31pr"},
            {"callsign": "KK4ODA-1", "grid": "EM73tr"},
            {"callsign": "N4NORTH-1", "grid": "EM74sc"},
        ]}),
    );
    store.save().expect("saved");
    let at = Panel::new(&home, None)
        .start("127.0.0.1:0")
        .expect("a port");

    // the grid square set before places is Home, the default, and the gateways are ranked from it
    let v = get(at, "/api/places");
    assert_eq!(v["places"][0]["name"], "Home");
    assert_eq!(v["places"][0]["source"], "grid");
    assert_eq!(v["here"]["grid"], "EM73tr");
    assert_eq!(order(at), ["KK4ODA-1", "N4NORTH-1", "W1AW-10"]);

    // a place typed as a latitude and longitude, chosen: the list follows it
    post(
        at,
        &json!({"op": "add", "name": "Field Day", "where": "34.10, -84.50"}),
    );
    post(
        at,
        &json!({"op": "choice", "use": "place", "id": "pl-field-day"}),
    );
    let g = get(at, "/api/gateways");
    assert_eq!(g["here"]["grid"], "EM74sc");
    assert_eq!(order(at), ["N4NORTH-1", "KK4ODA-1", "W1AW-10"]);

    // a GPS over UDP, automatic: its fix at Newington, Connecticut takes over. The client binds the
    // port it is told itself, so the port is freed for it rather than held; the phone's socket is
    // bound while it is still held, or it could be given that very port
    let free = UdpSocket::bind("127.0.0.1:0").expect("a port");
    let port = free.local_addr().expect("bound").port();
    let phone = UdpSocket::bind("127.0.0.1:0").expect("a port");
    drop(free);
    post(at, &json!({"op": "choice", "use": "automatic"}));
    post(
        at,
        &json!({"op": "gps", "source": {"kind": "udp", "port": port}}),
    );
    let start = Instant::now();
    let v = loop {
        let datagram = nmea("GPGGA,031522.00,4143.7520,N,07242.4980,W,1,08,0.9,25.0,M,0.0,M,,")
            + &nmea("GPRMC,031522.00,A,4143.7520,N,07242.4980,W,0.0,0.0,201126,,,A");
        phone
            .send_to(datagram.as_bytes(), ("127.0.0.1", port))
            .expect("sent");
        let v = get(at, "/api/places");
        if v["here"]["from"] == "gps" {
            break v;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "no fix: {v}");
        thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(v["here"]["grid"], "FN31pr");
    assert!(
        v["status"]["words"]
            .as_str()
            .is_some_and(|w| w.starts_with("reading: fix")),
        "{v}"
    );
    assert_eq!(order(at), ["W1AW-10", "N4NORTH-1", "KK4ODA-1"]);

    // the fix saved as a place, with its height
    let v = post(at, &json!({"op": "add-gps", "name": "Newington"}));
    let saved = &v["places"][2];
    assert_eq!(
        (saved["source"].as_str(), saved["square"].as_str()),
        (Some("gps"), Some("FN31pr"))
    );
    assert_eq!(saved["ground_m"], 25.0);

    // no GPS: the place chosen before automatic falls to the default, Home
    post(at, &json!({"op": "gps", "source": null}));
    assert_eq!(get(at, "/api/places")["here"]["grid"], "EM73tr");

    // a request that makes no sense is refused, and nothing is changed
    let (status, _) = http(
        at,
        "POST",
        "/api/places",
        &json!({"op": "remove", "id": "pl-nowhere"}),
    );
    assert_eq!(status, 400);
    assert_eq!(
        get(at, "/api/places")["places"].as_array().map(Vec::len),
        Some(3)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
