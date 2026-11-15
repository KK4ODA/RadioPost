// SPDX-License-Identifier: MIT OR Apache-2.0
//! A forwarding gateway's tests (the outline's §3.3, RP-7): F-01 to F-06. The tester plays the
//! gateway's partners, on the test adapter: it answers the gateway's forwarding calls as a partner
//! would (RP-7 §2), calls it as a partner with records of its own, and plays the users whose
//! sessions make the records. The gateway is cut off by cutting the test post office's link.
//!
//! Calls the gateway places by itself (F-01, F-04, F-05) follow its partners' windows, read from
//! the test post office's gateway list, and wait for them: those tests run with `--slow`. The
//! others have the gateway call when its operator asks (the driver's `forward`), as an attended
//! station may at any time (RP-7 §5).

use std::path::Path;
use std::time::{Duration, Instant};

use radiopost::ContentId;
use radiopost::cbor::Value;
use radiopost::credential;
use radiopost::forwarding::{Placed, Proof, record_id};
use radiopost::message::{Compression, Message};
use radiopost::pdu::kind;
use radiopost::statement::{Form, Statement};
use serde_json::{Value as Json, json};

use crate::gateway::{
    GATEWAY, Hello, OTHER, Run, answer, assemble, code, done, expect_welcome, kind_of, mail,
    msg_pieces, name_of, receipt, turn, wait_long,
};
use crate::harness::{Outcome, Peer, Running, TestAdapter, new_key, now, random};
use crate::office::{TestOffice, User};

/// The partner the gateway under test calls, played by the tester.
const PARTNER: &str = OTHER;
/// A second partner of the gateway's, played by the tester.
const PARTNER2: &str = "N4CALL-10";
/// A gateway further away, where the users of the records the tester forwards called.
const FAR: &str = "N3CALL-10";
/// A 40 m dial whose Aether signal lies inside the US segment 7 100 000 to 7 105 000 Hz (RP-7
/// §5.1): it occupies 7 101 240 to 7 103 760 Hz (RP-7 §5).
const INSIDE_HZ: u64 = 7_101_000;
/// A 40 m dial whose signal runs past that segment's top, to 7 106 260 Hz.
const OUTSIDE_HZ: u64 = 7_103_500;

/// RP-7 §5.1's table for the United States, as the specification states it: where a gateway
/// may publish a window and call in one.
const US_SEGMENTS: &[(u64, u64)] = &[
    (3_585_000, 3_600_000),
    (7_100_000, 7_105_000),
    (10_140_000, 10_150_000),
    (14_095_000, 14_099_500),
    (14_100_500, 14_112_000),
    (18_105_000, 18_110_000),
    (21_090_000, 21_100_000),
    (24_925_000, 24_930_000),
    (28_120_000, 28_189_000),
    (50_100_000, 54_000_000),
    (144_100_000, 148_000_000),
    (222_000_000, 225_000_000),
    (430_000_000, 450_000_000),
];

/// What Aether's 2 300 Hz signal occupies of the audio, by the wider reading of §97.3(a)(8)
/// (RP-7 §5).
const AETHER_2300: (u64, u64) = (240, 2760);

/// A window on `dial_hz` (USB, Aether at 2 300 Hz) that opens `from_now` seconds from now and
/// lasts `length_s`, once a day (RP-7 §5).
fn window(dial_hz: u64, from_now: u64, length_s: u64) -> Json {
    let every = 86_400;
    json!({
        "channel": {"dial_hz": dial_hz, "mode": "USB", "modem": "aether", "bandwidth_hz": 2300,
                    "occupied_hz": [dial_hz + AETHER_2300.0, dial_hz + AETHER_2300.1]},
        "every_s": every,
        "at_s": (now() + from_now) % every,
        "length_s": length_s,
    })
}

/// A partner's record in the test post office's gateway list (RP-5 §4.4), with its windows.
fn record(callsign: &str, windows: &[Json]) -> Json {
    let operator = callsign.split('-').next().unwrap_or(callsign);
    json!({"callsign": callsign, "operator": operator, "hours": "always", "internet": true,
           "forwarding": windows, "updated": now()})
}

fn ids_in(v: Option<&Value>) -> Vec<Vec<u8>> {
    v.and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(|e| match e {
            Value::Array(a) => a.first().and_then(Value::as_bytes).map(<[u8]>::to_vec),
            other => other.as_bytes().map(<[u8]>::to_vec),
        })
        .collect()
}

fn coded(v: Option<&Value>) -> Vec<(Vec<u8>, u64)> {
    v.and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(|d| {
            let d = d.as_array()?;
            Some((d.first()?.as_bytes()?.to_vec(), d.get(1)?.as_u64()?))
        })
        .collect()
}

fn named(codes: &[(Vec<u8>, u64)]) -> Vec<(String, String)> {
    codes
        .iter()
        .map(|(id, c)| (radiopost::cbor::hex(id), name_of(*c)))
        .collect()
}

/// A forward item the tester holds for the gateway: its forwarding serial, its message and its
/// placement (RP-7 §4.3).
struct Item {
    serial: u64,
    message: Message,
    placement: Vec<u8>,
}

impl Item {
    fn entry(&self) -> Value {
        let air = self.message.air_form(Compression::None);
        Value::Array(vec![
            Value::Uint(self.serial),
            Value::bytes(self.message.content_id().as_bytes()),
            Value::text(self.message.from().unwrap_or("")),
            Value::text(""),
            Value::Uint(air.len() as u64),
            Value::Uint(0),
            Value::Uint(0),
        ])
    }

    /// Its `MSG` PDUs, each with the forwarding serial and the placement (RP-7 §4.3).
    fn msgs(&self) -> Vec<Value> {
        msg_pieces(&self.message, 0, 1800)
            .into_iter()
            .map(|p| {
                let mut entries = p.as_map().map(<[_]>::to_vec).unwrap_or_default();
                entries.push((Value::Uint(5), Value::Uint(self.serial)));
                entries.push((Value::Uint(6), Value::bytes(&self.placement)));
                Value::Map(entries)
            })
            .collect()
    }
}

/// How the tester answers the gateway's call as its partner `me` (RP-7 §2).
struct Partnering<'a> {
    me: &'a str,
    /// `WELCOME` `online`.
    online: bool,
    items: Vec<Item>,
}

/// What the gateway sent, calling the tester as a partner.
struct Forwarded {
    hello: Value,
    proofs: Vec<Proof>,
    /// The messages it sent whole: their content IDs.
    messages: Vec<Vec<u8>>,
    /// Its `RECEIPT` of the forward items: `complete` and `failed`.
    complete: Vec<Vec<u8>>,
    failed: Vec<(Vec<u8>, u64)>,
}

