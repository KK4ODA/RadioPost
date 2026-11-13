// SPDX-License-Identifier: MIT OR Apache-2.0
//! The client role's tests (the outline's §3.1): C-01 to C-08 and C-11, the tester playing a
//! gateway or another client's station through the test adapter.
//!
//! The tester's side is scripted here from RP-3's tables, with the core library's encodings and
//! nothing of its session engines: a bug the two shared would pass unseen.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use radiopost::cbor::Value;
use radiopost::credential::{self, Trust, flags};
use radiopost::message::{Compression, Message, key};
use radiopost::pdu::kind;
use radiopost::statement::{self, Form, Freshness, Statement};
use radiopost::{ContentId, ErrorCode};

use crate::harness::{
    Heard, Outcome, Peer, PostOffices, Program, TestAdapter, new_key, now, random,
};

/// The gateways the tester plays.
pub const GATEWAY: &str = "N1CALL-10";
/// A second gateway, for resuming elsewhere (RP-3 §7).
pub const OTHER_GATEWAY: &str = "N2CALL-5";
/// The station the tester plays for station-to-station tests (RP-3 §10).
pub const STATION: &str = "N0CALL";
const STATION_ADDRESS: &str = "station@hampost.org";
/// The caller the tester plays when the program is called.
const CALLER: &str = "N9CALL";
const CALLER_ADDRESS: &str = "caller@radiopost.org";

fn code(name: &str) -> u64 {
    ErrorCode::from_name(name).map_or(0, |c| u64::from(c.code()))
}

fn id_of(v: &Value) -> Option<ContentId> {
    v.as_bytes().and_then(ContentId::from_slice)
}

fn ids(v: Option<&Value>) -> Vec<ContentId> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().filter_map(id_of).collect())
        .unwrap_or_default()
}

/// A message for the program's account, as a post office would hold it.
fn mail(from: &str, to: &str, subject: &str, body: &str) -> Message {
    Message::from_map(&Value::int_map(vec![
        (key::V, Value::Uint(1)),
        (key::FROM, Value::text(from)),
        (key::TO, Value::Array(vec![Value::text(to)])),
        (key::DATE, Value::Uint(now() - 3600)),
        (key::SUBJECT, Value::text(subject)),
        (key::BODY, Value::text(body)),
    ]))
    .expect("the tester's messages are messages")
}

/// A waiting entry (RP-3 §5.2).
fn entry(serial: u64, m: &Message) -> Value {
    let air = m.air_form(Compression::None);
    let subject = m
        .map()
        .get(key::SUBJECT)
        .and_then(Value::as_text)
        .unwrap_or("");
    Value::Array(vec![
        Value::Uint(serial),
        Value::bytes(m.content_id().as_bytes()),
        Value::text(m.from().unwrap_or("")),
        Value::text(subject),
        Value::Uint(air.len() as u64),
        Value::Uint(0),
        Value::Uint(0),
    ])
}

/// `n` words a compressor can do little with, different every time.
fn words(n: usize) -> String {
    const LETTERS: &[u8] = b"etaoinshrdlucmfwypvbgkqjxz";
    let seed = random::<8>();
    let mut x = u64::from_le_bytes(seed) | 1;
    let mut out = String::new();
    for _ in 0..n {
        // xorshift64
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let len = 2 + (x % 7) as usize;
        for i in 0..len {
            out.push(char::from(LETTERS[((x >> (8 + 5 * i)) % 26) as usize]));
        }
        out.push(' ');
    }
    out
}

fn caps(max_pdu: u64) -> Value {
    Value::int_map(vec![
        (1, Value::Array(vec![Value::Uint(0), Value::Uint(1)])),
        (2, Value::Uint(max_pdu)),
    ])
}

/// What a gateway's `WELCOME` `ready` says (RP-3 §5.2).
struct Ready<'a> {
    accept: Vec<(ContentId, u64)>,
    decline: Vec<(ContentId, u64)>,
    waiting: &'a [Value],
    max_pdu: u64,
    cached: bool,
    /// For a station called: its credential and the signature over its statement.
    proof: Option<(Vec<u8>, [u8; 64])>,
    time: u64,
}

fn welcome(r: &Ready<'_>) -> Value {
    let mut entries = vec![
        (0, Value::Uint(kind::WELCOME)),
        (1, Value::Uint(1)),
        (2, Value::bytes(&random::<8>())),
        (3, Value::Uint(0)),
        (
            5,
            Value::Array(
                r.accept
                    .iter()
                    .map(|(id, o)| Value::Array(vec![Value::bytes(id.as_bytes()), Value::Uint(*o)]))
                    .collect(),
            ),
        ),
        (
            6,
            Value::Array(
                r.decline
                    .iter()
                    .map(|(id, c)| Value::Array(vec![Value::bytes(id.as_bytes()), Value::Uint(*c)]))
                    .collect(),
            ),
        ),
        (7, Value::Array(r.waiting.to_vec())),
        (8, Value::Uint(0)),
        (9, caps(r.max_pdu)),
        (10, Value::Uint(r.time)),
        (11, Value::Bool(true)),
        (12, Value::Bool(r.cached)),
    ];
    if let Some((cred, sig)) = &r.proof {
        entries.push((13, Value::bytes(cred)));
        entries.push((15, Value::bytes(sig)));
    }
    Value::int_map(entries)
}

fn welcome_status(status: u64, challenge: Option<[u8; 16]>) -> Value {
    let mut entries = vec![
        (0, Value::Uint(kind::WELCOME)),
        (1, Value::Uint(1)),
        (2, Value::bytes(&random::<8>())),
        (3, Value::Uint(status)),
        (10, Value::Uint(now())),
        (11, Value::Bool(true)),
        (12, Value::Bool(false)),
    ];
    if let Some(c) = challenge {
        entries.push((4, Value::bytes(&c)));
    }
    Value::int_map(entries)
}

fn msg(id: ContentId, offset: usize, total: usize, data: &[u8], serial: Option<u64>) -> Value {
    let mut entries = vec![
        (0, Value::Uint(kind::MSG)),
        (1, Value::bytes(id.as_bytes())),
        (2, Value::Uint(offset as u64)),
        (3, Value::Uint(total as u64)),
        (4, Value::bytes(data)),
    ];
    if let Some(s) = serial {
        entries.push((5, Value::Uint(s)));
    }
    Value::int_map(entries)
}

/// A whole air form as `MSG` pieces of at most `piece` bytes of data.
fn msgs(m: &Message, from: usize, piece: usize, serial: Option<u64>) -> Vec<Value> {
    let air = m.air_form(Compression::None);
    let mut out = Vec::new();
    let mut at = from;
    while at < air.len() {
        let end = (at + piece).min(air.len());
        out.push(msg(m.content_id(), at, air.len(), &air[at..end], serial));
        at = end;
    }
    out
}

fn receipt(complete: &[ContentId], failed: &[(ContentId, u64)]) -> Value {
    Value::int_map(vec![
        (0, Value::Uint(kind::RECEIPT)),
        (
            1,
            Value::Array(
                complete
                    .iter()
                    .map(|id| Value::bytes(id.as_bytes()))
                    .collect(),
            ),
        ),
        (
            3,
            Value::Array(
                failed
                    .iter()
                    .map(|(id, c)| Value::Array(vec![Value::bytes(id.as_bytes()), Value::Uint(*c)]))
                    .collect(),
            ),
        ),
    ])
}

fn done() -> Value {
    Value::int_map(vec![(0, Value::Uint(kind::DONE))])
}

fn kind_name(k: u64) -> &'static str {
    match k {
        kind::HELLO => "HELLO",
        kind::WELCOME => "WELCOME",
        kind::AUTH => "AUTH",
        kind::MSG => "MSG",
        kind::RECEIPT => "RECEIPT",
        kind::REQUEST => "REQUEST",
        kind::DONE => "DONE",
        kind::ERROR => "ERROR",
        kind::CREDENTIAL => "CREDENTIAL",
        kind::NOTICE => "NOTICE",
        _ => "an unknown PDU",
    }
}

