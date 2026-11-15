// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-3, the gateway's side, online: the user's session carried to the user's post office
//! (RP-3 §9, RP-5 §5).
//!
//! The engine does no input or output of its own. The post office is an [`Upstream`] the program
//! gives it, which it calls when the session needs it: on `HELLO` to open the session, for each
//! message it sends, when the caller hands mail in, and when the caller's `RECEIPT` says what it
//! picked up. Tests give it the post office itself, in process; the gateway program gives it the
//! post office's API over HTTPS.

use std::collections::HashMap;

use serde_json::{Value as Json, json};

use super::{
    Caps, Event, Inbound, Outgoing, Receipts, VERSION, WaitingEntry, done, error_pdu,
    error_pdu_saying, flags, listed_ids, listed_pairs, msg_pdus, piece_for, transmission,
};
use crate::base64url;
use crate::cbor::Value;
use crate::credential::{self, Credential};
use crate::crypto::ContentId;
use crate::error::ErrorCode;
use crate::message::Message;
use crate::pdu::{Deframer, kind};
use crate::statement::{Form, Statement, TIME_WINDOW_S};

/// A user's post office, as a gateway reaches it for one session (RP-5 §5).
pub trait Upstream {
    /// `POST /sessions` with this body: the post office's answer (RP-5 §5.2).
    ///
    /// # Errors
    /// The code the post office refused the session with.
    fn open(&mut self, body: &Json) -> Result<Json, ErrorCode>;
    /// A waiting message's air form (RP-5 §5.3).
    ///
    /// # Errors
    /// The code the post office answered with.
    fn fetch(&mut self, serial: u64) -> Result<Vec<u8>, ErrorCode>;
    /// The messages the user sent, handed in (RP-5 §5.4): for each, `Ok` when the post office has
    /// it (`accepted` or `duplicate`), or the code it refused it with.
    ///
    /// # Errors
    /// The post office could not be reached: every message fails with this code.
    fn hand_in(
        &mut self,
        messages: &[(ContentId, Vec<u8>)],
    ) -> Result<Vec<Result<(), ErrorCode>>, ErrorCode>;
    /// Messages the user picked up (RP-5 §5.5).
    ///
    /// # Errors
    /// The code the post office answered with.
    fn picked_up(&mut self, serials: &[u64]) -> Result<(), ErrorCode>;
    /// A local message's air form, by content ID (RP-7 §4.4): only a cut-off gateway holds any.
    ///
    /// # Errors
    /// `unknown-message` when it holds no such message.
    fn fetch_local(&mut self, _id: &ContentId) -> Result<Vec<u8>, ErrorCode> {
        Err(ErrorCode::UnknownMessage)
    }
    /// Local messages the user picked up, by content ID (RP-7 §4.4): kept in the user's session
    /// record as its local pickups.
    ///
    /// # Errors
    /// The pickups could not be kept.
    fn picked_up_local(&mut self, _ids: &[ContentId]) -> Result<(), ErrorCode> {
        Ok(())
    }
    /// Held attachments the user asked for, each by its message's serial and its `ref`
    /// (RP-5 §5.3): the post office puts them in the mailbox as new messages (RP-6 §2.5).
    ///
    /// # Errors
    /// The code the post office answered with.
    fn files(&mut self, asked: &[(u64, Vec<u8>)]) -> Result<(), ErrorCode>;
    /// The session is over (RP-5 §5.7).
    fn close(&mut self);
    /// Whether this is the caller's post office itself, or the gateway answering from what it
    /// holds because the post office cannot be reached (`WELCOME` `online`, RP-3 §5.2, §9).
    fn online(&self) -> bool {
        true
    }
}

/// The post office of a domain, or why there is none to reach.
pub type Upstreams = Box<dyn FnMut(&str) -> Result<Box<dyn Upstream + Send>, ErrorCode> + Send>;

/// Whether third-party mail may be listed for a station with this callsign (RP-6 §5.1).
pub type ThirdParty = Box<dyn Fn(&str) -> bool + Send>;

