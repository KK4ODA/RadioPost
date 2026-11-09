// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-3, the caller's side: a client calling a gateway, or another client (§10).

use std::collections::BTreeSet;

use ed25519_dalek::SigningKey;

use super::{
    Caps, Event, Inbound, Outgoing, Partial, Receipts, VERSION, WaitingEntry, done, error_pdu,
    listed_ids, listed_pairs, msg_pdus, piece_for, transmission,
};
use crate::cbor::Value;
use crate::credential::{self, Body, Credential, Trust, flags};
use crate::crypto::ContentId;
use crate::error::ErrorCode;
use crate::pdu::{Deframer, kind};
use crate::statement::{self, Form, Freshness, Statement};

/// What a client brings to a session.
pub struct ClientConfig {
    /// Its credential (RP-1 §4).
    pub credential: Vec<u8>,
    /// Its device key.
    pub device: SigningKey,
    /// The callsign of the station it calls.
    pub called: String,
    /// Its clock.
    pub time: u64,
    /// A random nonce, new for this session.
    pub nonce: [u8; 8],
    /// Its messages to send, in the order to send them (RP-3 §6.1).
    pub outbox: Vec<Outgoing>,
    /// The highest mailbox serial it has picked up (RP-3 §6.2).
    pub cursor: u64,
    /// Waiting messages up to this many bytes come unasked; 0 lists only.
    pub auto: u64,
    /// Messages an earlier session left part-received (RP-3 §7).
    pub partials: Vec<Partial>,
    /// What it accepts.
    pub caps: Caps,
    /// Whether the station called holds its credential already, so the hash will do (RP-3 §5.1).
    pub credential_cached: bool,
    /// The most data bytes it puts in one `MSG`, below what the other station accepts.
    pub piece: usize,
    /// Calling another client directly (RP-3 §10): the keys to check that station's proof with.
    pub station: Option<Trust>,
}

/// Which waiting messages to ask for, from those that will not come unasked. The client cannot
/// stop to ask its user mid-turn, so this is the user's standing choice (for example: everything
/// up to some size, or what the user marked in the last listing).
pub type Chooser = Box<dyn FnMut(&[WaitingEntry]) -> Vec<u64>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Start,
    Welcome,
    Exchange,
    Over,
}

/// The caller's side of a session.
pub struct Client {
    cfg: ClientConfig,
    choose: Chooser,
    me: Body,
    stream: Deframer,
    inbound: Inbound,
    state: State,
    waiting_reply: bool,
    to_send: Option<Vec<(usize, u64)>>,
    peer_caps: Caps,
    requests: Vec<u64>,
    awaiting_done: bool,
    awaiting_receipt: BTreeSet<ContentId>,
    sent_done: bool,
    receipts: Receipts,
    events: Vec<Event>,
    queued: Vec<Vec<u8>>,
}

impl Client {
    /// A client ready to call.
    ///
    /// # Errors
    /// `credential-invalid` when its own credential does not decode.
    pub fn new(cfg: ClientConfig, choose: Chooser) -> Result<Self, ErrorCode> {
        let me = Credential::decode(&cfg.credential)
            .ok_or(ErrorCode::CredentialInvalid)?
            .body;
        let inbound = Inbound::resume(&cfg.partials);
        Ok(Client {
            cfg,
            choose,
            me,
            stream: Deframer::new(),
            inbound,
            state: State::Start,
            waiting_reply: false,
            to_send: None,
            peer_caps: Caps::from_value(None),
            requests: Vec::new(),
            awaiting_done: false,
            awaiting_receipt: BTreeSet::new(),
            sent_done: false,
            receipts: Receipts::default(),
            events: Vec::new(),
            queued: Vec::new(),
        })
    }

    fn statement(&self, form: Form) -> Statement {
        Statement {
            address: self.me.address.clone(),
            operator: self.me.operator.clone(),
            gateway: self.cfg.called.clone(),
            messages: self.cfg.outbox.iter().map(Outgoing::id).collect(),
            form,
        }
    }

    /// The first transmission: `HELLO`, signed in time form (RP-3 §5.1).
    pub fn hello(&mut self) -> Vec<u8> {
        let stmt = self.statement(Form::Time {
            time: self.cfg.time,
            nonce: self.cfg.nonce,
        });
        let mut entries = vec![
            (0, Value::Uint(kind::HELLO)),
            (
                1,
                Value::Array(vec![Value::Uint(VERSION), Value::Uint(VERSION)]),
            ),
            (2, Value::Uint(0)),
            (3, Value::text(&self.me.address)),
            (4, Value::text(&self.me.operator)),
            (5, Value::text(&self.cfg.called)),
            (6, Value::Uint(self.cfg.time)),
            (7, Value::bytes(&self.cfg.nonce)),
            (
                8,
                Value::Array(self.cfg.outbox.iter().map(Outgoing::offer).collect()),
            ),
            (9, Value::Uint(self.cfg.cursor)),
            (10, Value::Uint(self.cfg.auto)),
            (12, self.cfg.caps.to_value()),
            (15, Value::bytes(&stmt.sign(&self.cfg.device))),
        ];
        if !self.cfg.partials.is_empty() {
            let partials = self.cfg.partials.iter().map(|p| {
                Value::Array(vec![
                    Value::bytes(p.id.as_bytes()),
                    Value::Uint(p.held.len() as u64),
                ])
            });
            entries.push((11, Value::Array(partials.collect())));
        }
        if self.cfg.credential_cached {
            entries.push((14, Value::bytes(&credential::hash(&self.cfg.credential))));
        } else {
            entries.push((13, Value::bytes(&self.cfg.credential)));
        }
        self.state = State::Welcome;
        self.waiting_reply = true;
        transmission(&[Value::int_map(entries)])
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
        if self.state == State::Exchange {
            self.reply_when_due();
        }
        std::mem::take(&mut self.events)
    }

