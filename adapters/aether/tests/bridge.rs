// SPDX-License-Identifier: MIT OR Apache-2.0
//! The bridge against RP-4 §9.1's table, with a stand-in for Aether's control API that answers
//! as its specification says, and `rpmi-lines.json` replayed through it (RP-4 §2).

use std::path::PathBuf;

use radiopost_aether::bridge::{Bridge, Out};
use rpmi::wire::Message;
use serde_json::{Value, json};

/// Aether as its control API specification has it, for the methods the adapter uses.
#[allow(
    clippy::struct_excessive_bools,
    reason = "what each Aether version or radio can do"
)]
struct Aether {
    claims: bool,
    /// It knows `hello` (0.2.0-beta.159, its ADR-0114); an earlier one answers `unknown_method`.
    hello: bool,
    refuse: Vec<&'static str>,
    connect_error: Option<&'static str>,
    /// The radio's dial, read over CAT; none when the keying cannot ask the radio.
    dial: Option<u64>,
    /// `status.state`: another program's call is no event, only a state.
    state: &'static str,
    /// `frequency.set` refused, as when a probe is out.
    tune_refused: bool,
    /// `status.scan` is there (0.2.0-beta.162, its ADR-0129): it scans and takes `call_s`.
    scans: bool,
    /// `scan.start` refused with this code.
    scan_refused: Option<&'static str>,
    /// The bandwidth the station runs, and what a host program asked of it (`host_hz`).
    bandwidth_hz: u64,
    host_hz: Value,
    /// `bandwidth.set` refused, as when something is under way on the air.
    bandwidth_refused: bool,
    /// `tune` (the tuner tone) refused, as when the channel is busy.
    tone_refused: bool,
    /// Each connection's bandwidth request held apart, one for listening taken
    /// (`status.bandwidth.listening`, 0.2.0-beta.174).
    listening_requests: bool,
    /// `status.scan`'s dials, a scan another program runs; none when empty.
    scan_dials: Vec<u64>,
    /// The stations heard, as `heard.list` answers them.
    heard: Value,
    /// `status.beacon.every_s`: the panel's repeating beacon.
    beacon_every_s: Option<f64>,
    /// A program's beacon taken (`capabilities.beacon`, 0.2.0-beta.183), and refused with this
    /// code.
    beacons: bool,
    beacon_refused: Option<&'static str>,
    asked: Vec<Value>,
}

impl Aether {
    fn new(claims: bool) -> Aether {
        Aether {
            claims,
            hello: true,
            refuse: Vec::new(),
            connect_error: None,
            dial: None,
            state: "idle",
            tune_refused: false,
            scans: false,
            scan_refused: None,
            bandwidth_hz: 2300,
            host_hz: Value::Null,
            bandwidth_refused: false,
            tone_refused: false,
            listening_requests: false,
            scan_dials: Vec::new(),
            heard: json!([]),
            beacon_every_s: None,
            beacons: false,
            beacon_refused: None,
            asked: Vec::new(),
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one arm for each of Aether's methods the adapter asks for"
    )]
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
                let mut status = json!({"version": "0.2.0-beta.146", "state": self.state,
                                        "owner": null, "channel_busy": false,
                                        "frequency_hz": self.dial,
                                        "can_tune": self.dial.is_some(),
                                        "bandwidth": {"bandwidth_hz": self.bandwidth_hz,
                                                      "home_hz": 2300,
                                                      "host_hz": self.host_hz}});
                if self.claims {
                    status["claims"] = json!([]);
                }
                if self.scans {
                    status["scan"] = Value::Null;
                }
                if !self.scan_dials.is_empty() {
                    status["scan"] = json!({"owner": 9, "dials": self.scan_dials});
                }
                if self.listening_requests {
                    status["bandwidth"]["listening"] = json!(false);
                }
                status["beacon"] = json!({"every_s": self.beacon_every_s, "waiting": false});
                ok(status)
            }
            "capabilities" if self.beacons => ok(json!({
                "bandwidth_hz": 2300, "bandwidths_hz": [2300, 500],
                "beacon": {"program": true, "answers": true, "position": true}})),
            "capabilities" => ok(json!({"bandwidth_hz": 2300, "bandwidths_hz": [2300, 500]})),
            "beacon" => match self.beacon_refused {
                Some(code) => refused(code),
                None => ok(json!({"accepted": true})),
            },
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
            "frequency.set" if self.tune_refused => refused("refused"),
            "frequency.set" => {
                self.dial = params["hz"].as_u64();
                ok(json!({"hz": params["hz"]}))
            }
            "scan.start" => {
                if let Some(code) = self.scan_refused {
                    refused(code)
                } else {
                    let n = params["dials"].as_array().expect("dials").len();
                    ok(scan_status(&params["dials"], n))
                }
            }
            "scan.stop" => ok(json!({"stopped": true})),
            "bandwidth.set" if self.bandwidth_refused && !params["hz"].is_null() => {
                refused("refused")
            }
            "bandwidth.set" => {
                self.host_hz = params["hz"].clone();
                self.bandwidth_hz = params["hz"].as_u64().unwrap_or(2300);
                ok(json!({"bandwidth_hz": self.bandwidth_hz, "host_hz": self.host_hz}))
            }
            "tune" if self.tone_refused => refused("refused"),
            "tune" if params["duration_s"] == 0 => ok(json!({"stopped": true})),
            "tune" => ok(json!({"accepted": true, "duration_s": params["duration_s"]})),
            "heard.list" => ok(json!({"stations": self.heard, "limit": 200,
                                      "path": "/x/heard.json"})),
            "hello" if self.hello => ok(json!({"id": 7, "app": params["app"]})),
            "hello" => refused("unknown_method"),
            other => panic!("the adapter asked Aether for {other}"),
        }
    }
}

