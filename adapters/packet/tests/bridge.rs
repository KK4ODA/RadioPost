// SPDX-License-Identifier: MIT OR Apache-2.0
//! The bridge against RP-4 §9.4's table: the host's lines and the TNC's AGWPE frames, as SV2AGW's
//! *AGWPE TCP/IP API* and Dire Wolf write them, given by hand.

use radiopost_packet::bridge::{Bridge, Out, PACLEN, WINDOW, callsign_fault};
use radiopost_packet::frame::Frame;
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

/// The frames for the TNC among `outs`: kind, from, to.
fn asked(outs: &[Out]) -> Vec<(char, String, String)> {
    outs.iter()
        .filter_map(|o| match o {
            Out::Tnc(f) => Some((f.kind as char, f.from.clone(), f.to.clone())),
            _ => None,
        })
        .collect()
}

fn from(kind: u8, from: &str, to: &str, text: &str) -> Frame {
    let mut data = text.as_bytes().to_vec();
    if !text.is_empty() {
        data.extend_from_slice(b"\r\0");
    }
    Frame::new(0, kind).calls(from, to).data(&data)
}

fn y(local: &str, remote: &str, n: u32) -> Frame {
    Frame::new(0, b'Y')
        .calls(local, remote)
        .data(&n.to_le_bytes())
}

impl Rig {
    fn host(&mut self, line: &Value) -> Vec<Out> {
        self.bridge.from_host(&line.to_string())
    }

    fn tnc(&mut self, f: &Frame) -> Vec<Out> {
        self.bridge.from_tnc(f)
    }

    fn later(&mut self, ms: u64) -> Vec<Out> {
        self.now += ms;
        self.bridge.tick(self.now)
    }

    /// `hello` answered by a TNC with `ports` ports: what the host was told.
    fn hello(port: u8, ports: &str) -> (Rig, Vec<Value>) {
        let mut rig = Rig {
            bridge: Bridge::new(port),
            now: 0,
        };
        let outs = rig.host(&json!({"op": "hello", "id": "h", "version": [1, 1], "app": "test"}));
        assert_eq!(asked(&outs), [('R', String::new(), String::new())]);
        let mut version = 2005u32.to_le_bytes().to_vec();
        version.extend_from_slice(&127u32.to_le_bytes());
        let outs = rig.tnc(&Frame::new(0, b'R').data(&version));
        assert_eq!(asked(&outs), [('G', String::new(), String::new())]);
        let said = told(&rig.tnc(&Frame::new(0, b'G').data(ports.as_bytes())));
        (rig, said)
    }

    /// A host with these callsigns, not listening.
    fn with(callsigns: &[&str]) -> Rig {
        let (mut rig, _) = Rig::hello(0, "1;Port1 9600 baud;\0");
        let outs = rig.host(&json!({"op": "callsigns", "id": 2, "callsigns": callsigns}));
        assert_eq!(told(&outs), [json!({"op": "ok", "id": 2})]);
        assert_eq!(
            asked(&outs),
            [],
            "nothing registered while it does not listen"
        );
        rig
    }

    /// KK4ODA's call to N1CALL-10, answered: the link is `p1`.
    fn connected() -> Rig {
        let mut rig = Rig::with(&["KK4ODA"]);
        let outs = rig.host(&json!({"op": "connect", "id": 10, "remote": "N1CALL-10"}));
        assert_eq!(told(&outs), [json!({"op": "ok", "id": 10})]);
        assert_eq!(
            asked(&outs),
            [
                ('X', "KK4ODA".into(), String::new()),
                ('C', "KK4ODA".into(), "N1CALL-10".into())
            ],
            "the call's callsign registered for it"
        );
        rig.tnc(&Frame::new(0, b'X').calls("KK4ODA", "").data(&[1]));
        let outs = rig.tnc(&from(
            b'C',
            "N1CALL-10",
            "KK4ODA",
            "*** CONNECTED With Station N1CALL-10",
        ));
        assert_eq!(
            told(&outs),
            [
                json!({"op": "connected", "id": 10, "link": "p1", "remote": "N1CALL-10",
                    "local": "KK4ODA", "direction": "out"})
            ]
        );
        rig
    }
}

#[test]
fn hello_asks_the_version_and_the_ports() {
    let (_, said) = Rig::hello(0, "2;Port1 1200 baud;Port2 9600 baud;\0");
    assert_eq!(
        said,
        [
            json!({"op": "hello", "id": "h", "version": 1, "modem": "AX.25 packet",
               "caps": {"bandwidths": [], "max_links": 1, "delivery": false,
                        "broadcast": false, "quality": false, "busy": false,
                        "regulatory": false, "max_queue": 65536, "shared": false}})
        ]
    );
    let (_, said) = Rig::hello(2, "2;Port1 1200 baud;Port2 9600 baud;\0");
    assert_eq!(
        said,
        [json!({"op": "error", "id": "h", "code": "no-modem"})]
    );
    // a TNC that never answers is not there
    let mut rig = Rig {
        bridge: Bridge::new(0),
        now: 0,
    };
    rig.host(&json!({"op": "hello", "id": 1, "version": [1, 1], "app": "test"}));
    assert_eq!(told(&rig.later(4999)), Vec::<Value>::new());
    assert_eq!(
        told(&rig.later(1)),
        [json!({"op": "error", "id": 1, "code": "no-modem"})]
    );
}

