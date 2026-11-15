// SPDX-License-Identifier: MIT OR Apache-2.0
//! The bridge speaking VARA (RP-4 §9.2): what differs from Mercury's table, against the lines of
//! VARA's published TNC commands, given by hand. What is the same is Mercury's crate's to test.

use radiopost_mercury::bridge::{Bridge, Dialect, Out};
use serde_json::{Value, json};

/// The host's lines among `outs`, without their `text`.
fn told(outs: &[Out]) -> Vec<Value> {
    outs.iter()
        .filter_map(|o| match o {
            Out::Host(m) => {
                let mut m = m.clone();
                m.remove("text");
                Some(Value::Object(m))
            }
            _ => None,
        })
        .collect()
}

/// The host's lines among `outs`, whole.
fn told_whole(outs: &[Out]) -> Vec<Value> {
    outs.iter()
        .filter_map(|o| match o {
            Out::Host(m) => Some(Value::Object(m.clone())),
            _ => None,
        })
        .collect()
}

/// The commands for VARA among `outs`.
fn asked(outs: &[Out]) -> Vec<String> {
    outs.iter()
        .filter_map(|o| match o {
            Out::Command(c) => Some(c.clone()),
            _ => None,
        })
        .collect()
}

struct Rig {
    bridge: Bridge,
    now: u64,
}

impl Rig {
    fn host(&mut self, line: &Value) -> Vec<Out> {
        self.bridge.from_host(&line.to_string())
    }

    fn modem(&mut self, line: &str) -> Vec<Out> {
        self.bridge.from_modem(line)
    }

    fn later(&mut self, ms: u64) -> Vec<Out> {
        self.now += ms;
        self.bridge.tick(self.now)
    }

    /// `hello` said, and the modem's answer to `CHAT OFF`: what the host was told.
    fn hello(dialect: Dialect, answer: &str) -> (Rig, Vec<Value>) {
        let mut rig = Rig {
            bridge: Bridge::speaking(dialect),
            now: 0,
        };
        let outs = rig.host(&json!({"op": "hello", "id": "h", "version": [1, 1], "app": "test"}));
        assert_eq!(
            asked(&outs),
            ["CHAT OFF"],
            "a published command, VARA's own"
        );
        assert_eq!(told(&outs), Vec::<Value>::new(), "no answer before VARA's");
        let said = told_whole(&rig.modem(answer));
        (rig, said)
    }

    /// A VARA HF that answered, its host with these callsigns.
    fn with(callsigns: &[&str]) -> Rig {
        let (mut rig, _) = Rig::hello(Dialect::VaraHf, "OK");
        rig.modem("OK"); // LISTEN OFF
        rig.later(5000);
        let outs = rig.host(&json!({"op": "callsigns", "id": 2, "callsigns": callsigns}));
        assert_eq!(asked(&outs), [format!("MYCALL {}", callsigns.join(" "))]);
        assert_eq!(told(&rig.modem("OK")), [json!({"op": "ok", "id": 2})]);
        rig
    }

    /// KK4ODA's call to N1CALL-10, answered: the link is `m1`.
    fn connected() -> Rig {
        let mut rig = Rig::with(&["KK4ODA"]);
        let outs = rig.host(&json!({"op": "connect", "id": 10, "remote": "N1CALL-10"}));
        assert_eq!(asked(&outs), ["CONNECT KK4ODA N1CALL-10"]);
        rig.modem("OK");
        let outs = rig.modem("CONNECTED KK4ODA N1CALL-10 2300");
        assert_eq!(told(&outs)[0]["op"], "connected");
        rig
    }
}

#[test]
fn hello_asks_chat_off_and_says_vara_does_not_report_delivery() {
    let (_, said) = Rig::hello(Dialect::VaraHf, "OK");
    assert_eq!(
        said,
        [
            json!({"op": "hello", "id": "h", "version": 1, "modem": "VARA HF",
               "caps": {"bandwidths": [500, 2300, 2750], "max_links": 1, "delivery": false,
                        "broadcast": false, "quality": true, "busy": true,
                        "regulatory": false, "max_queue": 65536, "shared": false}})
        ]
    );
    let (_, said) = Rig::hello(Dialect::VaraFm, "OK");
    assert_eq!(said[0]["modem"], "VARA FM");
    assert_eq!(
        said[0]["caps"]["bandwidths"],
        json!([]),
        "VARA FM has no bandwidth commands"
    );
}

