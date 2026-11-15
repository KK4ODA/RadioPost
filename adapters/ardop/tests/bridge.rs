// SPDX-License-Identifier: MIT OR Apache-2.0
//! The bridge against RP-4 §9.3's table: the host's lines and ARDOP's, as ardopcf writes them,
//! given by hand.

use radiopost_ardop::bridge::{Bridge, callsign_fault};
use radiopost_mercury::bridge::Out;
use serde_json::{Value, json};

struct Rig {
    bridge: Bridge,
    now: u64,
}

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

/// The commands for ARDOP among `outs`.
fn asked(outs: &[Out]) -> Vec<String> {
    outs.iter()
        .filter_map(|o| match o {
            Out::Command(c) => Some(c.clone()),
            _ => None,
        })
        .collect()
}

/// The data-port bytes among `outs`.
fn fed(outs: &[Out]) -> Vec<Vec<u8>> {
    outs.iter()
        .filter_map(|o| match o {
            Out::Data(d) => Some(d.clone()),
            _ => None,
        })
        .collect()
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

    /// `hello`, ARDOP's `VERSION` answered, and the two set-up commands answered.
    fn greeted() -> Rig {
        let mut rig = Rig {
            bridge: Bridge::new(),
            now: 0,
        };
        let outs = rig.host(&json!({"op": "hello", "id": 1, "version": [1, 1], "app": "test"}));
        assert_eq!(asked(&outs), ["VERSION"]);
        let outs = rig.modem("VERSION ardopcf_1.0.4.1.3");
        assert_eq!(asked(&outs), ["PROTOCOLMODE ARQ", "LISTEN FALSE"]);
        assert_eq!(told(&outs)[0]["op"], "hello");
        rig.modem("PROTOCOLMODE now ARQ");
        rig.modem("LISTEN now FALSE");
        rig
    }

    fn with(callsigns: &[&str]) -> Rig {
        let mut rig = Rig::greeted();
        let outs = rig.host(&json!({"op": "callsigns", "id": 2, "callsigns": callsigns}));
        let mut want = vec![format!("MYCALL {}", callsigns[0])];
        if callsigns.len() > 1 {
            want.push(format!("MYAUX {}", callsigns[1..].join(",")));
        }
        assert_eq!(asked(&outs), want);
        let mut said = told(&rig.modem(&format!("MYCALL now {}", callsigns[0])));
        if callsigns.len() > 1 {
            said = told(&rig.modem(&format!("MYAUX now {}", callsigns[1..].join(","))));
        }
        assert_eq!(said, [json!({"op": "ok", "id": 2})]);
        rig
    }

    /// KK4ODA's call to N1CALL-10, answered: the link is `a1`.
    fn connected() -> Rig {
        let mut rig = Rig::with(&["KK4ODA"]);
        let outs =
            rig.host(&json!({"op": "connect", "id": 10, "remote": "N1CALL-10", "bandwidth": 500}));
        assert_eq!(told(&outs), [json!({"op": "ok", "id": 10})]);
        assert_eq!(asked(&outs), ["ARQBW 500MAX"]);
        let outs = rig.modem("ARQBW now 500MAX");
        assert_eq!(asked(&outs), ["ARQCALL N1CALL-10 5"]);
        rig.modem("ARQCALL N1CALL-10 5");
        let outs = rig.modem("CONNECTED N1CALL-10 500");
        assert_eq!(
            told(&outs)[0],
            json!({"op": "connected", "id": 10, "link": "a1", "remote": "N1CALL-10",
                   "local": "KK4ODA", "direction": "out", "bandwidth": 500})
        );
        rig
    }
}

#[test]
fn hello_asks_version_and_sets_arq_mode_with_no_answering() {
    let mut rig = Rig {
        bridge: Bridge::new(),
        now: 0,
    };
    rig.host(&json!({"op": "hello", "id": "h", "version": [1, 1], "app": "test"}));
    let outs = rig.modem("VERSION ardopcf_1.0.4.1.3");
    assert_eq!(
        told(&outs)[0],
        json!({"op": "hello", "id": "h", "version": 1, "modem": "ARDOP",
               "caps": {"bandwidths": [200, 500, 1000, 2000], "max_links": 1, "delivery": false,
                        "broadcast": false, "quality": false, "busy": true,
                        "regulatory": false, "max_queue": 65536, "shared": false}})
    );
    // a modem that refuses VERSION is not ARDOP
    let mut rig = Rig {
        bridge: Bridge::new(),
        now: 0,
    };
    rig.host(&json!({"op": "hello", "id": 1, "version": [1, 1], "app": "test"}));
    assert_eq!(
        told(&rig.modem("FAULT unknown command")),
        [json!({"op": "error", "id": 1, "code": "no-modem"})]
    );
}