/// `status.scan` as Aether has it for this connection's scan (its claim is 7): 19.6 s on each
/// dial.
fn scan_status(dials: &Value, n: usize) -> Value {
    #[allow(clippy::cast_precision_loss, reason = "a few dials")]
    let scan_s = ((n - 1) as f64 * 19.6).ceil();
    json!({"owner": 7, "app": "RadioPost gateway 0.1.0", "dials": dials, "at_hz": dials[0],
           "dwell_s": 19.6, "linger_s": 40.0, "scan_s": scan_s, "held": null,
           "lingering_until_ms": null, "hops": 0, "dropped": [], "since_ms": 0})
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
                "caps": {"bandwidths": [500, 2300], "max_links": 1, "delivery": true,
                         "broadcast": true, "quality": true, "busy": true, "regulatory": true,
                         "max_queue": 65536, "shared": true, "tone": true, "heard": true}})
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
        // refused only by an adapter without the cap `unless` names; this one may have it
        if case.get("unless").is_some() {
            continue;
        }
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
        // `status`'s station members are an adapter's to add when it can say (RP-4 §4); the
        // reference adapter the file was written with says none
        let said: Vec<Value> = lines(&said)
            .into_iter()
            .map(|mut m| {
                if m["op"] == "status"
                    && let Some(o) = m.as_object_mut()
                {
                    for key in ["dial_hz", "answers", "scanning"] {
                        o.remove(key);
                    }
                }
                m
            })
            .collect();
        assert_eq!(said, want, "{}", case["what"]);
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
                                  "snr_db": 12.5, "throughput_bps": 300, "receiving": false,
                                  "peer_snr_db": 8.0})
    };
    let said = event(&mut b, &mut a, "metrics", &metrics(3));
    assert_eq!(
        lines(&said),
        [
            json!({"op": "queued", "link": "a1", "bytes": 3}),
            json!({"op": "quality", "link": "a1", "snr_db": 12.5, "rate_bps": 300,
                   "receiving": false, "peer_snr_db": 8.0})
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
            [json!({"op": "failed", "id": 3, "code": code, "heard": false})],
            "{detail}"
        );
    }
}

#[test]
fn a_call_that_fails_says_whether_the_station_called_was_heard() {
    let (mut b, mut a) = opened(true, false);
    line(
        &mut b,
        &mut a,
        r#"{"op":"connect","id":3,"remote":"N1CALL-10"}"#,
    );
    // a frame not decoded, and one decoded from another station, say nothing of it
    let frame = |from: &str, decoded: bool, snr: f64| json!({"kind": "beacon", "decoded": decoded, "from": from, "snr_db": snr});
    event(&mut b, &mut a, "frame", &frame("N1CALL-10", false, 9.0));
    event(&mut b, &mut a, "frame", &frame("W1AW", true, 12.0));
    // two decoded from it: the strongest is said, whatever its case
    event(&mut b, &mut a, "frame", &frame("n1call-10", true, -3.5));
    event(&mut b, &mut a, "frame", &frame("N1CALL-10", true, 1.5));
    let said = event(&mut b, &mut a, "state", &mine("disconnected", "no answer"));
    assert_eq!(
        lines(&said),
        [json!({"op": "failed", "id": 3, "code": "timeout", "heard": true, "snr_db": 1.5})]
    );
    // the next call starts with nothing heard
    line(
        &mut b,
        &mut a,
        r#"{"op":"connect","id":4,"remote":"N1CALL-10"}"#,
    );
    let said = event(&mut b, &mut a, "state", &mine("disconnected", "no answer"));
    assert_eq!(
        lines(&said),
        [json!({"op": "failed", "id": 4, "code": "timeout", "heard": false})]
    );
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
                "busy": true, "links": [], "answers": [500, 2300]})
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
            r#"{"op":"connect","id":3,"remote":"X","bandwidth":1800}"#,
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

/// RP-4 1.5.0 §6, Aether's ADR-0129 §15: a broadcast heard says the dial Aether heard it on,
/// and an answer naming that dial asks Aether to send it there.
#[test]
fn a_broadcast_carries_its_dial_both_ways() {
    let (mut b, mut a) = opened(true, false);
    let said = event(
        &mut b,
        &mut a,
        "datagram",
        &json!({"source": "N1CALL-10", "frame_type": 2, "data": "BAUG", "bytes": 3,
               "snr_db": 4.5, "rung": 1, "dial_hz": 7_101_000}),
    );
    assert_eq!(
        lines(&said),
        [
            json!({"op": "broadcast", "source": "N1CALL-10", "data": "BAUG", "snr_db": 4.5,
                "dial_hz": 7_101_000})
        ]
    );
    // a dial Aether could not read says nothing of one
    let said = event(
        &mut b,
        &mut a,
        "datagram",
        &json!({"source": "N1CALL-10", "frame_type": 2, "data": "BAUG", "bytes": 3,
               "rung": 1, "dial_hz": null}),
    );
    assert_eq!(
        lines(&said),
        [json!({"op": "broadcast", "source": "N1CALL-10", "data": "BAUG"})]
    );
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"broadcast","id":3,"data":"AQID","ref":"b1","dial_hz":7101000}"#,
    );
    assert_eq!(lines(&said), [json!({"op": "ok", "id": 3})]);
    let sent = a.asked.last().expect("a datagram");
    assert_eq!(sent["method"], "datagram.send");
    assert_eq!(
        sent["params"],
        json!({"data": "AQID", "frame_type": 2, "ref": "rp1", "dial_hz": 7_101_000})
    );
    // a dial of 0 is malformed, and Aether is not asked
    let before = a.asked.len();
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"broadcast","id":4,"data":"AQID","dial_hz":0}"#,
    );
    assert_eq!(
        lines(&said),
        [json!({"op": "error", "id": 4, "code": "malformed"})]
    );
    assert_eq!(a.asked.len(), before);
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

