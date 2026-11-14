// SPDX-License-Identifier: MIT OR Apache-2.0
//! The bridge against RP-4 §9.1's table, with a stand-in for Aether's control API that answers
//! as its specification says, and `rpmi-lines.json` replayed through it (RP-4 §2).

use std::path::PathBuf;

use radiopost_aether::bridge::{Bridge, Out};
use rpmi::wire::Message;
use serde_json::{Value, json};

/// Aether as its control API specification has it, for the methods the adapter uses.
struct Aether {
    claims: bool,
    /// It knows `hello` (0.2.0-beta.159, its ADR-0114); an earlier one answers `unknown_method`.
    hello: bool,
    refuse: Vec<&'static str>,
    connect_error: Option<&'static str>,
    asked: Vec<Value>,
}

impl Aether {
    fn new(claims: bool) -> Aether {
        Aether {
            claims,
            hello: true,
            refuse: Vec::new(),
            connect_error: None,
            asked: Vec::new(),
        }
    }

    fn answer(&mut self, request: &Value) -> Value {
        self.asked.push(request.clone());
        let id = request["id"].clone();
        let params = &request["params"];
        let ok = |result: Value| json!({"id": id, "ok": true, "result": result});
        let refused = |code: &str| {
            json!({"id": id, "ok": false,
                   "error": {"code": code, "message": format!("Aether says {code}"),
                             "retryable": false}})
        };
        match request["method"].as_str().unwrap_or("") {
            "status" => {
                let mut status = json!({"version": "0.2.0-beta.146", "state": "idle",
                                        "owner": null, "channel_busy": false});
                if self.claims {
                    status["claims"] = json!([]);
                }
                ok(status)
            }
            "capabilities" => ok(json!({"bandwidth_hz": 2300, "bandwidths_hz": [2300, 500]})),
            "claim" => {
                let wanted: Vec<&str> = params["callsigns"]
                    .as_array()
                    .expect("callsigns")
                    .iter()
                    .filter_map(Value::as_str)
                    .collect();
                let refused: Vec<Value> = wanted
                    .iter()
                    .filter(|c| self.refuse.contains(c))
                    .map(|c| json!({"callsign": c, "holder": "Trimode"}))
                    .collect();
                let taken: Vec<&str> = wanted
                    .iter()
                    .filter(|c| !self.refuse.contains(c))
                    .copied()
                    .collect();
                ok(json!({"id": 7, "callsigns": taken, "refused": refused, "listening": false}))
            }
            "callsigns.set" => ok(json!({"callsigns": params["callsigns"], "applied": true})),
            "listen" => ok(json!({})),
            "connect" => match self.connect_error {
                Some(code) => refused(code),
                None => ok(json!({"session": 42})),
            },
            "send" => ok(json!({"accepted": 1})),
            "disconnect" => ok(json!({"orderly": true})),
            "abort" => ok(json!({"orderly": false})),
            "datagram.send" => ok(json!({"accepted": 1, "queued": 1, "limit": 16})),
            "share.include" => ok(json!({"app": params["app"], "path": params["path"], "logs": 1})),
            "hello" if self.hello => ok(json!({"id": 7, "app": params["app"]})),
            "hello" => refused("unknown_method"),
            other => panic!("the adapter asked Aether for {other}"),
        }
    }
}

/// Runs what the bridge said: Aether's requests answered at once, the host's lines collected.
fn settle(bridge: &mut Bridge, aether: &mut Aether, mut outs: Vec<Out>) -> Vec<Out> {
    let mut said = Vec::new();
    while !outs.is_empty() {
        let mut next = Vec::new();
        for out in outs {
            match out {
                Out::Aether(request) => {
                    let answer = aether.answer(&request);
                    next.extend(bridge.from_aether(&answer.to_string()));
                }
                other => said.push(other),
            }
        }
        outs = next;
    }
    said
}

fn line(bridge: &mut Bridge, aether: &mut Aether, text: &str) -> Vec<Out> {
    let outs = bridge.from_host(text);
    settle(bridge, aether, outs)
}

fn event(bridge: &mut Bridge, aether: &mut Aether, name: &str, data: &Value) -> Vec<Out> {
    let outs = bridge.from_aether(&json!({"event": name, "data": data}).to_string());
    settle(bridge, aether, outs)
}

