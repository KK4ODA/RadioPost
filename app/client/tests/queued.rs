// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-7 §3.6, the sender's side, against `vectors/queued.json`: when a message queued at a cut-off
//! gateway is offered again, and what the client shows of it; and the client's store following a
//! message from written, to queued, to sent.

use radiopost::cbor::Value as Cbor;
use radiopost::message::{Message, key};
use radiopost_client::store::{self, Store};
use serde_json::Value;

const QUEUED: &str = include_str!("../../../vectors/queued.json");

#[test]
fn offers_again_as_the_vectors_say() {
    let v: Value = serde_json::from_str(QUEUED).expect("JSON");
    let cases = v["offer_cases"].as_array().expect("offer_cases");
    assert_eq!(cases.len(), 7, "the vectors' seven offer cases");
    for c in cases {
        let cut_off = |g: &str| c["cut_off_when_last_called"][g].as_bool();
        let got = store::offer_again(
            c["queued_at"].as_u64().expect("queued_at"),
            c["calling"].as_str().expect("calling"),
            &cut_off,
            c["now"].as_u64().expect("now"),
        );
        assert_eq!(got, c["expect"].as_bool().expect("expect"), "{}", c["what"]);
    }
}

#[test]
fn shows_as_the_vectors_say() {
    let v: Value = serde_json::from_str(QUEUED).expect("JSON");
    for c in v["view_cases"].as_array().expect("view_cases") {
        let got = store::view(
            c["queued_at"].as_u64(),
            c["confirmed"].as_bool().expect("confirmed"),
            c["now"].as_u64().expect("now"),
        );
        assert_eq!(got, c["expect"].as_str().expect("expect"), "{}", c["what"]);
    }
    assert_eq!(store::view(None, false, 0), "written");
}

#[test]
fn a_message_goes_from_written_to_queued_to_sent() {
    let home = std::env::temp_dir().join(format!("radiopost-queued-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let mut s = Store::open(&home).expect("a home");
    let m = Message::from_map(&Cbor::int_map(vec![
        (key::V, Cbor::Uint(1)),
        (key::FROM, Cbor::text("shelter3@radiopost.org")),
        (
            key::TO,
            Cbor::Array(vec![Cbor::text("facundo@radiopost.org")]),
        ),
        (key::DATE, Cbor::Uint(1_794_494_000)),
        (key::SUBJECT, Cbor::text("Water")),
        (key::BODY, Cbor::text("We need water.\n")),
    ]))
    .expect("a message");
    let id = s.queue(&m).expect("queued");
    let t = 1_794_494_100;
    assert_eq!(s.state_of(&id, t), "written");
    // N2CALL-5, cut off, takes it
    s.set_gateway_online("N2CALL-5", false);
    s.queued_at(&id, "N2CALL-5", t);
    s.save().expect("saved");
    let mut s = Store::open(&home).expect("opened again");
    assert_eq!(s.state_of(&id, t + 3600), "queued");
    assert_eq!(s.outbox.len(), 1, "it stays in the outbox");
    assert!(
        s.outbox_for("N2CALL-5", t + 3600).is_empty(),
        "not offered to N2CALL-5 again"
    );
    assert_eq!(
        s.outbox_for("N1CALL-10", t + 3600).len(),
        1,
        "offered elsewhere"
    );
    assert_eq!(
        s.outbox_for("N2CALL-5", t + 30 * 86_400).len(),
        1,
        "after 30 days, again"
    );
    assert_eq!(s.doubtful(t + 30 * 86_400).len(), 1);
    // N1CALL-10, online, confirms it
    s.delivered(&id);
    assert_eq!(s.state_of(&id, t + 7200), "sent");
    assert!(s.queued.is_empty() && s.outbox.is_empty());
    let _ = std::fs::remove_dir_all(home);
}