    /// The next transmission to write to the link, when one is due.
    pub fn transmit(&mut self) -> Option<Vec<u8>> {
        (!self.queued.is_empty()).then(|| self.queued.remove(0))
    }

    /// The link broke (RP-3 §7): what was received in part, to offer under `partials` next time.
    pub fn link_broken(&mut self) -> Vec<Event> {
        let inbound = std::mem::take(&mut self.inbound);
        self.events
            .extend(inbound.held().into_iter().map(Event::Held));
        self.state = State::Over;
        std::mem::take(&mut self.events)
    }

    /// Whether the session is over and the client hangs up (RP-3 §8).
    #[must_use]
    pub fn is_over(&self) -> bool {
        self.state == State::Over
    }

    fn end(&mut self) {
        self.state = State::Over;
        self.events.push(Event::End);
    }

    fn fail(&mut self, code: ErrorCode) {
        self.queued.push(transmission(&[error_pdu(code)]));
        self.end();
    }

    fn handle(&mut self, pdu: &Value) {
        match pdu.get(0).and_then(Value::as_u64) {
            Some(kind::WELCOME) if self.state == State::Welcome => self.welcome(pdu),
            Some(kind::MSG) if self.state == State::Exchange => match self.inbound.take(pdu) {
                Some(Ok((serial, message))) => {
                    self.receipts.complete.push(message.content_id());
                    self.events.push(Event::Received { serial, message });
                }
                Some(Err((id, code))) => {
                    self.receipts.failed.push((id, code));
                    self.events.push(Event::Failed { id, code });
                }
                None => {}
            },
            Some(kind::RECEIPT) => {
                for id in listed_ids(pdu.get(1)) {
                    self.awaiting_receipt.remove(&id);
                    self.events.push(Event::Delivered(id));
                }
                for (id, _) in listed_pairs(pdu.get(2)) {
                    self.awaiting_receipt.remove(&id);
                }
                for (id, code) in listed_pairs(pdu.get(3)) {
                    self.awaiting_receipt.remove(&id);
                    let code = ErrorCode::from_code(code).unwrap_or(ErrorCode::Internal);
                    self.events.push(Event::NotTaken { id, code });
                }
            }
            Some(kind::DONE) => self.awaiting_done = false,
            Some(kind::NOTICE) => {
                let text = pdu.get(1).and_then(Value::as_text).unwrap_or_default();
                self.events.push(Event::Notice(text.to_owned()));
            }
            Some(kind::CREDENTIAL) => {
                if let Some(c) = pdu.get(1).and_then(Value::as_bytes) {
                    self.events.push(Event::NewCredential(c.to_vec()));
                }
            }
            Some(kind::ERROR) => {
                let code = pdu
                    .get(1)
                    .and_then(Value::as_u64)
                    .and_then(ErrorCode::from_code);
                let text = pdu.get(2).and_then(Value::as_text).map(str::to_owned);
                self.events.push(Event::Error {
                    code: code.unwrap_or(ErrorCode::Internal),
                    text,
                });
                if pdu.get(4) != Some(&Value::Bool(false)) {
                    self.end();
                }
            }
            _ => self.fail(ErrorCode::Malformed),
        }
    }

    fn welcome(&mut self, pdu: &Value) {
        if pdu.get(1).and_then(Value::as_u64) != Some(VERSION) {
            return self.fail(ErrorCode::UnsupportedVersion);
        }
        match pdu.get(3).and_then(Value::as_u64) {
            Some(1) => {
                // Sign the gateway's challenge (RP-3 §5.3).
                let Some(challenge) = pdu.get(4).and_then(Value::as_bytes) else {
                    return self.fail(ErrorCode::Malformed);
                };
                let Ok(challenge) = <[u8; 16]>::try_from(challenge) else {
                    return self.fail(ErrorCode::Malformed);
                };
                let sig = self
                    .statement(Form::Challenge(challenge))
                    .sign(&self.cfg.device);
                let auth =
                    Value::int_map(vec![(0, Value::Uint(kind::AUTH)), (1, Value::bytes(&sig))]);
                self.queued.push(transmission(&[auth]));
            }
            Some(2) => {
                let cred = Value::int_map(vec![
                    (0, Value::Uint(kind::CREDENTIAL)),
                    (1, Value::bytes(&self.cfg.credential)),
                ]);
                self.queued.push(transmission(&[cred]));
            }
            Some(0) => self.ready(pdu),
            _ => self.fail(ErrorCode::Malformed),
        }
    }