/// What a gateway brings to a session.
pub struct GatewayConfig {
    /// Its callsign, which the caller's statement must name.
    pub callsign: String,
    /// Its clock.
    pub time: u64,
    /// A random session ID (`WELCOME` key 2).
    pub session_id: [u8; 8],
    /// A random challenge, for a caller that did not sign (RP-3 §5.3).
    pub challenge: [u8; 16],
    /// What it accepts.
    pub caps: Caps,
    /// The most data bytes it puts in one `MSG`, below what the caller accepts.
    pub piece: usize,
    /// Credentials it holds from earlier visits, which a caller may name by hash (RP-3 §5.1).
    pub credentials: Vec<Vec<u8>>,
    /// Pieces of uploads broken in earlier sessions, by account and content ID, with the air
    /// form's whole length (RP-3 §7).
    pub uploads: HashMap<(String, ContentId), (Vec<u8>, u64)>,
    /// Callers its operator refuses: addresses, and callsigns, a callsign without an SSID
    /// refusing every SSID of it ([`refused_caller`]).
    pub refused: Vec<String>,
}

/// How long a copy placed ahead makes its user local at a cut-off gateway (RP-7 §4.4): 30 days.
pub const LOCAL_FOR_S: u64 = 30 * 86_400;

/// Whether `address` is a local user of a cut-off gateway at `now` (RP-7 §4.4): a copy placed
/// ahead for it arrived there in the last 30 days, `seen` giving when the last one did, by
/// address in lowercase.
#[must_use]
pub fn is_local(seen: &std::collections::BTreeMap<String, u64>, address: &str, now: u64) -> bool {
    seen.get(&address.to_lowercase())
        .is_some_and(|at| now.saturating_sub(*at) < LOCAL_FOR_S)
}

/// The local users a message handed to a cut-off gateway is kept for (RP-7 §4.4): its `to` and
/// `cc` that are local, in that order, each once, in lowercase.
#[must_use]
pub fn local_recipients(
    message: &Message,
    seen: &std::collections::BTreeMap<String, u64>,
    now: u64,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in message.recipients() {
        let r = r.to_lowercase();
        if is_local(seen, &r, now) && !out.contains(&r) {
            out.push(r);
        }
    }
    out
}

/// The entry of `refused` that refuses a caller with this address and this callsign, if any: an
/// address matches whatever its case, and a callsign without an SSID every SSID of it
/// (`N0CALL` refuses `N0CALL-5`; `N0CALL-5` refuses that one alone).
#[must_use]
pub fn refused_caller<'a>(refused: &'a [String], address: &str, callsign: &str) -> Option<&'a str> {
    let callsign = callsign.trim().to_ascii_uppercase();
    let base = callsign.split('-').next().unwrap_or("");
    refused
        .iter()
        .find(|entry| {
            let entry = entry.trim();
            if entry.contains('@') {
                !address.is_empty() && entry.eq_ignore_ascii_case(address.trim())
            } else {
                let entry = entry.to_ascii_uppercase();
                !callsign.is_empty()
                    && (entry == callsign || (!entry.contains('-') && entry == base))
            }
        })
        .map(String::as_str)
}

/// What the gateway's engine tells the program besides RP-3's events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    /// A credential to keep, so the caller may send its hash next time.
    Credential(Vec<u8>),
    /// The caller was refused because the operator refuses this entry of
    /// [`GatewayConfig::refused`].
    Refused(String),
    /// Pieces of an upload to keep for at least 24 hours (RP-3 §7); none held when it completed.
    Upload {
        /// The account.
        address: String,
        /// The message.
        id: ContentId,
        /// The bytes held, from the start.
        held: Vec<u8>,
        /// The air form's whole length.
        total: u64,
    },
    /// The session at the post office failed: the caller was told so.
    PostOffice(ErrorCode),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Hello,
    Credential,
    Challenged,
    Exchange,
    Over,
}

/// A message the post office listed for the caller: its serial and content ID, and whether it
/// is local (RP-7 §4.4: serial 0, sent and picked up by content ID).
#[derive(Clone, Debug)]
struct Listed {
    serial: u64,
    id: ContentId,
    local: bool,
}

/// The gateway's side of one session.
pub struct Gateway {
    cfg: GatewayConfig,
    upstreams: Upstreams,
    third_party: ThirdParty,
    upstream: Option<Box<dyn Upstream + Send>>,
    stream: Deframer,
    inbound: Inbound,
    state: State,
    hello: Option<Value>,
    address: String,
    caller_caps: Caps,
    listed: Vec<Listed>,
    /// The content IDs of the listed messages sent.
    sent: Vec<ContentId>,
    finished: Vec<(ContentId, Vec<u8>)>,
    receipts: Receipts,
    events: Vec<Event>,
    notes: Vec<Note>,
    queued: Vec<Vec<u8>>,
}

