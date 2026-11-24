// SPDX-License-Identifier: MIT OR Apache-2.0
//! The record of every call (choosing a gateway, §12.1), through the loopback adapter: a call that
//! completes, one nobody answers and one whose modem cannot be reached each leave one record in
//! the first home, classed by what happened, and none holds an address.

mod common;

use std::thread;
use std::time::Duration;

use radiopost_client::selection::attempts;
use radiopost_client::session::{self, Kind, Options};
use rpmi::host::{Event, Host};
use rpmi::tcp::Connection;
use serde_json::Value;

use common::{loopback, loopback_with, message, scratch, store_for};

#[test]
fn a_completed_call_is_recorded_with_its_link_and_what_it_carried() {
    let dir = scratch("attempts-done");
    let mut facundo = store_for(&dir.join("facundo"), "device:facundo", "facundo");
    let mut ana = store_for(&dir.join("ana"), "device:ana", "ana");
    facundo.queue(&message("facundo-direct")).expect("queued");
    facundo.set_grid(Some("EM73tr")).expect("a grid square");
    let (_server, at_f, at_a) = loopback();
    let (tx, rx) = std::sync::mpsc::channel();
    let ana_opts = Options::new(&at_a.to_string());
    let listener = thread::spawn(move || {
        session::answer(&mut ana, &ana_opts, &mut |s: &str| {
            if s.starts_with("listening") {
                let _ = tx.send(());
            }
        })
        .expect("answered")
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("Ana's station listens");
    let mut said = Vec::new();
    let called = session::call(
        &mut facundo,
        "N0CALL",
        Kind::Station,
        &Options::new(&at_f.to_string()),
        &mut |s| said.push(s.to_owned()),
    )
    .expect("called");
    listener.join().expect("the listener");
    assert_eq!(called.failed, None, "{said:?}");

    let all = attempts::read(&dir.join("facundo"));
    assert_eq!(all.len(), 1, "{all:?}");
    let a = &all[0];
    assert_eq!(a["outcome"]["class"], "completed", "{a}");
    assert_eq!(a["outcome"]["plan_done"], true, "{a}");
    assert_eq!(a["kind"], "station");
    assert_eq!(a["remote"], "N0CALL");
    assert_eq!(a["as"], "KK4ODA");
    assert_eq!(a["modem"], "the loopback adapter");
    assert_eq!(a["place"]["grid"], "EM73tr");
    assert_eq!(a["transaction"]["messages"], 1);
    assert!(a["transaction"]["out_bytes"].as_u64() > Some(0), "{a}");
    assert_eq!(a["call"]["result"], "connected");
    assert_eq!(a["session"]["taken"], 1);
    assert_eq!(a["session"]["over"], true);
    assert!(a["link"]["bytes_handed"].as_u64() > Some(0), "{a}");
    assert_eq!(a["link"]["end"], "local", "the caller hangs up (RP-3 §8)");
    assert_eq!(a["classifier"], 1);
    let text = a.to_string();
    assert!(!text.contains('@'), "no address in the record: {text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_call_nobody_answers_is_recorded_as_no_answer() {
    let dir = scratch("attempts-silent");
    let mut facundo = store_for(&dir.join("facundo"), "device:facundo", "facundo");
    facundo.queue(&message("facundo-direct")).expect("queued");
    let (_server, at_f, _at_a) = loopback_with(None, Duration::from_millis(300));
    let called = session::call(
        &mut facundo,
        "N0CALL",
        Kind::Station,
        &Options::new(&at_f.to_string()),
        &mut |_| {},
    )
    .expect("the call ends");
    assert!(called.failed.is_some(), "nobody listened");
    let all = attempts::read(&dir.join("facundo"));
    assert_eq!(all.len(), 1, "{all:?}");
    let a = &all[0];
    assert_eq!(a["outcome"]["class"], "no-answer", "{a}");
    assert_eq!(a["outcome"]["left_out"], 1, "{a}");
    assert_eq!(a["call"]["result"], "timeout");
    assert_eq!(a["call"]["heard_remote"], false);
    assert_eq!(a["link"], Value::Null);
    assert_eq!(a["session"], Value::Null);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_modem_that_cannot_be_reached_is_recorded_as_this_station_s() {
    let dir = scratch("attempts-local");
    let mut facundo = store_for(&dir.join("facundo"), "device:facundo", "facundo");
    // nothing listens on port 9 here
    let e = session::call(
        &mut facundo,
        "N0CALL",
        Kind::Station,
        &Options::new("127.0.0.1:9"),
        &mut |_| {},
    )
    .expect_err("no adapter");
    let all = attempts::read(&dir.join("facundo"));
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0]["outcome"]["class"], "local-station", "{}", all[0]);
    assert_eq!(all[0]["outcome"]["why"], e.as_str());
    assert_eq!(all[0]["modem"], Value::Null, "no hello was answered");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A link the other station ends in the middle is recorded as lost, not completed: a broken link
/// leaves the session's engine over too, and until 2026-11-23 the record took that for both
/// stations' `DONE`, so every session that broke (the other station gone, silence, a link
/// dropped) was recorded as completed, and learned as a call that worked.
#[test]
fn a_link_the_other_station_ends_is_recorded_as_lost() {
    let dir = scratch("attempts-lost");
    let mut facundo = store_for(&dir.join("facundo"), "device:facundo", "facundo");
    facundo.queue(&message("facundo-direct")).expect("queued");
    let (_server, at_f, at_a) = loopback();
    // N0CALL's station, by hand: it answers, and hangs up before a word
    let mut other = Connection::open(at_a).expect("the adapter");
    other
        .set_timeout(Some(Duration::from_secs(10)))
        .expect("a wait");
    let mut host = Host::new();
    for request in [
        host.hello("a test"),
        host.callsigns(&["N0CALL"]),
        host.listen(true),
    ] {
        other.send(&request).expect("sent");
    }
    let hangs_up = thread::spawn(move || {
        let link = loop {
            let line = other.read_message().expect("read").expect("a line");
            if let Ok(Event::Connected { link, .. }) = Event::read(&line) {
                break link;
            }
        };
        other.send(&host.disconnect(&link)).expect("sent");
        other
    });
    let called = session::call(
        &mut facundo,
        "N0CALL",
        Kind::Station,
        &Options::new(&at_f.to_string()),
        &mut |_| {},
    )
    .expect("the session ends");
    hangs_up.join().expect("N0CALL's station").close();
    assert_eq!(facundo.outbox.len(), 1, "nothing crossed: {called:?}");
    let all = attempts::read(&dir.join("facundo"));
    assert_eq!(all.len(), 1, "{all:?}");
    let a = &all[0];
    assert_eq!(a["outcome"]["class"], "link-lost", "{a}");
    assert_eq!(a["session"]["over"], false, "{a}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A gateway call nobody answers, on the channel at `dial_hz` of a gateway whose kept record lists
/// `channels`: what the call said, and its record.
fn gateway_call_on(name: &str, channels: &Value, dial_hz: u64) -> (Vec<String>, Value) {
    let dir = scratch(name);
    let mut facundo = store_for(&dir.join("facundo"), "device:facundo", "facundo");
    facundo.gateway_list = Some(serde_json::json!({
        "domain": "radiopost.org", "fetched": 1,
        "gateways": [{"callsign": "KK4ODA-1", "grid": "EM73tr", "post_office": "radiopost.org",
                      "channels": channels.clone()}],
    }));
    facundo.save().expect("saved");
    let (_server, at_f, _at_a) = loopback_with(Some(dial_hz), Duration::from_millis(300));
    let mut opts = Options::new(&at_f.to_string());
    opts.dial_hz = Some(dial_hz);
    let mut said = Vec::new();
    let called = session::call(&mut facundo, "KK4ODA-1", Kind::Gateway, &opts, &mut |s| {
        said.push(s.to_owned());
    })
    .expect("the call ends");
    assert!(called.failed.is_some(), "nobody answered: {said:?}");
    let all = attempts::read(&dir.join("facundo"));
    assert_eq!(all.len(), 1, "{all:?}");
    let _ = std::fs::remove_dir_all(&dir);
    (said, all[0].clone())
}

/// A gateway call asks the modem for its channel's bandwidth, so it goes at the bandwidth the
/// channel picked says: on 2026-11-24 a call picked on "7.101 MHz, 2300 Hz" asked for none and went
/// at the 500 Hz the caller's Aether was set to. One the modem cannot be asked for is left out and
/// said; an FM channel's width is no modem's bandwidth.
#[test]
fn a_gateway_call_asks_for_its_channel_s_bandwidth() {
    let (said, a) = gateway_call_on(
        "attempts-channel-bw",
        &serde_json::json!([{"dial_hz": 7_101_000, "mode": "USB", "modem": "aether",
                            "bandwidth_hz": 2300},
                           {"dial_hz": 3_590_000, "mode": "USB", "modem": "aether",
                            "bandwidth_hz": 500}]),
        7_101_000,
    );
    assert!(
        said.iter().any(|l| l == "calling KK4ODA-1 at 2300 Hz"),
        "{said:?}"
    );
    assert_eq!(a["channel"]["bandwidth_hz"], 2300, "{a}");
    assert_eq!(
        a["call"]["result"], "timeout",
        "the modem took the bandwidth: {a}"
    );

    let (said, a) = gateway_call_on(
        "attempts-channel-500",
        &serde_json::json!([{"dial_hz": 7_101_000, "mode": "USB", "modem": "aether",
                            "bandwidth_hz": 2300},
                           {"dial_hz": 3_590_000, "mode": "USB", "modem": "aether",
                            "bandwidth_hz": 500}]),
        3_590_000,
    );
    assert!(
        said.iter().any(|l| l == "calling KK4ODA-1 at 500 Hz"),
        "{said:?}"
    );
    assert_eq!(a["channel"]["bandwidth_hz"], 500, "{a}");

    // the loopback offers 500 and 2300 Hz
    let (said, a) = gateway_call_on(
        "attempts-channel-2750",
        &serde_json::json!([{"dial_hz": 14_105_000, "mode": "USB", "modem": "vara",
                            "bandwidth_hz": 2750}]),
        14_105_000,
    );
    assert!(
        said.iter().any(|l| l
            == "KK4ODA-1's channel is 2750 Hz, which this modem cannot be asked for (it offers \
                500 and 2300 Hz): the call goes at the bandwidth the modem runs"),
        "{said:?}"
    );
    assert!(said.iter().any(|l| l == "calling KK4ODA-1"), "{said:?}");
    assert_eq!(a["channel"]["bandwidth_hz"], Value::Null, "{a}");
    assert_eq!(a["call"]["result"], "timeout", "not refused: {a}");

    let (said, a) = gateway_call_on(
        "attempts-channel-fm",
        &serde_json::json!([{"dial_hz": 145_050_000, "mode": "FM", "modem": "aether-fm",
                            "bandwidth_hz": 12_500}]),
        145_050_000,
    );
    assert!(said.iter().any(|l| l == "calling KK4ODA-1"), "{said:?}");
    assert!(
        !said.iter().any(|l| l.contains("cannot be asked")),
        "{said:?}"
    );
    assert_eq!(a["channel"]["bandwidth_hz"], Value::Null, "{a}");
}