fn partner_welcome(offers: &[Vec<u8>], waiting: Vec<Value>, online: bool) -> Value {
    Value::int_map(vec![
        (0, Value::Uint(kind::WELCOME)),
        (1, Value::Uint(1)),
        (2, Value::bytes(&random::<8>())),
        (3, Value::Uint(0)),
        (
            5,
            Value::Array(
                offers
                    .iter()
                    .map(|id| Value::Array(vec![Value::bytes(id), Value::Uint(0)]))
                    .collect(),
            ),
        ),
        (6, Value::Array(Vec::new())),
        (7, Value::Array(waiting)),
        (8, Value::Uint(0)),
        (
            9,
            Value::int_map(vec![
                (1, Value::Array(vec![Value::Uint(0), Value::Uint(1)])),
                (2, Value::Uint(4096)),
            ]),
        ),
        (10, Value::Uint(now())),
        (11, Value::Bool(online)),
        (12, Value::Bool(false)),
    ])
}

/// The tester, listening as `p.me`, answers the gateway's call within `within` as a partner
/// would: it accepts every offer, sends the forward items it holds, and takes each record as
/// RP-7 §3.3 says (its path ending with the caller and not holding the partner, every message of
/// it arrived), failing the messages of the others `refused-by-policy`.
fn answer_call(peer: &mut Peer, p: &Partnering<'_>, within: Duration) -> Result<Forwarded, String> {
    peer.wait_link(within)?;
    let hello = peer.expect("the gateway's HELLO")?;
    if kind_of(&hello) != kind::HELLO {
        return Err(format!("{} where HELLO was due", hello.diagnostic()));
    }
    if hello.get(2).and_then(Value::as_u64) != Some(1) {
        return Err("HELLO `role` is not 1, a gateway forwarding (RP-7 §2)".into());
    }
    let text = |k| {
        hello
            .get(k)
            .and_then(Value::as_text)
            .unwrap_or("")
            .to_owned()
    };
    if text(4) != GATEWAY || text(5) != p.me {
        return Err(format!(
            "HELLO names {} calling {}, not {GATEWAY} calling {} (RP-7 §2: the gateway \
             credential's callsign)",
            text(4),
            text(5),
            p.me
        ));
    }
    let offers = ids_in(hello.get(8));
    let mut out = vec![partner_welcome(
        &offers,
        p.items.iter().map(Item::entry).collect(),
        p.online,
    )];
    for i in &p.items {
        out.extend(i.msgs());
    }
    out.push(done());
    peer.send(&out);
    let pdus = turn(peer, "the gateway's PROOF, MSG, RECEIPT and DONE")?;
    let mut proofs = Vec::new();
    for pdu in pdus.iter().filter(|q| kind_of(q) == kind::PROOF) {
        proofs.push(
            Proof::from_value(pdu)
                .map_err(|e| format!("a PROOF that is not one ({e:?}): RP-7 §3.2"))?,
        );
    }
    let mut messages = Vec::new();
    for id in &offers {
        let Some(cid) = ContentId::from_slice(id) else {
            continue;
        };
        let (air, _) = assemble(&pdus, cid, 0)?;
        if !air.is_empty() {
            messages.push(id.clone());
        }
    }
    let r = pdus.iter().find(|q| kind_of(q) == kind::RECEIPT);
    let complete = ids_in(r.and_then(|r| r.get(1)));
    let failed = coded(r.and_then(|r| r.get(3)));
    if !p.items.is_empty() && r.is_none() {
        return Err("no RECEIPT for the forward items it was sent (RP-3 §5.5, §5.7)".into());
    }
    if !proofs.is_empty() || !messages.is_empty() {
        peer.send(&[partner_receipt(&proofs, &messages, p.me)]);
    }
    peer.expect_hang_up(Duration::from_secs(30))?;
    Ok(Forwarded {
        hello,
        proofs,
        messages,
        complete,
        failed,
    })
}

/// The partner `me`'s `RECEIPT` (RP-7 §3.3, as far as the tester needs it): a record taken when
/// its path ends with the caller and does not hold the partner, and every message of it arrived;
/// the messages of the others failed `refused-by-policy`, those no record covers `not-authorized`.
fn partner_receipt(proofs: &[Proof], messages: &[Vec<u8>], me: &str) -> Value {
    let mut take = Vec::new();
    let mut whole = Vec::new();
    let mut refused = Vec::new();
    for proof in proofs {
        let listed: Vec<Vec<u8>> = Statement::decode(&proof.statement)
            .map(|s| s.messages.iter().map(|m| m.as_bytes().to_vec()).collect())
            .unwrap_or_default();
        let path_ok = proof.path.last().map(String::as_str) == Some(GATEWAY)
            && !proof.path.iter().any(|c| c == me);
        if path_ok && listed.iter().all(|m| messages.contains(m)) {
            take.push(proof.id());
            whole.extend(listed);
        } else {
            refused.extend(listed);
        }
    }
    let complete: Vec<Value> = messages
        .iter()
        .filter(|m| whole.contains(m))
        .map(|m| Value::bytes(m))
        .collect();
    let failed: Vec<Value> = messages
        .iter()
        .filter(|m| !whole.contains(m))
        .map(|m| {
            let c = if refused.contains(m) {
                "refused-by-policy"
            } else {
                "not-authorized"
            };
            Value::Array(vec![Value::bytes(m), Value::Uint(code(c))])
        })
        .collect();
    Value::int_map(vec![
        (0, Value::Uint(kind::RECEIPT)),
        (1, Value::Array(complete)),
        (3, Value::Array(failed)),
        (
            4,
            Value::Array(take.iter().map(|id| Value::bytes(id)).collect()),
        ),
    ])
}