fn kind_of(p: &Value) -> u64 {
    p.get(0).and_then(Value::as_u64).unwrap_or(u64::MAX)
}

/// The program's `HELLO`, checked against RP-3 §5.1 and RP-1 §6.
struct Hello {
    nonce: [u8; 8],
    cursor: u64,
    offers: Vec<(ContentId, u64)>,
    auto: u64,
    partials: Vec<(ContentId, u64)>,
    max_pdu: usize,
    sent_credential: bool,
}

fn check_hello(
    p: &Value,
    prog: &Program<'_>,
    called: &str,
    trust: &Trust,
) -> Result<Hello, String> {
    if kind_of(p) != kind::HELLO {
        return Err(format!("{} where HELLO was due", kind_name(kind_of(p))));
    }
    let text = |k| p.get(k).and_then(Value::as_text).unwrap_or("").to_owned();
    let versions: Vec<u64> = p
        .get(1)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default();
    if versions.len() != 2 || !(versions[0]..=versions[1]).contains(&1) {
        return Err(format!(
            "HELLO `versions` does not take version 1: {versions:?}"
        ));
    }
    if p.get(2).and_then(Value::as_u64) != Some(0) {
        return Err("HELLO `role` is not 0, a client".into());
    }
    if text(3) != prog.address || text(4) != prog.operator {
        return Err(format!(
            "HELLO names {} ({}), not the credential's {} ({})",
            text(3),
            text(4),
            prog.address,
            prog.operator
        ));
    }
    if text(5) != called {
        return Err(format!("HELLO `gateway` is {}, not {called}", text(5)));
    }
    let nonce: [u8; 8] = p
        .get(7)
        .and_then(Value::as_bytes)
        .and_then(|b| b.try_into().ok())
        .ok_or("HELLO has no 8-byte `nonce`")?;
    let time = p
        .get(6)
        .and_then(Value::as_u64)
        .ok_or("HELLO has no `time`")?;
    let offers: Vec<(ContentId, u64)> = p
        .get(8)
        .and_then(Value::as_array)
        .ok_or("HELLO has no `offers`")?
        .iter()
        .map(|o| {
            let a = o.as_array()?;
            Some((id_of(a.first()?)?, a.get(1)?.as_u64()?))
        })
        .collect::<Option<_>>()
        .ok_or("an offer is not [id, size, priority]")?;
    let cursor = p
        .get(9)
        .and_then(Value::as_u64)
        .ok_or("HELLO has no `cursor`")?;
    let auto = p
        .get(10)
        .and_then(Value::as_u64)
        .ok_or("HELLO has no `auto`")?;
    let partials = p
        .get(11)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    let x = x.as_array()?;
                    Some((id_of(x.first()?)?, x.get(1)?.as_u64()?))
                })
                .collect()
        })
        .unwrap_or_default();
    let max_pdu = p
        .get(12)
        .and_then(|c| c.get(2))
        .and_then(Value::as_u64)
        .map_or(1024, |m| usize::try_from(m).unwrap_or(1024));
    check_hello_proof(p, prog, &offers, Form::Time { time, nonce }, called, trust)?;
    Ok(Hello {
        nonce,
        cursor,
        offers,
        auto,
        partials,
        max_pdu,
        sent_credential: p.get(13).is_some(),
    })
}

/// `HELLO`'s credential, whole or by hash, and its signature over the statement (RP-1 §5, §6.3).
fn check_hello_proof(
    p: &Value,
    prog: &Program<'_>,
    offers: &[(ContentId, u64)],
    form: Form,
    called: &str,
    trust: &Trust,
) -> Outcome {
    let cred = match (
        p.get(13).and_then(Value::as_bytes),
        p.get(14).and_then(Value::as_bytes),
    ) {
        (Some(c), _) => c.to_vec(),
        (None, Some(h)) if h == credential::hash(&prog.credential) => prog.credential.clone(),
        (None, Some(_)) => return Err("HELLO's `credential_hash` is not its credential's".into()),
        (None, None) => return Err("HELLO has neither `credential` nor `credential_hash`".into()),
    };
    let need = if offers.is_empty() { 0 } else { flags::SEND } | flags::RECEIVE;
    let body = credential::verify(&cred, trust, now(), need)
        .map_err(|r| format!("HELLO's credential fails RP-1 §5: {}", r.code))?;
    let sig = p
        .get(15)
        .and_then(Value::as_bytes)
        .ok_or("HELLO is not signed (key 15)")?;
    let stmt = Statement {
        address: prog.address.clone(),
        operator: prog.operator.clone(),
        gateway: called.into(),
        messages: offers.iter().map(|(id, _)| *id).collect(),
        form,
    };
    let fresh = Freshness {
        now: now(),
        seen: &[],
        challenge: None,
        sent_nonce: None,
    };
    statement::verify(&stmt, sig, &body, called, &fresh)
        .map_err(|c| format!("HELLO's statement fails RP-1 §6.3: {c}"))
}

/// What the program sent in one turn, up to its `DONE` (RP-3 §5.7).
#[derive(Default)]
struct Turn {
    /// Air forms, as far as they came, by content ID.
    sent: HashMap<ContentId, Vec<u8>>,
    totals: HashMap<ContentId, u64>,
    complete: Vec<ContentId>,
    failed: Vec<(ContentId, u64)>,
    request: Vec<u64>,
}

/// The program's turn, with `held` the pieces of messages the tester already holds (and their
/// totals) from an earlier session. It ends with `DONE` or `REQUEST`, or once a `RECEIPT` has
/// listed every message of `answers`: after `MSG` PDUs the other station owes only that (RP-3
/// §5.7).
fn program_turn_answering(
    peer: &mut Peer,
    max_pdu: usize,
    held: &HashMap<ContentId, (Vec<u8>, u64)>,
    answers: &[ContentId],
) -> Result<Turn, String> {
    let mut t = Turn {
        sent: held
            .iter()
            .map(|(id, (air, _))| (*id, air.clone()))
            .collect(),
        totals: held.iter().map(|(id, (_, total))| (*id, *total)).collect(),
        ..Turn::default()
    };
    loop {
        let p = match peer.next(Duration::from_secs(20), max_pdu) {
            Heard::Pdu(p) => p,
            Heard::Gone { reason } => return Err(format!("the link ended ({reason}) in its turn")),
            Heard::Silence => return Err("nothing for 20 s, and no DONE".into()),
        };
        match kind_of(&p) {
            kind::MSG => {
                let id = p.get(1).and_then(id_of).ok_or("a MSG without an `id`")?;
                let offset = p
                    .get(2)
                    .and_then(Value::as_u64)
                    .ok_or("a MSG without `offset`")?;
                let total = p
                    .get(3)
                    .and_then(Value::as_u64)
                    .ok_or("a MSG without `total`")?;
                let data = p
                    .get(4)
                    .and_then(Value::as_bytes)
                    .ok_or("a MSG without `data`")?;
                let air = t.sent.entry(id).or_default();
                if offset != air.len() as u64 {
                    return Err(format!(
                        "a MSG piece at {offset} where {} bytes were held (RP-3 §5.4)",
                        air.len()
                    ));
                }
                air.extend_from_slice(data);
                if t.totals.insert(id, total).is_some_and(|was| was != total) {
                    return Err("a MSG's `total` changed between pieces".into());
                }
            }
            kind::RECEIPT => {
                t.complete.extend(ids(p.get(1)));
                for f in p.get(3).and_then(Value::as_array).into_iter().flatten() {
                    let f = f.as_array().ok_or("a RECEIPT `failed` entry")?;
                    let id = f.first().and_then(id_of).ok_or("a RECEIPT `failed` id")?;
                    t.failed
                        .push((id, f.get(1).and_then(Value::as_u64).unwrap_or(0)));
                }
                let listed = |id: &ContentId| {
                    t.complete.contains(id) || t.failed.iter().any(|(f, _)| f == id)
                };
                if !answers.is_empty() && answers.iter().all(listed) {
                    return Ok(t);
                }
            }
            kind::REQUEST => {
                // the caller's turn ends with DONE or REQUEST (RP-3 §5.7)
                t.request.extend(
                    p.get(1)
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_u64),
                );
                return Ok(t);
            }
            kind::DONE => return Ok(t),
            kind::ERROR => {
                return Err(format!(
                    "the program sent ERROR {}",
                    p.get(1).and_then(Value::as_u64).unwrap_or(0)
                ));
            }
            other => return Err(format!("{} in the program's turn", kind_name(other))),
        }
    }
}