#[test]
fn callsigns_are_mycall_and_myaux_and_a_call_goes_out_as_mycall() {
    for good in ["KK4ODA", "KK4ODA-1", "N1CALL-15", "W1AW-T"] {
        assert_eq!(callsign_fault(good), None, "{good}");
    }
    for bad in [
        "KK4ODA/P",
        "AB",
        "KK4ODABC",
        "KK4ODA-16",
        "KK4ODA-0",
        "KK4ODA-",
    ] {
        assert!(callsign_fault(bad).is_some(), "{bad}");
    }
    let mut rig = Rig::with(&["KK4ODA", "WD5EMA"]);
    // calling as the second: it becomes MYCALL first
    let outs = rig.host(&json!({"op": "connect", "id": 3, "remote": "N1CALL-10", "as": "WD5EMA"}));
    assert_eq!(asked(&outs), ["MYCALL WD5EMA", "MYAUX KK4ODA"]);
    rig.modem("MYCALL now WD5EMA");
    let outs = rig.modem("MYAUX now KK4ODA");
    assert_eq!(asked(&outs), ["ARQCALL N1CALL-10 5"]);
    // one callsign after two: MYAUX cleared, which ARDOP answers with FAULT
    let mut rig = Rig::with(&["KK4ODA", "WD5EMA"]);
    let outs = rig.host(&json!({"op": "callsigns", "id": 4, "callsigns": ["KK4ODA"]}));
    assert_eq!(asked(&outs), ["MYCALL KK4ODA", "MYAUX -"]);
    rig.modem("MYCALL now KK4ODA");
    assert_eq!(
        told(&rig.modem("FAULT Syntax Err: at callsign 0")),
        [json!({"op": "ok", "id": 4})]
    );
}

#[test]
fn a_call_answered_is_the_hosts_for_the_target() {
    let mut rig = Rig::with(&["N1CALL-10", "N1CALL-11"]);
    let outs = rig.host(&json!({"op": "listen", "id": 5, "on": true}));
    assert_eq!(asked(&outs), ["LISTEN TRUE"]);
    assert_eq!(
        told(&rig.modem("LISTEN now TRUE")),
        [json!({"op": "ok", "id": 5})]
    );
    rig.modem("PENDING");
    rig.modem("TARGET N1CALL-11");
    let outs = rig.modem("CONNECTED KK4ODA 2000");
    assert_eq!(
        told(&outs),
        [
            json!({"op": "incoming", "remote": "KK4ODA", "local": "N1CALL-11"}),
            json!({"op": "connected", "link": "a1", "remote": "KK4ODA", "local": "N1CALL-11",
                   "direction": "in", "bandwidth": 2000})
        ]
    );
}

#[test]
fn bytes_go_in_blocks_and_disconnect_waits_for_buffer_zero() {
    let mut rig = Rig::connected();
    let data = vec![9u8; 1500];
    let outs = rig.host(&json!({"op": "send", "id": 1, "link": "a1",
                                "data": rpmi::base64::encode(&data)}));
    let blocks = fed(&outs);
    assert_eq!(blocks.len(), 2);
    assert_eq!(&blocks[0][..2], &1024u16.to_be_bytes());
    assert_eq!(&blocks[1][..2], &476u16.to_be_bytes());
    assert!(told(&outs).contains(&json!({"op": "queued", "link": "a1", "bytes": 1500})));
    let outs = rig.host(&json!({"op": "disconnect", "id": 2, "link": "a1"}));
    assert_eq!(told(&outs), [json!({"op": "ok", "id": 2})]);
    assert_eq!(asked(&outs), Vec::<String>::new());
    rig.modem("BUFFER 1500");
    let outs = rig.later(1000);
    assert_eq!(asked(&outs), ["BUFFER"], "asked while the TNC holds bytes");
    let outs = rig.modem("BUFFER 0");
    assert_eq!(asked(&outs), ["DISCONNECT"]);
    rig.modem("DISCONNECT NOW TRUE");
    let outs = rig.modem("DISCONNECTED");
    assert_eq!(
        told(&outs),
        [json!({"op": "disconnected", "link": "a1", "reason": "local"})]
    );
    // the other way, ARQ-tagged blocks are the link's
    let mut rig = Rig::connected();
    let mut block = 5u16.to_be_bytes().to_vec();
    block.extend_from_slice(b"ARQhi");
    let mut id = 6u16.to_be_bytes().to_vec();
    id.extend_from_slice(b"IDFxyz");
    block.extend_from_slice(&id);
    let outs = rig.bridge.from_data(&block);
    assert_eq!(
        told(&outs),
        [json!({"op": "received", "link": "a1", "data": rpmi::base64::encode(b"hi")})]
    );
}

#[test]
fn a_call_fails_as_ardop_says_and_abort_ends_at_once() {
    let mut rig = Rig::with(&["KK4ODA"]);
    rig.host(&json!({"op": "connect", "id": 1, "remote": "N1CALL-10"}));
    rig.modem("ARQCALL N1CALL-10 5");
    assert_eq!(
        told(&rig.modem("STATUS CONNECT TO N1CALL-10 FAILED!")),
        [json!({"op": "failed", "id": 1, "code": "timeout"})]
    );
    rig.host(&json!({"op": "connect", "id": 2, "remote": "N1CALL-10"}));
    rig.modem("ARQCALL N1CALL-10 5");
    assert_eq!(
        told(&rig.modem("REJECTEDBUSY N1CALL-10")),
        [json!({"op": "failed", "id": 2, "code": "busy"})]
    );
    // ARDOP says nothing more after ABORT: the host is told at once
    let mut rig = Rig::connected();
    let outs = rig.host(&json!({"op": "abort", "id": 3, "link": "a1"}));
    assert_eq!(asked(&outs), ["ABORT"]);
    assert_eq!(
        told(&outs),
        [
            json!({"op": "ok", "id": 3}),
            json!({"op": "disconnected", "link": "a1", "reason": "local"})
        ]
    );
    // and no call for a second, so a late DISCONNECTED is not the next call's
    rig.modem("ABORT");
    let outs = rig.host(&json!({"op": "connect", "id": 4, "remote": "N1CALL-10"}));
    assert_eq!(asked(&outs), Vec::<String>::new());
    assert_eq!(asked(&rig.later(1000)), ["ARQCALL N1CALL-10 5"]);
}
