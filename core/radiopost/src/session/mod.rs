// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-3: the session protocol, as two engines without input or output of their own.
//!
//! An engine takes the bytes the link delivers and gives back the bytes to transmit, and events
//! for the program that drives it. It keeps no clock and draws no random numbers: the time,
//! nonces, session IDs and the user's choices come in through its configuration, so a session
//! can be replayed byte for byte against the vectors (`tests/sessions.rs`).
//!
//! * [`client::Client`]: the caller, calling a gateway or another client.
//! * [`called::Called`]: a client that answers a call, station to station (RP-3 §10).

pub mod called;
pub mod client;

use std::collections::HashMap;

use crate::cbor::Value;
use crate::crypto::ContentId;
use crate::error::ErrorCode;
use crate::message::{Compression, Message};
use crate::pdu::kind;

/// RP-3 §5.1: the protocol version this library speaks.
pub const VERSION: u64 = 1;

/// RP-3 §5.2: the waiting entry's flags.
pub mod flags {
    /// Attachments held at the post office.
    pub const HELD: u64 = 1;
    /// A copy placed ahead, served while offline.
    pub const COPY: u64 = 2;
    /// Handed between two users of a cut-off gateway (RP-7 §4.4).
    pub const LOCAL: u64 = 4;
    /// Its sender is not shown to be licensed (RP-6 §5.1).
    pub const THIRD_PARTY: u64 = 8;
}

/// RP-3 §5.1: what a station accepts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caps {
    /// Air-form compressions it accepts (RP-2 §4); 0 and 1 always.
    pub compression: Vec<u64>,
    /// The largest PDU it accepts; at least 1 024.
    pub max_pdu: u64,
    /// The caller would like a new credential (§5.8).
    pub renew: bool,
}

impl Caps {
    /// The map `HELLO` and `WELCOME` carry.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut entries = vec![
            (
                1,
                Value::Array(self.compression.iter().map(|c| Value::Uint(*c)).collect()),
            ),
            (2, Value::Uint(self.max_pdu)),
        ];
        if self.renew {
            entries.push((3, Value::Bool(true)));
        }
        Value::int_map(entries)
    }

    /// Caps from the map, with RP-3's defaults for what is missing.
    #[must_use]
    pub fn from_value(v: Option<&Value>) -> Self {
        let max_pdu = v.and_then(|c| c.get(2)).and_then(Value::as_u64);
        Caps {
            compression: v
                .and_then(|c| c.get(1))
                .and_then(Value::as_array)
                .map_or_else(
                    || vec![0, 1],
                    |a| a.iter().filter_map(Value::as_u64).collect(),
                ),
            max_pdu: max_pdu.unwrap_or(1024).max(1024),
            renew: v.and_then(|c| c.get(3)) == Some(&Value::Bool(true)),
        }
    }
}

/// A message a station will send, with the air form it keeps for it: pieces resume only from the
/// same bytes (RP-3 §7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    /// The message.
    pub message: Message,
    /// Its air form, made once.
    pub air: Vec<u8>,
}

impl Outgoing {
    /// A message to send, compressed as given.
    #[must_use]
    pub fn new(message: Message, compression: Compression) -> Self {
        let air = message.air_form(compression);
        Outgoing { message, air }
    }

    /// Its content ID.
    #[must_use]
    pub fn id(&self) -> ContentId {
        self.message.content_id()
    }

    /// Its priority (RP-2 §2): 0 routine, 1 priority, 2 emergency.
    #[must_use]
    pub fn priority(&self) -> u64 {
        self.message
            .map()
            .get(9)
            .and_then(Value::as_u64)
            .unwrap_or(0)
    }

    /// The offer `HELLO` carries for it: `[id, size, priority]` (RP-3 §5.1).
    #[must_use]
    pub fn offer(&self) -> Value {
        Value::Array(vec![
            Value::bytes(self.id().as_bytes()),
            Value::Uint(self.air.len() as u64),
            Value::Uint(self.priority()),
        ])
    }
}