/// The host's lines, `text` left out, as RPMI compares them.
fn lines(outs: &[Out]) -> Vec<Value> {
    outs.iter()
        .map(|o| match o {
            Out::Host(m) => {
                let mut m = m.clone();
                m.remove("text");
                Value::Object(m)
            }
            Out::CloseHost => json!("closed"),
            Out::Aether(_) => unreachable!("settled"),
        })
        .collect()
}

/// A host that has said hello and claimed KK4ODA, listening as asked.
fn opened(claims: bool, listening: bool) -> (Bridge, Aether) {
    let mut b = Bridge::new();
    let mut a = Aether::new(claims);
    line(
        &mut b,
        &mut a,
        r#"{"op":"hello","id":1,"version":[1,1],"app":"RadioPost client 0.1.0"}"#,
    );
    line(
        &mut b,
        &mut a,
        r#"{"op":"callsigns","id":2,"callsigns":["KK4ODA"]}"#,
    );
    if listening {
        line(&mut b, &mut a, r#"{"op":"listen","id":3,"on":true}"#);
    }
    (b, a)
}

fn mine(name: &str, detail: &str) -> Value {
    json!({"name": name, "detail": detail, "state": "Connected", "remote": "N1CALL-10",
           "callsign": "KK4ODA", "owner": 7, "identifier_pending": false})
}

#[test]
fn hello_is_status_and_capabilities() {
    let mut b = Bridge::new();
    let mut a = Aether::new(true);
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"hello","id":1,"version":[1,1],"app":"RadioPost client 0.1.0"}"#,
    );
    assert_eq!(
        lines(&said),
        [
            json!({"op": "hello", "id": 1, "version": 1, "modem": "Aether HF 0.2.0-beta.146",
                "caps": {"bandwidths": [2300], "max_links": 1, "delivery": true,
                         "broadcast": true, "quality": true, "busy": true, "regulatory": true,
                         "max_queue": 65536, "shared": true}})
        ]
    );
    let methods: Vec<&str> = a
        .asked
        .iter()
        .filter_map(|r| r["method"].as_str())
        .collect();
    assert_eq!(methods, ["status", "capabilities", "hello"]);
    // without claims the host has the modem to itself, and Aether names no programs
    let mut b = Bridge::new();
    let mut a = Aether::new(false);
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"hello","id":1,"version":[1,1],"app":"x"}"#,
    );
    assert_eq!(lines(&said)[0]["caps"]["shared"], false);
    assert!(
        a.asked.iter().all(|r| r["method"] != "hello"),
        "{:?}",
        a.asked
    );
}

/// Aether's header named no program between the RadioPost app's calls, and said "calls to KK4ODA-1
/// not answered" (2026-11-13). The host's `hello` names it to Aether, claiming nothing (Aether's
/// ADR-0114): a host that holds its connection open with no callsigns is seen, and changes nothing
/// the station answers. Its calls claim under the same name.
#[test]
fn a_hosts_hello_names_it_to_aether_and_claims_nothing() {
    let mut b = Bridge::new();
    let mut a = Aether::new(true);
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"hello","id":1,"version":[1,1],"app":"RadioPost client 0.1.0"}"#,
    );
    assert_eq!(
        lines(&said).len(),
        1,
        "the hello answer alone: {:?}",
        lines(&said)
    );
    let named: Vec<&Value> = a.asked.iter().filter(|r| r["method"] == "hello").collect();
    assert_eq!(named.len(), 1);
    assert_eq!(named[0]["params"], json!({"app": "RadioPost client 0.1.0"}));
    assert!(
        a.asked.iter().all(|r| r["method"] != "claim"),
        "a host between calls claims nothing: {:?}",
        a.asked
    );
    assert!(!b.awaiting());
    // events go on reaching it while it holds no callsigns: another's session is busy
    let others = json!({"name": "connected", "detail": "W1AW (master)", "owner": 9});
    assert_eq!(
        lines(&event(&mut b, &mut a, "state", &others)),
        [json!({"op": "busy", "busy": true})]
    );
    // a call's connection claims under the name it said hello with
    line(
        &mut b,
        &mut a,
        r#"{"op":"callsigns","id":2,"callsigns":["KK4ODA"]}"#,
    );
    let claim = a
        .asked
        .iter()
        .find(|r| r["method"] == "claim")
        .expect("a claim");
    assert_eq!(claim["params"]["app"], "RadioPost client 0.1.0");

    // an Aether from before `hello` refuses it: the host hears nothing of it, nothing waits
    let mut b = Bridge::new();
    let mut a = Aether::new(true);
    a.hello = false;
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"hello","id":1,"version":[1,1],"app":"RadioPost client 0.1.0"}"#,
    );
    assert_eq!(lines(&said).len(), 1, "{:?}", lines(&said));
    assert_eq!(lines(&said)[0]["op"], "hello");
    assert!(!b.awaiting());
    // a host with no name gives Aether none to show
    let mut b = Bridge::new();
    let mut a = Aether::new(true);
    line(
        &mut b,
        &mut a,
        r#"{"op":"hello","id":1,"version":[1,1],"app":""}"#,
    );
    assert!(
        a.asked.iter().all(|r| r["method"] != "hello"),
        "{:?}",
        a.asked
    );
}

