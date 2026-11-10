// SPDX-License-Identifier: MIT OR Apache-2.0
//! The gateway role's tests (the outline's §3.2) for a gateway online: G-01, G-02, G-04, G-07,
//! G-08 and G-09. The tester plays the caller through the test adapter and the post office
//! behind (the test post office, `office`), and judges what the gateway says on each side.
//!
//! The caller's side is scripted here from RP-3's tables, with the core library's encodings.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use radiopost::cbor::Value;
use radiopost::credential::{self, Body, Credential, flags};
use radiopost::message::{Compression, Message, key};
use radiopost::pdu::{self, kind};
use radiopost::statement::{Form, Statement};
use radiopost::{ContentId, ErrorCode, base64url};
use serde_json::Value as Json;

use crate::harness::{
    Driver, Heard, Outcome, Peer, PostOffices, Running, TestAdapter, now, random, unhex,
};
use crate::office::{Exchange, TestOffice, User};

/// The gateway under test's callsign.
pub const GATEWAY: &str = "N1CALL-10";
/// A second gateway, for resuming elsewhere (RP-3 §7).
pub const OTHER: &str = "N2CALL-5";

fn code(name: &str) -> u64 {
    ErrorCode::from_name(name).map_or(0, |c| u64::from(c.code()))
}

fn name_of(code: u64) -> String {
    ErrorCode::from_code(code).map_or_else(|| format!("code {code}"), |c| c.name().to_owned())
}

fn kind_of(p: &Value) -> u64 {
    p.get(0).and_then(Value::as_u64).unwrap_or(u64::MAX)
}

fn bytes_of(v: Option<&Value>) -> Vec<u8> {
    v.and_then(Value::as_bytes)
        .map(<[u8]>::to_vec)
        .unwrap_or_default()
}

/// A message from `from` to `to`; `words` of text a compressor can do little with.
pub(crate) fn mail(from: &str, to: &str, subject: &str, words: usize) -> Message {
    let mut x = u64::from_le_bytes(random::<8>()) | 1;
    let mut body = String::new();
    for _ in 0..words.max(1) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        for i in 0..(2 + x % 7) {
            body.push(char::from(
                b"etaoinshrdlucmfwypvbgkqjxz"[((x >> (8 + 5 * i)) % 26) as usize],
            ));
        }
        body.push(' ');
    }
    body.push('\n');
    Message::from_map(&Value::int_map(vec![
        (key::V, Value::Uint(1)),
        (key::FROM, Value::text(from)),
        (key::TO, Value::Array(vec![Value::text(to)])),
        (key::DATE, Value::Uint(now() - 600)),
        (key::SUBJECT, Value::text(subject)),
        (key::BODY, Value::text(&body)),
    ]))
    .expect("the tester's messages are messages")
}

/// What the caller says in `HELLO` (RP-3 §5.1).
struct Hello<'a> {
    user: &'a User,
    gateway: &'a str,
    offers: Vec<&'a Message>,
    auto: u64,
    partials: Vec<(ContentId, u64)>,
    renew: bool,
    time: u64,
    nonce: [u8; 8],
    /// Sign something other than the statement: a signature that does not verify.
    bad_signature: bool,
}

impl<'a> Hello<'a> {
    fn new(user: &'a User, gateway: &'a str) -> Hello<'a> {
        Hello {
            user,
            gateway,
            offers: Vec::new(),
            auto: 0,
            partials: Vec::new(),
            renew: false,
            time: now(),
            nonce: random(),
            bad_signature: false,
        }
    }

    fn statement(&self, form: Form) -> Statement {
        Statement {
            address: self.user.address.clone(),
            operator: self.user.operator.clone(),
            gateway: self.gateway.into(),
            messages: self.offers.iter().map(|m| m.content_id()).collect(),
            form,
        }
    }

    fn pdu(&self) -> Value {
        let stmt = self.statement(Form::Time {
            time: self.time,
            nonce: self.nonce,
        });
        let signature = if self.bad_signature {
            self.statement(Form::Time {
                time: self.time + 1,
                nonce: self.nonce,
            })
            .sign(&self.user.key)
        } else {
            stmt.sign(&self.user.key)
        };
        let offers = self
            .offers
            .iter()
            .map(|m| {
                Value::Array(vec![
                    Value::bytes(m.content_id().as_bytes()),
                    Value::Uint(m.air_form(Compression::None).len() as u64),
                    Value::Uint(0),
                ])
            })
            .collect();
        let mut caps = vec![
            (1, Value::Array(vec![Value::Uint(0), Value::Uint(1)])),
            (2, Value::Uint(4096)),
        ];
        if self.renew {
            caps.push((3, Value::Bool(true)));
        }
        let mut entries = vec![
            (0, Value::Uint(kind::HELLO)),
            (1, Value::Array(vec![Value::Uint(1), Value::Uint(1)])),
            (2, Value::Uint(0)),
            (3, Value::text(&self.user.address)),
            (4, Value::text(&self.user.operator)),
            (5, Value::text(self.gateway)),
            (6, Value::Uint(self.time)),
            (7, Value::bytes(&self.nonce)),
            (8, Value::Array(offers)),
            (9, Value::Uint(0)),
            (10, Value::Uint(self.auto)),
            (12, Value::int_map(caps)),
            (13, Value::bytes(&self.user.credential)),
            (15, Value::bytes(&signature)),
        ];
        if !self.partials.is_empty() {
            entries.push((
                11,
                Value::Array(
                    self.partials
                        .iter()
                        .map(|(id, o)| {
                            Value::Array(vec![Value::bytes(id.as_bytes()), Value::Uint(*o)])
                        })
                        .collect(),
                ),
            ));
        }
        Value::int_map(entries)
    }

    /// `AUTH` over the gateway's challenge (RP-3 §5.3, RP-1 §6.1).
    fn auth(&self, challenge: [u8; 16]) -> Value {
        let signature = self
            .statement(Form::Challenge(challenge))
            .sign(&self.user.key);
        Value::int_map(vec![
            (0, Value::Uint(kind::AUTH)),
            (1, Value::bytes(&signature)),
        ])
    }
}