#[test]
fn a_modem_that_refuses_chat_off_is_not_vara() {
    let (mut rig, said) = Rig::hello(Dialect::VaraHf, "WRONG");
    assert_eq!(
        said,
        [json!({"op": "error", "id": "h", "code": "no-modem",
                "text": "the modem there did not answer CHAT OFF as VARA's published commands do"})]
    );
    let outs = rig.host(&json!({"op": "hello", "id": 2, "version": [1, 1], "app": "test"}));
    assert_eq!(
        told(&outs),
        [json!({"op": "error", "id": 2, "code": "no-modem"})]
    );
}

#[test]
fn callsigns_and_calls_keep_to_varas_published_grammar() {
    let mut rig = Rig::with(&["KK4ODA", "KK4ODA-T"]);
    for bad in [
        "KK4ODA/P",
        "AB",
        "KK4ODABC",
        "KK4ODA-0",
        "KK4ODA-01",
        "KK4ODA-16",
        "KK4ODA-",
        "KK4ODA-X",
        "KK4ODA-1-T",
    ] {
        let outs = rig.host(&json!({"op": "callsigns", "id": 5, "callsigns": [bad]}));
        assert_eq!(
            told(&outs),
            [json!({"op": "error", "id": 5, "code": "refused"})],
            "{bad}"
        );
        assert_eq!(asked(&outs), Vec::<String>::new(), "{bad}");
        let outs = rig.host(&json!({"op": "connect", "id": 6, "remote": bad}));
        assert_eq!(
            told(&outs),
            [json!({"op": "error", "id": 6, "code": "refused"})],
            "{bad}"
        );
    }
    for good in ["N1CALL-15", "W1AW-R", "K1A"] {
        let outs = rig.host(&json!({"op": "callsigns", "id": 7, "callsigns": [good]}));
        assert_eq!(asked(&outs), [format!("MYCALL {good}")], "{good}");
        rig.modem("OK");
    }
}

#[test]
fn a_bandwidth_is_refused_on_vara_fm() {
    let (mut rig, _) = Rig::hello(Dialect::VaraFm, "OK");
    rig.modem("OK");
    rig.later(5000);
    rig.host(&json!({"op": "callsigns", "id": 2, "callsigns": ["KK4ODA"]}));
    rig.modem("OK");
    let outs =
        rig.host(&json!({"op": "connect", "id": 3, "remote": "N1CALL-10", "bandwidth": 2300}));
    assert_eq!(
        told_whole(&outs),
        [json!({"op": "error", "id": 3, "code": "unsupported",
                "text": "VARA FM has no bandwidth to choose"})]
    );
    let outs = rig.host(&json!({"op": "connect", "id": 4, "remote": "N1CALL-10"}));
    assert_eq!(asked(&outs), ["CONNECT KK4ODA N1CALL-10"]);
    // VARA FM's CONNECTED names no bandwidth
    rig.modem("OK");
    let outs = rig.modem("CONNECTED KK4ODA N1CALL-10");
    assert_eq!(
        told(&outs),
        [
            json!({"op": "connected", "id": 4, "link": "m1", "remote": "N1CALL-10",
                "local": "KK4ODA", "direction": "out"})
        ]
    );
}

#[test]
fn buffer_says_what_is_queued_and_nothing_is_said_delivered() {
    let mut rig = Rig::connected();
    let data = rpmi::base64::encode(&[7; 100]);
    rig.host(&json!({"op": "send", "link": "m1", "data": data, "ref": "t1"}));
    rig.later(3000);
    rig.modem("BUFFER 100");
    let outs = rig.modem("BUFFER 0");
    assert_eq!(
        told(&outs),
        [json!({"op": "queued", "link": "m1", "bytes": 0})],
        "VARA's BUFFER counts what is not yet sent"
    );
    assert_eq!(rig.later(60_000), Vec::<Out>::new());
}

#[test]
fn the_adapter_sends_no_identification_after_a_session() {
    let mut rig = Rig::connected();
    rig.host(&json!({"op": "disconnect", "id": 1, "link": "m1"}));
    rig.modem("OK");
    let outs = rig.modem("DISCONNECTED");
    assert_eq!(asked(&outs), Vec::<String>::new());
    assert_eq!(
        asked(&rig.later(120_000)),
        Vec::<String>::new(),
        "VARA identifies as its operator sets it"
    );
    // and the next call goes at once
    let outs = rig.host(&json!({"op": "connect", "id": 2, "remote": "N1CALL-10"}));
    assert_eq!(told(&outs), [json!({"op": "ok", "id": 2})]);
}