/// The mobile's client lost its connection to Aether twice on 2026-11-12, and why was in its
/// own log, which the files Aether shares did not carry. The program names its log; the adapter
/// offers it once the host has said hello, and the host hears nothing of it.
#[test]
fn the_programs_log_is_offered_after_hello() {
    let mut b = Bridge::new().offering_log(Some("C:\\Users\\op\\.radiopost\\radiopost.log".into()));
    let mut a = Aether::new(true);
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"hello","id":1,"version":[1,1],"app":"RadioPost client 0.1.0"}"#,
    );
    assert_eq!(
        lines(&said).len(),
        1,
        "the hello answer alone: {:?}",
        lines(&said)
    );
    let offered: Vec<&Value> = a
        .asked
        .iter()
        .filter(|r| r["method"] == "share.include")
        .collect();
    assert_eq!(offered.len(), 1);
    assert_eq!(
        offered[0]["params"],
        json!({"path": "C:\\Users\\op\\.radiopost\\radiopost.log",
               "app": "RadioPost client 0.1.0"})
    );
    // an Aether from before share.include refuses it; nothing waits on the answer
    assert!(!b.awaiting());
    // no log named, or an Aether without claims (older still): nothing offered
    for (log, claims) in [(None, true), (Some("radiopost.log".to_owned()), false)] {
        let mut b = Bridge::new().offering_log(log);
        let mut a = Aether::new(claims);
        line(
            &mut b,
            &mut a,
            r#"{"op":"hello","id":1,"version":[1,1],"app":"x"}"#,
        );
        assert!(
            a.asked.iter().all(|r| r["method"] != "share.include"),
            "{:?}",
            a.asked
        );
    }
}

#[test]
fn a_version_not_spoken_closes_and_no_aether_says_no_modem() {
    let mut b = Bridge::new();
    let mut a = Aether::new(true);
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"hello","id":1,"version":[2,3],"app":"x"}"#,
    );
    assert_eq!(
        lines(&said),
        [
            json!({"op": "error", "id": 1, "code": "unsupported-version"}),
            json!("closed")
        ]
    );
    let mut b = Bridge::without_modem();
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"hello","id":1,"version":[1,1],"app":"x"}"#,
    );
    assert_eq!(
        lines(&said),
        [json!({"op": "error", "id": 1, "code": "no-modem"})]
    );
}

/// RP-4 §2 through the bridge: every line of `rpmi-lines.json`, answered as the file says.
#[test]
fn every_line_is_read_or_refused() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vectors/rpmi-lines.json");
    let file: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("the vectors")).expect("JSON");
    let (mut b, mut a) = opened(true, false);
    for case in file["lines"].as_array().expect("lines") {
        let said = line(&mut b, &mut a, case["line"].as_str().expect("a line"));
        let want: Vec<Value> = case["answers"]
            .as_array()
            .expect("answers")
            .iter()
            .map(|m| {
                let mut m: Message = m.as_object().expect("an object").clone();
                m.remove("text");
                Value::Object(m)
            })
            .collect();
        assert_eq!(lines(&said), want, "{}", case["what"]);
    }
}