fn msg_pieces(m: &Message, from: usize, piece: usize) -> Vec<Value> {
    let air = m.air_form(Compression::None);
    let mut out = Vec::new();
    let mut at = from;
    while at < air.len() {
        let end = (at + piece).min(air.len());
        out.push(Value::int_map(vec![
            (0, Value::Uint(kind::MSG)),
            (1, Value::bytes(m.content_id().as_bytes())),
            (2, Value::Uint(at as u64)),
            (3, Value::Uint(air.len() as u64)),
            (4, Value::bytes(&air[at..end])),
        ]));
        at = end;
    }
    out
}

fn receipt(complete: &[ContentId]) -> Value {
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
    ])
}

fn request(serials: &[u64]) -> Value {
    Value::int_map(vec![
        (0, Value::Uint(kind::REQUEST)),
        (
            1,
            Value::Array(serials.iter().map(|s| Value::Uint(*s)).collect()),
        ),
    ])
}

fn done() -> Value {
    Value::int_map(vec![(0, Value::Uint(kind::DONE))])
}

/// The gateway's PDUs until its `DONE` (or an `ERROR`, which ends the turn too).
fn turn(peer: &mut Peer, what: &str) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    loop {
        let p = peer.expect(what)?;
        let k = kind_of(&p);
        out.push(p);
        if k == kind::DONE || k == kind::ERROR {
            return Ok(out);
        }
    }
}

/// The gateway's answer to the caller's last turn: up to its `RECEIPT` (RP-3 §5.7: a turn after
/// `MSG` PDUs ends with a `RECEIPT` listing each), or a `DONE` or `ERROR`.
fn answer(peer: &mut Peer, what: &str) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    loop {
        let p = peer.expect(what)?;
        let k = kind_of(&p);
        out.push(p);
        if k == kind::RECEIPT || k == kind::DONE || k == kind::ERROR {
            return Ok(out);
        }
    }
}

/// The gateway's first PDU, which must be an `ERROR` with `want`'s code.
fn expect_error(peer: &mut Peer, want: &str) -> Outcome {
    let p = peer.expect(&format!("ERROR {want}"))?;
    if kind_of(&p) != kind::ERROR {
        return Err(format!(
            "the gateway answered with {} where RP-3 §5.9 wants ERROR {want}",
            p.diagnostic()
        ));
    }
    let got = p.get(1).and_then(Value::as_u64).unwrap_or(0);
    if got != code(want) {
        return Err(format!("ERROR {} where it should be {want}", name_of(got)));
    }
    if matches!(p.get(4), Some(Value::Bool(false))) {
        return Err(format!("ERROR {want} not fatal: the session cannot go on"));
    }
    Ok(())
}

/// A `WELCOME` with `status`; its session ID.
fn expect_welcome(p: &Value, status: u64) -> Result<Vec<u8>, String> {
    if kind_of(p) != kind::WELCOME {
        return Err(format!("{} where WELCOME was due", p.diagnostic()));
    }
    let got = p.get(3).and_then(Value::as_u64).unwrap_or(u64::MAX);
    if got != status {
        let err = (kind_of(p) == kind::ERROR)
            .then(|| name_of(p.get(1).and_then(Value::as_u64).unwrap_or(0)));
        return Err(format!(
            "WELCOME status {got} where {status} was due{}",
            err.map(|e| format!(" ({e})")).unwrap_or_default()
        ));
    }
    let session = bytes_of(p.get(2));
    if session.len() != 8 {
        return Err("WELCOME's session is not 8 bytes (RP-3 §5.2)".into());
    }
    Ok(session)
}