#[test]
fn callsigns_are_ax25_addresses() {
    for good in ["KK4ODA", "KK4ODA-1", "N1CALL-15", "K1A"] {
        assert_eq!(callsign_fault(good), None, "{good}");
    }
    for bad in [
        "KK4ODA/P",
        "KK4ODAX",
        "KK4ODA-16",
        "KK4ODA-0",
        "KK4ODA-",
        "kk4oda",
        "KK4ODA-T",
    ] {
        assert!(callsign_fault(bad).is_some(), "{bad}");
    }
    let mut rig = Rig::with(&["KK4ODA"]);
    let outs = rig.host(&json!({"op": "callsigns", "id": 3, "callsigns": ["VE3/KK4ODA"]}));
    assert_eq!(
        told(&outs),
        [json!({"op": "error", "id": 3, "code": "refused"})]
    );
}

#[test]
fn listening_registers_the_callsigns_and_not_listening_lets_them_go() {
    let mut rig = Rig::with(&["N1CALL-10", "N1CALL-11"]);
    let outs = rig.host(&json!({"op": "listen", "id": 4, "on": true}));
    assert_eq!(
        asked(&outs),
        [
            ('X', "N1CALL-10".into(), String::new()),
            ('X', "N1CALL-11".into(), String::new())
        ]
    );
    assert_eq!(told(&outs), Vec::<Value>::new(), "the ok waits for the TNC");
    assert_eq!(
        told(&rig.tnc(&Frame::new(0, b'X').calls("N1CALL-10", "").data(&[1]))),
        Vec::<Value>::new()
    );
    assert_eq!(
        told(&rig.tnc(&Frame::new(0, b'X').calls("N1CALL-11", "").data(&[1]))),
        [json!({"op": "ok", "id": 4})]
    );
    let outs = rig.host(&json!({"op": "listen", "id": 5, "on": false}));
    assert_eq!(told(&outs), [json!({"op": "ok", "id": 5})]);
    assert_eq!(
        asked(&outs),
        [
            ('x', "N1CALL-10".into(), String::new()),
            ('x', "N1CALL-11".into(), String::new())
        ]
    );
    // a TNC that will not register a callsign: the host is told
    let mut rig = Rig::with(&["N1CALL-10"]);
    rig.host(&json!({"op": "listen", "id": 6, "on": true}));
    let outs = rig.tnc(&Frame::new(0, b'X').calls("N1CALL-10", "").data(&[0]));
    assert_eq!(
        told(&outs),
        [json!({"op": "error", "id": 6, "code": "refused"})]
    );
}

#[test]
fn a_call_answered_is_the_hosts_and_a_second_is_ended() {
    let mut rig = Rig::with(&["N1CALL-10"]);
    rig.host(&json!({"op": "listen", "id": 4, "on": true}));
    rig.tnc(&Frame::new(0, b'X').calls("N1CALL-10", "").data(&[1]));
    let outs = rig.tnc(&from(
        b'C',
        "KK4ODA",
        "N1CALL-10",
        "*** CONNECTED To Station KK4ODA",
    ));
    assert_eq!(
        told(&outs),
        [
            json!({"op": "incoming", "remote": "KK4ODA", "local": "N1CALL-10"}),
            json!({"op": "connected", "link": "p1", "remote": "KK4ODA", "local": "N1CALL-10",
                   "direction": "in"})
        ]
    );
    // another station's connection while the link is up: the TNC took it, the bridge ends it
    let outs = rig.tnc(&from(
        b'C',
        "W1AW",
        "N1CALL-10",
        "*** CONNECTED To Station W1AW",
    ));
    assert_eq!(told(&outs), Vec::<Value>::new());
    assert_eq!(asked(&outs), [('d', "N1CALL-10".into(), "W1AW".into())]);
    assert_eq!(
        told(&rig.tnc(&from(
            b'd',
            "W1AW",
            "N1CALL-10",
            "*** DISCONNECTED From Station W1AW"
        ))),
        Vec::<Value>::new()
    );
    // the bytes that come are the host's
    let outs = rig.tnc(&Frame::new(0, b'D').calls("KK4ODA", "N1CALL-10").data(b"hi"));
    assert_eq!(
        told(&outs),
        [json!({"op": "received", "link": "p1", "data": rpmi::base64::encode(b"hi")})]
    );
}