#[test]
fn callsigns_are_a_claim_and_kept_whole() {
    let (mut b, mut a) = opened(true, false);
    let claim = a
        .asked
        .iter()
        .find(|r| r["method"] == "claim")
        .expect("a claim");
    assert_eq!(
        claim["params"],
        json!({"callsigns": ["KK4ODA"], "app": "RadioPost client 0.1.0"})
    );
    a.refuse = vec!["N1CALL-1"];
    a.asked.clear();
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"callsigns","id":4,"callsigns":["N1CALL-10","N1CALL-1"]}"#,
    );
    assert_eq!(
        lines(&said),
        [json!({"op": "error", "id": 4, "code": "claimed"})]
    );
    let claimed: Vec<&Value> = a.asked.iter().map(|r| &r["params"]["callsigns"]).collect();
    assert_eq!(
        claimed,
        [&json!(["N1CALL-10", "N1CALL-1"]), &json!(["KK4ODA"])],
        "the earlier list claimed again"
    );
    let said = line(&mut b, &mut a, r#"{"op":"status","id":5}"#);
    assert_eq!(lines(&said)[0]["callsigns"], json!(["KK4ODA"]));
}

#[test]
fn without_claims_listen_off_is_refused() {
    let (mut b, mut a) = opened(false, false);
    assert!(a.asked.iter().any(|r| r["method"] == "callsigns.set"));
    let said = line(&mut b, &mut a, r#"{"op":"listen","id":4,"on":false}"#);
    assert_eq!(
        lines(&said),
        [json!({"op": "error", "id": 4, "code": "unsupported"})]
    );
    let said = line(&mut b, &mut a, r#"{"op":"listen","id":5,"on":true}"#);
    assert_eq!(lines(&said), [json!({"op": "ok", "id": 5})]);
}

#[test]
fn a_call_carries_a_session_and_ends() {
    let (mut b, mut a) = opened(true, false);
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"connect","id":3,"remote":"N1CALL-10","bandwidth":2300}"#,
    );
    assert_eq!(lines(&said), [json!({"op": "ok", "id": 3})]);
    let connect = a.asked.last().expect("a connect");
    assert_eq!(
        connect["params"],
        json!({"remote": "N1CALL-10", "callsign": "KK4ODA", "keep_bandwidth": true})
    );
    let said = event(
        &mut b,
        &mut a,
        "state",
        &mine("connected", "N1CALL-10 (master)"),
    );
    assert_eq!(
        lines(&said),
        [
            json!({"op": "connected", "id": 3, "link": "a1", "remote": "N1CALL-10",
                "local": "KK4ODA", "direction": "out", "bandwidth": 2300})
        ]
    );
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"send","link":"a1","data":"AQID","ref":"t1"}"#,
    );
    assert!(said.is_empty(), "a send with no id is answered by events");
    let send = a.asked.last().expect("a send");
    assert_eq!(send["params"], json!({"data": "AQID", "ref": "rp1"}));
    let metrics = |q: u64| {
        json!({"queued_bytes": q, "channel_busy": true, "link": {},
                                  "snr_db": 12.5, "throughput_bps": 300, "receiving": false})
    };
    let said = event(&mut b, &mut a, "metrics", &metrics(3));
    assert_eq!(
        lines(&said),
        [
            json!({"op": "queued", "link": "a1", "bytes": 3}),
            json!({"op": "quality", "link": "a1", "snr_db": 12.5, "rate_bps": 300,
                   "receiving": false})
        ],
        "the peer's signal is no busy channel"
    );
    let said = event(
        &mut b,
        &mut a,
        "sent",
        &json!({"ref": "rp1", "bytes": 3, "delivered": true}),
    );
    assert_eq!(
        lines(&said),
        [json!({"op": "delivered", "link": "a1", "ref": "t1"})]
    );
    let said = event(&mut b, &mut a, "metrics", &metrics(0));
    assert_eq!(
        lines(&said),
        [json!({"op": "queued", "link": "a1", "bytes": 0})]
    );
    let said = event(&mut b, &mut a, "data", &json!({"data": "BAUG"}));
    assert_eq!(
        lines(&said),
        [json!({"op": "received", "link": "a1", "data": "BAUG"})]
    );
    let said = line(&mut b, &mut a, r#"{"op":"disconnect","id":4,"link":"a1"}"#);
    assert_eq!(lines(&said), [json!({"op": "ok", "id": 4})]);
    let said = event(&mut b, &mut a, "state", &mine("disconnected", "closed"));
    assert_eq!(
        lines(&said),
        [json!({"op": "disconnected", "link": "a1", "reason": "local"})]
    );
}