/// The program's turn, up to its `DONE` or `REQUEST`.
fn program_turn(
    peer: &mut Peer,
    max_pdu: usize,
    held: &HashMap<ContentId, (Vec<u8>, u64)>,
) -> Result<Turn, String> {
    program_turn_answering(peer, max_pdu, held, &[])
}

/// Every message the program sent whole is the one it offered: total, size and content ID.
fn check_sent(turn: &Turn, offers: &[(ContentId, u64)]) -> Result<Vec<ContentId>, String> {
    let mut whole = Vec::new();
    for (id, air) in &turn.sent {
        let size = offers
            .iter()
            .find(|(o, _)| o == id)
            .map(|(_, s)| *s)
            .ok_or("a MSG for a message not offered")?;
        let total = turn.totals.get(id).copied().unwrap_or(0);
        if total != size {
            return Err(format!(
                "a MSG `total` of {total} for an offer of {size} bytes"
            ));
        }
        if air.len() as u64 == total {
            let m = Message::from_air(air)
                .map_err(|e| format!("a message sent is not RP-2: {}", e.0))?;
            if m.content_id() != *id {
                return Err("a message sent does not have the content ID offered (RP-2 §3)".into());
            }
            whole.push(*id);
        }
    }
    Ok(whole)
}

fn contains(listing: &str, id: ContentId) -> bool {
    listing.contains(&id.text()[..6])
}

/// A bad frame, given the program's `max_pdu`.
type BadFrame = fn(usize) -> Vec<u8>;

/// The runner: the program's driver, the test keys and where each test's home goes.
pub struct Run<'a> {
    /// The program.
    pub driver: &'a crate::harness::Driver,
    /// The test post offices.
    pub offices: PostOffices,
    /// Scratch space for each test's home.
    pub scratch: std::path::PathBuf,
}