fn entry_of(e: &Json) -> Option<(WaitingEntry, bool)> {
    let held = e.get("held").and_then(Json::as_bool).unwrap_or(false);
    let third = e
        .get("third_party")
        .and_then(Json::as_bool)
        .unwrap_or(false);
    let mut f = 0;
    if held {
        f |= flags::HELD;
    }
    if e.get("copy").and_then(Json::as_bool) == Some(true) {
        f |= flags::COPY;
    }
    let local = e.get("local").and_then(Json::as_bool) == Some(true);
    if local {
        f |= flags::LOCAL;
    }
    if third {
        f |= flags::THIRD_PARTY;
    }
    let id = ContentId::from_text(e.get("id")?.as_str()?)?;
    Some((
        WaitingEntry {
            // a local message has no serial (RP-7 §4.4)
            serial: if local { 0 } else { e.get("serial")?.as_u64()? },
            id,
            from: e.get("from")?.as_str()?.to_owned(),
            subject: super::subject40(e.get("subject").and_then(Json::as_str).unwrap_or("")),
            size: e.get("size")?.as_u64()?,
            priority: e.get("priority").and_then(Json::as_u64).unwrap_or(0),
            flags: f,
        },
        third,
    ))
}

impl Gateway {
    /// A gateway ready for a caller's `HELLO`. `upstreams` finds a domain's post office;
    /// `third_party` says whether a callsign may be offered third-party mail (RP-6 §5.1).
    #[must_use]
    pub fn new(cfg: GatewayConfig, upstreams: Upstreams, third_party: ThirdParty) -> Gateway {
        Gateway {
            cfg,
            upstreams,
            third_party,
            upstream: None,
            stream: Deframer::new(),
            inbound: Inbound::default(),
            state: State::Hello,
            hello: None,
            address: String::new(),
            caller_caps: Caps::from_value(None),
            listed: Vec::new(),
            sent: Vec::new(),
            finished: Vec::new(),
            receipts: Receipts::default(),
            events: Vec::new(),
            notes: Vec::new(),
            queued: Vec::new(),
        }
    }

    /// Bytes the link delivered. Returns what happened.
    pub fn receive(&mut self, bytes: &[u8]) -> Vec<Event> {
        self.stream.push(bytes);
        let max = usize::try_from(self.cfg.caps.max_pdu).unwrap_or(usize::MAX);
        while self.state != State::Over {
            match self.stream.next(max) {
                None => break,
                Some(Ok(pdu)) => self.handle(&pdu),
                Some(Err(code)) => self.fail(code),
            }
        }
        std::mem::take(&mut self.events)
    }

    /// The next transmission to write to the link, when one is due.
    pub fn transmit(&mut self) -> Option<Vec<u8>> {
        (!self.queued.is_empty()).then(|| self.queued.remove(0))
    }

    /// The account the caller's credential names, once read (RP-3 §5.1): whether the post office
    /// let the session open is what the session's events and notes say.
    #[must_use]
    pub fn account(&self) -> Option<&str> {
        (!self.address.is_empty()).then_some(self.address.as_str())
    }

    /// What to keep beyond the session: credentials seen, pieces of uploads.
    pub fn take_notes(&mut self) -> Vec<Note> {
        std::mem::take(&mut self.notes)
    }

    /// The caller hung up, or the link broke: uploads held in part are kept (RP-3 §7), and the
    /// session at the post office ends (RP-5 §5.7).
    pub fn link_closed(&mut self) -> Vec<Event> {
        let inbound = std::mem::take(&mut self.inbound);
        for p in inbound.held() {
            self.notes.push(Note::Upload {
                address: self.address.clone(),
                id: p.id,
                held: p.held,
                total: p.total,
            });
        }
        if let Some(mut up) = self.upstream.take() {
            up.close();
        }
        self.state = State::Over;
        self.events.push(Event::End);
        std::mem::take(&mut self.events)
    }