/// RP-3 §5.2: a waiting entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaitingEntry {
    /// The mailbox serial; 0 for a local message.
    pub serial: u64,
    /// The content ID.
    pub id: ContentId,
    /// The sender's address.
    pub from: String,
    /// The subject, cut to at most 40 bytes of UTF-8.
    pub subject: String,
    /// The air form's size in bytes.
    pub size: u64,
    /// The priority.
    pub priority: u64,
    /// Bits of [`flags`].
    pub flags: u64,
}

impl WaitingEntry {
    /// The entry for a message to send, with its serial and flags.
    #[must_use]
    pub fn for_outgoing(serial: u64, out: &Outgoing, flags: u64) -> Self {
        let map = out.message.map();
        WaitingEntry {
            serial,
            id: out.id(),
            from: out.message.from().unwrap_or_default().to_owned(),
            subject: subject40(map.get(6).and_then(Value::as_text).unwrap_or_default()),
            size: out.air.len() as u64,
            priority: out.priority(),
            flags,
        }
    }

    /// The array `WELCOME` carries.
    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::Array(vec![
            Value::Uint(self.serial),
            Value::bytes(self.id.as_bytes()),
            Value::text(&self.from),
            Value::text(&self.subject),
            Value::Uint(self.size),
            Value::Uint(self.priority),
            Value::Uint(self.flags),
        ])
    }

    /// An entry from its array; `None` when it is not one.
    #[must_use]
    pub fn from_value(v: &Value) -> Option<Self> {
        let [serial, id, from, subject, size, priority, flags] = v.as_array()? else {
            return None;
        };
        Some(WaitingEntry {
            serial: serial.as_u64()?,
            id: ContentId::from_slice(id.as_bytes()?)?,
            from: from.as_text()?.to_owned(),
            subject: subject.as_text()?.to_owned(),
            size: size.as_u64()?,
            priority: priority.as_u64()?,
            flags: flags.as_u64()?,
        })
    }
}

/// RP-3 §5.2: a subject cut to at most 40 bytes of UTF-8, on a character boundary.
#[must_use]
pub fn subject40(subject: &str) -> String {
    let mut end = subject.len().min(40);
    while !subject.is_char_boundary(end) {
        end -= 1;
    }
    subject[..end].to_owned()
}

/// What an engine tells the program driving it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The waiting list, from `WELCOME`.
    Listed(Vec<WaitingEntry>),
    /// A whole message arrived and checked (RP-2 §3), with its serial when it had one. It is in
    /// the next `RECEIPT`: the program stores it durably before that is sent (RP-3 §5.4).
    Received {
        /// The serial `MSG` carried.
        serial: Option<u64>,
        /// The message.
        message: Message,
    },
    /// A message that arrived failed its check and is dropped.
    Failed {
        /// The message concerned.
        id: ContentId,
        /// Why.
        code: ErrorCode,
    },
    /// The other station holds one of our messages: it is delivered (RP-3 §5.5).
    Delivered(ContentId),
    /// The other station declined or failed one of our messages; it stays to be sent.
    NotTaken {
        /// The message concerned.
        id: ContentId,
        /// Why.
        code: ErrorCode,
    },
    /// The link broke with this much of a message held: offered under `partials` next time.
    Held(Partial),
    /// The station called proved who it is (RP-1 §6.5, RP-3 §10): its credential's body.
    StationProved(crate::credential::Body),
    /// A note from the gateway for the operator.
    Notice(String),
    /// The other station sent `ERROR`.
    Error {
        /// The code.
        code: ErrorCode,
        /// Its text, if any.
        text: Option<String>,
    },
    /// A renewed credential, to use from the next session (RP-3 §5.8).
    NewCredential(Vec<u8>),
    /// The session is over: the caller hangs up (RP-3 §8).
    End,
}

/// A message received in part (RP-3 §7): the bytes held, and its serial if it had one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Partial {
    /// The message.
    pub id: ContentId,
    /// The air form's bytes held, from the start.
    pub held: Vec<u8>,
    /// The air form's whole length.
    pub total: u64,
    /// The serial its `MSG` carried.
    pub serial: Option<u64>,
}