impl Run<'_> {
    fn setup(&mut self, test: &str, frame: usize) -> Result<(TestAdapter, Program<'_>), String> {
        let adapter = TestAdapter::new(frame)?;
        let program = Program::setup(
            self.driver,
            &self.scratch,
            test,
            &mut self.offices,
            adapter.program_at,
        )?;
        Ok((adapter, program))
    }

    /// C-01: an ordinary session (RP-3 §3, §5, §8).
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn c01(&mut self, peer_out: &mut Vec<String>) -> Outcome {
        let trust = self.offices.trust();
        let (adapter, prog) = self.setup("C-01", 37)?;
        prog.compose(
            "maria@example.com",
            "From the program",
            "A message the program sends.",
        )?;
        let mut peer = adapter.peer(&[GATEWAY], true)?;
        let running = prog.call(GATEWAY, false)?;
        let result = (|| {
            peer.wait_link(Duration::from_secs(20))?;
            let hello = check_hello(&peer.expect("HELLO")?, &prog, GATEWAY, &trust)?;
            if hello.offers.len() != 1 {
                return Err(format!(
                    "{} offers for the one message in its outbox",
                    hello.offers.len()
                ));
            }
            let small = mail(
                "maria@example.com",
                &prog.address,
                "Are you safe?",
                "All well here?",
            );
            let long = "A longer message, asked for rather than sent unasked. ".repeat(40);
            let big = mail("eoc@radiopost.org", &prog.address, "Shelter roster", &long);
            let waiting = [entry(419, &small), entry(420, &big)];
            let unasked: Vec<&Message> = [&small, &big]
                .into_iter()
                .filter(|m| m.air_form(Compression::None).len() as u64 <= hello.auto)
                .collect();
            let mut first = vec![welcome(&Ready {
                accept: hello.offers.iter().map(|(id, _)| (*id, 0)).collect(),
                decline: vec![],
                waiting: &waiting,
                max_pdu: 4096,
                cached: true,
                proof: None,
                time: now(),
            })];
            for m in &unasked {
                let serial = if m.content_id() == small.content_id() {
                    419
                } else {
                    420
                };
                first.extend(msgs(m, 0, 900, Some(serial)));
            }
            first.push(done());
            peer.send(&first);
            let turn = program_turn(&mut peer, 4096, &HashMap::new())?;
            let whole = check_sent(&turn, &hello.offers)?;
            if whole.len() != hello.offers.len() {
                return Err("not every accepted offer was sent whole".into());
            }
            for m in &unasked {
                if !turn.complete.contains(&m.content_id()) {
                    return Err("a message sent unasked is not in the RECEIPT's `complete`".into());
                }
            }
            for s in &turn.request {
                if *s != 419 && *s != 420 {
                    return Err(format!("a REQUEST for serial {s}, which was not listed"));
                }
            }
            let mut second = vec![receipt(&whole, &[])];
            let mut asked = Vec::new();
            for (serial, m) in [(419, &small), (420, &big)] {
                if turn.request.contains(&serial) {
                    second.extend(msgs(m, 0, 900, Some(serial)));
                    asked.push(m.content_id());
                }
            }
            second.push(done());
            peer.send(&second);
            if !asked.is_empty() {
                let last = program_turn(&mut peer, 4096, &HashMap::new())?;
                for id in &asked {
                    if !last.complete.contains(id) {
                        return Err("a message asked for is not in the last RECEIPT".into());
                    }
                }
            }
            peer.expect_hang_up(Duration::from_secs(30))?;
            let inbox = prog.folder("inbox")?;
            for m in unasked.iter().map(|m| m.content_id()).chain(asked) {
                if !contains(&inbox, m) {
                    return Err(format!("{} received is not in the inbox", m.text()));
                }
            }
            let outbox = prog.folder("outbox")?;
            if whole.iter().any(|id| contains(&outbox, *id)) {
                return Err("a message the gateway took stays in the outbox".into());
            }
            Ok(())
        })();
        let (_, said) = running.finish(Duration::from_secs(10));
        peer_out.extend(peer.transcript);
        result.map_err(|e| format!("{e}\nthe program said: {}", said.trim()))
    }

    /// C-02: a challenge answered with `AUTH` (RP-3 §5.3, RP-1 §6.1).
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn c02(&mut self, peer_out: &mut Vec<String>) -> Outcome {
        let trust = self.offices.trust();
        let (adapter, prog) = self.setup("C-02", 4096)?;
        prog.compose("maria@example.com", "Challenged", "Sent after a challenge.")?;
        let mut peer = adapter.peer(&[GATEWAY], true)?;
        let running = prog.call(GATEWAY, false)?;
        let result = (|| {
            peer.wait_link(Duration::from_secs(20))?;
            let hello = check_hello(&peer.expect("HELLO")?, &prog, GATEWAY, &trust)?;
            let challenge = random::<16>();
            peer.send(&[welcome_status(1, Some(challenge))]);
            let auth = peer.expect("AUTH")?;
            if kind_of(&auth) != kind::AUTH {
                return Err(format!("{} where AUTH was due", kind_name(kind_of(&auth))));
            }
            let sig = auth
                .get(1)
                .and_then(Value::as_bytes)
                .ok_or("AUTH without a signature")?;
            let body = credential::verify(&prog.credential, &trust, now(), 0)
                .map_err(|r| r.code.to_string())?;
            let stmt = Statement {
                address: prog.address.clone(),
                operator: prog.operator.clone(),
                gateway: GATEWAY.into(),
                messages: hello.offers.iter().map(|(id, _)| *id).collect(),
                form: Form::Challenge(challenge),
            };
            let fresh = Freshness {
                now: now(),
                seen: &[],
                challenge: Some(challenge),
                sent_nonce: None,
            };
            statement::verify(&stmt, sig, &body, GATEWAY, &fresh)
                .map_err(|c| format!("AUTH's signature is not over the challenge form: {c}"))?;
            finish(&mut peer, &hello)
        })();
        let (_, said) = running.finish(Duration::from_secs(10));
        peer_out.extend(peer.transcript);
        result.map_err(|e| format!("{e}\nthe program said: {}", said.trim()))
    }

    /// C-03: the credential by hash on a later visit, and in full when asked (RP-3 §5.1, §5.8).
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn c03(&mut self, peer_out: &mut Vec<String>) -> Outcome {
        let trust = self.offices.trust();
        let (adapter, prog) = self.setup("C-03", 4096)?;
        let mut result = Ok(());
        for visit in 1..=2 {
            let mut peer = adapter.peer(&[GATEWAY], true)?;
            let running = prog.call(GATEWAY, false)?;
            let r = (|| {
                peer.wait_link(Duration::from_secs(20))?;
                let hello = check_hello(&peer.expect("HELLO")?, &prog, GATEWAY, &trust)?;
                if visit == 1 && !hello.sent_credential {
                    return Err(
                        "a first visit sent the credential's hash, not the credential".into(),
                    );
                }
                if visit == 2 {
                    if hello.sent_credential {
                        return Err(
                            "the gateway said `cached`, and the next visit sent it whole".into(),
                        );
                    }
                    peer.send(&[welcome_status(2, None)]);
                    let c = peer.expect("CREDENTIAL")?;
                    if kind_of(&c) != kind::CREDENTIAL
                        || c.get(1).and_then(Value::as_bytes) != Some(prog.credential.as_slice())
                    {
                        return Err(format!(
                            "{} where its CREDENTIAL was due",
                            kind_name(kind_of(&c))
                        ));
                    }
                }
                finish(&mut peer, &hello)
            })();
            let (_, said) = running.finish(Duration::from_secs(10));
            peer_out.extend(std::mem::take(&mut peer.transcript));
            if let Err(e) = r {
                result = Err(format!(
                    "visit {visit}: {e}\nthe program said: {}",
                    said.trim()
                ));
                break;
            }
        }
        result
    }

    /// C-04: an upload and a download broken at chosen bytes and resumed, at the same gateway and
    /// another; the same air form kept (RP-3 §7).
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn c04(&mut self, peer_out: &mut Vec<String>) -> Outcome {
        let trust = self.offices.trust();
        let (adapter, prog) = self.setup("C-04", 61)?;
        // text DEFLATE cannot shrink below a few PDUs, so the upload breaks mid-message
        prog.compose("eoc@radiopost.org", "Street report", &words(500))?;
        let photo = "Pixels, as text, so it compresses no differently. ".repeat(60);
        let mut resume = Resume {
            download: mail("maria@example.com", &prog.address, "A long one", &photo),
            held_up: HashMap::new(),
            offered: None,
        };
        // visit 1: the upload breaks after its first piece; visit 2, at another gateway: it resumes,
        // and the download breaks after 600 bytes; visit 3: the download resumes
        for (visit, gateway) in [(1, GATEWAY), (2, OTHER_GATEWAY), (3, GATEWAY)] {
            let mut peer = adapter.peer(&[GATEWAY, OTHER_GATEWAY], true)?;
            let running = prog.call(gateway, false)?;
            let r = (|| {
                peer.wait_link(Duration::from_secs(20))?;
                let hello = check_hello(&peer.expect("HELLO")?, &prog, gateway, &trust)?;
                match visit {
                    1 => resume.break_upload(&mut peer, &hello),
                    2 => resume.resume_upload(&mut peer, &hello),
                    _ => resume.resume_download(&mut peer, &hello, &prog),
                }
            })();
            let (_, said) = running.finish(Duration::from_secs(15));
            peer_out.extend(std::mem::take(&mut peer.transcript));
            r.map_err(|e| format!("visit {visit}: {e}\nthe program said: {}", said.trim()))?;
        }
        Ok(())
    }

    /// C-09: a message a cut-off gateway takes is queued, not sent: it stays in the outbox, is not
    /// offered to that gateway again while it was cut off at the last call, is offered to
    /// another, and is sent once an online gateway declines it `already-have` (RP-7 §3.6).
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn c09(&mut self, peer_out: &mut Vec<String>) -> Outcome {
        let trust = self.offices.trust();
        let (adapter, prog) = self.setup("C-09", 4096)?;
        prog.compose("maria@example.com", "Queued", "Taken by a cut-off gateway.")?;
        let mut queued = None;
        // visit 1: N1CALL-10, cut off, takes it; visit 2: N1CALL-10 again, still cut off; visit
        // 3: N2CALL-5, online, whose post office already holds it
        for (visit, gateway) in [(1, GATEWAY), (2, GATEWAY), (3, OTHER_GATEWAY)] {
            let mut peer = adapter.peer(&[GATEWAY, OTHER_GATEWAY], true)?;
            let running = prog.call(gateway, false)?;
            let r = (|| -> Outcome {
                peer.wait_link(Duration::from_secs(20))?;
                let hello = check_hello(&peer.expect("HELLO")?, &prog, gateway, &trust)?;
                let offered: Vec<ContentId> = hello.offers.iter().map(|(id, _)| *id).collect();
                match visit {
                    1 => {
                        let [a] = offered[..] else {
                            return Err(format!("{} offers for one message", offered.len()));
                        };
                        queued = Some(a);
                        peer.send(&[
                            with_online(&gateway_ready(vec![(a, 0)], &[], 4096), false),
                            done(),
                        ]);
                        let turn = program_turn(&mut peer, 4096, &HashMap::new())?;
                        let whole = check_sent(&turn, &hello.offers)?;
                        peer.send(&[receipt(&whole, &[]), done()]);
                        peer.expect_hang_up(Duration::from_secs(30))?;
                        if !contains(&prog.folder("outbox")?, a) {
                            return Err("a message a cut-off gateway took (`online` false) left \
                                        the outbox: it is queued there, not sent (RP-3 §5.5, \
                                        RP-7 §3.6)"
                                .into());
                        }
                        Ok(())
                    }
                    2 => {
                        if queued.is_some_and(|a| offered.contains(&a)) {
                            return Err("the message was offered again to the gateway that \
                                        queued it, cut off at the last call, within 30 days \
                                        (RP-7 §3.6)"
                                .into());
                        }
                        let mut w = with_online(&gateway_ready(vec![], &[], 4096), false);
                        if !offered.is_empty() {
                            w = with_online(
                                &gateway_ready(
                                    offered.iter().map(|id| (*id, 0)).collect(),
                                    &[],
                                    4096,
                                ),
                                false,
                            );
                        }
                        peer.send(&[w, done()]);
                        let turn = program_turn(&mut peer, 4096, &HashMap::new())?;
                        let whole = check_sent(&turn, &hello.offers)?;
                        peer.send(&[receipt(&whole, &[]), done()]);
                        peer.expect_hang_up(Duration::from_secs(30))
                    }
                    _ => {
                        let a = queued.ok_or("nothing was queued")?;
                        if !offered.contains(&a) {
                            return Err("the queued message was not offered again to another \
                                        gateway (RP-7 §3.6)"
                                .into());
                        }
                        let w = welcome(&Ready {
                            accept: vec![],
                            decline: vec![(a, code("already-have"))],
                            waiting: &[],
                            max_pdu: 4096,
                            cached: true,
                            proof: None,
                            time: now(),
                        });
                        peer.send(&[w, done()]);
                        let turn = program_turn(&mut peer, 4096, &HashMap::new())?;
                        if turn.sent.contains_key(&a) {
                            return Err("a MSG for an offer declined (RP-3 §5.2)".into());
                        }
                        peer.send(&[receipt(&[], &[]), done()]);
                        peer.expect_hang_up(Duration::from_secs(30))?;
                        if contains(&prog.folder("outbox")?, a) {
                            return Err("an online gateway declined it `already-have`, so the \
                                        post office holds it, yet it stays in the outbox \
                                        (RP-7 §3.6)"
                                .into());
                        }
                        Ok(())
                    }
                }
            })();
            let (_, said) = running.finish(Duration::from_secs(15));
            peer_out.extend(std::mem::take(&mut peer.transcript));
            r.map_err(|e| format!("visit {visit}: {e}\nthe program said: {}", said.trim()))?;
        }
        Ok(())
    }

    /// C-05: offers declined and messages failed are not sent, and stay in the outbox (RP-3 §5.2,
    /// §5.4, §5.9).
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn c05(&mut self, peer_out: &mut Vec<String>) -> Outcome {
        let trust = self.offices.trust();
        let (adapter, prog) = self.setup("C-05", 4096)?;
        for n in 1..=3 {
            prog.compose(
                "maria@example.com",
                &format!("Number {n}"),
                &format!("Message {n}."),
            )?;
        }
        let mut peer = adapter.peer(&[GATEWAY], true)?;
        let running = prog.call(GATEWAY, false)?;
        let result = (|| {
            peer.wait_link(Duration::from_secs(20))?;
            let hello = check_hello(&peer.expect("HELLO")?, &prog, GATEWAY, &trust)?;
            let [(a, _), (b, _), (c, _)] = hello.offers[..] else {
                return Err(format!("{} offers for three messages", hello.offers.len()));
            };
            peer.send(&[
                welcome(&Ready {
                    accept: vec![(a, 0)],
                    decline: vec![(b, code("too-large")), (c, code("refused-by-policy"))],
                    waiting: &[],
                    max_pdu: 4096,
                    cached: true,
                    proof: None,
                    time: now(),
                }),
                done(),
            ]);
            let turn = program_turn(&mut peer, 4096, &HashMap::new())?;
            if turn.sent.keys().any(|id| *id != a) {
                return Err("a MSG for an offer declined (RP-3 §5.2)".into());
            }
            check_sent(&turn, &hello.offers)?;
            peer.send(&[receipt(&[], &[(a, code("malformed"))]), done()]);
            peer.expect_hang_up(Duration::from_secs(30))?;
            let outbox = prog.folder("outbox")?;
            for id in [a, b, c] {
                if !contains(&outbox, id) {
                    return Err("a message declined or failed left the outbox".into());
                }
            }
            Ok(())
        })();
        let (_, said) = running.finish(Duration::from_secs(10));
        peer_out.extend(peer.transcript);
        result.map_err(|e| format!("{e}\nthe program said: {}", said.trim()))
    }

    /// C-06: bad frames from the gateway answered `ERROR` `malformed`, a PDU over the program's
    /// `max_pdu` `ERROR` `too-large`, and the session ended (RP-3 §2, §4).
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn c06(&mut self, peer_out: &mut Vec<String>) -> Outcome {
        let trust = self.offices.trust();
        let (adapter, prog) = self.setup("C-06", 4096)?;
        let cases: [(&str, &str, BadFrame); 3] = [
            ("a PDU of an unknown type", "malformed", |_| {
                radiopost::pdu::frame(&Value::int_map(vec![(0, Value::Uint(99))]))
                    .unwrap_or_default()
            }),
            ("a frame that is not CBOR", "malformed", |_| {
                vec![0, 2, 0xff, 0xff]
            }),
            ("a frame over the program's max_pdu", "too-large", |max| {
                let n = max + 1;
                let mut f = vec![
                    u8::try_from(n >> 8).unwrap_or(0xff),
                    u8::try_from(n & 0xff).unwrap_or(0),
                ];
                f.extend(std::iter::repeat_n(0xa0, n));
                f
            }),
        ];
        for (what, due, bad) in cases {
            let mut peer = adapter.peer(&[GATEWAY], true)?;
            let running = prog.call(GATEWAY, false)?;
            let r = (|| {
                peer.wait_link(Duration::from_secs(20))?;
                let hello = check_hello(&peer.expect("HELLO")?, &prog, GATEWAY, &trust)?;
                peer.send_raw(&bad(hello.max_pdu));
                let e = peer.expect("ERROR")?;
                if kind_of(&e) != kind::ERROR || e.get(1).and_then(Value::as_u64) != Some(code(due))
                {
                    return Err(format!(
                        "{} {} where ERROR `{due}` was due",
                        kind_name(kind_of(&e)),
                        e.get(1)
                            .and_then(Value::as_u64)
                            .map(|c| c.to_string())
                            .unwrap_or_default()
                    ));
                }
                peer.expect_hang_up(Duration::from_secs(30))
            })();
            let (_, said) = running.finish(Duration::from_secs(10));
            peer_out.extend(std::mem::take(&mut peer.transcript));
            r.map_err(|e| format!("{what}: {e}\nthe program said: {}", said.trim()))?;
        }
        Ok(())
    }

    /// C-07: calling a station: its proof checked before anything is taken; a bad one answered
    /// `ERROR` and the call ended; only mail for its account sent (RP-3 §10, RP-1 §6.5).
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn c07(&mut self, peer_out: &mut Vec<String>) -> Outcome {
        let trust = self.offices.trust();
        let station_key = new_key();
        let station_cred = self.offices.issue(
            STATION_ADDRESS,
            STATION,
            &station_key.verifying_key().to_bytes(),
        );
        let (adapter, prog) = self.setup("C-07", 53)?;
        prog.compose(STATION_ADDRESS, "For the station", "Yours.")?;
        prog.compose(
            "eoc@radiopost.org",
            "For someone else",
            "Not the station's to take.",
        )?;
        let note = mail(
            STATION_ADDRESS,
            &prog.address,
            "From the station",
            "Hello from N0CALL.",
        );
        for (honest, what) in [
            (false, "a proof over another nonce"),
            (true, "a good proof"),
        ] {
            let mut peer = adapter.peer(&[STATION], true)?;
            let running = prog.call(STATION, true)?;
            let r = (|| {
                peer.wait_link(Duration::from_secs(20))?;
                let hello = check_hello(&peer.expect("HELLO")?, &prog, STATION, &trust)?;
                if hello.cursor != 0 {
                    return Err(format!(
                        "`cursor` {} to a station, not 0 (RP-3 §10)",
                        hello.cursor
                    ));
                }
                let time = now();
                let waiting = [entry(1, &note)];
                let stmt = Statement {
                    address: STATION_ADDRESS.into(),
                    operator: STATION.into(),
                    gateway: prog.operator.clone(),
                    messages: vec![note.content_id()],
                    form: Form::Time {
                        time,
                        nonce: if honest { hello.nonce } else { random() },
                    },
                };
                let mut out = vec![welcome(&Ready {
                    accept: hello.offers.iter().map(|(id, _)| (*id, 0)).collect(),
                    decline: vec![],
                    waiting: &waiting,
                    max_pdu: 4096,
                    cached: false,
                    proof: Some((station_cred.clone(), stmt.sign(&station_key))),
                    time,
                })];
                if note.air_form(Compression::None).len() as u64 <= hello.auto {
                    out.extend(msgs(&note, 0, 4000, Some(1)));
                }
                out.push(done());
                peer.send(&out);
                if !honest {
                    let e = peer.expect("ERROR")?;
                    if kind_of(&e) != kind::ERROR {
                        return Err(format!(
                            "{} answered a station whose proof is not over the caller's nonce",
                            kind_name(kind_of(&e))
                        ));
                    }
                    return peer.expect_hang_up(Duration::from_secs(30));
                }
                let turn = program_turn(&mut peer, 4096, &HashMap::new())?;
                let whole = check_sent(&turn, &hello.offers)?;
                for id in turn.sent.keys() {
                    let m = Message::from_air(&turn.sent[id]).map_err(|e| e.0.to_string())?;
                    if !m.recipients().contains(&STATION_ADDRESS) {
                        return Err("mail for another account sent to a station (RP-3 §10)".into());
                    }
                }
                if whole.is_empty() {
                    return Err("the station's own mail was not sent".into());
                }
                let mut tail = vec![receipt(&whole, &[])];
                if note.air_form(Compression::None).len() as u64 > hello.auto
                    && turn.request.contains(&1)
                {
                    tail.extend(msgs(&note, 0, 4000, Some(1)));
                }
                tail.push(done());
                peer.send(&tail);
                if tail.len() > 2 {
                    program_turn(&mut peer, 4096, &HashMap::new())?;
                }
                peer.expect_hang_up(Duration::from_secs(30))
            })();
            let (_, said) = running.finish(Duration::from_secs(10));
            peer_out.extend(std::mem::take(&mut peer.transcript));
            r.map_err(|e| format!("{what}: {e}\nthe program said: {}", said.trim()))?;
        }
        Ok(())
    }

    /// C-08: called by a station: its own proof given, only mail for the caller's account listed,
    /// only mail for its own account taken (RP-3 §10, RP-1 §6.5).
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn c08(&mut self, peer_out: &mut Vec<String>) -> Outcome {
        let trust = self.offices.trust();
        let caller_key = new_key();
        let caller_cred = self.offices.issue(
            CALLER_ADDRESS,
            CALLER,
            &caller_key.verifying_key().to_bytes(),
        );
        let (adapter, prog) = self.setup("C-08", 4096)?;
        prog.compose(CALLER_ADDRESS, "For the caller", "Waiting for you.")?;
        prog.compose("eoc@radiopost.org", "For someone else", "Not the caller's.")?;
        let mine = mail(CALLER_ADDRESS, &prog.address, "For the program", "To you.");
        let theirs = mail(
            CALLER_ADDRESS,
            "someone@radiopost.org",
            "Not yours",
            "For someone else.",
        );
        let running = prog.listen()?;
        let mut peer = adapter.peer(&[CALLER], false)?;
        let result = (|| {
            peer.call_within(&prog.operator, Duration::from_secs(30))?;
            let nonce = random::<8>();
            let offers = [&mine, &theirs];
            peer.send(&[caller_hello(
                &prog.operator,
                &offers,
                nonce,
                &caller_cred,
                &caller_key,
            )]);
            let w = peer.expect("WELCOME")?;
            let Station { accepted, waiting } = check_station_welcome(&w, &prog, &trust, nonce)?;
            for m in offers {
                if !accepted.iter().any(|(id, _)| *id == m.content_id()) {
                    return Err(
                        "an offer not accepted: a station called cannot tell whom it is \
                                for, so it takes what it has room for (RP-3 §10)"
                            .into(),
                    );
                }
            }
            if waiting.len() != 1 {
                return Err(format!(
                    "{} messages listed for the caller, where one is for its account",
                    waiting.len()
                ));
            }
            // within the caller's `auto`, so sent after WELCOME unasked (RP-3 §5.2)
            let turn = program_turn(&mut peer, 4096, &HashMap::new())?;
            let listed: Vec<(ContentId, u64)> =
                waiting.iter().map(|(_, id, size)| (*id, *size)).collect();
            let got = check_sent(&turn, &listed)?;
            if got != [waiting[0].1] {
                return Err(
                    "the message waiting, within the caller's `auto`, was not sent after \
                            WELCOME (RP-3 §5.2)"
                        .into(),
                );
            }
            let mut out = Vec::new();
            for m in offers {
                let from = accepted
                    .iter()
                    .find(|(id, _)| *id == m.content_id())
                    .map_or(0, |(_, o)| usize::try_from(*o).unwrap_or(0));
                out.extend(msgs(m, from, 4000, None));
            }
            out.push(receipt(&got, &[]));
            out.push(done());
            peer.send(&out);
            let sent: Vec<ContentId> = offers.iter().map(|m| m.content_id()).collect();
            let last = program_turn_answering(&mut peer, 4096, &HashMap::new(), &sent)?;
            check_station_receipt(&last, &mine, &theirs)?;
            peer.hang_up();
            Ok(())
        })();
        let (ended, said) = running.finish(Duration::from_secs(15));
        peer_out.extend(peer.transcript);
        result.map_err(|e| format!("{e}\nthe program said: {}", said.trim()))?;
        if ended != Some(true) {
            return Err(format!(
                "the program did not end well after the caller hung up: {}",
                said.trim()
            ));
        }
        let inbox = prog.folder("inbox")?;
        if !contains(&inbox, mine.content_id()) || contains(&inbox, theirs.content_id()) {
            return Err("the inbox does not hold exactly the mail for its account".into());
        }
        Ok(())
    }

    /// C-12: a station-to-station session reported over the Internet (RP-5 §6.6): called by the
    /// tester's station, the program carries a session, then reports it to the test post office,
    /// with both proofs and the messages that crossed.
    ///
    /// # Errors
    ///
    /// The first deviation found.
    #[allow(
        clippy::too_many_lines,
        reason = "the session, then the report, in order"
    )]
    pub fn c12(&mut self, peer_out: &mut Vec<String>) -> Outcome {
        let trust = self.offices.trust();
        let office = crate::office::TestOffice::start(&self.offices)?;
        let caller_key = new_key();
        let caller_cred = self.offices.issue(
            CALLER_ADDRESS,
            CALLER,
            &caller_key.verifying_key().to_bytes(),
        );
        let (adapter, prog) = self.setup("C-12", 4096)?;
        prog.compose(CALLER_ADDRESS, "For the caller", "Waiting for you.")?;
        let mine = mail(CALLER_ADDRESS, &prog.address, "For the program", "To you.");
        let running = prog.listen()?;
        let mut peer = adapter.peer(&[CALLER], false)?;
        let nonce = random::<8>();
        let hello = caller_hello(&prog.operator, &[&mine], nonce, &caller_cred, &caller_key);
        let result = (|| {
            peer.call_within(&prog.operator, Duration::from_secs(30))?;
            peer.send(std::slice::from_ref(&hello));
            let w = peer.expect("WELCOME")?;
            let Station { accepted, waiting } = check_station_welcome(&w, &prog, &trust, nonce)?;
            let turn = program_turn(&mut peer, 4096, &HashMap::new())?;
            let listed: Vec<(ContentId, u64)> =
                waiting.iter().map(|(_, id, size)| (*id, *size)).collect();
            let got = check_sent(&turn, &listed)?;
            let from = accepted
                .iter()
                .find(|(id, _)| *id == mine.content_id())
                .map_or(0, |(_, o)| usize::try_from(*o).unwrap_or(0));
            let mut out = msgs(&mine, from, 4000, None);
            out.push(receipt(&got, &[]));
            out.push(done());
            peer.send(&out);
            let _ = program_turn_answering(&mut peer, 4096, &HashMap::new(), &[mine.content_id()])?;
            peer.hang_up();
            Ok((w, got))
        })();
        let (_, said) = running.finish(Duration::from_secs(15));
        peer_out.extend(peer.transcript);
        let (welcome, got) =
            result.map_err(|e: String| format!("{e}\nthe program said: {}", said.trim()))?;
        let home = prog.home.to_string_lossy().into_owned();
        prog.driver
            .run("post_office", &[("home", &home), ("url", &office.base)], "")?;
        let out = prog.driver.run("report", &[("home", &home)], "")?;
        peer_out.push(format!("the program reported: {}", out.trim()));
        let log = office.exchanges();
        let direct = log
            .iter()
            .find(|e| e.method == "POST" && e.path == "/direct")
            .ok_or("the program sent no report to its post office (RP-5 §6.6)")?;
        peer_out.push(format!(
            "POST /direct → {} {}",
            direct.status,
            direct.answer.clone().unwrap_or_default()
        ));
        let body = direct.body.clone().unwrap_or_default();
        let sessions = body["sessions"].as_array().cloned().unwrap_or_default();
        let [session] = sessions.as_slice() else {
            return Err(format!(
                "{} sessions reported, where one crossed the air",
                sessions.len()
            ));
        };
        let b64 = |v: &serde_json::Value| {
            v.as_str()
                .and_then(radiopost::base64url::decode)
                .unwrap_or_default()
        };
        if session["role"] != "called" {
            return Err("`role` is not `called`, the program's part (RP-5 §6.6)".into());
        }
        if session["started"].as_u64() != welcome.get(10).and_then(Value::as_u64) {
            return Err(
                "`started` is not the station called's clock as WELCOME gave it (RP-5 §6.6)".into(),
            );
        }
        // theirs: the caller's statement as HELLO gave it, its signature and credential
        let theirs = &session["theirs"];
        let stmt = Statement::decode(&b64(&theirs["statement"]))
            .ok_or("`theirs.statement` is not a statement")?;
        let hello_sig = hello
            .get(15)
            .and_then(Value::as_bytes)
            .map(<[u8]>::to_vec)
            .unwrap_or_default();
        if stmt.address != CALLER_ADDRESS
            || stmt.nonce() != Some(nonce)
            || b64(&theirs["signature"]) != hello_sig
            || b64(&theirs["credential"]) != caller_cred
        {
            return Err("`theirs` is not the caller's statement, signature and credential from HELLO (RP-5 §6.6)".into());
        }
        // mine: the program's own statement over the caller's nonce (RP-1 §6.5)
        let mine_proof = &session["mine"];
        let own = Statement::decode(&b64(&mine_proof["statement"]))
            .ok_or("`mine.statement` is not a statement")?;
        let cred = credential::verify(&b64(&mine_proof["credential"]), &trust, now(), 0)
            .map_err(|r| format!("`mine.credential` does not verify: {}", r.code.name()))?;
        let fresh = Freshness {
            now: now(),
            seen: &[],
            challenge: None,
            sent_nonce: Some(nonce),
        };
        statement::verify(&own, &b64(&mine_proof["signature"]), &cred, CALLER, &fresh).map_err(
            |c| {
                format!(
                    "`mine` is not the program's proof over the caller's nonce: {} (RP-1 §6.5)",
                    c.name()
                )
            },
        )?;
        if cred.address != prog.address {
            return Err("`mine` is not the program's account".into());
        }
        let ids = |k: &str| -> Vec<String> {
            session[k]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|m| m["id"].as_str().map(str::to_owned))
                .collect()
        };
        if ids("received") != [mine.content_id().text()] {
            return Err(format!(
                "`received` lists {:?}: the message the program took (RP-5 §6.6)",
                ids("received")
            ));
        }
        let sent: Vec<String> = got.iter().map(ContentId::text).collect();
        if ids("sent") != sent {
            return Err(format!(
                "`sent` lists {:?} where the caller listed {sent:?} complete (RP-5 §6.6)",
                ids("sent")
            ));
        }
        if direct.status != 200 {
            return Err(format!(
                "the test post office refused the report: {}",
                direct.status
            ));
        }
        let again = prog.driver.run("report", &[("home", &home)], "")?;
        let reports = office
            .exchanges()
            .iter()
            .filter(|e| e.path == "/direct")
            .count();
        if reports != 1 {
            return Err(format!(
                "the report was sent again ({reports} reports): an answered session leaves the home. It said: {}",
                again.trim()
            ));
        }
        Ok(())
    }

    /// C-11: 300 seconds of nothing end the session, and the caller hangs up (RP-3 §8). It takes
    /// five minutes.
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn c11(&mut self, peer_out: &mut Vec<String>) -> Outcome {
        let trust = self.offices.trust();
        let (adapter, prog) = self.setup("C-11", 4096)?;
        let mut peer = adapter.peer(&[GATEWAY], true)?;
        let running = prog.call(GATEWAY, false)?;
        let result = (|| {
            peer.wait_link(Duration::from_secs(20))?;
            check_hello(&peer.expect("HELLO")?, &prog, GATEWAY, &trust)?;
            let started = Instant::now();
            match peer.next(Duration::from_secs(330), 1 << 16) {
                Heard::Gone { reason } if reason == "remote" => {
                    let waited = started.elapsed().as_secs();
                    if waited < 290 {
                        return Err(format!("it hung up after {waited} s, before RP-3 §8's 300"));
                    }
                    Ok(())
                }
                Heard::Gone { reason } => Err(format!("the link ended otherwise: {reason}")),
                Heard::Pdu(p) => Err(format!("{} into the silence", kind_name(kind_of(&p)))),
                Heard::Silence => Err("it did not hang up after 330 s of nothing".into()),
            }
        })();
        let (_, said) = running.finish(Duration::from_secs(10));
        peer_out.extend(peer.transcript);
        result.map_err(|e| format!("{e}\nthe program said: {}", said.trim()))
    }
}

