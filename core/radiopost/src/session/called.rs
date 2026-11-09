// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-3 §10: a client that answers a call, station to station.

use ed25519_dalek::SigningKey;

use super::{
    Caps, Event, Inbound, Outgoing, Receipts, VERSION, WaitingEntry, done, error_pdu, listed_ids,
    listed_pairs, msg_pdus, piece_for, transmission,
};
use crate::cbor::Value;
use crate::credential::{self, Body, Credential, Trust, flags};
use crate::crypto::ContentId;
use crate::error::ErrorCode;
use crate::pdu::{Deframer, kind};
use crate::statement::{self, Form, Freshness, Statement};

/// What a client brings to answering a call.
pub struct CalledConfig {
    /// Its credential (RP-1 §4).
    pub credential: Vec<u8>,
    /// Its device key.
    pub device: SigningKey,
    /// Its clock.
    pub time: u64,
    /// A random session ID (`WELCOME` key 2).
    pub session_id: [u8; 8],
    /// A random challenge, used if the caller's statement must be signed again (RP-3 §5.3).
    pub challenge: [u8; 16],
    /// The post office keys and revocations it holds, to check the caller offline (RP-1 §5).
    pub trust: Trust,
    /// The (address, nonce) pairs seen in the last 20 minutes (RP-1 §6.3).
    pub seen: Vec<(String, [u8; 8])>,
    /// What it accepts.
    pub caps: Caps,
    /// The most data bytes it puts in one `MSG`, below what the caller accepts.
    pub piece: usize,
}

/// The messages its user addressed to an account, each with a serial of its own (RP-3 §10).
pub type MailFor = Box<dyn FnMut(&str) -> Vec<(u64, Outgoing)>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Hello,
    Challenged,
    Exchange,
    Over,
}

/// The side of a station-to-station session that was called.
pub struct Called {
    cfg: CalledConfig,
    mail_for: MailFor,
    me: Body,
    stream: Deframer,
    inbound: Inbound,
    state: State,
    hello: Option<Value>,
    caller: Option<Body>,
    waiting: Vec<(u64, Outgoing)>,
    sent: Vec<u64>,
    caller_caps: Caps,
    receipts: Receipts,
    events: Vec<Event>,
    queued: Vec<Vec<u8>>,
}

