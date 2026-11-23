// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Gateways page's views through the panel (choosing a gateway, §7.3, §16.1): favourites and
//! blocks by gateway and by channel, a gateway of one's own placed by distance, and what the call
//! records say: the recent calls, each gateway's last call, and those worked from here.

mod common;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};

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

fn mark(at: SocketAddr, body: &Value) -> Value {
    let (status, v) = http(at, "POST", "/api/gateway-marks", body);
    assert_eq!(status, 200, "{body}: {v}");
    v
}

fn find<'a>(v: &'a Value, callsign: &str) -> &'a Value {
    v["gateways"]
        .as_array()
        .expect("a list")
        .iter()
        .find(|g| g["callsign"] == callsign)
        .unwrap_or_else(|| panic!("{callsign} listed: {v}"))
}

#[test]
fn favourites_blocks_own_gateways_and_the_calls_made() {
    let dir = scratch("gateway-views");
    let home = dir.join("facundo");
    let mut store = store_for(&home, "device:facundo", "facundo");
    store.set_grid(Some("EM73tr")).expect("a grid square");
    store.gateway_list = Some(
        json!({"domain": "radiopost.org", "fetched": 1, "gateways": [
            {"callsign": "KK4ODA-1", "grid": "EM73tr", "post_office": "radiopost.org",
             "channels": [{"dial_hz": 3_591_500, "bandwidth_hz": 2300},
                          {"dial_hz": 7_101_000, "bandwidth_hz": 2300}]},
            {"callsign": "N1CALL-10", "grid": "EM10", "post_office": "radiopost.org",
             "channels": [{"dial_hz": 7_101_000, "bandwidth_hz": 2300}]},
        ]}),
    );
    store.save().expect("saved");
    // two calls already made from here: one that finished, one that was not answered
    let attempts = home.join("selection/attempts");
    std::fs::create_dir_all(&attempts).expect("a folder");
    let place = json!({"lat": 33.749, "lon": -84.388, "grid": "EM73tr", "source": "grid"});
    let records = [
        json!({"v": 1, "id": "a-1", "start": 1_795_100_000, "end": 1_795_100_300, "kind": "gateway",
               "remote": "KK4ODA-1", "channel": {"dial_hz": 7_101_000, "bandwidth_hz": 2300},
               "place": place, "outcome": {"class": "completed"}, "classifier": 1}),
        json!({"v": 1, "id": "a-2", "start": 1_795_140_000, "end": 1_795_140_060, "kind": "gateway",
               "remote": "N1CALL-10", "channel": {"dial_hz": 7_101_000, "bandwidth_hz": 2300},
               "place": place, "outcome": {"class": "no-answer"}, "classifier": 1}),
    ];
    let lines = records
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(attempts.join("2026-11.jsonl"), lines).expect("written");
    let at = Panel::new(&home, None)
        .start("127.0.0.1:0")
        .expect("a port");

    let (_, v) = http(at, "GET", "/api/gateways", &Value::Null);
    let kk = find(&v, "KK4ODA-1");
    assert_eq!(kk["worked"], true, "finished from here");
    assert_eq!(kk["last"]["class"], "completed");
    let n1 = find(&v, "N1CALL-10");
    assert_eq!(n1["worked"], false);
    assert_eq!(n1["last"]["words"], "no answer, nothing heard");

    // the recent calls, newest first, in words
    let (_, r) = http(at, "GET", "/api/attempts?limit=10", &Value::Null);
    assert_eq!(r["recent"][0]["remote"], "N1CALL-10");
    assert_eq!(r["recent"][1]["completed"], true);
    assert_eq!(r["recent"][1]["took_s"], 300);

    // the call box's memory: each gateway called, once and newest first, with the channel of its
    // last call and its listed channels; no station called yet
    let (status, c) = http(at, "GET", "/api/called", &Value::Null);
    assert_eq!(status, 200, "{c}");
    assert_eq!(c["gateway"][0]["callsign"], "N1CALL-10");
    assert_eq!(c["gateway"][1]["callsign"], "KK4ODA-1");
    assert_eq!(c["gateway"][1]["dial_hz"], 7_101_000);
    assert_eq!(
        c["gateway"][1]["gateway"]["channels"][1]["dial_hz"],
        7_101_000
    );
    assert_eq!(c["station"], json!([]));

    // a favourite channel, a blocked gateway, a gateway of one's own between them
    mark(
        at,
        &json!({"op": "favourite", "callsign": "KK4ODA-1", "dial_hz": 7_101_000}),
    );
    mark(at, &json!({"op": "block", "callsign": "N1CALL-10"}));
    let v = mark(
        at,
        &json!({"op": "add-own", "callsign": "n0call-10", "grid": "EM74",
                             "place": "The club", "dial_hz": 7_103_500, "bandwidth_hz": 500}),
    );
    let order: Vec<&str> = v["gateways"]
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|g| g["callsign"].as_str())
        .collect();
    assert_eq!(order, ["KK4ODA-1", "N0CALL-10", "N1CALL-10"]);
    let kk = find(&v, "KK4ODA-1");
    assert_eq!(kk["favourite"], false, "only one channel is");
    assert_eq!(kk["channels"][1]["favourite"], true);
    assert_eq!(find(&v, "N1CALL-10")["blocked"], true);
    assert_eq!(find(&v, "N0CALL-10")["own"], true);

    // the marks are kept for the next start
    let again = Panel::new(&home, None)
        .start("127.0.0.1:0")
        .expect("a port");
    let (_, v) = http(again, "GET", "/api/gateways", &Value::Null);
    assert_eq!(find(&v, "N1CALL-10")["blocked"], true);
    assert_eq!(find(&v, "N0CALL-10")["place"], "The club");

    // a listed gateway cannot be one's own, and a request that names no gateway is refused
    let (status, _) = http(
        at,
        "POST",
        "/api/gateway-marks",
        &json!({"op": "add-own", "callsign": "KK4ODA-1", "dial_hz": 7_101_000}),
    );
    assert_eq!(status, 400);
    let (status, _) = http(at, "POST", "/api/gateway-marks", &json!({"op": "block"}));
    assert_eq!(status, 400);
    let _ = std::fs::remove_dir_all(&dir);
}