/// The station called's last `RECEIPT`: mail for its account taken, and mail for another account
/// listed failed `refused-by-policy` (RP-3 §10).
fn check_station_receipt(last: &Turn, mine: &Message, theirs: &Message) -> Outcome {
    if !last.complete.contains(&mine.content_id()) {
        return Err("mail for its account is not in its RECEIPT's `complete`".into());
    }
    let refused = last
        .failed
        .iter()
        .any(|(id, c)| *id == theirs.content_id() && *c == code("refused-by-policy"));
    if last.complete.contains(&theirs.content_id()) || !refused {
        return Err(
            "mail for another account was not listed failed `refused-by-policy` (RP-3 §10)".into(),
        );
    }
    Ok(())
}

/// The `HELLO` of a client calling the program as a station (RP-3 §5.1, §10), signed.
fn caller_hello(
    called: &str,
    offers: &[&Message],
    nonce: [u8; 8],
    credential: &[u8],
    key: &ed25519_dalek::SigningKey,
) -> Value {
    let time = now();
    let stmt = Statement {
        address: CALLER_ADDRESS.into(),
        operator: CALLER.into(),
        gateway: called.into(),
        messages: offers.iter().map(|m| m.content_id()).collect(),
        form: Form::Time { time, nonce },
    };
    let offers = offers
        .iter()
        .map(|m| {
            Value::Array(vec![
                Value::bytes(m.content_id().as_bytes()),
                Value::Uint(m.air_form(Compression::None).len() as u64),
                Value::Uint(0),
            ])
        })
        .collect();
    Value::int_map(vec![
        (0, Value::Uint(kind::HELLO)),
        (1, Value::Array(vec![Value::Uint(1), Value::Uint(1)])),
        (2, Value::Uint(0)),
        (3, Value::text(CALLER_ADDRESS)),
        (4, Value::text(CALLER)),
        (5, Value::text(called)),
        (6, Value::Uint(time)),
        (7, Value::bytes(&nonce)),
        (8, Value::Array(offers)),
        (9, Value::Uint(0)),
        (10, Value::Uint(1 << 20)),
        (12, caps(4096)),
        (13, Value::bytes(credential)),
        (15, Value::bytes(&stmt.sign(key))),
    ])
}