impl Called {
    /// A client ready to answer.
    ///
    /// # Errors
    /// `credential-invalid` when its own credential does not decode.
    pub fn new(cfg: CalledConfig, mail_for: MailFor) -> Result<Self, ErrorCode> {
        let me = Credential::decode(&cfg.credential)
            .ok_or(ErrorCode::CredentialInvalid)?
            .body;
        Ok(Called {
            cfg,
            mail_for,
            me,
            stream: Deframer::new(),
            inbound: Inbound::default(),
            state: State::Hello,
            hello: None,
            caller: None,
            waiting: Vec::new(),
            sent: Vec::new(),
            caller_caps: Caps::from_value(None),
            receipts: Receipts::default(),
            events: Vec::new(),
            queued: Vec::new(),
        })
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

    /// The caller hung up, or the link broke: what was received in part.
    pub fn link_closed(&mut self) -> Vec<Event> {
        let inbound = std::mem::take(&mut self.inbound);
        self.events
            .extend(inbound.held().into_iter().map(Event::Held));
        self.state = State::Over;
        self.events.push(Event::End);
        std::mem::take(&mut self.events)
    }

    /// The caller's credential body, once checked.
    #[must_use]
    pub fn caller(&self) -> Option<&Body> {
        self.caller.as_ref()
    }

    fn fail(&mut self, code: ErrorCode) {
        self.queued.push(transmission(&[error_pdu(code)]));
        self.state = State::Over;
        self.events.push(Event::End);
    }

    fn handle(&mut self, pdu: &Value) {
        match (self.state, pdu.get(0).and_then(Value::as_u64)) {
            (State::Hello, Some(kind::HELLO)) => self.hello(pdu),
            (State::Challenged, Some(kind::AUTH)) => self.auth(pdu),
            (State::Hello, Some(kind::CREDENTIAL)) => self.credential_sent(pdu),
            (State::Exchange, Some(kind::MSG)) => self.msg(pdu),
            (State::Exchange, Some(kind::RECEIPT)) => {
                for id in listed_ids(pdu.get(1)) {
                    self.events.push(Event::Delivered(id));
                }
            }
            (State::Exchange, Some(kind::REQUEST)) => {
                let asked: Vec<u64> = pdu
                    .get(1)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_u64)
                    .collect();
                self.answer(&asked);
            }
            (State::Exchange, Some(kind::DONE)) => self.answer(&[]),
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
        let versions = pdu.get(1).and_then(Value::as_array);
        let speaks = versions.is_some_and(|v| match v {
            [lo, hi] => lo.as_u64() <= Some(VERSION) && Some(VERSION) <= hi.as_u64(),
            _ => false,
        });
        if !speaks {
            return self.fail(ErrorCode::UnsupportedVersion);
        }
        if pdu.get(2).and_then(Value::as_u64) != Some(0) {
            // A client takes no forwarding (RP-7 §2).
            return self.fail(ErrorCode::RefusedByPolicy);
        }
        self.caller_caps = Caps::from_value(pdu.get(12));
        self.hello = Some(pdu.clone());
        if pdu.get(13).is_none() {
            // It holds no one's credential: ask for it (RP-3 §5.8).
            self.queued
                .push(transmission(&[self.welcome_waiting(2, None)]));
            return;
        }
        self.check(pdu.get(15).and_then(Value::as_bytes), None);
    }

    fn credential_sent(&mut self, pdu: &Value) {
        let (Some(hello), Some(cred)) = (self.hello.take(), pdu.get(1).cloned()) else {
            return self.fail(ErrorCode::Malformed);
        };
        let mut entries = hello.as_map().map(<[_]>::to_vec).unwrap_or_default();
        entries.retain(|(k, _)| *k != Value::Uint(14));
        entries.push((Value::Uint(13), cred));
        let hello = Value::Map(entries);
        self.hello = Some(hello.clone());
        self.check(hello.get(15).and_then(Value::as_bytes), None);
    }

    fn auth(&mut self, pdu: &Value) {
        self.check(
            pdu.get(1).and_then(Value::as_bytes),
            Some(self.cfg.challenge),
        );
    }

    /// The caller's credential and statement, as a gateway checks them offline (RP-1 §5, §6.3).
    fn check(&mut self, signature: Option<&[u8]>, challenge: Option<[u8; 16]>) {
        let Some(hello) = self.hello.clone() else {
            return self.fail(ErrorCode::Malformed);
        };
        let text = |k| {
            hello
                .get(k)
                .and_then(Value::as_text)
                .unwrap_or_default()
                .to_owned()
        };
        let offers: Vec<ContentId> = hello
            .get(8)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|o| ContentId::from_slice(o.as_array()?.first()?.as_bytes()?))
            .collect();
        let Some(cred) = hello.get(13).and_then(Value::as_bytes) else {
            return self.fail(ErrorCode::CredentialRequired);
        };
        let address = Credential::decode(cred)
            .map(|c| c.body.address)
            .unwrap_or_default();
        self.waiting = (self.mail_for)(&address);
        let mut need = 0;
        if !offers.is_empty() {
            need |= flags::SEND;
        }
        if !self.waiting.is_empty() {
            need |= flags::RECEIVE;
        }
        let body = match credential::verify(cred, &self.cfg.trust, self.cfg.time, need) {
            Ok(body) => body,
            Err(refused) => return self.fail(refused.code),
        };
        let form = if let Some(c) = challenge {
            Form::Challenge(c)
        } else {
            let nonce = hello
                .get(7)
                .and_then(Value::as_bytes)
                .and_then(|n| n.try_into().ok());
            let time = hello.get(6).and_then(Value::as_u64);
            let (Some(nonce), Some(time)) = (nonce, time) else {
                return self.fail(ErrorCode::Malformed);
            };
            Form::Time { time, nonce }
        };
        let stmt = Statement {
            address: text(3),
            operator: text(4),
            gateway: text(5),
            messages: offers,
            form,
        };
        let freshness = Freshness {
            now: self.cfg.time,
            seen: &self.cfg.seen,
            challenge,
            sent_nonce: None,
        };
        let verdict = match signature {
            Some(sig) => statement::verify(&stmt, sig, &body, &self.me.operator, &freshness),
            None => Err(ErrorCode::TimeSkew),
        };
        match verdict {
            Ok(()) => {
                self.events.push(Event::StationProved(body.clone()));
                self.caller = Some(body);
                self.welcome_ready(&hello);
            }
            Err(ErrorCode::TimeSkew) if challenge.is_none() => {
                // Its clock is out, or it did not sign: ask it to sign a challenge (RP-3 §5.3).
                self.state = State::Challenged;
                self.queued.push(transmission(&[
                    self.welcome_waiting(1, Some(self.cfg.challenge))
                ]));
            }
            Err(code) => self.fail(code),
        }
    }