/// The air form carried by `MSG` PDUs for one message, from `from`: its bytes, and the serial.
fn assemble(pdus: &[Value], id: ContentId, from: u64) -> Result<(Vec<u8>, u64), String> {
    let mut out = Vec::new();
    let mut serial = 0;
    for p in pdus.iter().filter(|p| kind_of(p) == kind::MSG) {
        if p.get(1).and_then(Value::as_bytes) != Some(id.as_bytes()) {
            continue;
        }
        let offset = p.get(2).and_then(Value::as_u64).unwrap_or(0);
        if offset != from + out.len() as u64 {
            return Err(format!(
                "a MSG piece at {offset} where {} was next (RP-3 §5.4: in order, no gaps)",
                from + out.len() as u64
            ));
        }
        serial = p.get(5).and_then(Value::as_u64).unwrap_or(0);
        out.extend(bytes_of(p.get(4)));
    }
    Ok((out, serial))
}

/// A gateway under test, running.
struct Gateway {
    _running: Running,
}

fn parse_key(out: &str) -> Option<[u8; 32]> {
    out.split_whitespace().find_map(|w| {
        unhex(w)
            .or_else(|| base64url::decode(w))
            .and_then(|b| b.try_into().ok())
    })
}

/// What the gateway tests stand on.
pub struct Run<'a> {
    /// The gateway's driver.
    pub driver: &'a Driver,
    /// The test keys.
    pub offices: PostOffices,
    /// Where the gateways' directories go.
    pub scratch: PathBuf,
}

