// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-1 §6: the session statement, how a station signs it and how it is checked.

use ed25519_dalek::SigningKey;

use crate::cbor::{self, Value};
use crate::credential::Body;
use crate::crypto::{self, ContentId, context};
use crate::error::ErrorCode;

/// RP-1 §6.3: how far a statement's `time` may be from the verifier's clock (600 s is within).
pub const TIME_WINDOW_S: u64 = 600;
/// RP-1 §6.3: how long a gateway remembers the (address, nonce) pairs it has seen.
pub const REPLAY_S: u64 = 1200;

/// RP-1 §6.1: time form (`time` and `nonce`) or challenge form (`challenge`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Form {
    /// Built before the session, with the signer's clock and a random nonce.
    Time {
        /// The signer's clock.
        time: u64,
        /// Random, 8 bytes.
        nonce: [u8; 8],
    },
    /// Over the gateway's challenge.
    Challenge([u8; 16]),
}

/// RP-1 §6.1: a session statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Statement {
    /// The account.
    pub address: String,
    /// The operator's callsign, as on the air.
    pub operator: String,
    /// The station called; in a statement by the station called, the caller (RP-1 §6.5).
    pub gateway: String,
    /// The content IDs offered, in order.
    pub messages: Vec<ContentId>,
    /// Its form.
    pub form: Form,
}

impl Statement {
    /// The statement's CBOR map.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut entries = vec![
            (1, Value::text(&self.address)),
            (2, Value::text(&self.operator)),
            (3, Value::text(&self.gateway)),
            (
                6,
                Value::Array(self.messages.iter().map(|m| Value::bytes(&m.0)).collect()),
            ),
        ];
        match &self.form {
            Form::Time { time, nonce } => {
                entries.push((4, Value::Uint(*time)));
                entries.push((5, Value::bytes(nonce)));
            }
            Form::Challenge(challenge) => entries.push((7, Value::bytes(challenge))),
        }
        Value::int_map(entries)
    }

    /// The deterministic encoding the signature is over (RP-1 §6.2).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        self.to_value().encode().unwrap_or_default()
    }

    /// A statement from its map; `None` when it is not one form or the other.
    #[must_use]
    pub fn from_value(map: &Value) -> Option<Self> {
        let text = |k| map.get(k).and_then(Value::as_text).map(str::to_owned);
        let messages = map
            .get(6)?
            .as_array()?
            .iter()
            .map(|m| ContentId::from_slice(m.as_bytes()?))
            .collect::<Option<Vec<_>>>()?;
        let form = match (map.get(4), map.get(5), map.get(7)) {
            (Some(time), Some(nonce), None) => Form::Time {
                time: time.as_u64()?,
                nonce: nonce.as_bytes()?.try_into().ok()?,
            },
            (None, None, Some(challenge)) => {
                Form::Challenge(challenge.as_bytes()?.try_into().ok()?)
            }
            _ => return None,
        };
        Some(Statement {
            address: text(1)?,
            operator: text(2)?,
            gateway: text(3)?,
            messages,
            form,
        })
    }

    /// A statement from its encoding; `None` when it does not decode as one.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let statement = Self::from_value(&cbor::decode(bytes).ok()?)?;
        // Only the canonical encoding of what was decoded is signed, so nothing else may ride.
        (statement.encode() == bytes).then_some(statement)
    }

    /// The device's signature over the statement (RP-1 §6.2).
    #[must_use]
    pub fn sign(&self, device: &SigningKey) -> [u8; 64] {
        crypto::sign(context::SESSION, &self.encode(), device)
    }

    /// The nonce, in time form.
    #[must_use]
    pub fn nonce(&self) -> Option<[u8; 8]> {
        match self.form {
            Form::Time { nonce, .. } => Some(nonce),
            Form::Challenge(_) => None,
        }
    }
}

/// What a verifier checks a statement's freshness against (RP-1 §6.3 steps 5 and 6, §6.5).
#[derive(Clone, Copy, Debug)]
pub struct Freshness<'a> {
    /// The verifier's clock, or the session's `started` for a statement relayed later.
    pub now: u64,
    /// The (address, nonce) pairs its replay cache holds.
    pub seen: &'a [(String, [u8; 8])],
    /// The challenge it issued for this session, if it asked for one.
    pub challenge: Option<[u8; 16]>,
    /// For a caller checking the station it called (RP-1 §6.5): the nonce it sent. The clocks are
    /// then not compared.
    pub sent_nonce: Option<[u8; 8]>,
}

/// RP-1 §6.3 steps 2 to 6, once the credential (step 1) has passed: `body` is its body, and `me`
/// the callsign the statement must name.
///
/// # Errors
/// The first check that fails, with its code.
pub fn verify(
    statement: &Statement,
    signature: &[u8],
    body: &Body,
    me: &str,
    freshness: &Freshness<'_>,
) -> Result<(), ErrorCode> {
    if statement.address != body.address || statement.operator != body.operator {
        return Err(ErrorCode::NotAuthorized);
    }
    if statement.gateway != me {
        return Err(ErrorCode::NotAuthorized);
    }
    if !crypto::verify(
        context::SESSION,
        &statement.encode(),
        signature,
        &body.device,
    ) {
        return Err(ErrorCode::SignatureInvalid);
    }
    if let Some(sent) = freshness.sent_nonce {
        return if statement.nonce() == Some(sent) {
            Ok(())
        } else {
            Err(ErrorCode::SignatureInvalid)
        };
    }
    match &statement.form {
        Form::Challenge(c) if Some(*c) == freshness.challenge => Ok(()),
        Form::Challenge(_) => Err(ErrorCode::SignatureInvalid),
        Form::Time { time, nonce } => {
            let replayed = freshness
                .seen
                .iter()
                .any(|(a, n)| *a == statement.address && n == nonce);
            if time.abs_diff(freshness.now) > TIME_WINDOW_S || replayed {
                Err(ErrorCode::TimeSkew)
            } else {
                Ok(())
            }
        }
    }
}