    /// The caller refused by its operator's choice (wishlist W24): `refused-by-policy`, saying so.
    fn refuse(&mut self, entry: &str) {
        let text = format!(
            "{}'s operator does not carry sessions for {entry}",
            self.cfg.callsign
        );
        self.queued.push(transmission(&[error_pdu_saying(
            ErrorCode::RefusedByPolicy,
            &text,
        )]));
        self.events.push(Event::Refused(ErrorCode::RefusedByPolicy));
        self.notes.push(Note::Refused(entry.to_owned()));
        self.state = State::Over;
        if let Some(mut up) = self.upstream.take() {
            up.close();
        }
    }

    /// Whether the caller is one its operator refuses, by the address and callsign it gave; if so,
    /// it is refused.
    fn refuses(&mut self, address: &str, callsign: &str) -> bool {
        match refused_caller(&self.cfg.refused, address, callsign).map(str::to_owned) {
            Some(entry) => {
                self.refuse(&entry);
                true
            }
            None => false,
        }
    }

    fn fail(&mut self, code: ErrorCode) {
        self.queued.push(transmission(&[error_pdu(code)]));
        self.events.push(Event::Refused(code));
        self.state = State::Over;
        if let Some(mut up) = self.upstream.take() {
            up.close();
        }
    }

    fn handle(&mut self, pdu: &Value) {
        match (self.state, pdu.get(0).and_then(Value::as_u64)) {
            (State::Hello, Some(kind::HELLO)) => self.hello(pdu),
            (State::Credential, Some(kind::CREDENTIAL)) => self.credential_sent(pdu),
            (State::Challenged, Some(kind::AUTH)) => {
                let signature = pdu.get(1).and_then(Value::as_bytes).map(<[u8]>::to_vec);
                self.open(signature.as_deref(), Some(self.cfg.challenge));
            }
            (State::Exchange, Some(kind::MSG)) => self.msg(pdu),
            (State::Exchange, Some(kind::RECEIPT)) => self.picked(pdu),
            (State::Exchange, Some(kind::REQUEST)) => {
                let asked: Vec<u64> = pdu
                    .get(1)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_u64)
                    .collect();
                let files: Vec<(u64, Vec<u8>)> = pdu
                    .get(2)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|f| match f.as_array()? {
                        [serial, r] => Some((serial.as_u64()?, r.as_bytes()?.to_vec())),
                        _ => None,
                    })
                    .collect();
                // RP-3 §5.6 key 4: local messages, by content ID (RP-7 §4.4)
                let ids = listed_ids(pdu.get(4));
                self.answer(&asked, &ids, &files, true);
            }
            (State::Exchange, Some(kind::DONE)) => self.answer(&[], &[], &[], false),
            (_, Some(kind::ERROR)) => {
                let code = pdu
                    .get(1)
                    .and_then(Value::as_u64)
                    .and_then(ErrorCode::from_code);
                let text = pdu.get(2).and_then(Value::as_text).map(str::to_owned);
                self.events.push(Event::Error {
                    code: code.unwrap_or(ErrorCode::Internal),
                    text,
                });
            }
            _ => self.fail(ErrorCode::Malformed),
        }
    }

    fn hello(&mut self, pdu: &Value) {
        let speaks = pdu
            .get(1)
            .and_then(Value::as_array)
            .is_some_and(|v| match v {
                [lo, hi] => lo.as_u64() <= Some(VERSION) && Some(VERSION) <= hi.as_u64(),
                _ => false,
            });
        if !speaks {
            return self.fail(ErrorCode::UnsupportedVersion);
        }
        if pdu.get(2).and_then(Value::as_u64) != Some(0) {
            // Forwarding between gateways is RP-7's, not taken here.
            return self.fail(ErrorCode::RefusedByPolicy);
        }
        // the operator's refusals, by what the statement says (RP-3 §5.1 keys 3 and 4)
        let said = |k| pdu.get(k).and_then(Value::as_text).unwrap_or("").to_owned();
        if self.refuses(&said(3), &said(4)) {
            return;
        }
        self.caller_caps = Caps::from_value(pdu.get(12));
        let mut hello = pdu.clone();
        if hello.get(13).is_none() {
            let hash = hello.get(14).and_then(Value::as_bytes).map(<[u8]>::to_vec);
            let held = self
                .cfg
                .credentials
                .iter()
                .find(|c| hash.as_deref() == Some(credential::hash(c).as_slice()))
                .cloned();
            let Some(cred) = held else {
                // It does not hold the credential the caller named: ask for it (RP-3 §5.8).
                self.hello = Some(hello);
                self.state = State::Credential;
                self.queued
                    .push(transmission(&[self.welcome_waiting(2, None)]));
                return;
            };
            hello = with_credential(&hello, cred);
        }
        self.hello = Some(hello);
        let signature = pdu.get(15).and_then(Value::as_bytes).map(<[u8]>::to_vec);
        if signature.is_none() {
            return self.challenge();
        }
        self.open(signature.as_deref(), None);
    }

    fn credential_sent(&mut self, pdu: &Value) {
        let (Some(hello), Some(cred)) = (
            self.hello.take(),
            pdu.get(1).and_then(Value::as_bytes).map(<[u8]>::to_vec),
        ) else {
            return self.fail(ErrorCode::Malformed);
        };
        let hello = with_credential(&hello, cred);
        let signature = hello.get(15).and_then(Value::as_bytes).map(<[u8]>::to_vec);
        self.hello = Some(hello);
        if signature.is_none() {
            return self.challenge();
        }
        self.open(signature.as_deref(), None);
    }

    /// RP-5 §5.2: the session opened at the user's post office, which checks the credential and
    /// the statement; then `WELCOME` with what it said.
    fn open(&mut self, signature: Option<&[u8]>, challenge: Option<[u8; 16]>) {
        let Some(hello) = self.hello.clone() else {
            return self.fail(ErrorCode::Malformed);
        };
        let Some(cred) = hello.get(13).and_then(Value::as_bytes).map(<[u8]>::to_vec) else {
            return self.fail(ErrorCode::CredentialRequired);
        };
        let Some(body) = Credential::decode(&cred).map(|c| c.body) else {
            return self.fail(ErrorCode::CredentialInvalid);
        };
        // and by what the credential says, which the post office holds the statement to
        if self.refuses(&body.address, &body.operator) {
            return;
        }
        self.address.clone_from(&body.address);
        let text = |k| {
            hello
                .get(k)
                .and_then(Value::as_text)
                .unwrap_or("")
                .to_owned()
        };
        let offers: Vec<ContentId> = hello
            .get(8)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|o| ContentId::from_slice(o.as_array()?.first()?.as_bytes()?))
            .collect();
        let form = if let Some(c) = challenge {
            Form::Challenge(c)
        } else {
            let nonce = hello
                .get(7)
                .and_then(Value::as_bytes)
                .and_then(|n| <[u8; 8]>::try_from(n).ok());
            let time = hello.get(6).and_then(Value::as_u64);
            let (Some(nonce), Some(time)) = (nonce, time) else {
                return self.fail(ErrorCode::Malformed);
            };
            Form::Time { time, nonce }
        };
        // RP-1 §6.3 step 5: a clock more than 600 s off is answered with a challenge (RP-3 §5.2)
        if let Form::Time { time, .. } = form
            && time.abs_diff(self.cfg.time) > TIME_WINDOW_S
        {
            return self.challenge();
        }
        let stmt = Statement {
            address: text(3),
            operator: text(4),
            gateway: text(5),
            messages: offers,
            form,
        };
        let partials: Vec<Json> = listed_pairs(hello.get(11))
            .iter()
            .map(|(id, offset)| json!({"id": id.text(), "offset": offset}))
            .collect();
        let mut request = json!({
            "statement": base64url::encode(&stmt.encode()),
            "signature": base64url::encode(signature.unwrap_or_default()),
            "credential": base64url::encode(&cred),
            "started": self.cfg.time,
            "gateway_session": crate::cbor::hex(&self.cfg.session_id),
            "cursor": hello.get(9).and_then(Value::as_u64).unwrap_or(0),
        });
        if !partials.is_empty() {
            request["partials"] = Json::Array(partials);
        }
        if self.caller_caps.renew {
            request["renew"] = Json::Bool(true);
        }
        let domain = body
            .address
            .rsplit_once('@')
            .map_or("", |(_, d)| d)
            .to_owned();
        let answer = (self.upstreams)(&domain).and_then(|mut up| {
            let a = up.open(&request)?;
            self.upstream = Some(up);
            Ok(a)
        });
        match answer {
            Ok(a) => {
                self.notes.push(Note::Credential(cred));
                self.welcome_ready(&hello, &a);
            }
            // the post office's replay cache saw the nonce, or its clock disagrees: a challenge
            Err(ErrorCode::TimeSkew) if challenge.is_none() => self.challenge(),
            Err(code) => {
                self.notes.push(Note::PostOffice(code));
                self.fail(code);
            }
        }
    }

    /// `WELCOME` `sign`: the caller signs the gateway's challenge (RP-3 §5.2, §5.3).
    fn challenge(&mut self) {
        self.state = State::Challenged;
        self.queued.push(transmission(&[
            self.welcome_waiting(1, Some(self.cfg.challenge))
        ]));
    }

    /// A `WELCOME` that is not `ready`: `sign` (1) or `credential` (2).
    fn welcome_waiting(&self, status: u64, challenge: Option<[u8; 16]>) -> Value {
        let mut entries = vec![
            (0, Value::Uint(kind::WELCOME)),
            (1, Value::Uint(VERSION)),
            (2, Value::bytes(&self.cfg.session_id)),
            (3, Value::Uint(status)),
            (10, Value::Uint(self.cfg.time)),
            (11, Value::Bool(true)),
            (12, Value::Bool(false)),
        ];
        if let Some(c) = challenge {
            entries.push((4, Value::bytes(&c)));
        }
        Value::int_map(entries)
    }

    /// The waiting entries the post office (or the gateway's own store) gave that this caller may
    /// be handed, kept as listed; and how many third-party ones were held back (RP-6 §5.1).
    fn list(&mut self, hello: &Value, answer: &Json) -> (Vec<WaitingEntry>, usize) {
        let operator = hello
            .get(4)
            .and_then(Value::as_text)
            .unwrap_or("")
            .to_owned();
        let may_list_third_party = (self.third_party)(&operator);
        let mut held_back = 0;
        let mut entries = Vec::new();
        for e in answer
            .get("waiting")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            let Some((entry, third)) = entry_of(e) else {
                continue;
            };
            if third && !may_list_third_party {
                held_back += 1;
                continue; // RP-6 §5.1: not listed for this station
            }
            self.listed.push(Listed {
                serial: entry.serial,
                id: entry.id,
                local: entry.flags & flags::LOCAL != 0,
            });
            entries.push(entry);
        }
        (entries, held_back)
    }

    fn welcome_ready(&mut self, hello: &Value, answer: &Json) {
        let online = self.upstream.as_ref().is_none_or(|u| u.online());
        let (entries, held_back) = self.list(hello, answer);
        // An upload broken in an earlier session resumes from what this gateway holds (RP-3 §7).
        let accept: Vec<Value> = answer
            .get("accept")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(|a| {
                let id = ContentId::from_text(a.get("id")?.as_str()?)?;
                let (held, total) = self
                    .cfg
                    .uploads
                    .get(&(self.address.clone(), id))
                    .cloned()
                    .unwrap_or_default();
                if !held.is_empty() {
                    self.inbound.resume_one(id, held.clone(), total);
                }
                Some(Value::Array(vec![
                    Value::bytes(id.as_bytes()),
                    Value::Uint(held.len() as u64),
                ]))
            })
            .collect();
        let decline: Vec<Value> = answer
            .get("decline")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(|d| {
                let id = ContentId::from_text(d.get("id")?.as_str()?)?;
                Some(Value::Array(vec![
                    Value::bytes(id.as_bytes()),
                    Value::Uint(d.get("code")?.as_u64()?),
                ]))
            })
            .collect();
        let more = answer.get("more").and_then(Json::as_u64).unwrap_or(0);
        let welcome = Value::int_map(vec![
            (0, Value::Uint(kind::WELCOME)),
            (1, Value::Uint(VERSION)),
            (2, Value::bytes(&self.cfg.session_id)),
            (3, Value::Uint(0)),
            (5, Value::Array(accept)),
            (6, Value::Array(decline)),
            (
                7,
                Value::Array(entries.iter().map(WaitingEntry::to_value).collect()),
            ),
            (8, Value::Uint(more)),
            (9, self.cfg.caps.to_value()),
            (10, Value::Uint(self.cfg.time)),
            (11, Value::Bool(online)),
            (12, Value::Bool(true)),
        ]);
        let mut pdus = vec![welcome];
        if let Some(renewed) = answer
            .get("credential")
            .and_then(Json::as_str)
            .and_then(base64url::decode)
        {
            pdus.push(Value::int_map(vec![
                (0, Value::Uint(kind::CREDENTIAL)),
                (1, Value::bytes(&renewed)),
            ]));
        }
        pdus.extend(self.notices(answer, held_back));
        // Unasked: what fits the caller's `auto`, and the rest of what it holds in part (§5.2).
        let auto = hello.get(10).and_then(Value::as_u64).unwrap_or(0);
        let partials = listed_pairs(hello.get(11));
        for e in &entries {
            let resume = partials.iter().find(|(id, _)| *id == e.id).map(|(_, o)| *o);
            if e.size <= auto || resume.is_some() {
                pdus.extend(self.message_pdus(&e.id, resume.unwrap_or(0)));
            }
        }
        pdus.push(done());
        self.queued.push(transmission(&pdus));
        self.state = State::Exchange;
    }

    /// Why the gateway answers from what it holds (RP-3 §9), and the third-party mail it held back
    /// (RP-6 §5.1), as `NOTICE`s: said before the mail, so the caller reads them first.
    fn notices(&mut self, answer: &Json, held_back: usize) -> Vec<Value> {
        let mut texts: Vec<String> = answer
            .get("notice")
            .and_then(Json::as_str)
            .map(str::to_owned)
            .into_iter()
            .collect();
        if held_back > 0 {
            texts.push(third_party_notice(held_back));
        }
        texts
            .into_iter()
            .map(|text| {
                self.events.push(Event::Notice(text.clone()));
                Value::int_map(vec![(0, Value::Uint(kind::NOTICE)), (1, Value::Text(text))])
            })
            .collect()
    }

    /// A listed message's `MSG` pieces from `offset`, fetched from the post office, or, for a
    /// local message, from the gateway's own store and sent without a serial (RP-7 §4.4).
    fn message_pdus(&mut self, id: &ContentId, offset: u64) -> Vec<Value> {
        let Some(listed) = self.listed.iter().find(|l| l.id == *id).cloned() else {
            return Vec::new();
        };
        let Some(up) = self.upstream.as_mut() else {
            return Vec::new();
        };
        let fetched = if listed.local {
            up.fetch_local(id)
        } else {
            up.fetch(listed.serial)
        };
        let Ok(air) = fetched else {
            return Vec::new();
        };
        let Ok(message) = Message::from_air(&air) else {
            return Vec::new();
        };
        // the post office's air form, as it made it once (RP-5 §5.3)
        let out = Outgoing { message, air };
        self.sent.push(*id);
        let piece = piece_for(self.caller_caps.max_pdu, self.cfg.piece);
        msg_pdus(
            &out,
            offset,
            piece,
            (!listed.local).then_some(listed.serial),
        )
    }

    fn msg(&mut self, pdu: &Value) {
        match self.inbound.take(pdu) {
            Some(Ok((_, message, air))) => {
                let id = message.content_id();
                self.notes.push(Note::Upload {
                    address: self.address.clone(),
                    id,
                    held: Vec::new(),
                    total: 0,
                });
                self.finished.push((id, air));
            }
            Some(Err((id, code))) => {
                self.receipts.failed.push((id, code));
                self.events.push(Event::Failed { id, code });
            }
            None => {}
        }
    }

    /// The caller's `RECEIPT`: what it picked up, reported to the post office (RP-5 §5.5); local
    /// messages kept as local pickups (RP-7 §4.4).
    fn picked(&mut self, pdu: &Value) {
        let complete = listed_ids(pdu.get(1));
        let picked: Vec<Listed> = complete
            .iter()
            .filter_map(|id| self.listed.iter().find(|l| l.id == *id))
            .cloned()
            .collect();
        let serials: Vec<u64> = picked
            .iter()
            .filter(|l| !l.local)
            .map(|l| l.serial)
            .collect();
        let local: Vec<ContentId> = picked.iter().filter(|l| l.local).map(|l| l.id).collect();
        for id in complete {
            self.events.push(Event::Delivered(id));
        }
        if let Some(up) = self.upstream.as_mut() {
            if !serials.is_empty() {
                let _ = up.picked_up(&serials);
            }
            if !local.is_empty() {
                let _ = up.picked_up_local(&local);
            }
        }
    }

    /// The caller's turn ended with `DONE` or `REQUEST`: its mail handed in and receipted, then
    /// what it asked for, a `NOTICE` saying how its held attachments were asked for, and `DONE`.
    fn answer(
        &mut self,
        asked: &[u64],
        asked_ids: &[ContentId],
        files: &[(u64, Vec<u8>)],
        requested: bool,
    ) {
        let finished = std::mem::take(&mut self.finished);
        if !finished.is_empty() {
            let results = match self.upstream.as_mut() {
                Some(up) => up.hand_in(&finished),
                None => Err(ErrorCode::PostOfficeUnreachable),
            };
            for (i, (id, air)) in finished.iter().enumerate() {
                let result = match &results {
                    Ok(r) => r.get(i).copied().unwrap_or(Err(ErrorCode::Internal)),
                    Err(code) => Err(*code),
                };
                match result {
                    Ok(()) => {
                        self.receipts.complete.push(*id);
                        if let Ok(message) = Message::from_air(air) {
                            self.events.push(Event::Received {
                                serial: None,
                                message,
                                air: air.clone(),
                            });
                        }
                    }
                    Err(code) => {
                        self.receipts.failed.push((*id, code));
                        self.events.push(Event::Failed { id: *id, code });
                    }
                }
            }
        }
        let mut pdus: Vec<Value> = self.receipts.take_pdu().into_iter().collect();
        if requested {
            // by serial what the post office listed, and by content ID what is local (§5.6)
            let wanted: Vec<ContentId> = self
                .listed
                .iter()
                .filter(|l| {
                    if l.local {
                        asked_ids.contains(&l.id)
                    } else {
                        asked.contains(&l.serial)
                    }
                })
                .map(|l| l.id)
                .filter(|id| !self.sent.contains(id))
                .collect();
            for id in wanted {
                pdus.extend(self.message_pdus(&id, 0));
            }
            if !files.is_empty() {
                let result = match self.upstream.as_mut() {
                    Some(up) => up.files(files),
                    None => Err(ErrorCode::PostOfficeUnreachable),
                };
                let text = files_notice(result);
                self.events.push(Event::Notice(text.clone()));
                pdus.push(Value::int_map(vec![
                    (0, Value::Uint(kind::NOTICE)),
                    (1, Value::Text(text)),
                ]));
            }
            pdus.push(done());
        }
        if !pdus.is_empty() {
            self.queued.push(transmission(&pdus));
        }
    }
}