/// RP-4 §4.1 on Aether (§9.1): `caps.dial` is `status.can_tune`, `dial` reads
/// `status.frequency_hz`, `tune` is `frequency.set` once Aether's state says nothing is up on the
/// station, and Aether's `dial` event is RPMI's.
#[test]
fn the_dial_is_read_and_set_through_aether() {
    let mut b = Bridge::new();
    let mut a = Aether::new(true);
    a.dial = Some(7_101_000);
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"hello","id":1,"version":[1,1],"app":"RadioPost client 0.1.0"}"#,
    );
    assert_eq!(lines(&said)[0]["caps"]["dial"], true);
    line(
        &mut b,
        &mut a,
        r#"{"op":"callsigns","id":2,"callsigns":["KK4ODA"]}"#,
    );
    let said = line(&mut b, &mut a, r#"{"op":"dial","id":3}"#);
    assert_eq!(
        lines(&said),
        [json!({"op": "dial", "id": 3, "dial_hz": 7_101_000})]
    );
    a.asked.clear();
    let said = line(&mut b, &mut a, r#"{"op":"tune","id":4,"dial_hz":3590000}"#);
    assert_eq!(lines(&said), [json!({"op": "ok", "id": 4})]);
    let methods: Vec<&str> = a
        .asked
        .iter()
        .filter_map(|r| r["method"].as_str())
        .collect();
    assert_eq!(methods, ["status", "frequency.set"]);
    assert_eq!(a.asked[1]["params"], json!({"hz": 3_590_000}));
    // the change comes back as Aether's event, and every host hears it
    let said = event(&mut b, &mut a, "dial", &json!({"frequency_hz": 3_590_000}));
    assert_eq!(lines(&said), [json!({"op": "dial", "dial_hz": 3_590_000})]);

    // another program's call: only Aether's state says so, and the dial stays
    a.state = "connecting";
    a.asked.clear();
    let said = line(&mut b, &mut a, r#"{"op":"tune","id":5,"dial_hz":7101000}"#);
    assert_eq!(
        lines(&said),
        [json!({"op": "error", "id": 5, "code": "busy"})]
    );
    assert!(a.asked.iter().all(|r| r["method"] != "frequency.set"));
    a.state = "idle";
    // Aether's own refusals (a probe out, a Test, something queued for this dial) are `refused`
    a.tune_refused = true;
    let said = line(&mut b, &mut a, r#"{"op":"tune","id":6,"dial_hz":7101000}"#);
    assert_eq!(
        lines(&said),
        [json!({"op": "error", "id": 6, "code": "refused"})]
    );
    a.tune_refused = false;
    // the host's own session holds the dial without asking Aether
    line(
        &mut b,
        &mut a,
        r#"{"op":"connect","id":7,"remote":"N1CALL-10"}"#,
    );
    event(
        &mut b,
        &mut a,
        "state",
        &mine("connected", "N1CALL-10 (master)"),
    );
    a.asked.clear();
    let said = line(&mut b, &mut a, r#"{"op":"tune","id":8,"dial_hz":7101000}"#);
    assert_eq!(
        lines(&said),
        [json!({"op": "error", "id": 8, "code": "busy"})]
    );
    assert!(a.asked.is_empty(), "{:?}", a.asked);

    // an Aether whose keying cannot ask the radio has no dial
    let (mut b, mut a) = opened(true, false);
    let said = line(&mut b, &mut a, r#"{"op":"dial","id":4}"#);
    assert_eq!(
        lines(&said),
        [json!({"op": "error", "id": 4, "code": "unsupported"})]
    );
    let said = line(&mut b, &mut a, r#"{"op":"tune","id":5,"dial_hz":7101000}"#);
    assert_eq!(
        lines(&said),
        [json!({"op": "error", "id": 5, "code": "unsupported"})]
    );
    let said = event(&mut b, &mut a, "dial", &json!({"frequency_hz": 3_590_000}));
    assert_eq!(lines(&said), Vec::<Value>::new(), "no dial to tell");
}

/// A scan is Aether's `scan.start` on the host's own connection (RP-4 §4.2, §9.1; Aether's
/// ADR-0129): its answer carries `scan_s` in whole seconds, Aether's echo of a request is not told
/// again, a dial dropped is told with the new `scan_s`, the scan's end is told once with RPMI's
/// `why`, and a stop with no scan of the host's own asks Aether nothing, since a connection with
/// no scan and no claim would stop another program's.
#[test]
#[allow(clippy::too_many_lines, reason = "one story, told in order")]
fn a_scan_is_aethers_on_the_hosts_connection() {
    let mut b = Bridge::new();
    let mut a = Aether::new(true);
    a.dial = Some(7_101_000);
    a.scans = true;
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"hello","id":1,"version":[1,1],"app":"RadioPost gateway 0.1.0"}"#,
    );
    let caps = &lines(&said)[0]["caps"];
    assert_eq!(
        (&caps["scan"], &caps["call_s"]),
        (&json!(true), &json!(true))
    );
    line(
        &mut b,
        &mut a,
        r#"{"op":"callsigns","id":2,"callsigns":["N1CALL-10"]}"#,
    );
    a.asked.clear();
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"scan","id":3,"dials":[3590000,7101000,10145000]}"#,
    );
    assert_eq!(
        lines(&said),
        [
            json!({"op": "scan", "id": 3, "dials": [3_590_000, 7_101_000, 10_145_000],
                "scan_s": 40})
        ]
    );
    assert_eq!(a.asked[0]["method"], "scan.start");
    assert_eq!(
        a.asked[0]["params"],
        json!({"dials": [3_590_000, 7_101_000, 10_145_000]})
    );
    // Aether's event for the scan it just started says nothing new
    let started = json!({"scan": scan_status(&json!([3_590_000, 7_101_000, 10_145_000]), 3),
                         "why": null});
    assert_eq!(
        lines(&event(&mut b, &mut a, "scan", &started)),
        Vec::<Value>::new()
    );
    // a dial the radio would not take is dropped: the host hears the dials left
    let dropped = json!({"scan": scan_status(&json!([3_590_000, 7_101_000]), 2), "why": null});
    assert_eq!(
        lines(&event(&mut b, &mut a, "scan", &dropped)),
        [json!({"op": "scan", "dials": [3_590_000, 7_101_000], "scan_s": 20})]
    );
    // another program tunes the radio: the operator wins, said once, in RPMI's words
    let ended = json!({"scan": null, "why": "tuned by another program"});
    assert_eq!(
        lines(&event(&mut b, &mut a, "scan", &ended)),
        [json!({"op": "scan", "dials": [], "why": "tuned"})]
    );
    assert_eq!(
        lines(&event(&mut b, &mut a, "scan", &ended)),
        Vec::<Value>::new()
    );
    // scanned again, then stopped: the stop's own end is not told again
    line(
        &mut b,
        &mut a,
        r#"{"op":"scan","id":4,"dials":[3590000,7101000]}"#,
    );
    a.asked.clear();
    let said = line(&mut b, &mut a, r#"{"op":"scan","id":5,"dials":[]}"#);
    assert_eq!(lines(&said), [json!({"op": "scan", "id": 5, "dials": []})]);
    assert_eq!(a.asked[0]["method"], "scan.stop");
    let stopped = json!({"scan": null, "why": "asked"});
    assert_eq!(
        lines(&event(&mut b, &mut a, "scan", &stopped)),
        Vec::<Value>::new()
    );
    // nothing to stop: answered here, and Aether is not asked
    a.asked.clear();
    let said = line(&mut b, &mut a, r#"{"op":"scan","id":6,"dials":[]}"#);
    assert_eq!(lines(&said), [json!({"op": "scan", "id": 6, "dials": []})]);
    assert!(a.asked.is_empty(), "{:?}", a.asked);
    // another program's scan is not this host's business
    let theirs = json!({"scan": {"owner": 9, "dials": [14_105_000, 7_101_000], "scan_s": 19.6},
                        "why": null});
    assert_eq!(
        lines(&event(&mut b, &mut a, "scan", &theirs)),
        Vec::<Value>::new()
    );
    // Aether's refusals
    for (aether, rpmi) in [
        ("claimed", "busy"),
        ("regulatory", "refused"),
        ("bad_params", "refused"),
        ("unsupported", "unsupported"),
    ] {
        a.scan_refused = Some(aether);
        let said = line(
            &mut b,
            &mut a,
            r#"{"op":"scan","id":7,"dials":[3590000,7101000]}"#,
        );
        assert_eq!(
            lines(&said),
            [json!({"op": "error", "id": 7, "code": rpmi})],
            "{aether}"
        );
    }
    // a call to a gateway that scans keeps trying as long as asked
    a.asked.clear();
    line(
        &mut b,
        &mut a,
        r#"{"op":"connect","id":8,"remote":"KK4ODA-1","call_s":40}"#,
    );
    assert_eq!(a.asked[0]["method"], "connect");
    assert_eq!(a.asked[0]["params"]["call_s"], 40);

    // an Aether before 0.2.0-beta.162 neither scans nor takes `call_s`
    let mut b = Bridge::new();
    let mut a = Aether::new(true);
    a.dial = Some(7_101_000);
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"hello","id":1,"version":[1,1],"app":"RadioPost client 0.1.0"}"#,
    );
    let caps = &lines(&said)[0]["caps"];
    assert!(
        caps.get("scan").is_none() && caps.get("call_s").is_none(),
        "{caps}"
    );
    line(
        &mut b,
        &mut a,
        r#"{"op":"callsigns","id":2,"callsigns":["KK4ODA"]}"#,
    );
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"scan","id":3,"dials":[3590000,7101000]}"#,
    );
    assert_eq!(
        lines(&said),
        [json!({"op": "error", "id": 3, "code": "unsupported"})]
    );
    a.asked.clear();
    line(
        &mut b,
        &mut a,
        r#"{"op":"connect","id":4,"remote":"KK4ODA-1","call_s":40}"#,
    );
    assert!(
        a.asked[0]["params"].get("call_s").is_none(),
        "{:?}",
        a.asked
    );

    // one that cannot set the dial takes `call_s` but does not scan
    let mut b = Bridge::new();
    let mut a = Aether::new(true);
    a.scans = true;
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"hello","id":1,"version":[1,1],"app":"RadioPost client 0.1.0"}"#,
    );
    let caps = &lines(&said)[0]["caps"];
    assert!(caps.get("scan").is_none(), "{caps}");
    assert_eq!(caps["call_s"], true);
}

