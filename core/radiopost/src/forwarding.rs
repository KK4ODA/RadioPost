// SPDX-License-Identifier: MIT OR Apache-2.0
//! Forwarding between gateways (RP-7 §3): session records, their IDs, the `PROOF` PDU that
//! carries one, and the checks a partner makes of it.
//!
//! A gateway cut off from the Internet keeps a **session record** for every session it carried:
//! what RP-5 §5.6 would report. In a forwarding session it sends each record to a partner as a
//! `PROOF` (type 11), the record without its messages, and offers the messages themselves. The
//! partner checks each `PROOF` before it takes the messages it covers ([`check_proof`]), lists
//! the records it took in its `RECEIPT` (key 4), adds its own callsign to each record's path, and
//! reports it, or forwards it in turn.

use crate::cbor::Value;
use crate::credential::{self, Body, Trust, flags};
use crate::crypto::{self, ContentId};
use crate::error::ErrorCode;
use crate::pdu::kind;
use crate::statement::{self, Form, Freshness, Statement, TIME_WINDOW_S};

/// The most callsigns a record's path holds (RP-7 §3.3).
pub const MAX_PATH: usize = 8;

/// The `RECEIPT` key that lists the records a partner took (RP-7 §3.3).
pub const RECORDS: i64 = 4;

/// A record's ID (RP-7 §3.1): the first 12 bytes of SHA-256 of its statement's encoding.
#[must_use]
pub fn record_id(statement: &[u8]) -> [u8; 12] {
    let h = crypto::sha256(statement);
    let mut id = [0u8; 12];
    id.copy_from_slice(&h[..12]);
    id
}

/// A session record as a `PROOF` carries it (RP-7 §3.2): everything but its messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proof {
    /// The user's credential, or `None` when only its hash is sent.
    pub credential: Option<Vec<u8>>,
    /// The credential's hash (RP-1 §4.3), sent in its place when the partner holds it.
    pub credential_hash: Option<[u8; 8]>,
    /// The user's statement's encoding.
    pub statement: Vec<u8>,
    /// Its signature.
    pub signature: Vec<u8>,
    /// When the user's session started, on the gateway's clock.
    pub started: u64,
    /// The gateway's session ID.
    pub session: [u8; 8],
    /// The callsigns of the gateways that held it, ending with the caller.
    pub path: Vec<String>,
    /// The serials the user picked up from copies.
    pub pickups: Vec<u64>,
    /// The content IDs the user picked up by local delivery (RP-7 §4.4).
    pub local: Vec<ContentId>,
}

impl Proof {
    /// Its ID (RP-7 §3.1).
    #[must_use]
    pub fn id(&self) -> [u8; 12] {
        record_id(&self.statement)
    }

    /// The `PROOF` PDU (RP-7 §3.2); pickups and local pickups only when there are some.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut entries = vec![(0, Value::Uint(kind::PROOF))];
        if let Some(c) = &self.credential {
            entries.push((1, Value::bytes(c)));
        }
        entries.push((2, Value::bytes(&self.statement)));
        entries.push((3, Value::bytes(&self.signature)));
        entries.push((4, Value::Uint(self.started)));
        entries.push((5, Value::bytes(&self.session)));
        entries.push((
            6,
            Value::Array(self.path.iter().map(|c| Value::text(c)).collect()),
        ));
        if !self.pickups.is_empty() {
            entries.push((
                7,
                Value::Array(self.pickups.iter().map(|s| Value::Uint(*s)).collect()),
            ));
        }
        if !self.local.is_empty() {
            entries.push((
                8,
                Value::Array(
                    self.local
                        .iter()
                        .map(|id| Value::bytes(id.as_bytes()))
                        .collect(),
                ),
            ));
        }
        if let Some(h) = &self.credential_hash {
            entries.push((9, Value::bytes(h)));
        }
        Value::int_map(entries)
    }

    /// A `PROOF` read back, its fields' types checked.
    ///
    /// # Errors
    ///
    /// `malformed`: not a `PROOF`, a field missing or of the wrong type.
    pub fn from_value(v: &Value) -> Result<Proof, ErrorCode> {
        if v.get(0).and_then(Value::as_u64) != Some(kind::PROOF) {
            return Err(ErrorCode::Malformed);
        }
        let bytes = |k| v.get(k).and_then(Value::as_bytes).map(<[u8]>::to_vec);
        let fixed = |k, n: usize| -> Result<Option<Vec<u8>>, ErrorCode> {
            match v.get(k) {
                None => Ok(None),
                Some(x) => x
                    .as_bytes()
                    .filter(|b| b.len() == n)
                    .map(|b| Some(b.to_vec()))
                    .ok_or(ErrorCode::Malformed),
            }
        };
        let list = |k| v.get(k).and_then(Value::as_array).unwrap_or(&[]);
        let credential_hash = fixed(9, 8)?.map(|h| {
            let mut a = [0u8; 8];
            a.copy_from_slice(&h);
            a
        });
        let session: [u8; 8] = fixed(5, 8)?
            .ok_or(ErrorCode::Malformed)?
            .try_into()
            .map_err(|_| ErrorCode::Malformed)?;
        let signature = fixed(3, 64)?.ok_or(ErrorCode::Malformed)?;
        let path = v
            .get(6)
            .and_then(Value::as_array)
            .ok_or(ErrorCode::Malformed)?
            .iter()
            .map(|c| c.as_text().map(str::to_owned))
            .collect::<Option<Vec<String>>>()
            .ok_or(ErrorCode::Malformed)?;
        let pickups = list(7)
            .iter()
            .map(Value::as_u64)
            .collect::<Option<Vec<u64>>>()
            .ok_or(ErrorCode::Malformed)?;
        let local = list(8)
            .iter()
            .map(|b| b.as_bytes().and_then(ContentId::from_slice))
            .collect::<Option<Vec<ContentId>>>()
            .ok_or(ErrorCode::Malformed)?;
        Ok(Proof {
            credential: bytes(1),
            credential_hash,
            statement: bytes(2).ok_or(ErrorCode::Malformed)?,
            signature,
            started: v
                .get(4)
                .and_then(Value::as_u64)
                .ok_or(ErrorCode::Malformed)?,
            session,
            path,
            pickups,
            local,
        })
    }
}