/// What a station called says in its `WELCOME`: the offers it takes, and what waits for the caller
/// (serial, content ID, size).
struct Station {
    accepted: Vec<(ContentId, u64)>,
    waiting: Vec<(u64, ContentId, u64)>,
}

/// The program's `WELCOME` as a station called: `ready`, its own credential, and its statement
/// over the caller's nonce and the messages it lists (RP-3 §10, RP-1 §6.5).
fn check_station_welcome(
    w: &Value,
    prog: &Program<'_>,
    trust: &Trust,
    nonce: [u8; 8],
) -> Result<Station, String> {
    if kind_of(w) != kind::WELCOME || w.get(3).and_then(Value::as_u64) != Some(0) {
        return Err(format!(
            "{} where WELCOME `ready` was due",
            kind_name(kind_of(w))
        ));
    }
    let cred = w
        .get(13)
        .and_then(Value::as_bytes)
        .ok_or("WELCOME without its credential")?;
    let sig = w
        .get(15)
        .and_then(Value::as_bytes)
        .ok_or("WELCOME without its signature")?;
    let body = credential::verify(cred, trust, now(), 0)
        .map_err(|r| format!("its credential fails RP-1 §5: {}", r.code))?;
    if body.address != prog.address {
        return Err("its credential is not the program's".into());
    }
    if !matches!(w.get(11), Some(Value::Bool(false))) {
        return Err("its WELCOME's `online` is not false (RP-3 §10)".into());
    }
    if w.get(8).and_then(Value::as_u64) != Some(0) {
        return Err("its WELCOME's `more` is not 0 (RP-3 §10)".into());
    }
    let accepted = w
        .get(5)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let e = e.as_array()?;
            Some((id_of(e.first()?)?, e.get(1)?.as_u64()?))
        })
        .collect();
    // [serial, id, from, subject, size, priority, flags]
    let waiting: Vec<(u64, ContentId, u64)> = w
        .get(7)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let e = e.as_array()?;
            Some((e.first()?.as_u64()?, id_of(e.get(1)?)?, e.get(4)?.as_u64()?))
        })
        .collect();
    let time = w
        .get(10)
        .and_then(Value::as_u64)
        .ok_or("WELCOME without `time`")?;
    let own = Statement {
        address: prog.address.clone(),
        operator: prog.operator.clone(),
        gateway: CALLER.into(),
        messages: waiting.iter().map(|(_, id, _)| *id).collect(),
        form: Form::Time { time, nonce },
    };
    let fresh = Freshness {
        now: now(),
        seen: &[],
        challenge: None,
        sent_nonce: Some(nonce),
    };
    statement::verify(&own, sig, &body, CALLER, &fresh)
        .map_err(|c| format!("its statement as the station called fails RP-1 §6.5: {c}"))?;
    Ok(Station { accepted, waiting })
}