/// A user's session at the gateway: `hello`'s offers handed in, any mail it sends receipted. The
/// gateway's session ID and its `WELCOME`.
fn hand_in(peer: &mut Peer, hello: &Hello<'_>) -> Result<(Vec<u8>, Value), String> {
    peer.call_as_within(GATEWAY, &hello.user.operator, Duration::from_secs(20))?;
    peer.send(&[hello.pdu()]);
    let pdus = turn(peer, "WELCOME and DONE")?;
    let session = expect_welcome(&pdus[0], 0)?;
    let accepted = ids_in(pdus[0].get(5));
    for m in &hello.offers {
        if !accepted.contains(&m.content_id().as_bytes().to_vec()) {
            return Err(format!(
                "WELCOME did not accept the new message {} (declined {:?})",
                m.content_id().text(),
                named(&coded(pdus[0].get(6)))
            ));
        }
    }
    let mut got: Vec<ContentId> = Vec::new();
    for p in pdus.iter().filter(|p| kind_of(p) == kind::MSG) {
        if let Some(id) = p
            .get(1)
            .and_then(Value::as_bytes)
            .and_then(ContentId::from_slice)
            && !got.contains(&id)
        {
            got.push(id);
        }
    }
    let mut mine: Vec<Value> = hello
        .offers
        .iter()
        .flat_map(|m| msg_pieces(m, 0, 900))
        .collect();
    if !got.is_empty() {
        mine.push(receipt(&got));
    }
    mine.push(done());
    peer.send(&mine);
    if !hello.offers.is_empty() {
        let pdus = answer(peer, "a RECEIPT for the messages handed in")?;
        let r = pdus
            .iter()
            .find(|p| kind_of(p) == kind::RECEIPT)
            .ok_or("no RECEIPT for the messages handed in (RP-3 §5.5)")?;
        let complete = ids_in(r.get(1));
        for m in &hello.offers {
            if !complete.contains(&m.content_id().as_bytes().to_vec()) {
                return Err("the RECEIPT does not list a message handed in complete".into());
            }
        }
    }
    peer.hang_up();
    Ok((session, pdus[0].clone()))
}

/// A user offers `m` again: what the gateway's `WELCOME` declined, with codes.
fn offer_again(peer: &mut Peer, user: &User, m: &Message) -> Result<Vec<(Vec<u8>, u64)>, String> {
    let mut hello = Hello::new(user, GATEWAY);
    hello.offers = vec![m];
    peer.call_as_within(GATEWAY, &user.operator, Duration::from_secs(20))?;
    peer.send(&[hello.pdu()]);
    let pdus = turn(peer, "WELCOME and DONE")?;
    expect_welcome(&pdus[0], 0)?;
    let declined = coded(pdus[0].get(6));
    let accepted = ids_in(pdus[0].get(5));
    let mut mine = Vec::new();
    if accepted.contains(&m.content_id().as_bytes().to_vec()) {
        mine.extend(msg_pieces(m, 0, 900));
    }
    mine.push(done());
    peer.send(&mine);
    if accepted.contains(&m.content_id().as_bytes().to_vec()) {
        answer(peer, "a RECEIPT")?;
    }
    peer.hang_up();
    Ok(declined)
}

/// The gateway's keys and revocation list fetched, so it can check users while cut off.
///
/// The request is logged before it is answered, so the answer has still to reach the gateway,
/// with whatever it fetches next: the cut waits until the gateway has asked nothing for two
/// seconds (ten at most). Cut at once, the answer was lost on a busy machine, and the gateway
/// rightly had no keys to check users with.
fn ready(office: &TestOffice) -> Outcome {
    if !wait_long(office, Duration::from_secs(60), |e| {
        e.method == "GET" && e.path == "/revocations"
    }) {
        return Err(
            "the gateway did not fetch its post office's revocation list (RP-1 §7.3)".into(),
        );
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = office.exchanges().len();
    let mut since = Instant::now();
    while Instant::now() < deadline && since.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(100));
        let n = office.exchanges().len();
        if n != seen {
            seen = n;
            since = Instant::now();
        }
    }
    Ok(())
}

/// The gateway's partners' records read from the post office's gateway list (RP-5 §4.4, RP-7 §5).
fn listed(office: &TestOffice) -> Outcome {
    if wait_long(office, Duration::from_secs(60), |e| {
        e.method == "GET" && e.path == "/gateways"
    }) {
        Ok(())
    } else {
        Err(
            "the gateway, with partners, did not fetch its post office's gateway list, where \
             their windows are (RP-5 §4.4, RP-7 §5)"
                .into(),
        )
    }
}

/// The calls the gateway asked its modem for to `partner`, each seen at a wall-clock second and
/// an instant: those the modem placed, and those it refused `busy`, which the gateway should not
/// have asked for (RP-7 §5, RP-4 §5).
struct Calls {
    seen: Vec<(usize, u64, Instant)>,
}

impl Calls {
    fn new() -> Calls {
        Calls { seen: Vec::new() }
    }

    fn look(&mut self, adapter: &TestAdapter, partner: &str) {
        for (at, _) in adapter.calls_to(partner) {
            if !self.seen.iter().any(|(s, _, _)| *s == at) {
                self.seen.push((at, now(), Instant::now()));
            }
        }
    }

    fn between(&self, from: u64, to: u64) -> Vec<u64> {
        self.seen
            .iter()
            .filter(|(_, t, _)| (from..to).contains(t))
            .map(|(_, t, _)| *t)
            .collect()
    }
}

/// A user's record as the tester forwards it, from the gateway `FAR`'s session: its statement
/// signed by `user`, listing `m`.
fn record_of(user: &User, m: &Message, path: &[&str]) -> Proof {
    let started = now() - 120;
    let statement = Statement {
        address: user.address.clone(),
        operator: user.operator.clone(),
        gateway: FAR.into(),
        messages: vec![m.content_id()],
        form: Form::Time {
            time: started,
            nonce: random(),
        },
    };
    Proof {
        credential: Some(user.credential.clone()),
        credential_hash: None,
        statement: statement.encode(),
        signature: statement.sign(&user.key).to_vec(),
        started,
        session: random(),
        path: path.iter().map(|c| (*c).to_owned()).collect(),
        pickups: Vec::new(),
        local: Vec::new(),
    }
}

/// What the gateway's `PROOF` of `hello`'s session should hold (RP-7 §3.1, §3.2): the user's
/// statement and signature exactly, the session the gateway gave it, a path of the gateway alone.
fn check_own_proof(proof: &Proof, hello: &Hello<'_>, session: &[u8]) -> Outcome {
    let statement = hello.statement(Form::Time {
        time: hello.time,
        nonce: hello.nonce,
    });
    if proof.statement != statement.encode() {
        return Err(
            "the PROOF's statement is not the user's statement's encoding (RP-7 §3.1)".into(),
        );
    }
    if proof.signature != statement.sign(&hello.user.key).to_vec() {
        return Err("the PROOF's signature is not the user's (RP-7 §3.1)".into());
    }
    if proof.session != session {
        return Err(
            "the PROOF's session is not the session ID the gateway gave the user in WELCOME \
             (RP-7 §3.1, RP-5 §5.2)"
                .into(),
        );
    }
    if proof.started.abs_diff(hello.time) > 120 {
        return Err(format!(
            "the PROOF's `started` {} is not when the user's session started (about {})",
            proof.started, hello.time
        ));
    }
    if proof.path != [GATEWAY] {
        return Err(format!(
            "the PROOF's path is {:?}: the gateway the user called, which holds it, alone was due \
             (RP-7 §3.1)",
            proof.path
        ));
    }
    Ok(())
}