/// The methods asked of Aether since `from`.
fn asked_since(a: &Aether, from: usize) -> Vec<(String, Value)> {
    a.asked[from..]
        .iter()
        .map(|r| {
            (
                r["method"].as_str().unwrap_or("").to_owned(),
                r["params"].clone(),
            )
        })
        .collect()
}

/// RP-4 §1.1, §9.1: a call in a bandwidth Aether runs but is not running now moves the whole
/// station there first (`bandwidth.set`, once its state says nothing is up), calls keeping it, and
/// sets back what a host program had asked of it once the call's link has ended.
#[test]
fn a_call_in_another_bandwidth_moves_the_station_and_back() {
    let (mut b, mut a) = opened(true, false);
    let from = a.asked.len();
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"connect","id":3,"remote":"WD5EMA","bandwidth":500}"#,
    );
    assert_eq!(lines(&said), [json!({"op": "ok", "id": 3})]);
    assert_eq!(
        asked_since(&a, from),
        [
            ("status".to_owned(), json!({})),
            ("bandwidth.set".to_owned(), json!({"hz": 500})),
            (
                "connect".to_owned(),
                json!({"remote": "WD5EMA", "callsign": "KK4ODA", "keep_bandwidth": true})
            ),
        ]
    );
    // Aether says where it went, and the link carries it
    event(&mut b, &mut a, "bandwidth", &json!({"bandwidth_hz": 500}));
    let mut up = mine("connected", "WD5EMA (master)");
    up["remote"] = json!("WD5EMA");
    let said = event(&mut b, &mut a, "state", &up);
    assert_eq!(
        lines(&said),
        [
            json!({"op": "connected", "id": 3, "link": "a1", "remote": "WD5EMA",
                   "local": "KK4ODA", "direction": "out", "bandwidth": 500})
        ]
    );
    let from = a.asked.len();
    let said = event(&mut b, &mut a, "state", &mine("disconnected", "closed"));
    assert_eq!(
        lines(&said),
        [json!({"op": "disconnected", "link": "a1", "reason": "local"})]
    );
    assert_eq!(
        asked_since(&a, from),
        [("bandwidth.set".to_owned(), json!({"hz": null}))],
        "the station goes back to its own"
    );

    // a call that fails sets it back too, and a call in the bandwidth running moves nothing
    event(&mut b, &mut a, "bandwidth", &json!({"bandwidth_hz": 2300}));
    line(
        &mut b,
        &mut a,
        r#"{"op":"connect","id":4,"remote":"WD5EMA","bandwidth":500}"#,
    );
    let from = a.asked.len();
    let said = event(&mut b, &mut a, "state", &mine("disconnected", "no answer"));
    assert_eq!(
        lines(&said),
        [json!({"op": "failed", "id": 4, "code": "timeout", "heard": false})]
    );
    assert_eq!(
        asked_since(&a, from),
        [("bandwidth.set".to_owned(), json!({"hz": null}))]
    );
    event(&mut b, &mut a, "bandwidth", &json!({"bandwidth_hz": 2300}));
    let from = a.asked.len();
    line(
        &mut b,
        &mut a,
        r#"{"op":"connect","id":5,"remote":"WD5EMA","bandwidth":2300}"#,
    );
    let methods: Vec<String> = asked_since(&a, from).into_iter().map(|(m, _)| m).collect();
    assert_eq!(methods, ["connect"]);
}