/// C-04's three visits, and what the tester holds between them.
struct Resume {
    download: Message,
    /// The upload's pieces the gateway kept, and its total.
    held_up: HashMap<ContentId, (Vec<u8>, u64)>,
    offered: Option<(ContentId, u64)>,
}

impl Resume {
    /// Visit 1: the upload accepted, and the link broken after its first piece.
    fn break_upload(&mut self, peer: &mut Peer, hello: &Hello) -> Outcome {
        let (id, size) = *hello.offers.first().ok_or("the report is not offered")?;
        self.offered = Some((id, size));
        peer.send(&[gateway_ready(vec![(id, 0)], &[], 1024), done()]);
        let p = peer.expect("the first MSG piece")?;
        if kind_of(&p) != kind::MSG {
            return Err(format!("{} where MSG was due", kind_name(kind_of(&p))));
        }
        let data = p.get(4).and_then(Value::as_bytes).unwrap_or_default();
        let total = p.get(3).and_then(Value::as_u64).unwrap_or(0);
        if data.len() as u64 >= total {
            return Err("the whole report went in one piece; the test needs it to break".into());
        }
        self.held_up.insert(id, (data.to_vec(), total));
        peer.abort();
        Ok(())
    }

    /// Visit 2, at another gateway: the upload resumed from the bytes held; the download asked
    /// for, and the link broken after 600 of its bytes.
    fn resume_upload(&mut self, peer: &mut Peer, hello: &Hello) -> Outcome {
        let (id, size) = self.offered.ok_or("no offer kept")?;
        if !hello.offers.contains(&(id, size)) {
            return Err("the broken upload is not offered again with the same size".into());
        }
        let have = self.held_up.get(&id).map_or(0, |(air, _)| air.len());
        let waiting = [entry(421, &self.download)];
        peer.send(&[
            gateway_ready(vec![(id, have as u64)], &waiting, 1024),
            done(),
        ]);
        let turn = program_turn(peer, 1024, &self.held_up)?;
        let whole = check_sent(&turn, &hello.offers)?;
        if !whole.contains(&id) {
            return Err("the upload did not resume to the same air form".into());
        }
        if !turn.request.contains(&421) {
            return Err("the download was not asked for (`--fetch` is all)".into());
        }
        let mut out = vec![receipt(&[id], &[])];
        out.extend(msgs(&self.download, 0, 300, Some(421)).into_iter().take(2));
        peer.send(&out);
        std::thread::sleep(Duration::from_millis(300));
        peer.abort();
        Ok(())
    }