/// What a `MSG` completed: the message and its serial, or the message that failed and why.
type Taken = Result<(Option<u64>, Message), (ContentId, ErrorCode)>;

/// Messages arriving in pieces, joined (RP-3 §5.4, §7).
#[derive(Clone, Debug, Default)]
struct Inbound {
    pieces: HashMap<ContentId, Partial>,
}

impl Inbound {
    /// Start from what an earlier session left.
    fn resume(partials: &[Partial]) -> Self {
        Inbound {
            pieces: partials.iter().map(|p| (p.id, p.clone())).collect(),
        }
    }

    /// Take a `MSG`: the whole message once its last piece is in, or why it failed.
    fn take(&mut self, pdu: &Value) -> Option<Taken> {
        let id = ContentId::from_slice(pdu.get(1)?.as_bytes()?)?;
        let fail = |code| Some(Err((id, code)));
        let (Some(offset), Some(total), Some(data)) = (
            pdu.get(2).and_then(Value::as_u64),
            pdu.get(3).and_then(Value::as_u64),
            pdu.get(4).and_then(Value::as_bytes),
        ) else {
            return fail(ErrorCode::Malformed);
        };
        let serial = pdu.get(5).and_then(Value::as_u64);
        let entry = self.pieces.entry(id).or_insert_with(|| Partial {
            id,
            held: Vec::new(),
            total,
            serial,
        });
        // In order and without gaps, from the same air form (RP-3 §5.4, §7).
        if entry.total != total || offset != entry.held.len() as u64 {
            self.pieces.remove(&id);
            return fail(ErrorCode::Malformed);
        }
        entry.held.extend_from_slice(data);
        if (entry.held.len() as u64) < total {
            return None;
        }
        let whole = self.pieces.remove(&id)?;
        match Message::from_air(&whole.held) {
            Ok(message) if whole.held.len() as u64 == total && message.content_id() == id => {
                Some(Ok((whole.serial.or(serial), message)))
            }
            _ => fail(ErrorCode::Malformed),
        }
    }

    /// What is held when the link breaks.
    fn held(self) -> Vec<Partial> {
        let mut held: Vec<Partial> = self
            .pieces
            .into_values()
            .filter(|p| !p.held.is_empty())
            .collect();
        held.sort_by_key(|p| p.id);
        held
    }
}

/// The `MSG` PDUs that carry an air form from `offset`, `piece` data bytes at most each.
fn msg_pdus(out: &Outgoing, offset: u64, piece: usize, serial: Option<u64>) -> Vec<Value> {
    let start = usize::try_from(offset)
        .unwrap_or(usize::MAX)
        .min(out.air.len());
    out.air[start..]
        .chunks(piece.max(1))
        .enumerate()
        .map(|(i, chunk)| {
            let mut entries = vec![
                (0, Value::Uint(kind::MSG)),
                (1, Value::bytes(out.id().as_bytes())),
                (2, Value::Uint((start + i * piece.max(1)) as u64)),
                (3, Value::Uint(out.air.len() as u64)),
                (4, Value::bytes(chunk)),
            ];
            if let Some(s) = serial {
                entries.push((5, Value::Uint(s)));
            }
            Value::int_map(entries)
        })
        .collect()
}

/// The largest data a `MSG` piece may carry for a peer that takes PDUs of `max_pdu` bytes: what
/// is left once the other keys are counted, generously.
fn piece_for(max_pdu: u64, wanted: usize) -> usize {
    let room = usize::try_from(max_pdu)
        .unwrap_or(usize::MAX)
        .saturating_sub(64);
    wanted.min(room).max(1)
}

/// RP-3 §5.5: what a `RECEIPT` lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Receipts {
    complete: Vec<ContentId>,
    failed: Vec<(ContentId, ErrorCode)>,
}

impl Receipts {
    fn is_empty(&self) -> bool {
        self.complete.is_empty() && self.failed.is_empty()
    }