/// A bandwidth a host program asked for (VARA's `BW500` on Aether's other port) is what goes back,
/// not the station's own: this host's call borrowed the station, and returns it as it was.
#[test]
fn a_bandwidth_another_program_asked_for_is_what_goes_back() {
    let (mut b, mut a) = opened(true, false);
    a.host_hz = json!(2300);
    line(
        &mut b,
        &mut a,
        r#"{"op":"connect","id":3,"remote":"WD5EMA","bandwidth":500}"#,
    );
    let from = a.asked.len();
    event(&mut b, &mut a, "state", &mine("disconnected", "no answer"));
    assert_eq!(
        asked_since(&a, from),
        [("bandwidth.set".to_owned(), json!({"hz": 2300}))]
    );
}

/// RP-4 §1.1: the station cannot move while anything is under way on it. Another program's
/// session, Aether's state not idle, or Aether's own refusal is `busy`, and no call goes out.
#[test]
fn a_station_that_cannot_move_is_busy() {
    // Aether will not move: something is on the air
    let (mut b, mut a) = opened(true, false);
    a.bandwidth_refused = true;
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"connect","id":3,"remote":"WD5EMA","bandwidth":500}"#,
    );
    assert_eq!(
        lines(&said),
        [json!({"op": "error", "id": 3, "code": "busy"})]
    );
    assert!(a.asked.iter().all(|r| r["method"] != "connect"));
    // another program's call, no event of Aether's: its state says so
    let (mut b, mut a) = opened(true, false);
    a.state = "connecting";
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"connect","id":3,"remote":"WD5EMA","bandwidth":500}"#,
    );
    assert_eq!(
        lines(&said),
        [json!({"op": "error", "id": 3, "code": "busy"})]
    );
    assert!(
        a.asked
            .iter()
            .all(|r| r["method"] != "connect" && r["method"] != "bandwidth.set")
    );
    // another program's session, told: refused before Aether is asked
    let (mut b, mut a) = opened(true, false);
    let mut theirs = mine("connected", "N3CALL (slave)");
    theirs["owner"] = json!(9);
    event(&mut b, &mut a, "state", &theirs);
    let before = a.asked.len();
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"connect","id":3,"remote":"WD5EMA","bandwidth":500}"#,
    );
    assert_eq!(
        lines(&said),
        [json!({"op": "error", "id": 3, "code": "busy"})]
    );
    assert_eq!(a.asked.len(), before);
}