impl Run<'_> {
    /// The partners named in the gateway's settings, through its driver.
    fn partners(&self, home: &Path, partners: &[&str]) -> Outcome {
        for p in partners {
            self.driver.run(
                "partner",
                &[("home", &home.to_string_lossy()), ("partner", p)],
                "",
            )?;
        }
        Ok(())
    }

    /// The gateway calls `partner` now, as its operator asks (the driver's `forward`).
    fn forward(&self, home: &Path, partner: &str) -> Result<Running, String> {
        self.driver.start(
            "forward",
            &[("home", &home.to_string_lossy()), ("partner", partner)],
        )
    }

    /// F-01: calls in a partner's window only, three tries a minute apart at most, none while
    /// the channel is busy (RP-7 §5). The partner's window opens 90 seconds into the test, once
    /// the gateway is cut off, and lasts five minutes; the channel is busy for its first 45
    /// seconds; nobody answers.
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn f01(&mut self, _transcript: &mut Vec<String>) -> Outcome {
        let office = TestOffice::start(&self.offices)?;
        let adapter = TestAdapter::new(4096)?;
        let (from_now, length) = (90, 300);
        let open = now() + from_now;
        office.office().state.gateways.insert(
            PARTNER.into(),
            record(PARTNER, &[window(INSIDE_HZ, from_now, length)]),
        );
        let home = self.prepare("F-01", &office, adapter.program_at, GATEWAY, (&[], false))?;
        self.partners(&home, &[PARTNER])?;
        let _gw = self.serve(&home)?;
        listed(&office)?;
        office.cut();
        let (busy_from, busy_until, close) = (open - 5, open + 45, open + length);
        let mut calls = Calls::new();
        let mut busy = None;
        let mut busied = false;
        while now() < close + 40 {
            let t = now();
            if !busied && t >= busy_from {
                busy = Some(adapter.busy()?);
                busied = true;
            }
            if t >= busy_until {
                busy = None;
            }
            calls.look(&adapter, PARTNER);
            std::thread::sleep(Duration::from_millis(200));
        }
        drop(busy);
        let before = calls.between(0, open);
        if !before.is_empty() {
            return Err(format!(
                "it called {PARTNER} {} s before its window opened (RP-7 §5)",
                open - before[0]
            ));
        }
        let while_busy = calls.between(open, busy_until - 1);
        if !while_busy.is_empty() {
            return Err(format!(
                "it asked its modem to call {PARTNER} {} s into the window, on a channel the \
                 modem reported busy (RP-7 §5, RP-4 §5)",
                while_busy[0] - open
            ));
        }
        let inside = calls.between(open, close);
        if inside.is_empty() {
            return Err(format!(
                "cut off, it never called {PARTNER} in its window ({length} s, the channel clear \
                 after the first 45) (RP-7 §5)"
            ));
        }
        if inside.len() > 3 {
            return Err(format!(
                "{} calls in one window: at most three, the first and two more (RP-7 §5)",
                inside.len()
            ));
        }
        for pair in calls.seen.windows(2) {
            let gap = pair[1].2.duration_since(pair[0].2);
            if gap < Duration::from_secs(59) {
                return Err(format!(
                    "tries {:.0} s apart: a minute apart at least (RP-7 §5)",
                    gap.as_secs_f64()
                ));
            }
        }
        let after = calls.between(close + 1, u64::MAX);
        if !after.is_empty() {
            return Err(format!(
                "it called {PARTNER} {} s after its window closed (RP-7 §5)",
                after[0] - close
            ));
        }
        Ok(())
    }

    /// F-02: records sent as `PROOF` and checked (RP-7 §3.2, §3.3). Cut off, the gateway takes a
    /// user's message; the tester, as a partner, calls it with nine records and messages, eight
    /// of them wrong in one way each, and checks each refusal's code; called by the gateway, it
    /// checks the user's `PROOF` (the statement, signature, session, a path of the gateway
    /// alone, the credential), then, a second record of the same user later, the credential by
    /// its hash; and the record taken from the tester goes on to another partner with the
    /// gateway at the end of its path.
    ///
    /// # Errors
    ///
    /// The first deviation found.
    #[allow(clippy::too_many_lines, reason = "one outage, in order")]
    pub fn f02(&mut self, transcript: &mut Vec<String>) -> Outcome {
        let office = TestOffice::start(&self.offices)?;
        let adapter = TestAdapter::new(4096)?;
        let user = office.user("shelter3@radiopost.org", "WD5EMA");
        let home = self.prepare("F-02", &office, adapter.program_at, GATEWAY, (&[], false))?;
        self.partners(&home, &[PARTNER, PARTNER2])?;
        let mut peer = adapter.peer(&[PARTNER, PARTNER2, "WD5EMA"], true)?;
        let result = (|| -> Outcome {
            let gw = self.serve(&home)?;
            ready(&office)?;
            office.cut();
            let m1 = mail(&user.address, "maria@example.com", "Power is out", 60);
            let mut h1 = Hello::new(&user, GATEWAY);
            h1.offers = vec![&m1];
            let (s1, _) = hand_in(&mut peer, &h1)?;
            let good = Self::partner_calls(&office, &mut peer)?;
            stop(gw);
            // the user's record, forwarded: in full the first time
            let run = self.forward(&home, PARTNER)?;
            let p = Partnering {
                me: PARTNER,
                online: true,
                items: Vec::new(),
            };
            let got = answer_call(&mut peer, &p, Duration::from_secs(30))?;
            finished(run)?;
            let r1 = record_id(
                &h1.statement(Form::Time {
                    time: h1.time,
                    nonce: h1.nonce,
                })
                .encode(),
            );
            let proof = got
                .proofs
                .iter()
                .find(|q| q.id() == r1)
                .ok_or("the user's session record was not sent as a PROOF (RP-7 §3.2)")?;
            check_own_proof(proof, &h1, &s1)?;
            if proof.credential.as_deref() != Some(user.credential.as_slice())
                || proof.credential_hash.is_some()
            {
                return Err(
                    "the first PROOF to this partner carries the user's credential, not its \
                     hash (RP-7 §3.2)"
                        .into(),
                );
            }
            if !got.messages.contains(&m1.content_id().as_bytes().to_vec()) {
                return Err("the user's message was not sent with its record (RP-7 §3.2)".into());
            }
            // a second record of the same user: its credential by hash
            let gw = self.serve(&home)?;
            let m2 = mail(&user.address, "maria@example.com", "Still out", 40);
            let mut h2 = Hello::new(&user, GATEWAY);
            h2.offers = vec![&m2];
            let (s2, _) = hand_in(&mut peer, &h2)?;
            stop(gw);
            let run = self.forward(&home, PARTNER)?;
            let got = answer_call(&mut peer, &p, Duration::from_secs(30))?;
            finished(run)?;
            if got.proofs.iter().any(|q| q.id() == r1) {
                return Err(
                    "a record the partner listed in `records` was sent again (RP-7 §3.5)".into(),
                );
            }
            let r2 = record_id(
                &h2.statement(Form::Time {
                    time: h2.time,
                    nonce: h2.nonce,
                })
                .encode(),
            );
            let proof = got
                .proofs
                .iter()
                .find(|q| q.id() == r2)
                .ok_or("the user's second record was not sent as a PROOF (RP-7 §3.2)")?;
            check_own_proof(proof, &h2, &s2)?;
            if proof.credential.is_some()
                || proof.credential_hash != Some(credential::hash(&user.credential))
            {
                return Err(
                    "a credential the partner took in an earlier session's records went in \
                     full again, where `credential_hash` was due (RP-7 §3.2)"
                        .into(),
                );
            }
            // the record taken from the tester goes on, the gateway at the end of its path
            let run = self.forward(&home, PARTNER2)?;
            let p2 = Partnering {
                me: PARTNER2,
                online: true,
                items: Vec::new(),
            };
            let got = answer_call(&mut peer, &p2, Duration::from_secs(30))?;
            finished(run)?;
            let proof = got.proofs.iter().find(|q| q.id() == good).ok_or(
                "the record it took from a partner while cut off was not forwarded to its \
                     other partner (RP-7 §3.4)",
            )?;
            if proof.path != [FAR, PARTNER, GATEWAY] {
                return Err(format!(
                    "the record taken from {PARTNER} went on with the path {:?}, where \
                     [{FAR}, {PARTNER}, {GATEWAY}] was due (RP-7 §3.4)",
                    proof.path
                ));
            }
            if proof.credential.is_none() {
                return Err(
                    "the first PROOF to another partner went by the credential's hash, which \
                     it never held (RP-7 §3.2)"
                        .into(),
                );
            }
            if got.proofs.iter().any(|q| q.id() == r1 || q.id() == r2) {
                return Err("records a partner took were sent again (RP-7 §3.5)".into());
            }
            Ok(())
        })();
        transcript.append(&mut peer.transcript);
        result
    }

    /// F-02's second part: the tester calls the gateway as a partner (RP-7 §2) with one good
    /// record and eight wrong ones, each with a message; the gateway refuses each wrong one with
    /// its code (RP-7 §3.3) and takes the good one. Its ID.
    #[allow(clippy::too_many_lines, reason = "the cases, then the checks")]
    fn partner_calls(office: &TestOffice, peer: &mut Peer) -> Result<[u8; 12], String> {
        let key = new_key();
        let me = User {
            address: "operator@radiopost.org".into(),
            operator: PARTNER.into(),
            credential: office.gateway_credential(PARTNER, &key.verifying_key().to_bytes()),
            key,
        };
        let users: Vec<User> = (0..9)
            .map(|i| office.user(&format!("user{i}@radiopost.org"), "N5CALL"))
            .collect();
        let messages: Vec<Message> = users
            .iter()
            .map(|u| mail(&u.address, "maria@example.com", "Water", 30))
            .collect();
        let long: Vec<String> = std::iter::once(FAR.to_owned())
            .chain((1..=7).map(|i| format!("N5CALL-{i}")))
            .chain(std::iter::once(PARTNER.to_owned()))
            .collect();
        let long: Vec<&str> = long.iter().map(String::as_str).collect();
        let mut cases: Vec<(Option<Proof>, &str)> = Vec::new();
        // the credential and its hash both
        let mut p = record_of(&users[0], &messages[0], &[FAR, PARTNER]);
        p.credential_hash = Some(credential::hash(&users[0].credential));
        cases.push((Some(p), "malformed"));
        // a hash of a credential it never held
        let mut p = record_of(&users[1], &messages[1], &[FAR, PARTNER]);
        p.credential = None;
        p.credential_hash = Some(credential::hash(&users[1].credential));
        cases.push((Some(p), "credential-required"));
        // a signature that is not the statement's
        let mut p = record_of(&users[2], &messages[2], &[FAR, PARTNER]);
        p.signature = statement_of(&p).map_or_else(Vec::new, |mut s| {
            s.address = "someone@radiopost.org".into();
            s.sign(&users[2].key).to_vec()
        });
        cases.push((Some(p), "signature-invalid"));
        // a path that does not start at the gateway the user called
        let p = record_of(&users[3], &messages[3], &["N9CALL-9", PARTNER]);
        cases.push((Some(p), "refused-by-policy"));
        // a path that does not end with the caller
        let p = record_of(&users[4], &messages[4], &[FAR, "N9CALL-9"]);
        cases.push((Some(p), "refused-by-policy"));
        // a loop: the path holds the gateway itself
        let p = record_of(&users[5], &messages[5], &[FAR, GATEWAY, PARTNER]);
        cases.push((Some(p), "refused-by-policy"));
        // nine callsigns
        let p = record_of(&users[6], &messages[6], &long);
        cases.push((Some(p), "refused-by-policy"));
        // a message no record covers
        cases.push((None, "not-authorized"));
        // the good one
        let p = record_of(&users[8], &messages[8], &[FAR, PARTNER]);
        let good = p.id();
        cases.push((Some(p), ""));
        let mut hello = Hello::new(&me, GATEWAY);
        hello.role = 1;
        hello.offers = messages.iter().collect();
        peer.call_as_within(GATEWAY, PARTNER, Duration::from_secs(20))?;
        peer.send(&[hello.pdu()]);
        let pdus = turn(peer, "WELCOME and DONE")?;
        expect_welcome(&pdus[0], 0)?;
        let accepted = ids_in(pdus[0].get(5));
        if accepted.len() != messages.len() {
            return Err(format!(
                "WELCOME accepted {} of the {} new messages a partner offered, declining {:?}",
                accepted.len(),
                messages.len(),
                named(&coded(pdus[0].get(6)))
            ));
        }
        let mut mine: Vec<Value> = cases
            .iter()
            .filter_map(|(p, _)| p.as_ref().map(Proof::to_value))
            .collect();
        for m in &messages {
            mine.extend(msg_pieces(m, 0, 900));
        }
        mine.push(done());
        peer.send(&mine);
        let pdus = answer(peer, "a RECEIPT for the records and messages")?;
        let r = pdus
            .iter()
            .find(|p| kind_of(p) == kind::RECEIPT)
            .ok_or("no RECEIPT for the records a partner forwarded (RP-7 §3.3)")?;
        peer.hang_up();
        let complete = ids_in(r.get(1));
        let failed = coded(r.get(3));
        let records = ids_in(r.get(4));
        for ((_, want), m) in cases.iter().zip(&messages) {
            let id = m.content_id().as_bytes().to_vec();
            if want.is_empty() {
                if !complete.contains(&id) {
                    return Err(format!(
                        "the good record's message is not `complete` (RP-7 §3.3); failed {:?}",
                        named(&failed)
                    ));
                }
                continue;
            }
            let got = failed.iter().find(|(f, _)| *f == id).map(|(_, c)| *c);
            if got != Some(code(want)) {
                return Err(format!(
                    "a message of a record that is {want} listed as {}, where failed `{want}` \
                     was due (RP-7 §3.3)",
                    got.map_or_else(|| "complete or not at all".into(), name_of)
                ));
            }
        }
        if records != [good.to_vec()] {
            return Err(format!(
                "RECEIPT `records` lists {} records: the good one alone was due (RP-7 §3.3)",
                records.len()
            ));
        }
        Ok(good)
    }

    /// F-03: placements checked in order, each with its code (RP-7 §4.3). Called by the gateway,
    /// the tester sends seven forward items for a user of the gateway's: a placement that does
    /// not decode, one signed by a key no post office has, one with a bad signature and for
    /// another message, one for another message, one for another gateway and expired, one
    /// expired, and a good one. The gateway lists each failed with the first check's code,
    /// takes the good one, and serves it as a copy placed ahead; its next call's `cursor` is
    /// the last forwarding serial.
    ///
    /// # Errors
    ///
    /// The first deviation found.
    #[allow(clippy::too_many_lines, reason = "the cases, then the checks")]
    pub fn f03(&mut self, transcript: &mut Vec<String>) -> Outcome {
        let office = TestOffice::start(&self.offices)?;
        let adapter = TestAdapter::new(4096)?;
        let user = office.user("shelter3@radiopost.org", "N3CALL");
        let home = self.prepare("F-03", &office, adapter.program_at, GATEWAY, (&[], false))?;
        self.partners(&home, &[PARTNER])?;
        let mut peer = adapter.peer(&[PARTNER, "N3CALL"], true)?;
        let result = (|| -> Outcome {
            let gw = self.serve(&home)?;
            ready(&office)?;
            stop(gw);
            office.cut();
            let key = self
                .offices
                .key_of(crate::office::DOMAIN)
                .ok_or("the test keys have no key for the test post office")?;
            let at = now();
            let m: Vec<Message> = (0..7)
                .map(|i| mail("ana@radiopost.org", &user.address, &format!("Item {i}"), 25))
                .collect();
            let placed = |i: usize, serial: u64| Placed {
                address: user.address.clone(),
                serial,
                id: m[i].content_id(),
                gateway: GATEWAY.into(),
                expires: at + 7 * 86_400,
            };
            let mut flipped = Placed {
                id: m[0].content_id(),
                ..placed(2, 3)
            }
            .sign(key);
            if let Some(last) = flipped.last_mut() {
                *last ^= 1;
            }
            let cases: Vec<(Vec<u8>, &str)> = vec![
                (vec![0xa1, 0x01], "malformed"),
                (placed(1, 2).sign(&new_key()), "signature-invalid"),
                (flipped, "signature-invalid"),
                (
                    Placed {
                        id: m[0].content_id(),
                        ..placed(3, 4)
                    }
                    .sign(key),
                    "not-authorized",
                ),
                (
                    Placed {
                        gateway: "N9CALL-9".into(),
                        expires: at - 60,
                        ..placed(4, 5)
                    }
                    .sign(key),
                    "not-authorized",
                ),
                (
                    Placed {
                        expires: at - 60,
                        ..placed(5, 6)
                    }
                    .sign(key),
                    "refused-by-policy",
                ),
                (placed(6, 11).sign(key), ""),
            ];
            let items: Vec<Item> = cases
                .iter()
                .zip(&m)
                .zip(1..)
                .map(|(((placement, _), message), serial)| Item {
                    serial,
                    message: message.clone(),
                    placement: placement.clone(),
                })
                .collect();
            let run = self.forward(&home, PARTNER)?;
            let p = Partnering {
                me: PARTNER,
                online: true,
                items,
            };
            let got = answer_call(&mut peer, &p, Duration::from_secs(30))?;
            finished(run)?;
            if got.hello.get(9).and_then(Value::as_u64) != Some(0) {
                return Err("the first call's HELLO `cursor` is not 0 (RP-7 §2)".into());
            }
            for ((_, want), message) in cases.iter().zip(&m) {
                let id = message.content_id().as_bytes().to_vec();
                if want.is_empty() {
                    if !got.complete.contains(&id) {
                        return Err(format!(
                            "the forward item with a good placement is not `complete` \
                             (RP-7 §4.3); failed {:?}",
                            named(&got.failed)
                        ));
                    }
                    continue;
                }
                let c = got.failed.iter().find(|(f, _)| *f == id).map(|(_, c)| *c);
                if c != Some(code(want)) {
                    return Err(format!(
                        "a forward item whose placement should fail `{want}` listed as {} \
                         (RP-7 §4.3: the checks in order)",
                        c.map_or_else(|| "complete or not at all".into(), name_of)
                    ));
                }
            }
            // the next call says how far it got (RP-7 §2 `cursor`)
            let run = self.forward(&home, PARTNER)?;
            let none = Partnering {
                me: PARTNER,
                online: true,
                items: Vec::new(),
            };
            let again = answer_call(&mut peer, &none, Duration::from_secs(30))?;
            finished(run)?;
            let cursor = again.hello.get(9).and_then(Value::as_u64);
            if cursor != Some(7) {
                return Err(format!(
                    "the next call's HELLO `cursor` is {cursor:?}: 7, the highest forwarding \
                     serial it received, was due (RP-7 §2)"
                ));
            }
            // the good one held as a copy placed ahead for the user (RP-7 §4.3, RP-5 §5.8)
            let _gw = self.serve(&home)?;
            let hello = Hello::new(&user, GATEWAY);
            let (_, welcome) = hand_in(&mut peer, &hello)?;
            let listed: Vec<(u64, Vec<u8>, u64)> = welcome
                .get(7)
                .and_then(Value::as_array)
                .unwrap_or(&[])
                .iter()
                .filter_map(|e| {
                    let e = e.as_array()?;
                    Some((
                        e.first()?.as_u64()?,
                        e.get(1)?.as_bytes()?.to_vec(),
                        e.get(6)?.as_u64()?,
                    ))
                })
                .collect();
            let good = m[6].content_id().as_bytes().to_vec();
            if !listed
                .iter()
                .any(|(s, id, f)| *s == 11 && *id == good && f & 2 != 0)
            {
                return Err(format!(
                    "the forward item taken is not served to its user as a copy placed ahead \
                     with the placement's serial 11 and the flag `copy` (RP-7 §4.3): listed \
                     {:?}",
                    listed.iter().map(|(s, _, f)| (s, f)).collect::<Vec<_>>()
                ));
            }
            let others: Vec<Vec<u8>> = m[..6]
                .iter()
                .map(|x| x.content_id().as_bytes().to_vec())
                .collect();
            if listed.iter().any(|(_, id, _)| others.contains(id)) {
                return Err("a forward item whose placement failed is served as a copy".into());
            }
            Ok(())
        })();
        transcript.append(&mut peer.transcript);
        result
    }

    /// F-04: records kept for a partner that reaches the post offices (RP-7 §6.2). Two
    /// gateways, each with two partners, both called once before: the first said `online` false
    /// and its window opens half a minute after the gateway starts, by when it is cut off and its
    /// user has called; the second said `online` true. In
    /// one, the second's window opens within the hour: in the first's window the gateway calls
    /// with no offers, to collect its mail, keeping its records. In the other it opens in two
    /// hours: the gateway sends its records to the first.
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn f04(&mut self, transcript: &mut Vec<String>) -> Outcome {
        let this = &*self;
        let (a, b) = std::thread::scope(|s| {
            let a = s.spawn(|| this.f04_world("F-04a", 1800));
            let b = this.f04_world("F-04b", 7200);
            (a.join(), b)
        });
        let (a, mut ta) = a.map_err(|_| "F-04's first gateway's thread failed".to_owned())?;
        let (b, mut tb) = b;
        transcript.append(&mut ta);
        transcript.push("—".into());
        transcript.append(&mut tb);
        a.map_err(|e| format!("the online partner's window within the hour: {e}"))?;
        b.map_err(|e| format!("the online partner's window two hours away: {e}"))
    }

    fn f04_world(&self, test: &str, second_in: u64) -> (Outcome, Vec<String>) {
        let mut transcript = Vec::new();
        let result = (|| -> Outcome {
            let office = TestOffice::start(&self.offices)?;
            let adapter = TestAdapter::new(4096)?;
            let user = office.user("shelter3@radiopost.org", "WD5EMA");
            let first_in = 30;
            let home = self.prepare(test, &office, adapter.program_at, GATEWAY, (&[], false))?;
            self.partners(&home, &[PARTNER, PARTNER2])?;
            let mut peer = adapter.peer(&[PARTNER, PARTNER2, "WD5EMA"], true)?;
            let inner = (|| -> Outcome {
                // each partner called once: what its last WELCOME said
                for (who, online) in [(PARTNER, false), (PARTNER2, true)] {
                    let run = self.forward(&home, who)?;
                    let p = Partnering {
                        me: who,
                        online,
                        items: Vec::new(),
                    };
                    answer_call(&mut peer, &p, Duration::from_secs(30))?;
                    finished(run)?;
                }
                // the partners' windows, read when the gateway starts: the first opens once it
                // is cut off and its user has called
                {
                    let mut o = office.office();
                    o.state.gateways.insert(
                        PARTNER.into(),
                        record(PARTNER, &[window(INSIDE_HZ, first_in, 600)]),
                    );
                    o.state.gateways.insert(
                        PARTNER2.into(),
                        record(PARTNER2, &[window(INSIDE_HZ, second_in, 600)]),
                    );
                }
                let _gw = self.serve(&home)?;
                listed(&office)?;
                ready(&office)?;
                office.cut();
                let m = mail(&user.address, "maria@example.com", "Power is out", 60);
                let mut hello = Hello::new(&user, GATEWAY);
                hello.offers = vec![&m];
                hand_in(&mut peer, &hello)?;
                let p = Partnering {
                    me: PARTNER,
                    online: false,
                    items: Vec::new(),
                };
                let got = answer_call(&mut peer, &p, Duration::from_secs(first_in + 120))
                    .map_err(|e| format!("in {PARTNER}'s window: {e}"))?;
                let offers = ids_in(got.hello.get(8));
                let id = m.content_id().as_bytes().to_vec();
                if second_in <= 3600 {
                    if !offers.is_empty() || !got.proofs.is_empty() {
                        return Err(format!(
                            "in the window of {PARTNER}, which last said `online` false, it sent \
                             its records, though {PARTNER2}, which last said `online` true, \
                             opens a window within the hour (RP-7 §6.2)"
                        ));
                    }
                } else if !offers.contains(&id) || got.proofs.is_empty() {
                    return Err(format!(
                        "in the window of {PARTNER}, with no partner that last said `online` \
                         true opening a window within the hour, it kept its records (RP-7 §6.2)"
                    ));
                }
                Ok(())
            })();
            transcript.append(&mut peer.transcript);
            inner
        })();
        (result, transcript)
    }

    /// F-05: windows published and used only inside the country's segments (RP-7 §5.1). The
    /// gateway is given a window inside a US segment and one that runs past it; registered, its
    /// record carries the first alone, with the channel and the occupied band RP-7 §5 states.
    /// With `slow`, its partner's record has a window outside the segments open first, and one
    /// inside 90 seconds later: cut off, it calls in the second only.
    ///
    /// # Errors
    ///
    /// The first deviation found.
    #[allow(clippy::too_many_lines, reason = "published, then used")]
    pub fn f05(&mut self, _transcript: &mut Vec<String>, slow: bool) -> Outcome {
        let office = TestOffice::start(&self.offices)?;
        let adapter = TestAdapter::new(4096)?;
        let (out_in, in_in, length) = (60, 150, 300);
        let in_open = now() + in_in;
        office.office().state.gateways.insert(
            PARTNER.into(),
            record(
                PARTNER,
                &[
                    window(OUTSIDE_HZ, out_in, 600),
                    window(INSIDE_HZ, in_in, length),
                ],
            ),
        );
        let home = self.prepare("F-05", &office, adapter.program_at, GATEWAY, (&[], false))?;
        let h = home.to_string_lossy().into_owned();
        let windows = [(INSIDE_HZ, true), (OUTSIDE_HZ, false)];
        for (dial, _) in windows {
            #[allow(clippy::cast_precision_loss, reason = "a dial frequency in MHz")]
            let mhz = format!("{:.4}", dial as f64 / 1e6);
            // the window outside may be refused here, or kept and not published
            let _ = self.driver.run(
                "window",
                &[
                    ("home", &h),
                    ("mhz", &mhz),
                    ("bandwidth", "2300"),
                    ("every", "3600"),
                    ("at", "900"),
                    ("length", "600"),
                ],
                "",
            );
        }
        let operator = office.user("operator@radiopost.org", "N1CALL");
        let op_home = home.with_file_name("operator");
        std::fs::create_dir_all(&op_home).map_err(|e| e.to_string())?;
        std::fs::write(
            op_home.join("device.key"),
            radiopost::cbor::hex(&operator.key.to_bytes()),
        )
        .map_err(|e| e.to_string())?;
        std::fs::write(op_home.join("credential"), &operator.credential)
            .map_err(|e| e.to_string())?;
        self.driver.run(
            "register",
            &[("home", &h), ("operator_home", &op_home.to_string_lossy())],
            "",
        )?;
        let reg = office
            .exchanges()
            .into_iter()
            .find(|e| e.method == "POST" && e.path == "/gateways")
            .ok_or("the gateway did not register (RP-5 §6.4)")?;
        let published = reg
            .body
            .as_ref()
            .and_then(|b| b["record"]["forwarding"].as_array().cloned())
            .unwrap_or_default();
        for w in &published {
            let occ: Vec<u64> = w["channel"]["occupied_hz"]
                .as_array()
                .map(|a| a.iter().filter_map(Json::as_u64).collect())
                .unwrap_or_default();
            let inside = occ.len() == 2
                && US_SEGMENTS
                    .iter()
                    .any(|(lo, hi)| *lo <= occ[0] && occ[1] <= *hi);
            if !inside {
                return Err(format!(
                    "it published a window whose occupied band {occ:?} is not inside one of the \
                     US segments for unattended data (RP-7 §5.1)"
                ));
            }
        }
        let want = json!({"dial_hz": INSIDE_HZ, "mode": "USB", "modem": "aether",
                          "bandwidth_hz": 2300,
                          "occupied_hz": [INSIDE_HZ + AETHER_2300.0, INSIDE_HZ + AETHER_2300.1]});
        let found = published.iter().any(|w| {
            w["channel"] == want && w["every_s"] == 3600 && w["at_s"] == 900 && w["length_s"] == 600
        });
        if !found {
            return Err(format!(
                "its record does not publish its window on 7.101 MHz with the channel and \
                 occupied band RP-7 §5 states ({want}): forwarding {published:?}"
            ));
        }
        if !slow {
            return Ok(());
        }
        // used: a partner's window outside the segments is no window to call in
        self.partners(&home, &[PARTNER])?;
        let _gw = self.serve(&home)?;
        listed(&office)?;
        office.cut();
        let mut calls = Calls::new();
        while now() < in_open + 60 {
            calls.look(&adapter, PARTNER);
            std::thread::sleep(Duration::from_millis(200));
        }
        let early = calls.between(0, in_open);
        if !early.is_empty() {
            return Err(format!(
                "it called {PARTNER} {} s before its window inside the segments opened, in one \
                 that runs past them (RP-7 §5, §5.1)",
                in_open - early[0]
            ));
        }
        if calls.between(in_open, in_open + 60).is_empty() {
            return Err(format!(
                "cut off, it did not call {PARTNER} in its window inside the segments (RP-7 §5)"
            ));
        }
        Ok(())
    }

    /// F-06: mail offered again (RP-7 §3.6). Cut off, the gateway takes a user's message; offered
    /// again, it is declined `already-have` while in the queue, and again once forwarded to a
    /// partner that took it; a new message is taken.
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn f06(&mut self, transcript: &mut Vec<String>) -> Outcome {
        let office = TestOffice::start(&self.offices)?;
        let adapter = TestAdapter::new(4096)?;
        let user = office.user("shelter3@radiopost.org", "WD5EMA");
        let home = self.prepare("F-06", &office, adapter.program_at, GATEWAY, (&[], false))?;
        self.partners(&home, &[PARTNER])?;
        let mut peer = adapter.peer(&[PARTNER, "WD5EMA"], true)?;
        let result = (|| -> Outcome {
            let gw = self.serve(&home)?;
            ready(&office)?;
            office.cut();
            let m1 = mail(&user.address, "maria@example.com", "Power is out", 60);
            let mut hello = Hello::new(&user, GATEWAY);
            hello.offers = vec![&m1];
            hand_in(&mut peer, &hello)?;
            let have = vec![(m1.content_id().as_bytes().to_vec(), code("already-have"))];
            let declined = offer_again(&mut peer, &user, &m1)?;
            if declined != have {
                return Err(format!(
                    "a message in its queue offered again: declined {:?}, where `already-have` \
                     was due (RP-7 §3.6)",
                    named(&declined)
                ));
            }
            stop(gw);
            let run = self.forward(&home, PARTNER)?;
            let p = Partnering {
                me: PARTNER,
                online: true,
                items: Vec::new(),
            };
            let got = answer_call(&mut peer, &p, Duration::from_secs(30))?;
            finished(run)?;
            if !got.messages.contains(&m1.content_id().as_bytes().to_vec()) {
                return Err("the queued message was not forwarded to the partner".into());
            }
            let _gw = self.serve(&home)?;
            let declined = offer_again(&mut peer, &user, &m1)?;
            if declined != have {
                return Err(format!(
                    "a message it passed on to a partner offered again: declined {:?}, where \
                     `already-have` was due for 30 days (RP-7 §3.5, §3.6)",
                    named(&declined)
                ));
            }
            let m2 = mail(&user.address, "maria@example.com", "Water at noon", 40);
            let mut hello = Hello::new(&user, GATEWAY);
            hello.offers = vec![&m2];
            hand_in(&mut peer, &hello)
                .map_err(|e| format!("a new message, cut off: {e} (RP-7 §3.6: taken as new)"))?;
            Ok(())
        })();
        transcript.append(&mut peer.transcript);
        result
    }
}

/// The gateway stops serving, once the session just ended is stored: a session is kept when
/// its link ends, which the user's hanging up only starts.
fn stop(gw: crate::gateway::Gateway) {
    std::thread::sleep(Duration::from_secs(2));
    drop(gw);
}

/// The statement a `PROOF` carries.
fn statement_of(p: &Proof) -> Option<Statement> {
    Statement::decode(&p.statement)
}

/// The gateway's `forward` ended well.
fn finished(run: Running) -> Outcome {
    match run.finish(Duration::from_secs(30)) {
        (Some(true), _) => Ok(()),
        (Some(false), said) => Err(format!("the gateway's `forward` failed: {}", said.trim())),
        (None, said) => Err(format!(
            "the gateway's `forward` did not end within 30 s: {}",
            said.trim()
        )),
    }
}