#[test]
fn a_call_answered_is_incoming_then_connected() {
    let (mut b, mut a) = opened(true, true);
    let said = event(
        &mut b,
        &mut a,
        "state",
        &mine("connected", "N1CALL-10 (slave)"),
    );
    assert_eq!(
        lines(&said),
        [
            json!({"op": "incoming", "remote": "N1CALL-10", "local": "KK4ODA"}),
            json!({"op": "connected", "link": "a1", "remote": "N1CALL-10", "local": "KK4ODA",
                   "direction": "in", "bandwidth": 2300})
        ]
    );
    let said = event(
        &mut b,
        &mut a,
        "state",
        &mine("disconnected", "peer disconnected"),
    );
    assert_eq!(
        lines(&said),
        [json!({"op": "disconnected", "link": "a1", "reason": "remote"})]
    );
}

#[test]
fn calls_that_fail_say_how() {
    for (detail, abort, code) in [
        ("no answer", false, "timeout"),
        ("aborted", true, "aborted"),
        ("N1CALL-10 runs another link protocol", false, "refused"),
    ] {
        let (mut b, mut a) = opened(true, false);
        line(
            &mut b,
            &mut a,
            r#"{"op":"connect","id":3,"remote":"N1CALL-10"}"#,
        );
        if abort {
            let said = line(&mut b, &mut a, r#"{"op":"abort","id":4}"#);
            assert_eq!(lines(&said), [json!({"op": "ok", "id": 4})]);
        }
        let said = event(&mut b, &mut a, "state", &mine("disconnected", detail));
        assert_eq!(
            lines(&said),
            [json!({"op": "failed", "id": 3, "code": code})],
            "{detail}"
        );
    }
}

#[test]
fn aethers_refusals_of_a_call() {
    for (aether, rpmi) in [
        ("claimed", "not-authorized"),
        ("bad_params", "not-authorized"),
        ("already_connected", "busy"),
        ("refused", "refused"),
        ("regulatory", "refused"),
    ] {
        let (mut b, mut a) = opened(true, false);
        a.connect_error = Some(aether);
        let said = line(
            &mut b,
            &mut a,
            r#"{"op":"connect","id":3,"remote":"N1CALL-10"}"#,
        );
        assert_eq!(
            lines(&said),
            [json!({"op": "error", "id": 3, "code": rpmi})],
            "{aether}"
        );
        // the call is not under way: another may be placed
        a.connect_error = None;
        let said = line(
            &mut b,
            &mut a,
            r#"{"op":"connect","id":4,"remote":"N1CALL-10"}"#,
        );
        assert_eq!(lines(&said), [json!({"op": "ok", "id": 4})]);
    }
}

#[test]
fn another_programs_session_is_busy() {
    let (mut b, mut a) = opened(true, true);
    let mut theirs = mine("connected", "N3CALL (slave)");
    theirs["owner"] = json!(9);
    let said = event(&mut b, &mut a, "state", &theirs.clone());
    assert_eq!(lines(&said), [json!({"op": "busy", "busy": true})]);
    let said = event(&mut b, &mut a, "data", &json!({"data": "AQID"}));
    assert!(
        said.is_empty(),
        "another program's payload is not this host's"
    );
    let said = line(&mut b, &mut a, r#"{"op":"status","id":4}"#);
    assert_eq!(
        lines(&said),
        [
            json!({"op": "status", "id": 4, "listening": true, "callsigns": ["KK4ODA"],
                "busy": true, "links": []})
        ]
    );
    theirs["name"] = json!("disconnected");
    let said = event(&mut b, &mut a, "state", &theirs);
    assert_eq!(lines(&said), [json!({"op": "busy", "busy": false})]);
}

#[test]
fn checks_made_before_aether_is_asked() {
    let (mut b, mut a) = opened(true, false);
    for (request, code) in [
        (
            r#"{"op":"connect","id":3,"remote":"X","as":"W1AW"}"#,
            "not-authorized",
        ),
        (
            r#"{"op":"connect","id":3,"remote":"X","bandwidth":500}"#,
            "unsupported",
        ),
        (
            r#"{"op":"send","id":3,"link":"a1","data":"AQ=="}"#,
            "not-connected",
        ),
        (r#"{"op":"disconnect","id":3,"link":"a1"}"#, "not-connected"),
        (r#"{"op":"abort","id":3}"#, "not-connected"),
    ] {
        let before = a.asked.len();
        let said = line(&mut b, &mut a, request);
        assert_eq!(
            lines(&said),
            [json!({"op": "error", "id": 3, "code": code})],
            "{request}"
        );
        assert_eq!(a.asked.len(), before, "{request}: Aether not asked");
    }
}

#[test]
fn broadcasts_are_datagrams() {
    let (mut b, mut a) = opened(true, false);
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"broadcast","id":3,"data":"AQID","ref":"b1"}"#,
    );
    assert_eq!(lines(&said), [json!({"op": "ok", "id": 3})]);
    let sent = a.asked.last().expect("a datagram");
    assert_eq!(sent["method"], "datagram.send");
    assert_eq!(
        sent["params"],
        json!({"data": "AQID", "frame_type": 2, "ref": "rp1"})
    );
    let said = event(
        &mut b,
        &mut a,
        "datagram-sent",
        &json!({"ref": "rp1", "sent": true}),
    );
    assert_eq!(
        lines(&said),
        [json!({"op": "broadcast-sent", "ref": "b1", "sent": true})]
    );
    let said = event(
        &mut b,
        &mut a,
        "datagram",
        &json!({"source": "N1CALL-10", "frame_type": 2, "data": "BAUG", "bytes": 3,
               "snr_db": 4.5, "rung": 1}),
    );
    assert_eq!(
        lines(&said),
        [json!({"op": "broadcast", "source": "N1CALL-10", "data": "BAUG", "snr_db": 4.5})]
    );
}

#[test]
fn aether_going_away_ends_the_link() {
    let (mut b, mut a) = opened(true, false);
    line(
        &mut b,
        &mut a,
        r#"{"op":"connect","id":3,"remote":"N1CALL-10"}"#,
    );
    event(
        &mut b,
        &mut a,
        "state",
        &mine("connected", "N1CALL-10 (master)"),
    );
    let lost = b.modem_lost("a read failed: Connection reset without closing handshake");
    assert_eq!(
        lines(&lost),
        [json!({"op": "disconnected", "link": "a1", "reason": "failure"})]
    );
    // the host is told why, in words
    let Out::Host(told) = &lost[0] else {
        panic!("{lost:?}")
    };
    assert_eq!(
        told["text"],
        "Aether went away: a read failed: Connection reset without closing handshake"
    );
    let said = line(&mut b, &mut a, r#"{"op":"status","id":4}"#);
    assert_eq!(
        lines(&said),
        [json!({"op": "error", "id": 4, "code": "no-modem"})]
    );
}

#[test]
fn a_host_that_leaves_ends_its_link_and_its_call() {
    // a link up: the bridge asks Aether to abort it
    let (mut b, mut a) = opened(true, false);
    line(
        &mut b,
        &mut a,
        r#"{"op":"connect","id":3,"remote":"N1CALL-10"}"#,
    );
    event(
        &mut b,
        &mut a,
        "state",
        &mine("connected", "N1CALL-10 (master)"),
    );
    let asked = a.asked.len();
    let outs = b.host_left();
    assert_eq!(lines(&settle(&mut b, &mut a, outs)), Vec::<Value>::new());
    assert_eq!(a.asked[asked]["method"], json!("abort"));
    assert!(!b.awaiting());
    // a call under way: the same
    let (mut b, mut a) = opened(true, false);
    line(
        &mut b,
        &mut a,
        r#"{"op":"connect","id":3,"remote":"N1CALL-10"}"#,
    );
    let asked = a.asked.len();
    let outs = b.host_left();
    settle(&mut b, &mut a, outs);
    assert_eq!(a.asked[asked]["method"], json!("abort"));
    // nothing held: nothing asked
    let (mut b, _) = opened(true, true);
    assert_eq!(b.host_left().len(), 0, "nothing held, nothing asked");
}