/// RP-4 §1.1: a host that leaves during a call it moved the station for takes the call with it,
/// and the station goes back.
#[test]
fn a_host_that_leaves_puts_the_bandwidth_back() {
    let (mut b, mut a) = opened(true, false);
    line(
        &mut b,
        &mut a,
        r#"{"op":"connect","id":3,"remote":"WD5EMA","bandwidth":500}"#,
    );
    let from = a.asked.len();
    let outs = b.host_left();
    settle(&mut b, &mut a, outs);
    assert_eq!(
        asked_since(&a, from),
        [
            ("abort".to_owned(), json!({})),
            ("bandwidth.set".to_owned(), json!({"hz": null}))
        ]
    );
}

/// RP-4 §4.3 on Aether (§9.1): the tuner tone is Aether's `tune`, `seconds` its `duration_s`, 0
/// to stop it; refused `busy` while a link or a call is up, except to stop; Aether's refusal (a
/// busy channel, the rules) is `refused`.
#[test]
fn the_tuner_tone_is_aethers_tune() {
    let (mut b, mut a) = opened(true, false);
    let from = a.asked.len();
    let said = line(&mut b, &mut a, r#"{"op":"tone","id":3,"seconds":10}"#);
    assert_eq!(lines(&said), [json!({"op": "ok", "id": 3})]);
    let said = line(&mut b, &mut a, r#"{"op":"tone","id":4,"seconds":0}"#);
    assert_eq!(lines(&said), [json!({"op": "ok", "id": 4})]);
    assert_eq!(
        asked_since(&a, from),
        [
            ("tune".to_owned(), json!({"duration_s": 10})),
            ("tune".to_owned(), json!({"duration_s": 0}))
        ]
    );
    let said = line(&mut b, &mut a, r#"{"op":"tone","id":5,"seconds":11}"#);
    assert_eq!(
        lines(&said),
        [json!({"op": "error", "id": 5, "code": "malformed"})]
    );
    a.tone_refused = true;
    let said = line(&mut b, &mut a, r#"{"op":"tone","id":6,"seconds":10}"#);
    assert_eq!(
        lines(&said),
        [json!({"op": "error", "id": 6, "code": "refused"})]
    );
    a.tone_refused = false;
    line(
        &mut b,
        &mut a,
        r#"{"op":"connect","id":7,"remote":"N1CALL-10"}"#,
    );
    let before = a.asked.len();
    let said = line(&mut b, &mut a, r#"{"op":"tone","id":8,"seconds":10}"#);
    assert_eq!(
        lines(&said),
        [json!({"op": "error", "id": 8, "code": "busy"})]
    );
    assert_eq!(a.asked.len(), before, "Aether not asked");
    let said = line(&mut b, &mut a, r#"{"op":"tone","id":9,"seconds":0}"#);
    assert_eq!(
        lines(&said),
        [json!({"op": "ok", "id": 9})],
        "a stop is taken"
    );
}

/// RP-4 §1.1: while the host listens, an Aether that takes a request for listening runs its widest
/// bandwidth, which answers 500 Hz calls too, and is asked for none once the host stops. A profile
/// had left KK4ODA-1 at 500 Hz while its client listened, and WD5EMA's 2300 Hz call went
/// unanswered (2026-11-23).
#[test]
fn a_host_listening_asks_for_the_widest_bandwidth() {
    let mut b = Bridge::new();
    let mut a = Aether::new(true);
    a.listening_requests = true;
    a.bandwidth_hz = 500;
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
    let from = a.asked.len();
    let said = line(&mut b, &mut a, r#"{"op":"listen","id":3,"on":true}"#);
    assert_eq!(lines(&said), [json!({"op": "ok", "id": 3})]);
    assert_eq!(
        asked_since(&a, from),
        [
            ("listen".to_owned(), json!({"enabled": true})),
            (
                "bandwidth.set".to_owned(),
                json!({"hz": 2300, "listening": true})
            ),
        ]
    );
    // a call of its own in 500 Hz moves the station; when it ends, listening's request is back
    let from = a.asked.len();
    line(
        &mut b,
        &mut a,
        r#"{"op":"connect","id":4,"remote":"WD5EMA","bandwidth":500}"#,
    );
    event(&mut b, &mut a, "state", &mine("disconnected", "no answer"));
    let set: Vec<Value> = asked_since(&a, from)
        .into_iter()
        .filter(|(m, _)| m == "bandwidth.set")
        .map(|(_, p)| p)
        .collect();
    assert_eq!(
        set,
        [json!({"hz": 500}), json!({"hz": 2300, "listening": true})]
    );
    // stopping withdraws it
    let from = a.asked.len();
    line(&mut b, &mut a, r#"{"op":"listen","id":5,"on":false}"#);
    assert_eq!(
        asked_since(&a, from),
        [
            ("listen".to_owned(), json!({"enabled": false})),
            ("bandwidth.set".to_owned(), json!({"hz": null})),
        ]
    );
    // an earlier Aether, which holds one request for the whole station, is not asked
    let (_, a) = opened(true, true);
    assert!(!a.asked.iter().any(|r| r["method"] == "bandwidth.set"));
}

/// RP-4 §4 `status`: the station's dial, the bandwidths a call is answered in, and the dials a
/// scan of another program's takes the radio over, from Aether's `status`.
#[test]
fn status_says_where_and_how_the_station_is_called() {
    let (mut b, mut a) = opened(true, true);
    a.dial = Some(7_082_000);
    let said = line(&mut b, &mut a, r#"{"op":"status","id":6}"#);
    assert_eq!(
        lines(&said),
        [
            json!({"op": "status", "id": 6, "listening": true, "callsigns": ["KK4ODA"],
                "busy": false, "links": [], "dial_hz": 7_082_000, "answers": [500, 2300]})
        ]
    );
    // at 500 Hz only 500 Hz calls are answered; a gateway's scan is said
    a.bandwidth_hz = 500;
    a.scan_dials = vec![7_101_000, 3_590_000, 14_107_000];
    let said = line(&mut b, &mut a, r#"{"op":"status","id":7}"#);
    let status = &lines(&said)[0];
    assert_eq!(status["answers"], json!([500]));
    assert_eq!(
        status["scanning"],
        json!([7_101_000, 3_590_000, 14_107_000])
    );
}

/// An entry of Aether's stations heard, as `heard.list` and the `heard` event carry it.
fn entry(callsign: &str, activity: &str, detail: Option<&str>, ms: u64) -> Value {
    json!({"callsign": callsign, "first_heard_ms": 1_795_000_000_000_u64, "last_heard_ms": ms,
           "count": 3, "snr_db": -4.5, "best_snr_db": 2.0, "frequency_hz": 7_101_000,
           "mode": 0, "activity": activity, "detail": detail, "connected": false,
           "beacons": 1, "last_beacon_ms": ms, "bandwidth_hz": 500})
}

/// RP-4 §4.4, §9.1: `stations` is Aether's `heard.list`, and each `heard` event goes to the host
/// in RPMI's words: the activity as `kind`, whom it called as `to`, the dial, the time in seconds.
/// A beacon's own word on answering and its position pass through. Another program's session is
/// its link (§1.1): a frame of it is not told, and its station is listed without a kind.
#[test]
fn the_stations_heard_are_aethers() {
    let (mut b, mut a) = opened(true, true);
    let mut beacon = entry("N2CALL", "beacon", None, 1_795_213_320_500);
    beacon["answering"] = json!(true);
    beacon["position"] = json!({"lat": 34.47916666666667, "lon": -83.79166666666667,
                                "precision": "grid6", "grid": "EM84cl"});
    a.heard = json!([
        beacon,
        entry("N5CALL-7", "calling", Some("N3CALL"), 1_795_213_000_000),
        entry("N3CALL", "connected", None, 1_795_212_000_000),
    ]);
    let said = line(&mut b, &mut a, r#"{"op":"stations","id":4}"#);
    assert_eq!(
        lines(&said),
        [json!({"op": "stations", "id": 4, "stations": [
            {"callsign": "N2CALL", "kind": "beacon", "snr_db": -4.5, "bandwidth": 500,
             "dial_hz": 7_101_000, "at": 1_795_213_320_u64, "answers": true,
             "position": {"lat": 34.47916666666667, "lon": -83.79166666666667,
                          "precision": "grid6", "grid": "EM84cl"}},
            {"callsign": "N5CALL-7", "kind": "call", "to": "N3CALL", "snr_db": -4.5,
             "bandwidth": 500, "dial_hz": 7_101_000, "at": 1_795_213_000_u64},
            {"callsign": "N3CALL", "snr_db": -4.5, "bandwidth": 500, "dial_hz": 7_101_000,
             "at": 1_795_212_000_u64}]})]
    );
    // a frame of another program's session is not this host's to hear of
    let said = event(
        &mut b,
        &mut a,
        "heard",
        &entry("N3CALL", "connected", None, 1),
    );
    assert!(said.is_empty(), "{said:?}");
    // a beacon is, with nothing it did not say
    let said = event(
        &mut b,
        &mut a,
        "heard",
        &entry("N4CALL", "beacon", None, 9_999),
    );
    assert_eq!(
        lines(&said),
        [
            json!({"op": "heard", "callsign": "N4CALL", "kind": "beacon", "snr_db": -4.5,
                "bandwidth": 500, "dial_hz": 7_101_000, "at": 9})
        ]
    );
    // and so is the far end of the host's own link, whose frames are a session
    line(
        &mut b,
        &mut a,
        r#"{"op":"connect","id":5,"remote":"N1CALL-10"}"#,
    );
    event(
        &mut b,
        &mut a,
        "state",
        &mine("connected", "N1CALL-10 (master)"),
    );
    let said = event(
        &mut b,
        &mut a,
        "heard",
        &entry("N1CALL-10", "connected", None, 20_000),
    );
    assert_eq!(lines(&said)[0]["kind"], "session");
}

/// RP-4 §4.4: Aether's own repeating beacon, its panel's Repeat, is `status`'s `beaconing`; a
/// program's beacon waits for the Aether that takes one (its ADR-0139).
/// The adapter over an Aether that takes a program's beacon, its host holding KK4ODA.
fn opened_beaconing() -> (Bridge, Aether) {
    let mut b = Bridge::new();
    let mut a = Aether::new(true);
    a.beacons = true;
    a.dial = Some(7_101_000);
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
    (b, a)
}

#[test]
fn a_beacon_is_aethers_with_a_reference_of_its_own_and_comes_back_as_beacon_sent() {
    // RP-4 1.7.0 §4.4 and §9.1 over Aether's ADR-0139: the host's callsign, a reference the
    // adapter made, the window, the dial and the position (null for none, or Aether sends its own
    // grid); Aether's `beacon-sent` comes back with the host's reference
    let (mut b, mut a) = opened_beaconing();
    let hello = a.asked.iter().position(|r| r["method"] == "capabilities");
    assert!(hello.is_some());
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"beacon","id":5,"ref":"k1","dial_hz":7101000,
            "position":{"lat":34.4621,"lon":-83.7514,"precision":"grid6"}}"#,
    );
    assert_eq!(lines(&said), [json!({"op": "ok", "id": 5})]);
    let asked = a.asked.last().expect("asked").clone();
    assert_eq!(asked["method"], "beacon");
    assert_eq!(asked["params"]["callsign"], "KK4ODA");
    assert_eq!(asked["params"]["within_s"], 120);
    assert_eq!(asked["params"]["dial_hz"], 7_101_000);
    assert_eq!(asked["params"]["position"]["precision"], "grid6");
    let theirs = asked["params"]["ref"]
        .as_str()
        .expect("a reference")
        .to_owned();
    assert_ne!(
        theirs, "k1",
        "the adapter's own, so two programs' never meet"
    );

    // a second while the first waits
    let said = line(&mut b, &mut a, r#"{"op":"beacon","id":6}"#);
    assert_eq!(lines(&said)[0]["code"], "busy");

    let said = event(
        &mut b,
        &mut a,
        "beacon-sent",
        &json!({"ref": theirs, "callsign": "KK4ODA", "sent": true, "dial_hz": 7_101_000,
                "reason": null}),
    );
    assert_eq!(
        lines(&said),
        [json!({"op": "beacon-sent", "ref": "k1", "sent": true, "dial_hz": 7_101_000})]
    );
    // another program's beacon is not this host's
    let said = event(
        &mut b,
        &mut a,
        "beacon-sent",
        &json!({"ref": null, "callsign": "N2CALL", "sent": true, "dial_hz": 7_101_000}),
    );
    assert_eq!(lines(&said), Vec::<Value>::new());

    // none given: Aether is given none, and puts its own grid square in
    let said = line(&mut b, &mut a, r#"{"op":"beacon","id":7,"within_s":30}"#);
    assert_eq!(lines(&said), [json!({"op": "ok", "id": 7})]);
    let asked = a.asked.last().expect("asked").clone();
    assert!(asked["params"].get("position").is_none(), "{asked}");
    assert_eq!(asked["params"]["within_s"], 30);
    let theirs = asked["params"]["ref"]
        .as_str()
        .expect("a reference")
        .to_owned();
    let said = event(
        &mut b,
        &mut a,
        "beacon-sent",
        &json!({"ref": theirs, "callsign": "KK4ODA", "sent": false,
                "reason": "the channel stayed busy"}),
    );
    assert_eq!(lines(&said), [json!({"op": "beacon-sent", "sent": false})]);
    // the operator chose to say nothing: Aether is told so, or it would send its own grid square
    let said = line(
        &mut b,
        &mut a,
        r#"{"op":"beacon","id":10,"modem_position":false}"#,
    );
    assert_eq!(lines(&said), [json!({"op": "ok", "id": 10})]);
    let asked = a.asked.last().expect("asked").clone();
    assert_eq!(asked["params"]["position"], Value::Null);
    let theirs = asked["params"]["ref"]
        .as_str()
        .expect("a reference")
        .to_owned();
    event(
        &mut b,
        &mut a,
        "beacon-sent",
        &json!({"ref": theirs, "callsign": "KK4ODA", "sent": true}),
    );

    // what Aether refuses, in RPMI's words
    for (aether, rpmi) in [
        ("claimed", "not-authorized"),
        ("not_idle", "busy"),
        ("refused", "refused"),
    ] {
        a.beacon_refused = Some(aether);
        let said = line(&mut b, &mut a, r#"{"op":"beacon","id":8}"#);
        assert_eq!(lines(&said)[0]["code"], rpmi, "{aether}");
    }
    // and what RP-4 refuses before Aether is asked
    a.beacon_refused = None;
    let said = line(&mut b, &mut a, r#"{"op":"beacon","id":9,"as":"N2CALL"}"#);
    assert_eq!(lines(&said)[0]["code"], "not-authorized");
}

#[test]
fn the_panels_repeating_beacon_is_said_and_a_beacon_waits_for_aether() {
    let (mut b, mut a) = opened(true, false);
    a.beacon_every_s = Some(900.0);
    let said = line(&mut b, &mut a, r#"{"op":"status","id":4}"#);
    assert_eq!(lines(&said)[0]["beaconing"], 900);
    a.beacon_every_s = None;
    let said = line(&mut b, &mut a, r#"{"op":"status","id":5}"#);
    assert!(lines(&said)[0].get("beaconing").is_none());
    let said = line(&mut b, &mut a, r#"{"op":"beacon","id":6}"#);
    assert_eq!(lines(&said)[0]["code"], "unsupported");
}