    fn take_pdu(&mut self) -> Option<Value> {
        if self.is_empty() {
            return None;
        }
        let mut entries = vec![(0, Value::Uint(kind::RECEIPT))];
        if !self.complete.is_empty() {
            entries.push((1, ids(&self.complete)));
        }
        if !self.failed.is_empty() {
            let failed = self.failed.iter().map(|(id, code)| {
                Value::Array(vec![
                    Value::bytes(id.as_bytes()),
                    Value::Uint(u64::from(code.code())),
                ])
            });
            entries.push((3, Value::Array(failed.collect())));
        }
        *self = Receipts::default();
        Some(Value::int_map(entries))
    }
}

fn ids(list: &[ContentId]) -> Value {
    Value::Array(list.iter().map(|id| Value::bytes(id.as_bytes())).collect())
}

fn done() -> Value {
    Value::int_map(vec![(0, Value::Uint(kind::DONE))])
}

fn error_pdu(code: ErrorCode) -> Value {
    Value::int_map(vec![
        (0, Value::Uint(kind::ERROR)),
        (1, Value::Uint(u64::from(code.code()))),
    ])
}

/// Frames a transmission's PDUs into the bytes written to the link.
fn transmission(pdus: &[Value]) -> Vec<u8> {
    pdus.iter()
        .flat_map(|p| crate::pdu::frame(p).unwrap_or_default())
        .collect()
}

/// The content IDs listed under a receipt key.
fn listed_ids(v: Option<&Value>) -> Vec<ContentId> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|i| ContentId::from_slice(i.as_bytes()?))
        .collect()
}

/// The `[id, n]` pairs listed under a key (`accept`, `decline`, `partials`, `failed`).
fn listed_pairs(v: Option<&Value>) -> Vec<(ContentId, u64)> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| match p.as_array()? {
            [id, n] => Some((ContentId::from_slice(id.as_bytes()?)?, n.as_u64()?)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note() -> Outgoing {
        let map = Value::int_map(vec![
            (1, Value::Uint(1)),
            (2, Value::text("ana@hampost.org")),
            (3, Value::Array(vec![Value::text("facundo@radiopost.org")])),
            (5, Value::Uint(1_794_249_600)),
            (6, Value::text("Net")),
            (7, Value::text("Tonight at 20:00 on 3.590 MHz.\n")),
        ]);
        Outgoing::new(
            Message::from_map(&map).expect("a message"),
            Compression::None,
        )
    }

    #[test]
    fn pieces_join_in_order() {
        let out = note();
        let mut inbound = Inbound::default();
        let pieces = msg_pdus(&out, 0, 10, Some(3));
        let last = pieces.len() - 1;
        for (i, p) in pieces.iter().enumerate() {
            let got = inbound.take(p);
            if i < last {
                assert!(got.is_none());
            } else {
                assert_eq!(got, Some(Ok((Some(3), out.message.clone()))));
            }
        }
    }

    #[test]
    fn a_gap_fails_the_message() {
        let out = note();
        let mut inbound = Inbound::default();
        let pieces = msg_pdus(&out, 0, 10, None);
        assert!(inbound.take(&pieces[0]).is_none());
        assert_eq!(
            inbound.take(&pieces[2]),
            Some(Err((out.id(), ErrorCode::Malformed)))
        );
    }

    #[test]
    fn bytes_that_are_not_the_message_fail() {
        let mut out = note();
        let last = out.air.len() - 2;
        out.air[last] ^= 0x20; // still a message, but not the one its content ID names
        let mut inbound = Inbound::default();
        let piece = &msg_pdus(&out, 0, 4096, None)[0];
        assert_eq!(
            inbound.take(piece),
            Some(Err((out.id(), ErrorCode::Malformed)))
        );
    }

    #[test]
    fn a_subject_is_cut_on_a_character_boundary() {
        assert_eq!(subject40("short"), "short");
        let long = "é".repeat(30); // 60 bytes
        assert_eq!(subject40(&long).len(), 40);
        let odd = format!("a{}", "é".repeat(30)); // a, then 2-byte characters
        assert_eq!(subject40(&odd).len(), 39);
    }
}