/// A record a partner took: the `PROOF` read, its statement and the credential it was checked
/// with (whole, though it may have come by its hash).
#[derive(Clone, Debug)]
pub struct Checked {
    /// The `PROOF`.
    pub proof: Proof,
    /// Its statement, decoded.
    pub statement: Statement,
    /// The user's credential.
    pub credential: Vec<u8>,
    /// The credential's body.
    pub body: Body,
}

/// RP-7 §3.3: what the partner `me` makes of a `PROOF` from `caller`, holding the credentials
/// `held` (by which a `credential_hash` is found) and checking with `trust`. Checked in order, the
/// first failure refusing the record: the credential is there exactly once; the proof, as
/// RP-1 §6.3 says for a statement relayed later (the credential as it was at `started`, with
/// `send` when the statement lists messages and `receive` when the record has pickups or local
/// pickups, the signature, the statement's time within 600 seconds of `started`); the path.
///
/// # Errors
///
/// The code that refuses the record: `malformed`, `credential-required`, the credential's or the
/// statement's own (RP-1 §5, §6.3), or `refused-by-policy` for a path that does not start at the
/// gateway the user called, does not end at the caller, is longer than [`MAX_PATH`], or already
/// holds the partner.
pub fn check_proof(
    pdu: &Value,
    caller: &str,
    me: &str,
    held: &[Vec<u8>],
    trust: &Trust,
) -> Result<Checked, ErrorCode> {
    let proof = Proof::from_value(pdu)?;
    // 1. exactly one of the credential and its hash
    let credential = match (&proof.credential, &proof.credential_hash) {
        (Some(c), None) => c.clone(),
        (None, Some(h)) => held
            .iter()
            .find(|c| credential::hash(c) == *h)
            .cloned()
            .ok_or(ErrorCode::CredentialRequired)?,
        _ => return Err(ErrorCode::Malformed),
    };
    // 2. the proof, as at `started`
    let statement = Statement::decode(&proof.statement).ok_or(ErrorCode::Malformed)?;
    let mut need = 0;
    if !statement.messages.is_empty() {
        need |= flags::SEND;
    }
    if !proof.pickups.is_empty() || !proof.local.is_empty() {
        need |= flags::RECEIVE;
    }
    let body = credential::verify(&credential, trust, proof.started, need).map_err(|r| r.code)?;
    let fresh = Freshness {
        now: proof.started,
        seen: &[],
        challenge: None,
        sent_nonce: None,
    };
    statement::verify(
        &statement,
        &proof.signature,
        &body,
        &statement.gateway,
        &fresh,
    )?;
    if let Form::Time { time, .. } = statement.form
        && time.abs_diff(proof.started) > TIME_WINDOW_S
    {
        return Err(ErrorCode::TimeSkew);
    }
    // 3. the path
    let path_ok = proof.path.first() == Some(&statement.gateway)
        && proof.path.last().map(String::as_str) == Some(caller)
        && proof.path.len() <= MAX_PATH
        && !proof.path.iter().any(|c| c == me);
    if !path_ok {
        return Err(ErrorCode::RefusedByPolicy);
    }
    Ok(Checked {
        proof,
        statement,
        credential,
        body,
    })
}