impl Run<'_> {
    /// A gateway set up as its operator would, in conformance mode: initialised on the test
    /// adapter, asking the test zone, with a credential the test post office issued for its key
    /// (RP-5 §6.4), and serving.
    fn gateway(
        &self,
        test: &str,
        office: &TestOffice,
        adapter: SocketAddr,
        callsign: &str,
        block: &[&str],
    ) -> Result<Gateway, String> {
        let home = self.scratch.join(test).join(callsign);
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.parent().unwrap_or(&home)).map_err(|e| e.to_string())?;
        let h = home.to_string_lossy().into_owned();
        let at = adapter.to_string();
        self.driver.run(
            "init",
            &[("home", &h), ("callsign", callsign), ("adapter", &at)],
            "",
        )?;
        self.driver.run(
            "conformance",
            &[("home", &h), ("resolver", &office.resolver)],
            "",
        )?;
        self.driver.run(
            "post_office",
            &[
                ("home", &h),
                ("domain", crate::office::DOMAIN),
                ("url", &office.base),
            ],
            "",
        )?;
        for d in block {
            self.driver
                .run("block", &[("home", &h), ("domain", d)], "")?;
        }
        let out = self.driver.run("key", &[("home", &h)], "")?;
        let public = parse_key(&out).ok_or_else(|| format!("`key` printed no key: {out:?}"))?;
        let file = home.join("credential.test");
        std::fs::write(
            &file,
            radiopost::cbor::hex(&office.gateway_credential(callsign, &public)),
        )
        .map_err(|e| e.to_string())?;
        self.driver.run(
            "credential",
            &[("home", &h), ("credential", &file.to_string_lossy())],
            "",
        )?;
        let running = self.driver.start("serve", &[("home", &h)])?;
        Ok(Gateway { _running: running })
    }

    fn world(&self, test: &str) -> Result<(TestOffice, TestAdapter, Gateway, Peer), String> {
        let office = TestOffice::start(&self.offices)?;
        let adapter = TestAdapter::new(4096)?;
        let gw = self.gateway(test, &office, adapter.program_at, GATEWAY, &[])?;
        let peer = adapter.peer(&["WD5EMA"], false)?;
        Ok((office, adapter, gw, peer))
    }

    /// G-01: `session-01`'s shape at an online gateway.
    ///
    /// # Errors
    ///
    /// The first deviation found.
    #[allow(clippy::too_many_lines, reason = "one session, both sides, in order")]
    pub fn g01(&mut self, transcript: &mut Vec<String>) -> Outcome {
        let (office, _adapter, _gw, mut peer) = self.world("G-01")?;
        let user = office.user("club@radiopost.org", "WD5EMA");
        let waiting = mail("maria@example.com", &user.address, "Are you safe?", 80);
        let air_waiting = waiting.air_form(Compression::None);
        office.office().deliver(
            &user.address,
            1,
            &air_waiting,
            "maria@example.com",
            "Are you safe?",
            0,
            false,
        );
        let out = mail(&user.address, "maria@example.com", "We are fine", 60);
        let mut hello = Hello::new(&user, GATEWAY);
        hello.offers = vec![&out];
        hello.auto = 1 << 20;
        let result = (|| -> Outcome {
            peer.call_within(GATEWAY, Duration::from_secs(15))?;
            let sent_at = Instant::now();
            peer.send(&[hello.pdu()]);
            let pdus = turn(&mut peer, "WELCOME, MSG and DONE")?;
            let took = sent_at.elapsed();
            let session = expect_welcome(&pdus[0], 0)?;
            if took > Duration::from_secs(1) {
                transcript.push(format!(
                    "SHOULD (RP-3 §9): WELCOME came {} ms after HELLO, over a second",
                    took.as_millis()
                ));
            }
            let w = &pdus[0];
            let accept = w.get(5).and_then(Value::as_array).unwrap_or(&[]);
            let accepted: Vec<(Vec<u8>, u64)> = accept
                .iter()
                .filter_map(|a| {
                    let a = a.as_array()?;
                    Some((a.first()?.as_bytes()?.to_vec(), a.get(1)?.as_u64()?))
                })
                .collect();
            if accepted != [(out.content_id().as_bytes().to_vec(), 0)] {
                return Err(format!(
                    "WELCOME accept {accepted:?}: the one offer, at 0, was due"
                ));
            }
            let entries = w.get(7).and_then(Value::as_array).unwrap_or(&[]);
            let listed: Vec<(u64, Vec<u8>)> = entries
                .iter()
                .filter_map(|e| {
                    let e = e.as_array()?;
                    Some((e.first()?.as_u64()?, e.get(1)?.as_bytes()?.to_vec()))
                })
                .collect();
            if listed != [(1, waiting.content_id().as_bytes().to_vec())] {
                return Err(format!(
                    "WELCOME waiting {listed:?}: serial 1, the message waiting"
                ));
            }
            if !matches!(w.get(11), Some(Value::Bool(true))) {
                return Err("WELCOME online is not true at an online gateway (RP-3 §5.2)".into());
            }
            // the post office was asked while the link turned around (RP-3 §9, RP-5 §5.2)
            let log = office.exchanges();
            let open = log
                .iter()
                .find(|e| e.method == "POST" && e.path == "/sessions")
                .ok_or("the gateway did not open the session at the post office (RP-5 §5.2)")?;
            check_open(open, &hello, &session)?;
            let (air, serial) = assemble(&pdus, waiting.content_id(), 0)?;
            if air != air_waiting || serial != 1 {
                return Err("the waiting message's MSG pieces are not the post office's air form, serial 1 (RP-5 §5.3)".into());
            }
            let mut mine = msg_pieces(&out, 0, 900);
            mine.push(receipt(&[waiting.content_id()]));
            mine.push(done());
            peer.send(&mine);
            let answer = answer(&mut peer, "a RECEIPT for the message handed in")?;
            let r = answer
                .iter()
                .find(|p| kind_of(p) == kind::RECEIPT)
                .ok_or("no RECEIPT for the message handed in (RP-3 §5.5)")?;
            let complete: Vec<&[u8]> = r
                .get(1)
                .and_then(Value::as_array)
                .unwrap_or(&[])
                .iter()
                .filter_map(Value::as_bytes)
                .collect();
            if complete != [out.content_id().as_bytes()] {
                return Err("the RECEIPT does not list the message handed in as complete".into());
            }
            peer.hang_up();
            let sid = open
                .answer
                .as_ref()
                .and_then(|a| a.get("session"))
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_owned();
            let closed = wait_for(&office, |e| {
                e.method == "DELETE" && e.path == format!("/sessions/{sid}")
            });
            let log = office.exchanges();
            let handed = log.iter().any(|e| {
                e.path == format!("/sessions/{sid}/messages")
                    && e.body
                        .as_ref()
                        .is_some_and(|b| b.to_string().contains(&out.content_id().text()))
                    && e.status == 200
            });
            if !handed {
                return Err("the message was not handed in to the post office (RP-5 §5.4)".into());
            }
            let picked = log.iter().any(|e| {
                e.path == format!("/sessions/{sid}/pickups")
                    && e.body.as_ref().and_then(|b| b.get("serials"))
                        == Some(&serde_json::json!([1]))
            });
            if !picked {
                return Err("the pickup of serial 1 was not reported (RP-5 §5.5)".into());
            }
            if !closed {
                return Err("the session was not closed at the post office (RP-5 §5.7)".into());
            }
            Ok(())
        })();
        transcript.append(&mut peer.transcript);
        result
    }

    /// One call with one `HELLO`, and the gateway's first PDU.
    fn first_answer(peer: &mut Peer, hello: &Value) -> Result<Value, String> {
        peer.call_within(GATEWAY, Duration::from_secs(15))?;
        peer.send(std::slice::from_ref(hello));
        peer.expect("the gateway's answer to HELLO")
    }

    /// G-02: credentials and statements refused in order, each with its code.
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn g02(&mut self, transcript: &mut Vec<String>) -> Outcome {
        let (office, _adapter, _gw, mut peer) = self.world("G-02")?;
        let result = (|| -> Outcome {
            let expired_body = |u: &User| Body {
                v: 1,
                serial: 77,
                address: u.address.clone(),
                operator: u.operator.clone(),
                device: u.key.verifying_key().to_bytes(),
                issued: now() - 100 * 86_400,
                expires: now() - 10 * 86_400,
                flags: flags::SEND | flags::RECEIVE,
            };
            // expired
            let mut u = office.user("a@radiopost.org", "WD5EMA");
            u.credential = self.offices.issue_body(&expired_body(&u));
            let p = Self::first_answer(&mut peer, &Hello::new(&u, GATEWAY).pdu())?;
            peer.transcript.push("— expired credential".into());
            check_error(&p, "credential-expired")?;
            peer.hang_up();
            // expired and badly signed: the credential is checked first (RP-1 §6.3 step 1)
            let mut h = Hello::new(&u, GATEWAY);
            h.bad_signature = true;
            let p = Self::first_answer(&mut peer, &h.pdu())?;
            check_error(&p, "credential-expired")?;
            peer.hang_up();
            // revoked
            let u = office.user("b@radiopost.org", "WD5EMA");
            let serial = Credential::decode(&u.credential).map_or(0, |c| c.body.serial);
            office.office().state.revoked.push(serial);
            let p = Self::first_answer(&mut peer, &Hello::new(&u, GATEWAY).pdu())?;
            check_error(&p, "credential-revoked")?;
            peer.hang_up();
            // a post office no one can find
            let key = crate::harness::new_key();
            let ana = User {
                address: "ana@hampost.org".into(),
                operator: "N0CALL".into(),
                credential: self.offices.issue(
                    "ana@hampost.org",
                    "N0CALL",
                    &key.verifying_key().to_bytes(),
                ),
                key,
            };
            let p = Self::first_answer(&mut peer, &Hello::new(&ana, GATEWAY).pdu())?;
            check_error(&p, "unknown-post-office")?;
            peer.hang_up();
            // a signature that does not verify
            let u = office.user("c@radiopost.org", "WD5EMA");
            let mut h = Hello::new(&u, GATEWAY);
            h.bad_signature = true;
            let p = Self::first_answer(&mut peer, &h.pdu())?;
            check_error(&p, "signature-invalid")?;
            peer.hang_up();
            // a clock an hour off: a challenge, then the session (RP-1 §6.3 step 5, RP-3 §5.3)
            let mut h = Hello::new(&u, GATEWAY);
            h.time = now() - 3600;
            let p = Self::first_answer(&mut peer, &h.pdu())?;
            expect_welcome(&p, 1)?;
            let challenge: [u8; 16] = bytes_of(p.get(4))
                .try_into()
                .map_err(|_| "WELCOME sign without a 16-byte challenge (RP-3 §5.2)")?;
            peer.send(&[h.auth(challenge)]);
            let pdus = turn(&mut peer, "WELCOME ready after AUTH")?;
            expect_welcome(&pdus[0], 0)?;
            peer.send(&[done()]);
            peer.hang_up();
            // a nonce seen before: a challenge (RP-1 §6.3 step 5)
            let h = Hello::new(&u, GATEWAY);
            let hello = h.pdu();
            let p = Self::first_answer(&mut peer, &hello)?;
            expect_welcome(&p, 0)?;
            let _ = turn_rest(&mut peer, &p);
            peer.send(&[done()]);
            peer.hang_up();
            let p = Self::first_answer(&mut peer, &hello)?;
            expect_welcome(&p, 1).map_err(|e| format!("the same HELLO again: {e}"))?;
            peer.hang_up();
            Ok(())
        })();
        transcript.append(&mut peer.transcript);
        result
    }

    /// G-04: a download resumed at another gateway, and an upload at the same one (RP-3 §7).
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn g04(&mut self, transcript: &mut Vec<String>) -> Outcome {
        let office = TestOffice::start(&self.offices)?;
        let adapter = TestAdapter::with_programs(4096, 2)?;
        let _one = self.gateway("G-04", &office, adapter.programs_at[0], GATEWAY, &[])?;
        let _two = self.gateway("G-04", &office, adapter.programs_at[1], OTHER, &[])?;
        let mut peer = adapter.peer(&["WD5EMA"], false)?;
        let user = office.user("club@radiopost.org", "WD5EMA");
        let big = mail("maria@example.com", &user.address, "The list", 900);
        let air = big.air_form(Compression::None);
        office.office().deliver(
            &user.address,
            1,
            &air,
            "maria@example.com",
            "The list",
            0,
            false,
        );
        let result = (|| -> Outcome {
            // the download, broken after the first piece at N1CALL-10
            peer.call_within(GATEWAY, Duration::from_secs(15))?;
            peer.send(&[Hello::new(&user, GATEWAY).pdu()]);
            let first = turn(&mut peer, "WELCOME and DONE")?;
            expect_welcome(&first[0], 0)?;
            peer.send(&[request(&[1])]);
            let piece = peer.expect("the first MSG piece")?;
            let (held, _) = assemble(std::slice::from_ref(&piece), big.content_id(), 0)?;
            if held.is_empty() || held.len() >= air.len() {
                return Err("the first MSG piece is empty or the whole message".into());
            }
            peer.abort();
            // resumed at N2CALL-5 from what the caller holds
            peer.call_within(OTHER, Duration::from_secs(15))?;
            let mut h = Hello::new(&user, OTHER);
            h.partials = vec![(big.content_id(), held.len() as u64)];
            peer.send(&[h.pdu()]);
            let pdus = turn(&mut peer, "WELCOME, the rest of the message and DONE")?;
            expect_welcome(&pdus[0], 0)?;
            let (rest, serial) = assemble(&pdus, big.content_id(), held.len() as u64)
                .map_err(|e| format!("at N2CALL-5: {e}"))?;
            if [held.as_slice(), rest.as_slice()].concat() != air || serial != 1 {
                return Err("the download resumed elsewhere is not the post office's air form from the caller's offset (RP-3 §7, RP-5 §5.3)".into());
            }
            peer.send(&[receipt(&[big.content_id()]), done()]);
            peer.hang_up();
            // an upload broken after two pieces at N1CALL-10, resumed there
            let out = mail(&user.address, "maria@example.com", "Re: The list", 900);
            peer.call_within(GATEWAY, Duration::from_secs(15))?;
            let mut h = Hello::new(&user, GATEWAY);
            h.offers = vec![&out];
            peer.send(&[h.pdu()]);
            let w = turn(&mut peer, "WELCOME and DONE")?;
            expect_welcome(&w[0], 0)?;
            let pieces = msg_pieces(&out, 0, 900);
            peer.send(&pieces[..2]);
            // a broken link loses what is in flight: the two pieces cross first
            std::thread::sleep(Duration::from_secs(3));
            peer.abort();
            std::thread::sleep(Duration::from_millis(500));
            peer.call_within(GATEWAY, Duration::from_secs(15))?;
            let mut h = Hello::new(&user, GATEWAY);
            h.offers = vec![&out];
            peer.send(&[h.pdu()]);
            let w = turn(&mut peer, "WELCOME and DONE")?;
            expect_welcome(&w[0], 0)?;
            let offset = w[0]
                .get(5)
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_array)
                .and_then(|a| a.get(1))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if offset != 1800 {
                return Err(format!(
                    "the upload was accepted at {offset}, where the 1800 bytes it holds were due (RP-3 §7)"
                ));
            }
            let mut rest = msg_pieces(&out, 1800, 900);
            rest.push(done());
            peer.send(&rest);
            let r = answer(&mut peer, "a RECEIPT for the resumed upload")?;
            if !r.iter().any(|p| {
                kind_of(p) == kind::RECEIPT
                    && p.get(1).and_then(Value::as_array).is_some_and(|a| {
                        a.iter()
                            .any(|x| x.as_bytes() == Some(out.content_id().as_bytes()))
                    })
            }) {
                return Err("the resumed upload was not receipted complete".into());
            }
            peer.hang_up();
            if !office.office().state.had.contains(&out.content_id().text()) {
                return Err("the resumed upload did not reach the post office whole".into());
            }
            Ok(())
        })();
        transcript.append(&mut peer.transcript);
        result
    }

    /// G-07: a caller that misbehaves.
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn g07(&mut self, transcript: &mut Vec<String>, slow: bool) -> Outcome {
        let (office, _adapter, _gw, mut peer) = self.world("G-07")?;
        let user = office.user("club@radiopost.org", "WD5EMA");
        let result = (|| -> Outcome {
            // a frame over the gateway's max_pdu (RP-3 §2)
            let p = Self::first_answer(&mut peer, &Hello::new(&user, GATEWAY).pdu())?;
            expect_welcome(&p, 0)?;
            let max = p
                .get(9)
                .and_then(|c| c.get(2))
                .and_then(Value::as_u64)
                .unwrap_or(1024);
            let _ = turn_rest(&mut peer, &p);
            let big = vec![0x41u8; usize::try_from(max).unwrap_or(4096) + 100];
            let frame = pdu::frame(&Value::int_map(vec![
                (0, Value::Uint(kind::NOTICE)),
                (1, Value::bytes(&big)),
            ]))
            .map_err(|e| e.to_string())?;
            peer.transcript.push(format!(
                "tester  → a frame of {} bytes, over the gateway's max_pdu {max}",
                frame.len() - 2
            ));
            peer.send_raw(&frame);
            expect_error(&mut peer, "too-large")?;
            peer.hang_up();
            // an unknown type (RP-3 §4)
            let p = Self::first_answer(&mut peer, &Hello::new(&user, GATEWAY).pdu())?;
            expect_welcome(&p, 0)?;
            let _ = turn_rest(&mut peer, &p);
            peer.send(&[Value::int_map(vec![(0, Value::Uint(99))])]);
            expect_error(&mut peer, "malformed")?;
            peer.hang_up();
            // a message the gateway did not accept (RP-3 §5.4): never taken
            let stray = mail(&user.address, "maria@example.com", "Not offered", 30);
            let p = Self::first_answer(&mut peer, &Hello::new(&user, GATEWAY).pdu())?;
            expect_welcome(&p, 0)?;
            let _ = turn_rest(&mut peer, &p);
            let mut pdus = msg_pieces(&stray, 0, 900);
            pdus.push(done());
            peer.send(&pdus);
            // silence, a RECEIPT that does not list it complete, or an ERROR are all allowed
            let taken = match peer.next(Duration::from_secs(5), 1 << 16) {
                Heard::Pdu(p) => {
                    kind_of(&p) == kind::RECEIPT
                        && p.get(1).and_then(Value::as_array).is_some_and(|a| {
                            a.iter()
                                .any(|x| x.as_bytes() == Some(stray.content_id().as_bytes()))
                        })
                }
                _ => false,
            };
            if taken
                || office
                    .office()
                    .state
                    .had
                    .contains(&stray.content_id().text())
            {
                return Err("a message never offered nor accepted was taken (RP-3 §5.4)".into());
            }
            peer.hang_up();
            // a caller that never hangs up: the gateway ends it after 300 s of nothing (RP-3 §8)
            if slow {
                let p = Self::first_answer(&mut peer, &Hello::new(&user, GATEWAY).pdu())?;
                expect_welcome(&p, 0)?;
                let _ = turn_rest(&mut peer, &p);
                peer.send(&[done()]);
                let started = Instant::now();
                loop {
                    match peer.next(Duration::from_secs(340), 1 << 16) {
                        Heard::Gone { .. } => break,
                        Heard::Pdu(_) => {}
                        Heard::Silence => {
                            return Err(
                                "the gateway kept a silent link over 340 s (RP-3 §8)".into()
                            );
                        }
                    }
                }
                let took = started.elapsed();
                if took < Duration::from_secs(290) {
                    return Err(format!(
                        "the gateway ended a quiet session after {} s, before 300 (RP-3 §8)",
                        took.as_secs()
                    ));
                }
            } else {
                peer.transcript
                    .push("— the 300-second wait is run with --slow".into());
            }
            Ok(())
        })();
        transcript.append(&mut peer.transcript);
        result
    }

    /// G-08: a credential renewed by radio (RP-3 §5.8, RP-5 §6.2).
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn g08(&mut self, transcript: &mut Vec<String>) -> Outcome {
        let (office, _adapter, _gw, mut peer) = self.world("G-08")?;
        let user = office.user("club@radiopost.org", "WD5EMA");
        let result = (|| -> Outcome {
            let mut h = Hello::new(&user, GATEWAY);
            h.renew = true;
            peer.call_within(GATEWAY, Duration::from_secs(15))?;
            peer.send(&[h.pdu()]);
            let pdus = turn(&mut peer, "WELCOME, CREDENTIAL and DONE")?;
            expect_welcome(&pdus[0], 0)?;
            let c = pdus
                .iter()
                .find(|p| kind_of(p) == kind::CREDENTIAL)
                .ok_or("no CREDENTIAL after a HELLO that asked to renew (RP-3 §5.8)")?;
            let bytes = bytes_of(c.get(1));
            let new = credential::verify(&bytes, &self.offices.trust(), now(), 0).map_err(|r| {
                format!("the renewed credential does not verify: {}", r.code.name())
            })?;
            let old = Credential::decode(&user.credential)
                .ok_or("the old credential")?
                .body;
            if (
                new.address.as_str(),
                new.operator.as_str(),
                new.device,
                new.flags,
            ) != (
                old.address.as_str(),
                old.operator.as_str(),
                old.device,
                old.flags,
            ) || new.serial == old.serial
            {
                return Err("the renewed credential is not the same account, operator, device and flags with a new serial (RP-5 §6.2)".into());
            }
            let asked = office.exchanges().iter().any(|e| {
                e.path == "/sessions"
                    && e.body.as_ref().and_then(|b| b.get("renew")) == Some(&Json::Bool(true))
            });
            if !asked {
                return Err(
                    "the gateway did not pass `renew` to the post office (RP-5 §5.2)".into(),
                );
            }
            peer.send(&[done()]);
            peer.hang_up();
            Ok(())
        })();
        transcript.append(&mut peer.transcript);
        result
    }

    /// G-09: which post offices it carries sessions to (RP-1 §7.1, §7.3), online: those it
    /// was given or finds in the DNS it was given, and none its operator blocked.
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn g09(&mut self, transcript: &mut Vec<String>) -> Outcome {
        let office = TestOffice::start(&self.offices)?;
        let adapter = TestAdapter::new(4096)?;
        let user = office.user("club@radiopost.org", "WD5EMA");
        // a caller whose post office the gateway must look up
        let mut at = |domain: &str| -> User {
            let key = crate::harness::new_key();
            let address = format!("ana@{domain}");
            User {
                credential: self
                    .offices
                    .issue(&address, "N0CALL", &key.verifying_key().to_bytes()),
                address,
                operator: "N0CALL".into(),
                key,
            }
        };
        let not_listed = at("hampost.org");
        // RP-1 §7.1: an API record whose URL is not https:// is ignored
        office.publish(
            "hampost.org",
            &["v=RP1; t=api; url=http://127.0.0.1:1/rp/v1".to_owned()],
        );
        (|| -> Outcome {
            {
                let _gw = self.gateway("G-09", &office, adapter.program_at, GATEWAY, &[])?;
                let mut peer = adapter.peer(&["WD5EMA"], false)?;
                let p = Self::first_answer(&mut peer, &Hello::new(&user, GATEWAY).pdu())?;
                expect_welcome(&p, 0).map_err(|e| format!("radiopost.org: {e}"))?;
                let _ = turn_rest(&mut peer, &p);
                peer.send(&[done()]);
                peer.hang_up();
                let p = Self::first_answer(&mut peer, &Hello::new(&not_listed, GATEWAY).pdu())?;
                check_error(&p, "unknown-post-office")
                    .map_err(|e| format!("hampost.org, whose only API record is not https: {e}"))?;
                peer.hang_up();
                if !office
                    .asked()
                    .iter()
                    .any(|q| q.contains("_radiopost.hampost.org"))
                {
                    return Err(
                        "the gateway did not ask the DNS it was given about hampost.org (§2.3)"
                            .into(),
                    );
                }
                // a record it may use: the session goes there (and finds nothing listening)
                office.publish(
                    "hampost.org",
                    &["v=RP1; t=api; url=https://127.0.0.1:1/rp/v1".to_owned()],
                );
                let p = Self::first_answer(&mut peer, &Hello::new(&not_listed, GATEWAY).pdu())?;
                check_error(&p, "post-office-unreachable").map_err(|e| {
                    format!("hampost.org, published in DNS at a URL nothing answers: {e}")
                })?;
                peer.hang_up();
                transcript.append(&mut peer.transcript);
            }
            {
                let _gw = self.gateway(
                    "G-09b",
                    &office,
                    adapter.program_at,
                    GATEWAY,
                    &["radiopost.org"],
                )?;
                let mut peer = adapter.peer(&["WD5EMA"], false)?;
                let before = office.exchanges().len();
                let p = Self::first_answer(&mut peer, &Hello::new(&user, GATEWAY).pdu())?;
                transcript.append(&mut peer.transcript);
                check_error(&p, "unknown-post-office").map_err(|e| format!("blocked: {e}"))?;
                if office.exchanges()[before..]
                    .iter()
                    .any(|e| e.path == "/sessions")
                {
                    return Err(
                        "the gateway carried a session to a post office its operator blocked"
                            .into(),
                    );
                }
                peer.hang_up();
            }
            Ok(())
        })()
    }
}