/// What a `NOTICE` tells a station abroad of the third-party mail held back for it (RP-6 §5.1).
#[must_use]
pub fn third_party_notice(held: usize) -> String {
    let what = if held == 1 {
        "1 message waits".to_owned()
    } else {
        format!("{held} messages wait")
    };
    format!(
        "{what} for you from senders not shown to be licensed. Third-party rules keep this \
         gateway from handing them to a station in your country: they stay at your post office."
    )
}

/// What a `NOTICE` tells the user of held attachments asked for (RP-3 §5.6).
#[must_use]
pub fn files_notice(result: Result<(), ErrorCode>) -> String {
    match result {
        Ok(()) => "Your attachments are in a new message, waiting for your next call.".into(),
        Err(ErrorCode::TooLarge) => {
            "An attachment you asked for is too large to send by radio: nothing was sent.".into()
        }
        Err(ErrorCode::UnknownMessage) => {
            "The attachments you asked for are not held for a message of this call.".into()
        }
        Err(ErrorCode::PostOfficeUnreachable) => {
            "Your post office cannot be reached now: ask for the attachments on a later call."
                .into()
        }
        Err(code) => format!("Your attachments could not be asked for ({code})."),
    }
}

/// `HELLO` with the full credential in place of its hash.
fn with_credential(hello: &Value, credential: Vec<u8>) -> Value {
    let mut entries = hello.as_map().map(<[_]>::to_vec).unwrap_or_default();
    entries.retain(|(k, _)| *k != Value::Uint(14) && *k != Value::Uint(13));
    entries.push((Value::Uint(13), Value::Bytes(credential)));
    Value::Map(entries)
}