    /// A `WELCOME` that is not `ready`: `sign` (1) or `credential` (2).
    fn welcome_waiting(&self, status: u64, challenge: Option<[u8; 16]>) -> Value {
        let mut entries = vec![
            (0, Value::Uint(kind::WELCOME)),
            (1, Value::Uint(VERSION)),
            (2, Value::bytes(&self.cfg.session_id)),
            (3, Value::Uint(status)),
            (10, Value::Uint(self.cfg.time)),
            (11, Value::Bool(false)),
            (12, Value::Bool(false)),
        ];
        if let Some(c) = challenge {
            entries.push((4, Value::bytes(&c)));
        }
        Value::int_map(entries)
    }

    fn welcome_ready(&mut self, hello: &Value) {
        let entries: Vec<WaitingEntry> = self
            .waiting
            .iter()
            .map(|(s, out)| WaitingEntry::for_outgoing(*s, out, 0))
            .collect();
        let nonce = hello
            .get(7)
            .and_then(Value::as_bytes)
            .and_then(|n| n.try_into().ok());
        let caller_operator = hello.get(4).and_then(Value::as_text).unwrap_or_default();
        let stmt = Statement {
            address: self.me.address.clone(),
            operator: self.me.operator.clone(),
            gateway: caller_operator.to_owned(),
            messages: entries.iter().map(|e| e.id).collect(),
            form: Form::Time {
                time: self.cfg.time,
                nonce: nonce.unwrap_or_default(),
            },
        };
        // It cannot tell from an offer whom a message is for, so it takes what it has room for.
        let accept = hello
            .get(8)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|o| o.as_array()?.first().cloned())
            .map(|id| Value::Array(vec![id, Value::Uint(0)]))
            .collect();
        let welcome = Value::int_map(vec![
            (0, Value::Uint(kind::WELCOME)),
            (1, Value::Uint(VERSION)),
            (2, Value::bytes(&self.cfg.session_id)),
            (3, Value::Uint(0)),
            (5, Value::Array(accept)),
            (6, Value::Array(vec![])),
            (
                7,
                Value::Array(entries.iter().map(WaitingEntry::to_value).collect()),
            ),
            (8, Value::Uint(0)),
            (9, self.cfg.caps.to_value()),
            (10, Value::Uint(self.cfg.time)),
            (11, Value::Bool(false)),
            (12, Value::Bool(false)),
            (13, Value::bytes(&self.cfg.credential)),
            (15, Value::bytes(&stmt.sign(&self.cfg.device))),
        ]);
        // Unasked: what fits the caller's `auto`, and the rest of what it holds in part (§5.2).
        let auto = hello.get(10).and_then(Value::as_u64).unwrap_or(0);
        let partials = listed_pairs(hello.get(11));
        let piece = piece_for(self.caller_caps.max_pdu, self.cfg.piece);
        let mut pdus = vec![welcome];
        for (serial, out) in &self.waiting {
            let resume = partials
                .iter()
                .find(|(id, _)| *id == out.id())
                .map(|(_, o)| *o);
            if out.air.len() as u64 <= auto || resume.is_some() {
                pdus.extend(msg_pdus(out, resume.unwrap_or(0), piece, Some(*serial)));
                self.sent.push(*serial);
            }
        }
        pdus.push(done());
        self.queued.push(transmission(&pdus));
        self.state = State::Exchange;
    }

    fn msg(&mut self, pdu: &Value) {
        match self.inbound.take(pdu) {
            Some(Ok((_, message))) => {
                let id = message.content_id();
                if message.recipients().contains(&self.me.address.as_str()) {
                    self.receipts.complete.push(id);
                    self.events.push(Event::Received {
                        serial: None,
                        message,
                    });
                } else {
                    // Not for this account: refused and dropped (RP-3 §10).
                    self.receipts.failed.push((id, ErrorCode::RefusedByPolicy));
                    self.events.push(Event::Failed {
                        id,
                        code: ErrorCode::RefusedByPolicy,
                    });
                }
            }
            Some(Err((id, code))) => {
                self.receipts.failed.push((id, code));
                self.events.push(Event::Failed { id, code });
            }
            None => {}
        }
    }

    /// The caller's turn ended with `DONE` or `REQUEST`: receipts, then what it asked for.
    fn answer(&mut self, asked: &[u64]) {
        let mut pdus: Vec<Value> = self.receipts.take_pdu().into_iter().collect();
        if !asked.is_empty() {
            let piece = piece_for(self.caller_caps.max_pdu, self.cfg.piece);
            for (serial, out) in &self.waiting {
                if asked.contains(serial) && !self.sent.contains(serial) {
                    pdus.extend(msg_pdus(out, 0, piece, Some(*serial)));
                    self.sent.push(*serial);
                }
            }
            pdus.push(done());
        }
        if !pdus.is_empty() {
            self.queued.push(transmission(&pdus));
        }
    }
}