#[test]
fn bytes_go_a_paclen_at_a_time_a_window_ahead_and_disconnect_waits_for_them() {
    let mut rig = Rig::connected();
    let data = vec![7u8; PACLEN * (WINDOW + 2) + 10];
    let outs = rig.host(&json!({"op": "send", "id": 1, "link": "p1",
                                "data": rpmi::base64::encode(&data)}));
    let frames: Vec<&Frame> = outs
        .iter()
        .filter_map(|o| match o {
            Out::Tnc(f) if f.kind == b'D' => Some(f),
            _ => None,
        })
        .collect();
    assert_eq!(frames.len(), WINDOW);
    assert!(
        frames
            .iter()
            .all(|f| f.data.len() == PACLEN && f.pid == 0xF0)
    );
    assert_eq!(
        asked(&outs).last(),
        Some(&('Y', "KK4ODA".into(), "N1CALL-10".into()))
    );
    assert!(told(&outs).contains(&json!({"op": "queued", "link": "p1", "bytes": data.len()})));
    // the host asks to end in order: nothing is cut while the TNC holds frames
    let outs = rig.host(&json!({"op": "disconnect", "id": 2, "link": "p1"}));
    assert_eq!(told(&outs), [json!({"op": "ok", "id": 2})]);
    assert!(!asked(&outs).iter().any(|a| a.0 == 'd'));
    // five acknowledged: five more go
    let outs = rig.tnc(&y("KK4ODA", "N1CALL-10", 2));
    let more = asked(&outs).iter().filter(|a| a.0 == 'D').count();
    assert_eq!(more, 3, "the rest of the data");
    assert!(
        !asked(&outs).iter().any(|a| a.0 == 'd' || a.0 == 'Y'),
        "asked again later"
    );
    let outs = rig.later(500);
    assert_eq!(asked(&outs), [('Y', "KK4ODA".into(), "N1CALL-10".into())]);
    let outs = rig.tnc(&y("KK4ODA", "N1CALL-10", 0));
    assert_eq!(
        asked(&outs),
        [('d', "KK4ODA".into(), "N1CALL-10".into())],
        "all acknowledged"
    );
    let outs = rig.tnc(&from(
        b'd',
        "N1CALL-10",
        "KK4ODA",
        "*** DISCONNECTED From Station N1CALL-10",
    ));
    assert_eq!(
        told(&outs),
        [json!({"op": "disconnected", "link": "p1", "reason": "local"})]
    );
    assert_eq!(
        asked(&outs),
        [('x', "KK4ODA".into(), String::new())],
        "the call's callsign let go"
    );
}

#[test]
fn a_call_fails_as_the_tnc_says() {
    let mut rig = Rig::with(&["KK4ODA"]);
    rig.host(&json!({"op": "connect", "id": 1, "remote": "N1CALL-10"}));
    let outs = rig.tnc(&from(
        b'd',
        "N1CALL-10",
        "KK4ODA",
        "*** DISCONNECTED RETRYOUT With N1CALL-10",
    ));
    assert_eq!(
        told(&outs),
        [json!({"op": "failed", "id": 1, "code": "timeout"})]
    );
    rig.host(&json!({"op": "connect", "id": 2, "remote": "N1CALL-10"}));
    let outs = rig.tnc(&from(
        b'd',
        "N1CALL-10",
        "KK4ODA",
        "*** DISCONNECTED From Station N1CALL-10",
    ));
    assert_eq!(
        told(&outs),
        [json!({"op": "failed", "id": 2, "code": "refused"})]
    );
    // given up: the late answer is ended, not taken
    rig.host(&json!({"op": "connect", "id": 3, "remote": "N1CALL-10"}));
    let outs = rig.host(&json!({"op": "abort", "id": 4}));
    assert_eq!(
        told(&outs),
        [
            json!({"op": "ok", "id": 4}),
            json!({"op": "failed", "id": 3, "code": "aborted"})
        ]
    );
    let outs = rig.tnc(&from(
        b'C',
        "N1CALL-10",
        "KK4ODA",
        "*** CONNECTED With Station N1CALL-10",
    ));
    assert_eq!(told(&outs), Vec::<Value>::new());
    assert_eq!(asked(&outs), [('d', "KK4ODA".into(), "N1CALL-10".into())]);
    // a bandwidth is the TNC's
    let outs =
        rig.host(&json!({"op": "connect", "id": 5, "remote": "N1CALL-10", "bandwidth": 1200}));
    assert_eq!(
        told(&outs),
        [json!({"op": "error", "id": 5, "code": "unsupported"})]
    );
}

#[test]
fn a_host_that_leaves_ends_its_link_and_lets_its_callsigns_go() {
    let mut rig = Rig::connected();
    let outs = rig.bridge.host_left();
    assert_eq!(
        asked(&outs),
        [
            ('d', "KK4ODA".into(), "N1CALL-10".into()),
            ('x', "KK4ODA".into(), String::new())
        ]
    );
}