    fn ready(&mut self, pdu: &Value) {
        let waiting: Vec<WaitingEntry> = pdu
            .get(7)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(WaitingEntry::from_value)
            .collect();
        let mut peer_account = None;
        if let Some(trust) = &self.cfg.station {
            match self.check_station(pdu, &waiting, trust) {
                Ok(body) => {
                    peer_account = Some(body.address.clone());
                    self.events.push(Event::StationProved(body));
                }
                Err(code) => return self.fail(code),
            }
        }
        self.peer_caps = Caps::from_value(pdu.get(9));
        for (id, code) in listed_pairs(pdu.get(6)) {
            let code = ErrorCode::from_code(code).unwrap_or(ErrorCode::Internal);
            self.events.push(Event::NotTaken { id, code });
        }
        // Of the offers taken, what to send and from where; station to station, only the
        // messages for the account reached (RP-3 §10).
        let accepted = listed_pairs(pdu.get(5));
        let to_send = self
            .cfg
            .outbox
            .iter()
            .enumerate()
            .filter_map(|(i, out)| {
                let (_, offset) = accepted.iter().find(|(id, _)| *id == out.id())?;
                let for_peer = peer_account
                    .as_deref()
                    .is_none_or(|a| out.message.recipients().contains(&a));
                for_peer.then_some((i, *offset))
            })
            .collect();
        self.to_send = Some(to_send);
        let held: Vec<ContentId> = self.cfg.partials.iter().map(|p| p.id).collect();
        let asked: Vec<WaitingEntry> = waiting
            .iter()
            .filter(|w| w.size > self.cfg.auto && !held.contains(&w.id))
            .cloned()
            .collect();
        let chosen = (self.choose)(&asked);
        self.requests = asked
            .iter()
            .map(|w| w.serial)
            .filter(|s| chosen.contains(s))
            .collect();
        self.events.push(Event::Listed(waiting));
        self.awaiting_done = true;
        self.state = State::Exchange;
    }

    /// RP-1 §6.5, RP-3 §10: the station called proves itself before anything is taken from it.
    fn check_station(
        &self,
        pdu: &Value,
        waiting: &[WaitingEntry],
        trust: &Trust,
    ) -> Result<Body, ErrorCode> {
        let cred = pdu
            .get(13)
            .and_then(Value::as_bytes)
            .ok_or(ErrorCode::NotAuthorized)?;
        let sig = pdu
            .get(15)
            .and_then(Value::as_bytes)
            .ok_or(ErrorCode::NotAuthorized)?;
        let time = pdu
            .get(10)
            .and_then(Value::as_u64)
            .ok_or(ErrorCode::Malformed)?;
        let mut need = 0;
        if !waiting.is_empty() {
            need |= flags::SEND;
        }
        if !self.cfg.outbox.is_empty() {
            need |= flags::RECEIVE;
        }
        let body = credential::verify(cred, trust, self.cfg.time, need).map_err(|r| r.code)?;
        let stmt = Statement {
            address: body.address.clone(),
            operator: body.operator.clone(),
            gateway: self.me.operator.clone(),
            messages: waiting.iter().map(|w| w.id).collect(),
            form: Form::Time {
                time,
                nonce: self.cfg.nonce,
            },
        };
        let freshness = Freshness {
            now: self.cfg.time,
            seen: &[],
            challenge: None,
            sent_nonce: Some(self.cfg.nonce),
        };
        statement::verify(&stmt, sig, &body, &self.me.operator, &freshness)?;
        Ok(body)
    }

    /// Answer once the other station has said all it owes this turn: its `DONE` (after `WELCOME`
    /// or a `REQUEST`) and receipts for everything sent.
    fn reply_when_due(&mut self) {
        if !self.waiting_reply || self.awaiting_done || !self.awaiting_receipt.is_empty() {
            return;
        }
        let mut pdus = Vec::new();
        if let Some(to_send) = self.to_send.take() {
            let piece = piece_for(self.peer_caps.max_pdu, self.cfg.piece);
            for (i, offset) in to_send {
                let out = &self.cfg.outbox[i];
                pdus.extend(msg_pdus(out, offset, piece, None));
                self.awaiting_receipt.insert(out.id());
            }
            pdus.extend(self.receipts.take_pdu());
            if self.requests.is_empty() {
                pdus.push(done());
                self.sent_done = true;
            } else {
                let serials = self.requests.iter().map(|s| Value::Uint(*s)).collect();
                pdus.push(Value::int_map(vec![
                    (0, Value::Uint(kind::REQUEST)),
                    (1, Value::Array(serials)),
                ]));
                self.awaiting_done = true;
            }
        } else {
            pdus.extend(self.receipts.take_pdu());
            if !self.sent_done {
                pdus.push(done());
                self.sent_done = true;
            }
        }
        if !pdus.is_empty() {
            self.queued.push(transmission(&pdus));
        }
        // Over when both have said DONE and every message sent is receipted (RP-3 §8).
        if !self.awaiting_done && self.awaiting_receipt.is_empty() {
            self.end();
        }
    }
}
