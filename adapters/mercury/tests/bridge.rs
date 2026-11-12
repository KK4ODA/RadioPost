// SPDX-License-Identifier: MIT OR Apache-2.0
//! The bridge against RP-4 §9.6's table: the host's lines and Mercury's, as Mercury's
//! `docs/TNC.md` writes them, given by hand, and `rpmi-lines.json` replayed through it (RP-4 §2).

use std::path::PathBuf;

use radiopost_mercury::bridge::{Bridge, Out};
use serde_json::{Value, json};

/// A bridge and its clock.
struct Rig {
    bridge: Bridge,
    now: u64,
}

fn b64(bytes: &[u8]) -> String {
    rpmi::base64::encode(bytes)
}

/// The host's lines among `outs`, as JSON values without their `text`.
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

/// The commands for Mercury among `outs`.
fn asked(outs: &[Out]) -> Vec<String> {
    outs.iter()
        .filter_map(|o| match o {
            Out::Command(c) => Some(c.clone()),
            _ => None,
        })
        .collect()
}

/// The bytes for Mercury's data port among `outs`.
fn fed(outs: &[Out]) -> Vec<u8> {
    outs.iter()
        .filter_map(|o| match o {
            Out::Data(d) => Some(d.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

impl Rig {
    fn new() -> Rig {
        Rig {
            bridge: Bridge::new(),
            now: 0,
        }
    }

    fn host(&mut self, line: &Value) -> Vec<Out> {
        self.bridge.from_host(&line.to_string())
    }

    fn modem(&mut self, line: &str) -> Vec<Out> {
        self.bridge.from_modem(line)
    }

    /// The clock moved on by `ms`.
    fn later(&mut self, ms: u64) -> Vec<Out> {
        self.now += ms;
        self.bridge.tick(self.now)
    }

    /// `hello` said and answered, Mercury's busy detector as `detector` says.
    fn greeted(detector: &str) -> Rig {
        let mut rig = Rig::new();
        let outs = rig.host(&json!({"op": "hello", "id": 1, "version": [1, 1], "app": "test"}));
        assert_eq!(asked(&outs), ["BUSYDETECT"]);
        let outs = rig.modem(&format!("BUSYDETECT {detector}"));
        assert_eq!(
            asked(&outs),
            ["LISTEN OFF"],
            "Mercury answers nobody at first"
        );
        assert_eq!(told(&outs).len(), 1);
        assert_eq!(told(&rig.modem("OK")), Vec::<Value>::new());
        rig.later(5000); // past the LISTEN OFF's quiet second
        rig
    }

    /// A host with these callsigns.
    fn with(callsigns: &[&str]) -> Rig {
        let mut rig = Rig::greeted("OFF");
        let outs = rig.host(&json!({"op": "callsigns", "id": 2, "callsigns": callsigns}));
        assert_eq!(asked(&outs), [format!("MYCALL {}", callsigns.join(" "))]);
        assert_eq!(told(&rig.modem("OK")), [json!({"op": "ok", "id": 2})]);
        for c in callsigns {
            assert_eq!(rig.modem(&format!("REGISTERED {c}")), Vec::<Out>::new());
        }
        rig
    }

    /// A host that listens.
    fn listening(callsigns: &[&str]) -> Rig {
        let mut rig = Rig::with(callsigns);
        let outs = rig.host(&json!({"op": "listen", "id": 3, "on": true}));
        assert_eq!(asked(&outs), ["LISTEN ON"]);
        assert_eq!(told(&outs), [json!({"op": "ok", "id": 3})]);
        assert_eq!(rig.modem("OK"), Vec::<Out>::new());
        rig
    }

    /// KK4ODA's call to N1CALL-10, answered: the link is `m1`.
    fn connected() -> Rig {
        let mut rig = Rig::with(&["KK4ODA"]);
        let outs = rig.host(&json!({"op": "connect", "id": 10, "remote": "N1CALL-10"}));
        assert_eq!(asked(&outs), ["CONNECT KK4ODA N1CALL-10"]);
        assert_eq!(told(&outs), [json!({"op": "ok", "id": 10})]);
        assert_eq!(told(&rig.modem("OK")), Vec::<Value>::new());
        let outs = rig.modem("CONNECTED KK4ODA N1CALL-10 2300");
        assert_eq!(
            told(&outs),
            [
                json!({"op": "connected", "id": 10, "link": "m1", "remote": "N1CALL-10",
                       "local": "KK4ODA", "direction": "out", "bandwidth": 2300})
            ]
        );
        rig
    }

    /// N1CALL-10, listening, called by KK4ODA: the link is `m1`.
    fn answered() -> Rig {
        let mut rig = Rig::listening(&["N1CALL-10"]);
        assert_eq!(rig.modem("PENDING"), Vec::<Out>::new());
        let outs = rig.modem("CONNECTED KK4ODA N1CALL-10 2300");
        assert_eq!(
            told(&outs),
            [
                json!({"op": "incoming", "remote": "KK4ODA", "local": "N1CALL-10"}),
                json!({"op": "connected", "link": "m1", "remote": "KK4ODA",
                       "local": "N1CALL-10", "direction": "in", "bandwidth": 2300})
            ]
        );
        rig
    }
}

#[test]
fn hello_asks_what_only_mercury_answers_and_gives_its_caps() {
    let mut rig = Rig::new();
    let outs = rig.host(&json!({"op": "hello", "id": "h", "version": [1, 3], "app": "test"}));
    assert_eq!(asked(&outs), ["BUSYDETECT"]);
    assert_eq!(
        told(&outs),
        Vec::<Value>::new(),
        "no answer before Mercury's"
    );
    let outs = rig.modem("BUSYDETECT ON");
    assert_eq!(
        told(&outs),
        [
            json!({"op": "hello", "id": "h", "version": 1, "modem": "Mercury",
               "caps": {"bandwidths": [500, 2300, 2750], "max_links": 1, "delivery": true,
                        "broadcast": false, "quality": true, "busy": true,
                        "regulatory": false, "max_queue": 65536, "shared": false}})
        ]
    );
    let mut rig = Rig::new();
    rig.host(&json!({"op": "hello", "id": "h", "version": [1, 1], "app": "test"}));
    let hello = &told(&rig.modem("BUSYDETECT OFF"))[0];
    assert_eq!(
        hello["caps"]["busy"],
        json!(false),
        "the detector is off: no `busy`"
    );
}

#[test]
fn a_modem_that_does_not_know_busydetect_is_not_mercury() {
    for answer in ["WRONG", "OK"] {
        let mut rig = Rig::new();
        rig.host(&json!({"op": "hello", "id": 1, "version": [1, 1], "app": "test"}));
        let outs = rig.modem(answer);
        assert_eq!(
            told(&outs),
            [json!({"op": "error", "id": 1, "code": "no-modem"})]
        );
        assert_eq!(
            asked(&outs),
            Vec::<String>::new(),
            "nothing more is asked of it"
        );
        let outs = rig.host(&json!({"op": "hello", "id": 2, "version": [1, 1], "app": "test"}));
        assert_eq!(
            told(&outs),
            [json!({"op": "error", "id": 2, "code": "no-modem"})]
        );
    }
}

#[test]
fn no_version_in_common_closes_and_requests_wait_for_hello() {
    let mut rig = Rig::new();
    let outs = rig.host(&json!({"op": "status", "id": 1}));
    assert_eq!(
        told(&outs),
        [json!({"op": "error", "id": 1, "code": "refused"})]
    );
    let outs = rig.host(&json!({"op": "hello", "id": 2, "version": [2, 3], "app": "test"}));
    assert_eq!(
        told(&outs),
        [json!({"op": "error", "id": 2, "code": "unsupported-version"})]
    );
    assert_eq!(outs.last(), Some(&Out::CloseHost));
}

#[test]
fn callsigns_are_mycall_and_what_mercury_cannot_carry_is_refused() {
    let mut rig = Rig::with(&["KK4ODA", "KK4ODA-7"]);
    for bad in [
        json!(["EA5/KK4ODA"]),        // no `/` in a CALL
        json!(["kk4oda"]),            // RP-0 §6.1: uppercase
        json!(["KK4ODA-1234567890"]), // 16 characters
        json!(["A", "B", "C", "D", "E", "F"]),
    ] {
        let outs = rig.host(&json!({"op": "callsigns", "id": 5, "callsigns": bad}));
        assert_eq!(
            told(&outs),
            [json!({"op": "error", "id": 5, "code": "refused"})]
        );
        assert_eq!(asked(&outs), Vec::<String>::new());
    }
    let status = &told(&rig.host(&json!({"op": "status", "id": 6})))[0];
    assert_eq!(
        status["callsigns"],
        json!(["KK4ODA", "KK4ODA-7"]),
        "kept whole"
    );
    // Mercury's refusal is the host's
    let outs = rig.host(&json!({"op": "callsigns", "id": 7, "callsigns": ["W1AW"]}));
    assert_eq!(asked(&outs), ["MYCALL W1AW"]);
    assert_eq!(
        told(&rig.modem("WRONG")),
        [json!({"op": "error", "id": 7, "code": "refused"})]
    );
}

#[test]
fn listen_needs_callsigns_and_is_mercurys_listen() {
    let mut rig = Rig::greeted("OFF");
    let outs = rig.host(&json!({"op": "listen", "id": 1, "on": true}));
    assert_eq!(
        told(&outs),
        [json!({"op": "error", "id": 1, "code": "refused"})]
    );
    let mut rig = Rig::listening(&["N1CALL-10"]);
    let outs = rig.host(&json!({"op": "listen", "id": 4, "on": true}));
    assert_eq!(
        asked(&outs),
        Vec::<String>::new(),
        "Mercury listens already"
    );
    let outs = rig.host(&json!({"op": "listen", "id": 5, "on": false}));
    assert_eq!(asked(&outs), ["LISTEN OFF"]);
    assert_eq!(told(&outs), [json!({"op": "ok", "id": 5})]);
}

#[test]
fn connect_is_refused_as_rp4_says() {
    let mut rig = Rig::greeted("OFF");
    let refused = |rig: &mut Rig, request: Value, code: &str| {
        let id = request["id"].clone();
        let outs = rig.host(&request);
        assert_eq!(
            told(&outs),
            [json!({"op": "error", "id": id, "code": code})],
            "{request}"
        );
        assert_eq!(asked(&outs), Vec::<String>::new(), "{request}");
    };
    refused(
        &mut rig,
        json!({"op": "connect", "id": 1, "remote": "N1CALL-10"}),
        "refused",
    );
    let mut rig = Rig::with(&["KK4ODA"]);
    refused(
        &mut rig,
        json!({"op": "connect", "id": 2, "remote": "N1CALL-10", "as": "W1AW"}),
        "not-authorized",
    );
    refused(
        &mut rig,
        json!({"op": "connect", "id": 3, "remote": "N1CALL-10", "bandwidth": 1200}),
        "unsupported",
    );
    refused(
        &mut rig,
        json!({"op": "connect", "id": 4, "remote": "N1CALL-10-LONG-ONE"}),
        "refused",
    );
    refused(
        &mut rig,
        json!({"op": "connect", "id": 5, "remote": "N1 CALL"}),
        "refused",
    );
    let mut rig = Rig::connected();
    refused(
        &mut rig,
        json!({"op": "connect", "id": 6, "remote": "N1CALL-10"}),
        "busy",
    );
    let mut rig = Rig::listening(&["KK4ODA"]);
    rig.modem("PENDING");
    refused(
        &mut rig,
        json!({"op": "connect", "id": 7, "remote": "N1CALL-10"}),
        "busy",
    );
    assert_eq!(rig.modem("CANCELPENDING"), Vec::<Out>::new());
    let outs = rig.host(&json!({"op": "connect", "id": 8, "remote": "N1CALL-10"}));
    assert_eq!(asked(&outs), ["CONNECT KK4ODA N1CALL-10"]);
}

#[test]
fn a_call_goes_out_under_the_callsign_it_names_at_the_bandwidth_it_names() {
    let mut rig = Rig::with(&["KK4ODA", "KK4ODA-7"]);
    let outs = rig.host(
        &json!({"op": "connect", "id": 1, "remote": "N1CALL-10", "as": "KK4ODA-7",
                       "bandwidth": 500}),
    );
    // Mercury's CALL carries MYCALL's first callsign, whatever CONNECT names
    assert_eq!(asked(&outs), ["MYCALL KK4ODA-7 KK4ODA"]);
    assert_eq!(
        told(&outs),
        [json!({"op": "ok", "id": 1})],
        "the call is under way"
    );
    assert_eq!(asked(&rig.modem("OK")), ["BW500"]);
    let outs = rig.modem("OK");
    assert_eq!(asked(&outs), ["CONNECT KK4ODA-7 N1CALL-10"]);
    assert_eq!(told(&outs), Vec::<Value>::new());
    assert_eq!(told(&rig.modem("OK")), Vec::<Value>::new());
    let outs = rig.modem("CONNECTED KK4ODA-7 N1CALL-10 500");
    assert_eq!(
        told(&outs),
        [
            json!({"op": "connected", "id": 1, "link": "m1", "remote": "N1CALL-10",
                   "local": "KK4ODA-7", "direction": "out", "bandwidth": 500})
        ]
    );
    rig.modem("DISCONNECTED");
    rig.later(2000);
    // the next call, as the first callsign and at the same bandwidth: MYCALL again, no BW
    let outs =
        rig.host(&json!({"op": "connect", "id": 2, "remote": "N1CALL-10", "bandwidth": 500}));
    assert_eq!(asked(&outs), ["MYCALL KK4ODA KK4ODA-7"]);
    assert_eq!(
        asked(&rig.modem("OK")),
        Vec::<String>::new(),
        "the station identifies first"
    );
    rig.modem("CQFRAME N1CALL-10 500");
    assert_eq!(asked(&rig.later(700)), ["CQFRAME KK4ODA-7 500"]);
    rig.modem("OK");
    rig.modem("PENDING");
    assert_eq!(
        asked(&rig.modem("CANCELPENDING")),
        ["CONNECT KK4ODA N1CALL-10"]
    );
}

#[test]
fn a_call_answered_is_the_hosts_for_the_callsign_called() {
    let mut rig = Rig::listening(&["N1CALL-10", "N1CALL-11"]);
    rig.modem("PENDING");
    let outs = rig.modem("CONNECTED KK4ODA N1CALL-11 2300");
    assert_eq!(
        told(&outs),
        [
            json!({"op": "incoming", "remote": "KK4ODA", "local": "N1CALL-11"}),
            json!({"op": "connected", "link": "m1", "remote": "KK4ODA", "local": "N1CALL-11",
                   "direction": "in", "bandwidth": 2300})
        ]
    );
    let status = &told(&rig.host(&json!({"op": "status", "id": 9})))[0];
    assert_eq!(
        status["links"],
        json!([{"link": "m1", "remote": "KK4ODA", "direction": "in", "queued": 0}])
    );
    assert_eq!(status["listening"], json!(true));
}

#[test]
fn a_call_answered_while_the_host_does_not_listen_is_ended_and_not_told() {
    let mut rig = Rig::with(&["KK4ODA"]);
    assert_eq!(rig.modem("PENDING"), Vec::<Out>::new());
    let outs = rig.modem("CONNECTED W1AW KK4ODA 2300");
    assert_eq!(asked(&outs), ["ABORT"]);
    assert_eq!(told(&outs), Vec::<Value>::new());
    assert_eq!(rig.bridge.from_data(b"hello?"), Vec::<Out>::new());
    assert_eq!(rig.modem("DISCONNECTED"), Vec::<Out>::new());
}

#[test]
fn bytes_go_to_the_data_port_and_buffer_says_what_the_other_station_holds() {
    let mut rig = Rig::connected();
    let outs = rig.host(&json!({"op": "send", "link": "m1", "data": b64(&[7; 100]), "ref": "t1"}));
    assert_eq!(fed(&outs), vec![7; 100]);
    assert_eq!(
        told(&outs),
        [json!({"op": "queued", "link": "m1", "bytes": 100})]
    );
    // counted before Mercury had taken the bytes in: not believed
    assert_eq!(rig.modem("BUFFER 0"), Vec::<Out>::new());
    assert_eq!(rig.later(1000), Vec::<Out>::new());
    assert_eq!(
        rig.modem("BUFFER 100"),
        Vec::<Out>::new(),
        "as the host was told already"
    );
    assert_eq!(
        rig.later(5000),
        Vec::<Out>::new(),
        "the last count was not 0: nothing is believed"
    );
    let outs = rig.modem("BUFFER 40");
    assert_eq!(
        told(&outs),
        [json!({"op": "queued", "link": "m1", "bytes": 40})]
    );
    let outs = rig.modem("BUFFER 0");
    assert_eq!(
        told(&outs),
        [
            json!({"op": "delivered", "link": "m1", "ref": "t1"}),
            json!({"op": "queued", "link": "m1", "bytes": 0})
        ]
    );
}

#[test]
fn a_buffer_of_zero_is_believed_once_the_bytes_have_had_time_to_show() {
    let mut rig = Rig::connected();
    rig.host(&json!({"op": "send", "link": "m1", "data": b64(b"ref 7"), "ref": 7}));
    assert_eq!(rig.modem("BUFFER 0"), Vec::<Out>::new());
    assert_eq!(rig.later(2400), Vec::<Out>::new());
    let outs = rig.later(100);
    assert_eq!(
        told(&outs),
        [
            json!({"op": "delivered", "link": "m1", "ref": 7}),
            json!({"op": "queued", "link": "m1", "bytes": 0})
        ]
    );
    // a ref with nothing left to deliver is delivered at once
    let outs = rig.host(&json!({"op": "send", "link": "m1", "data": "", "ref": 8, "id": 1}));
    assert_eq!(
        told(&outs),
        [
            json!({"op": "ok", "id": 1}),
            json!({"op": "delivered", "link": "m1", "ref": 8})
        ]
    );
}

#[test]
fn mercury_is_fed_no_more_than_half_its_queue_and_disconnect_follows_the_last_byte() {
    let mut rig = Rig::connected();
    let outs = rig.host(&json!({"op": "send", "link": "m1", "data": b64(&vec![1; 40000])}));
    assert_eq!(fed(&outs).len(), 32768);
    let outs = rig.host(&json!({"op": "disconnect", "id": 1, "link": "m1"}));
    assert_eq!(told(&outs), [json!({"op": "ok", "id": 1})]);
    assert_eq!(
        asked(&outs),
        Vec::<String>::new(),
        "bytes are still held: what is queued goes first"
    );
    assert_eq!(fed(&rig.modem("BUFFER 32768")), Vec::<u8>::new());
    let outs = rig.modem("BUFFER 20000");
    assert_eq!(fed(&outs).len(), 7232);
    assert_eq!(asked(&outs), ["DISCONNECT"]);
    assert_eq!(
        told(&outs),
        [json!({"op": "queued", "link": "m1", "bytes": 27232})]
    );
    // a second `disconnect` would end Mercury's delivery at once, as ABORT does: it is not sent
    let outs = rig.host(&json!({"op": "disconnect", "id": 2, "link": "m1"}));
    assert_eq!(told(&outs), [json!({"op": "ok", "id": 2})]);
    assert_eq!(asked(&outs), Vec::<String>::new());
    let outs = rig.host(&json!({"op": "send", "id": 3, "link": "m1", "data": "AA=="}));
    assert_eq!(
        told(&outs),
        [json!({"op": "error", "id": 3, "code": "refused"})]
    );
    rig.modem("OK");
    let outs = rig.modem("DISCONNECTED");
    assert_eq!(
        told(&outs),
        [json!({"op": "disconnected", "link": "m1", "reason": "local"})]
    );
    let outs = rig.host(&json!({"op": "disconnect", "id": 4, "link": "m1"}));
    assert_eq!(
        told(&outs),
        [json!({"op": "error", "id": 4, "code": "not-connected"})]
    );
}

#[test]
fn a_session_mercury_ends_unasked_is_the_other_stations() {
    let mut rig = Rig::answered();
    let outs = rig.bridge.from_data(b"over the air");
    assert_eq!(
        told(&outs),
        [json!({"op": "received", "link": "m1", "data": b64(b"over the air")})]
    );
    let outs = rig.modem("DISCONNECTED");
    assert_eq!(
        told(&outs),
        [json!({"op": "disconnected", "link": "m1", "reason": "remote"})]
    );
    assert_eq!(asked(&outs), Vec::<String>::new(), "the host listens on");
}

#[test]
fn listen_off_waits_for_the_link_because_mercurys_would_end_it() {
    let mut rig = Rig::answered();
    let outs = rig.host(&json!({"op": "listen", "id": 1, "on": false}));
    assert_eq!(told(&outs), [json!({"op": "ok", "id": 1})]);
    assert_eq!(
        asked(&outs),
        Vec::<String>::new(),
        "LISTEN OFF would drop the session unsaid"
    );
    let status = &told(&rig.host(&json!({"op": "status", "id": 2})))[0];
    assert_eq!(status["listening"], json!(false));
    let outs = rig.modem("DISCONNECTED");
    assert_eq!(asked(&outs), ["LISTEN OFF"]);
}

#[test]
fn abort_ends_the_link_now_and_listen_off_waits_for_its_disconnect_frame() {
    let mut rig = Rig::answered();
    assert_eq!(rig.modem("PTT ON"), Vec::<Out>::new());
    let outs = rig.host(&json!({"op": "abort", "id": 1, "link": "m1"}));
    assert_eq!(asked(&outs), ["ABORT"]);
    assert_eq!(told(&outs), [json!({"op": "ok", "id": 1})]);
    rig.modem("OK");
    let outs = rig.modem("DISCONNECTED");
    assert_eq!(
        told(&outs),
        [json!({"op": "disconnected", "link": "m1", "reason": "local"})]
    );
    let outs = rig.host(&json!({"op": "listen", "id": 2, "on": false}));
    assert_eq!(told(&outs), [json!({"op": "ok", "id": 2})]);
    assert_eq!(
        asked(&outs),
        Vec::<String>::new(),
        "the DISCONNECT frame is not out yet"
    );
    // the frame that was on the air at the ABORT ends; then the DISCONNECT frame is keyed
    assert_eq!(rig.modem("PTT OFF"), Vec::<Out>::new());
    assert_eq!(rig.modem("PTT ON"), Vec::<Out>::new());
    assert_eq!(asked(&rig.modem("PTT OFF")), ["LISTEN OFF"]);
    assert_eq!(asked(&rig.later(2000)), ["CQFRAME N1CALL-10 2300"]);

    // with no transmission to see, a minute is waited
    let mut rig = Rig::answered();
    rig.host(&json!({"op": "abort", "id": 1, "link": "m1"}));
    rig.modem("OK");
    rig.modem("DISCONNECTED");
    rig.host(&json!({"op": "listen", "id": 2, "on": false}));
    assert_eq!(rig.later(59_000), Vec::<Out>::new());
    assert_eq!(
        asked(&rig.later(1000)),
        ["LISTEN OFF", "CQFRAME N1CALL-10 2300"]
    );

    // a call that comes meanwhile is not answered for a host that no longer listens
    let mut rig = Rig::answered();
    rig.host(&json!({"op": "abort", "id": 1, "link": "m1"}));
    rig.modem("OK");
    rig.modem("DISCONNECTED");
    rig.host(&json!({"op": "listen", "id": 2, "on": false}));
    assert_eq!(asked(&rig.modem("PENDING")), ["LISTEN OFF"]);
}

#[test]
fn a_call_ends_failed_with_no_answer_or_at_the_hosts_abort() {
    let mut rig = Rig::with(&["KK4ODA"]);
    rig.host(&json!({"op": "connect", "id": 1, "remote": "N1CALL-10"}));
    rig.modem("OK");
    let outs = rig.modem("DISCONNECTED");
    assert_eq!(
        told(&outs),
        [json!({"op": "failed", "id": 1, "code": "timeout"})]
    );

    rig.later(2000);
    rig.host(&json!({"op": "connect", "id": 2, "remote": "N1CALL-10"}));
    rig.modem("OK");
    let outs = rig.host(&json!({"op": "abort", "id": 3}));
    assert_eq!(asked(&outs), ["ABORT"]);
    assert_eq!(told(&outs), [json!({"op": "ok", "id": 3})]);
    rig.modem("OK");
    let outs = rig.modem("DISCONNECTED");
    assert_eq!(
        told(&outs),
        [json!({"op": "failed", "id": 2, "code": "aborted"})]
    );
    let outs = rig.host(&json!({"op": "abort", "id": 4}));
    assert_eq!(
        told(&outs),
        [json!({"op": "error", "id": 4, "code": "not-connected"})]
    );

    // answered as the host gave it up: the session Mercury reports is not the host's
    rig.later(2000);
    rig.host(&json!({"op": "connect", "id": 5, "remote": "N1CALL-10"}));
    rig.modem("OK");
    rig.host(&json!({"op": "abort", "id": 6}));
    assert_eq!(
        rig.modem("CONNECTED KK4ODA N1CALL-10 2300"),
        Vec::<Out>::new()
    );
    rig.modem("OK");
    let outs = rig.modem("DISCONNECTED");
    assert_eq!(
        told(&outs),
        [json!({"op": "failed", "id": 5, "code": "aborted"})]
    );

    // Mercury's refusal of the call, after its `ok`, is `failed`
    rig.later(2000);
    rig.host(&json!({"op": "connect", "id": 7, "remote": "N1CALL-10"}));
    let outs = rig.modem("WRONG");
    assert_eq!(
        told(&outs),
        [json!({"op": "failed", "id": 7, "code": "refused"})]
    );
}

#[test]
fn no_connect_follows_an_abort_within_a_second_so_a_late_disconnected_is_not_the_calls() {
    let mut rig = Rig::with(&["KK4ODA"]);
    rig.host(&json!({"op": "connect", "id": 1, "remote": "N1CALL-10"}));
    rig.modem("OK");
    rig.host(&json!({"op": "abort", "id": 2}));
    rig.modem("OK");
    rig.modem("DISCONNECTED");
    let outs = rig.host(&json!({"op": "connect", "id": 3, "remote": "N1CALL-10"}));
    assert_eq!(told(&outs), [json!({"op": "ok", "id": 3})]);
    assert_eq!(
        asked(&outs),
        Vec::<String>::new(),
        "held: a DISCONNECTED may still be on its way"
    );
    // Mercury's answer to a command that found no session: it is nobody's
    assert_eq!(rig.modem("DISCONNECTED"), Vec::<Out>::new());
    assert_eq!(rig.later(900), Vec::<Out>::new());
    let outs = rig.later(100);
    assert_eq!(asked(&outs), ["CONNECT KK4ODA N1CALL-10"]);
    assert_eq!(told(&rig.modem("OK")), Vec::<Value>::new());

    // a call the host gives up before it has gone to Mercury ends at once, with no ABORT
    let mut rig = Rig::connected();
    rig.host(&json!({"op": "abort", "id": 1, "link": "m1"}));
    rig.modem("OK");
    rig.modem("DISCONNECTED");
    let outs = rig.host(&json!({"op": "connect", "id": 2, "remote": "N1CALL-10"}));
    assert_eq!(told(&outs), [json!({"op": "ok", "id": 2})]);
    let outs = rig.host(&json!({"op": "abort", "id": 3}));
    assert_eq!(asked(&outs), Vec::<String>::new());
    assert_eq!(
        told(&outs),
        [
            json!({"op": "ok", "id": 3}),
            json!({"op": "failed", "id": 2, "code": "aborted"})
        ]
    );
    let later = asked(&rig.later(5000));
    assert!(
        !later.iter().any(|c| c.starts_with("CONNECT")),
        "no CONNECT for a call given up: {later:?}"
    );
}

#[test]
fn a_call_answered_first_fails_the_hosts_own() {
    let mut rig = Rig::listening(&["KK4ODA"]);
    rig.host(&json!({"op": "connect", "id": 1, "remote": "N1CALL-10"}));
    rig.modem("OK");
    let outs = rig.modem("CONNECTED W1AW KK4ODA 2300");
    assert_eq!(
        told(&outs),
        [
            json!({"op": "failed", "id": 1, "code": "busy"}),
            json!({"op": "incoming", "remote": "W1AW", "local": "KK4ODA"}),
            json!({"op": "connected", "link": "m1", "remote": "W1AW", "local": "KK4ODA",
                   "direction": "in", "bandwidth": 2300})
        ]
    );
}

#[test]
fn bytes_read_before_their_connected_are_the_links_and_older_ones_are_not() {
    let mut rig = Rig::listening(&["N1CALL-10"]);
    assert_eq!(rig.bridge.from_data(b"HELLO"), Vec::<Out>::new());
    let outs = rig.modem("CONNECTED KK4ODA N1CALL-10 2300");
    assert_eq!(
        told(&outs)[2],
        json!({"op": "received", "link": "m1", "data": b64(b"HELLO")})
    );
    rig.modem("DISCONNECTED");
    // what a session left behind is not the next one's
    assert_eq!(rig.bridge.from_data(b"late"), Vec::<Out>::new());
    rig.later(1500);
    let outs = rig.modem("CONNECTED KK4ODA N1CALL-10 2300");
    assert_eq!(told(&outs).len(), 2, "{outs:?}");
}

#[test]
fn quality_and_busy_are_mercurys_lines() {
    let mut rig = Rig::greeted("ON");
    assert_eq!(
        told(&rig.modem("BUSY ON")),
        [json!({"op": "busy", "busy": true})]
    );
    assert_eq!(rig.modem("BUSY ON"), Vec::<Out>::new());
    assert_eq!(
        told(&rig.modem("BUSY OFF")),
        [json!({"op": "busy", "busy": false})]
    );
    assert_eq!(
        rig.modem("SN 8.2"),
        Vec::<Out>::new(),
        "no link: nothing to say it of"
    );

    let mut rig = Rig::connected();
    assert_eq!(
        rig.modem("BUSY ON"),
        Vec::<Out>::new(),
        "the detector was off at hello"
    );
    assert_eq!(
        told(&rig.modem("SN 8.2")),
        [json!({"op": "quality", "link": "m1", "snr_db": 8.2})]
    );
    assert_eq!(
        told(&rig.modem("BITRATE (4) 87 BPS")),
        [json!({"op": "quality", "link": "m1", "snr_db": 8.2, "rate_bps": 87})]
    );
    assert_eq!(rig.modem("BITRATE (4) 87 BPS"), Vec::<Out>::new());
    for other in [
        "IAMALIVE",
        "PTT ON",
        "PTT OFF",
        "CQFRAME W1AW 2300",
        "MONITOR DATAC16 CQ FROM=W1AW BW=2300 SNR=3.0",
        "",
    ] {
        assert_eq!(rig.modem(other), Vec::<Out>::new(), "{other}");
    }
}

#[test]
fn a_host_that_leaves_has_its_link_aborted_and_mercury_stops_answering() {
    let mut rig = Rig::answered();
    let outs = rig.bridge.host_left();
    assert_eq!(asked(&outs), ["ABORT"], "as `abort` would (RP-4 §1.1)");
    assert!(!rig.bridge.settled());
    rig.modem("OK");
    assert_eq!(
        rig.modem("DISCONNECTED"),
        Vec::<Out>::new(),
        "there is no host to tell"
    );
    // the next host may come while the last one's DISCONNECT frame goes out
    let outs = rig.host(&json!({"op": "hello", "id": 1, "version": [1, 1], "app": "next"}));
    assert_eq!(
        asked(&outs),
        Vec::<String>::new(),
        "Mercury is known already"
    );
    assert_eq!(told(&outs)[0]["op"], json!("hello"));
    let status = &told(&rig.host(&json!({"op": "status", "id": 2})))[0];
    assert_eq!(
        status["callsigns"],
        json!([]),
        "the last host's are not this one's"
    );
    assert_eq!(status["listening"], json!(false));
    assert_eq!(rig.bridge.host_left(), Vec::<Out>::new());
    rig.modem("PTT ON");
    assert_eq!(asked(&rig.modem("PTT OFF")), ["LISTEN OFF"]);
    assert!(!rig.bridge.settled(), "LISTEN OFF is unanswered");
    rig.modem("OK");
    assert!(
        !rig.bridge.settled(),
        "the station identifies, though its host has gone"
    );
    assert_eq!(asked(&rig.later(2000)), ["CQFRAME N1CALL-10 2300"]);
    for line in ["OK", "PENDING", "PTT ON", "PTT OFF"] {
        rig.modem(line);
        assert!(!rig.bridge.settled());
    }
    rig.modem("CANCELPENDING");
    assert!(
        rig.bridge.settled(),
        "the connections to Mercury may be closed"
    );

    // a host with nothing on the air: Mercury is quiet already
    let mut rig = Rig::with(&["KK4ODA"]);
    assert_eq!(rig.bridge.host_left(), Vec::<Out>::new());
    assert!(rig.bridge.settled());

    // a call under way goes with its host
    let mut rig = Rig::with(&["KK4ODA"]);
    rig.host(&json!({"op": "connect", "id": 1, "remote": "N1CALL-10"}));
    rig.modem("OK");
    assert_eq!(asked(&rig.bridge.host_left()), ["ABORT"]);
}

#[test]
fn mercury_going_away_is_told_and_closes_the_host() {
    let mut rig = Rig::connected();
    let outs = rig.bridge.modem_lost("its control port closed");
    assert_eq!(
        told(&outs),
        [json!({"op": "disconnected", "link": "m1", "reason": "failure"})]
    );
    assert_eq!(outs.last(), Some(&Out::CloseHost), "RP-4 §2.1");
    let outs = rig.host(&json!({"op": "status", "id": 1}));
    assert_eq!(
        told(&outs),
        [json!({"op": "error", "id": 1, "code": "no-modem"})]
    );

    let mut rig = Rig::with(&["KK4ODA"]);
    rig.host(&json!({"op": "connect", "id": 1, "remote": "N1CALL-10"}));
    rig.modem("OK");
    let outs = rig.bridge.modem_lost("gone");
    assert_eq!(
        told(&outs),
        [json!({"op": "failed", "id": 1, "code": "refused"})]
    );

    let mut gone = Bridge::without_modem("Mercury could not be reached");
    let outs = gone.from_host(r#"{"op":"hello","id":1,"version":[1,1],"app":"x"}"#);
    assert_eq!(
        told(&outs),
        [json!({"op": "error", "id": 1, "code": "no-modem"})]
    );
}

#[test]
fn what_mercury_does_not_carry_is_unsupported() {
    let mut rig = Rig::with(&["KK4ODA"]);
    let outs = rig.host(&json!({"op": "broadcast", "id": 1, "data": "AA=="}));
    assert_eq!(
        told(&outs),
        [json!({"op": "error", "id": 1, "code": "unsupported"})]
    );
}

/// Mercury's answer to the station's `CQFRAME`, and the frame keyed: `PENDING` is its own, not a
/// call. `CANCELPENDING`, when it is sent, follows.
fn identified(rig: &mut Rig) {
    for answer in ["OK", "PENDING", "PTT ON", "PTT OFF"] {
        let outs = rig.modem(answer);
        assert_eq!(
            told(&outs),
            Vec::<Value>::new(),
            "{answer} told the host {outs:?}"
        );
        assert_eq!(
            asked(&outs),
            Vec::<String>::new(),
            "{answer} asked {outs:?}"
        );
    }
}

#[test]
fn the_called_station_identifies_after_the_session_with_the_callsign_called() {
    let mut rig = Rig::listening(&["N1CALL-10", "N1CALL-11"]);
    rig.modem("PENDING");
    rig.modem("CONNECTED KK4ODA N1CALL-11 500");
    let outs = rig.modem("DISCONNECTED");
    assert_eq!(
        asked(&outs),
        Vec::<String>::new(),
        "the other station's answer may still come"
    );
    assert_eq!(rig.later(1900), Vec::<Out>::new());
    assert_eq!(asked(&rig.later(100)), ["CQFRAME N1CALL-11 500"]);
    identified(&mut rig);
    let outs = rig.modem("CANCELPENDING");
    assert_eq!(outs, Vec::<Out>::new(), "{outs:?}");
    assert_eq!(rig.later(60_000), Vec::<Out>::new(), "once");
    // Mercury answers the next call as before: its PENDING is a call again
    rig.modem("PENDING");
    let outs = rig.host(&json!({"op": "connect", "id": 9, "remote": "W1AW"}));
    assert_eq!(
        told(&outs),
        [json!({"op": "error", "id": 9, "code": "busy"})]
    );
}

#[test]
fn the_caller_identifies_after_the_station_it_called() {
    let mut rig = Rig::connected();
    rig.host(&json!({"op": "disconnect", "id": 1, "link": "m1"}));
    rig.modem("OK");
    rig.modem("DISCONNECTED");
    assert_eq!(
        rig.later(10_000),
        Vec::<Out>::new(),
        "N1CALL-10 identifies first"
    );
    // another station's CQ is not the one waited for
    assert_eq!(rig.modem("CQFRAME W1AW 2300"), Vec::<Out>::new());
    assert_eq!(rig.modem("CQFRAME n1call-10 2300"), Vec::<Out>::new());
    // as Mercury waits before it answers a frame: the frame decodes before its sender unkeys
    assert_eq!(rig.later(699), Vec::<Out>::new());
    assert_eq!(asked(&rig.later(1)), ["CQFRAME KK4ODA 2300"]);
    identified(&mut rig);
    assert_eq!(rig.modem("CANCELPENDING"), Vec::<Out>::new());

    // a called station that never identifies is waited for 35 s
    let mut rig = Rig::connected();
    rig.modem("DISCONNECTED");
    assert_eq!(rig.later(34_900), Vec::<Out>::new());
    assert_eq!(asked(&rig.later(100)), ["CQFRAME KK4ODA 2300"]);
}

#[test]
fn a_call_waits_for_the_stations_identification() {
    let mut rig = Rig::answered();
    rig.modem("DISCONNECTED");
    let outs = rig.host(&json!({"op": "connect", "id": 4, "remote": "W1AW"}));
    assert_eq!(told(&outs), [json!({"op": "ok", "id": 4})]);
    assert_eq!(
        asked(&outs),
        Vec::<String>::new(),
        "the station identifies first"
    );
    let outs = rig.later(2000);
    assert_eq!(asked(&outs), ["CQFRAME N1CALL-10 2300"]);
    identified(&mut rig);
    assert_eq!(
        asked(&rig.modem("CANCELPENDING")),
        ["CONNECT N1CALL-10 W1AW"]
    );
}

#[test]
fn an_answer_mercury_sent_for_a_call_that_never_came_up_is_identified() {
    let mut rig = Rig::listening(&["N1CALL-10"]);
    rig.modem("PENDING");
    rig.modem("PTT ON");
    rig.modem("PTT OFF");
    assert_eq!(rig.modem("CANCELPENDING"), Vec::<Out>::new());
    assert_eq!(asked(&rig.later(2000)), ["CQFRAME N1CALL-10 2300"]);

    // a call given up before Mercury keyed owes nothing
    let mut rig = Rig::listening(&["N1CALL-10"]);
    rig.modem("PENDING");
    rig.modem("CANCELPENDING");
    assert_eq!(rig.later(60_000), Vec::<Out>::new());
}

#[test]
fn a_session_answered_for_nobody_is_identified_with_the_stations_callsign() {
    let mut rig = Rig::with(&["KK4ODA"]);
    rig.modem("PENDING");
    rig.modem("CONNECTED W1AW KK4ODA 500");
    rig.modem("OK");
    rig.modem("DISCONNECTED");
    rig.modem("PTT ON");
    rig.modem("PTT OFF");
    assert_eq!(asked(&rig.later(2000)), ["CQFRAME KK4ODA 500"]);
}

#[test]
fn identifications_go_one_at_a_time_and_a_refused_or_silent_one_is_given_up() {
    let mut rig = Rig::listening(&["N1CALL-10"]);
    rig.modem("PENDING");
    rig.modem("CONNECTED KK4ODA N1CALL-10 2300");
    rig.modem("DISCONNECTED");
    rig.host(&json!({"op": "callsigns", "id": 5, "callsigns": ["N1CALL-11"]}));
    rig.modem("OK");
    rig.modem("PENDING");
    rig.modem("CONNECTED KK4ODA N1CALL-11 2300");
    rig.modem("DISCONNECTED");
    assert_eq!(asked(&rig.later(2000)), ["CQFRAME N1CALL-10 2300"]);
    assert_eq!(
        asked(&rig.later(2000)),
        Vec::<String>::new(),
        "the second waits for the first"
    );
    // Mercury could not build the frame: it says nothing, and a minute is waited
    rig.modem("OK");
    assert_eq!(rig.later(57_000), Vec::<Out>::new());
    assert_eq!(asked(&rig.later(1000)), ["CQFRAME N1CALL-11 2300"]);
    // refused: nothing comes of it
    let outs = rig.modem("WRONG");
    assert_eq!(outs, Vec::<Out>::new(), "{outs:?}");
    assert_eq!(rig.later(60_000), Vec::<Out>::new());
}

/// `vectors/rpmi-lines.json`: each line, after `hello` and the callsign KK4ODA, answered as the
/// vector says.
#[test]
fn rpmi_lines_are_read_or_answered_with_an_error() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vectors/rpmi-lines.json");
    let file: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("the vector")).expect("JSON");
    let mut rig = Rig::with(&["KK4ODA"]);
    let cases = file["lines"].as_array().expect("lines");
    assert!(cases.len() > 30);
    for case in cases {
        let outs = rig.bridge.from_host(case["line"].as_str().expect("a line"));
        let want: Vec<Value> = case["answers"]
            .as_array()
            .expect("answers")
            .iter()
            .map(|a| {
                let mut a = a.as_object().expect("an object").clone();
                a.remove("text");
                Value::Object(a)
            })
            .collect();
        assert_eq!(told(&outs), want, "{}", case["what"]);
        assert_eq!(asked(&outs), Vec::<String>::new(), "{}", case["what"]);
    }
}