/// The rest of the gateway's transmission after its `WELCOME`, up to its `DONE`.
fn turn_rest(peer: &mut Peer, first: &Value) -> Result<Vec<Value>, String> {
    if kind_of(first) == kind::DONE {
        return Ok(Vec::new());
    }
    turn(peer, "the rest of the gateway's turn")
}

fn check_error(p: &Value, want: &str) -> Outcome {
    if kind_of(p) != kind::ERROR {
        return Err(format!(
            "{} where ERROR {want} was due (RP-3 §5.9)",
            p.diagnostic()
        ));
    }
    let got = p.get(1).and_then(Value::as_u64).unwrap_or(0);
    if got == code(want) {
        Ok(())
    } else {
        Err(format!("ERROR {} where {want} was due", name_of(got)))
    }
}

/// RP-5 §5.2: the gateway's `POST /sessions` carries the caller's statement as rebuilt from
/// `HELLO`, its signature and credential, the start, and the session's ID.
fn check_open(open: &Exchange, hello: &Hello<'_>, session: &[u8]) -> Outcome {
    let body = open.body.as_ref().ok_or("POST /sessions with no body")?;
    let field = |k: &str| {
        body.get(k)
            .and_then(Json::as_str)
            .and_then(base64url::decode)
            .unwrap_or_default()
    };
    let stmt = hello.statement(Form::Time {
        time: hello.time,
        nonce: hello.nonce,
    });
    if field("statement") != stmt.encode() {
        return Err(
            "POST /sessions: `statement` is not the caller's, rebuilt from HELLO (RP-5 §5.2)"
                .into(),
        );
    }
    if field("signature") != stmt.sign(&hello.user.key).to_vec() {
        return Err("POST /sessions: `signature` is not HELLO's".into());
    }
    if field("credential") != hello.user.credential {
        return Err("POST /sessions: `credential` is not HELLO's".into());
    }
    let started = body.get("started").and_then(Json::as_u64).unwrap_or(0);
    if started.abs_diff(now()) > 600 {
        return Err("POST /sessions: `started` is not the gateway's clock".into());
    }
    if body.get("gateway_session").and_then(Json::as_str)
        != Some(radiopost::cbor::hex(session).as_str())
    {
        return Err("POST /sessions: `gateway_session` is not WELCOME's session ID in hex".into());
    }
    if open.status != 201 {
        return Err(format!(
            "the post office refused the session: {}",
            open.status
        ));
    }
    Ok(())
}

/// Waits up to five seconds for an exchange the gateway is due to make.
fn wait_for(office: &TestOffice, mut found: impl FnMut(&Exchange) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if office.exchanges().iter().any(&mut found) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// The directory a test's gateways go in.
#[must_use]
pub fn home_of(scratch: &Path, test: &str) -> PathBuf {
    scratch.join(test)
}