    /// Visit 3: the download listed in `partials` and finished from byte 600.
    fn resume_download(&self, peer: &mut Peer, hello: &Hello, prog: &Program<'_>) -> Outcome {
        let id = self.download.content_id();
        let held = hello
            .partials
            .iter()
            .find(|(p, _)| *p == id)
            .map(|(_, h)| *h)
            .ok_or("HELLO does not list the broken download in `partials`")?;
        if held != 600 {
            return Err(format!(
                "`partials` says {held} bytes held, not the 600 sent"
            ));
        }
        let mut out = vec![gateway_ready(vec![], &[entry(421, &self.download)], 1024)];
        out.extend(msgs(&self.download, 600, 300, Some(421)));
        out.push(done());
        peer.send(&out);
        let turn = program_turn(peer, 1024, &HashMap::new())?;
        if !turn.complete.contains(&id) {
            return Err("the resumed download is not in `complete`".into());
        }
        peer.send(&[receipt(&[], &[]), done()]);
        peer.expect_hang_up(Duration::from_secs(30))?;
        if !contains(&prog.folder("inbox")?, id) {
            return Err("the resumed download is not in the inbox".into());
        }
        Ok(())
    }
}

/// A `WELCOME` with key 11 `online` set as given (RP-3 §5.2).
fn with_online(welcome: &Value, online: bool) -> Value {
    let entries = welcome
        .as_map()
        .map(<[_]>::to_vec)
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| {
            if k == Value::Uint(11) {
                (k, Value::Bool(online))
            } else {
                (k, v)
            }
        })
        .collect();
    Value::Map(entries)
}

/// A gateway's `WELCOME` `ready`, online, holding the caller's credential.
fn gateway_ready(accept: Vec<(ContentId, u64)>, waiting: &[Value], max_pdu: u64) -> Value {
    welcome(&Ready {
        accept,
        decline: vec![],
        waiting,
        max_pdu,
        cached: true,
        proof: None,
        time: now(),
    })
}

/// The end of a gateway session with nothing waiting: the program's messages taken and receipted.
fn finish(peer: &mut Peer, hello: &Hello) -> Outcome {
    peer.send(&[
        welcome(&Ready {
            accept: hello.offers.iter().map(|(id, _)| (*id, 0)).collect(),
            decline: vec![],
            waiting: &[],
            max_pdu: 4096,
            cached: true,
            proof: None,
            time: now(),
        }),
        done(),
    ]);
    let turn = program_turn(peer, 4096, &HashMap::new())?;
    let whole = check_sent(&turn, &hello.offers)?;
    peer.send(&[receipt(&whole, &[]), done()]);
    peer.expect_hang_up(Duration::from_secs(30))
}
