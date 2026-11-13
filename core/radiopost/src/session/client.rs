// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-3, the caller's side: a client calling a gateway, or another client (§10).

use std::collections::BTreeSet;

use ed25519_dalek::SigningKey;

use super::{
    Caps, Event, Inbound, Outgoing, Partial, Progress, Proof, Proofs, Receipts, VERSION,
    WaitingEntry, done, error_pdu, listed_ids, listed_pairs, msg_pdus, piece_for, take_counted,
    transmission,
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
pub type Chooser = Box<dyn FnMut(&[WaitingEntry]) -> Vec<u64> + Send>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Start,
    Welcome,
    Exchange,
    Over,
}

/// Where the held attachments' request stands (RP-3 §5.6 `files`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum FilesAsked {
    /// Not sent.
    Not,
    /// In a `REQUEST` handed to the link.
    Sent,
    /// The gateway's `DONE` came after it.
    Answered,
}

/// The caller's side of a session.
pub struct Client {
    hello_proof: Option<Proof>,
    station_proof: Option<(u64, Proof)>,
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
    /// Held attachments to ask for (RP-3 §5.6 `files`), whether the `REQUEST` carrying them went,
    /// and whether the gateway's `DONE` answered it.
    files: Vec<(u64, Vec<u8>)>,
    files_asked: FilesAsked,
    awaiting_done: bool,
    awaiting_receipt: BTreeSet<ContentId>,
    sent_done: bool,
    receipts: Receipts,
    events: Vec<Event>,
    queued: Vec<Vec<u8>>,
    progress: Progress,
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
            hello_proof: None,
            station_proof: None,
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
            files: Vec::new(),
            files_asked: FilesAsked::Not,
            awaiting_done: false,
            awaiting_receipt: BTreeSet::new(),
            sent_done: false,
            receipts: Receipts::default(),
            events: Vec::new(),
            queued: Vec::new(),
            progress: Progress::default(),
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
        let signature = stmt.sign(&self.cfg.device);
        self.hello_proof = Some(Proof {
            statement: stmt.encode(),
            signature: signature.to_vec(),
            credential: self.cfg.credential.clone(),
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
            (15, Value::bytes(&signature)),
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

    /// Held attachments to ask for in this call (RP-3 §5.6 `files`), each by its message's
    /// serial and its `ref`. A call to another station asks for none: there is no post office.
    pub fn ask_for_files(&mut self, files: Vec<(u64, Vec<u8>)>) {
        self.files = files;
    }

    /// How far the session has carried the messages in it, both ways.
    #[must_use]
    pub fn progress(&self) -> &Progress {
        &self.progress
    }

    /// Whether the held attachments were asked for and the gateway answered: its `DONE` came after
    /// the `REQUEST` carrying them, and its `NOTICE` before that said how the post office took
    /// them. Until then a broken link leaves them to ask for on the next call.
    #[must_use]
    pub fn files_answered(&self) -> bool {
        self.files_asked == FilesAsked::Answered
    }

    /// Both stations' proofs, once the station called has proved itself (RP-3 §10): what a
    /// station-to-station session is reported with (RP-5 §6.6).
    #[must_use]
    pub fn proofs(&self) -> Option<Proofs> {
        let (started, theirs) = self.station_proof.clone()?;
        Some(Proofs {
            started,
            mine: self.hello_proof.clone()?,
            theirs,
        })
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
        self.events.push(Event::Refused(code));
        self.end();
    }

    fn handle(&mut self, pdu: &Value) {
        match pdu.get(0).and_then(Value::as_u64) {
            Some(kind::WELCOME) if self.state == State::Welcome => self.welcome(pdu),
            Some(kind::MSG) if self.state == State::Exchange => match self.take_msg(pdu) {
                Some(Ok((serial, message, air))) => {
                    self.progress.received(message.content_id(), true);
                    self.receipts.complete.push(message.content_id());
                    self.events.push(Event::Received {
                        serial,
                        message,
                        air,
                    });
                }
                Some(Err((id, code))) => {
                    self.progress.received(id, false);
                    self.receipts.failed.push((id, code));
                    self.events.push(Event::Failed { id, code });
                }
                None => {}
            },
            Some(kind::RECEIPT) => {
                for id in listed_ids(pdu.get(1)) {
                    self.awaiting_receipt.remove(&id);
                    self.progress.answered(id, true);
                    self.events.push(Event::Delivered(id));
                }
                for (id, _) in listed_pairs(pdu.get(2)) {
                    self.awaiting_receipt.remove(&id);
                }
                for (id, code) in listed_pairs(pdu.get(3)) {
                    self.awaiting_receipt.remove(&id);
                    self.progress.answered(id, false);
                    let code = ErrorCode::from_code(code).unwrap_or(ErrorCode::Internal);
                    self.events.push(Event::NotTaken { id, code });
                }
            }
            Some(kind::DONE) => {
                self.awaiting_done = false;
                // the gateway's answer to the request: what it did not send will not come now
                if self.to_send.is_none() {
                    self.progress.turn_ended();
                }
                if self.files_asked == FilesAsked::Sent {
                    self.files_asked = FilesAsked::Answered;
                }
            }
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
                Ok((body, proof)) => {
                    self.station_proof = Some(proof);
                    peer_account = Some(body.address.clone());
                    self.events.push(Event::StationProved(body));
                }
                Err(code) => return self.fail(code),
            }
        }
        self.peer_caps = Caps::from_value(pdu.get(9));
        self.events.push(Event::Welcome {
            online: pdu.get(11) == Some(&Value::Bool(true)),
        });
        for (id, code) in listed_pairs(pdu.get(6)) {
            let code = ErrorCode::from_code(code).unwrap_or(ErrorCode::Internal);
            self.events.push(Event::NotTaken { id, code });
        }
        // Of the offers taken, what to send and from where; station to station, only the
        // messages for the account reached (RP-3 §10).
        let accepted = listed_pairs(pdu.get(5));
        let to_send: Vec<(usize, u64)> = self
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
        for &(i, offset) in &to_send {
            let out = &self.cfg.outbox[i];
            self.progress
                .will_send(out.id(), out.air.len() as u64, offset);
        }
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
        // what will come: unasked what fits `auto` and the rest of what is held in part (RP-3
        // §5.2), and what is asked for
        for w in &waiting {
            if (w.size <= self.cfg.auto && !held.contains(&w.id))
                || self.requests.contains(&w.serial)
            {
                self.progress.will_receive(w.id, w.size, 0);
            }
        }
        for p in &self.cfg.partials {
            self.progress
                .will_receive(p.id, p.total, p.held.len() as u64);
        }
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
    ) -> Result<(Body, (u64, Proof)), ErrorCode> {
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
        let proof = Proof {
            statement: stmt.encode(),
            signature: sig.to_vec(),
            credential: cred.to_vec(),
        };
        Ok((body, (time, proof)))
    }

    /// Takes a `MSG`, counting what of it arrived first.
    fn take_msg(&mut self, pdu: &Value) -> Option<super::Taken> {
        take_counted(&mut self.inbound, &mut self.progress, pdu)
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
                self.progress.handed(out.id());
            }
            pdus.extend(self.receipts.take_pdu());
            let files = if self.cfg.station.is_none() && self.files_asked == FilesAsked::Not {
                std::mem::take(&mut self.files)
            } else {
                Vec::new()
            };
            if self.requests.is_empty() && files.is_empty() {
                pdus.push(done());
                self.sent_done = true;
            } else {
                let mut request = vec![(0, Value::Uint(kind::REQUEST))];
                if !self.requests.is_empty() {
                    let serials = self.requests.iter().map(|s| Value::Uint(*s)).collect();
                    request.push((1, Value::Array(serials)));
                }
                if !files.is_empty() {
                    let asked = files
                        .into_iter()
                        .map(|(serial, r)| Value::Array(vec![Value::Uint(serial), Value::Bytes(r)]))
                        .collect();
                    request.push((2, Value::Array(asked)));
                    self.files_asked = FilesAsked::Sent;
                }
                pdus.push(Value::int_map(request));
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
