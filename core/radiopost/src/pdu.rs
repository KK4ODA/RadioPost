// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-3 §2 and §4: PDU framing and types.

use crate::cbor::{self, Value};
use crate::error::ErrorCode;

/// RP-3 §4 and RP-7 §3.2: the PDU types, the value of key 0.
pub mod kind {
    #![allow(missing_docs)] // each is its PDU's name
    pub const HELLO: u64 = 1;
    pub const WELCOME: u64 = 2;
    pub const AUTH: u64 = 3;
    pub const MSG: u64 = 4;
    pub const RECEIPT: u64 = 5;
    pub const REQUEST: u64 = 6;
    pub const DONE: u64 = 7;
    pub const ERROR: u64 = 8;
    pub const CREDENTIAL: u64 = 9;
    pub const NOTICE: u64 = 10;
    pub const PROOF: u64 = 11;
}

/// RP-3 §2: the largest PDU any station must take (both may agree on more, up to 65 535).
pub const MIN_MAX_PDU: usize = 1024;

/// A PDU's type, if it is a map whose key 0 names one RP-3 (or RP-7) defines.
#[must_use]
pub fn kind_of(pdu: &Value) -> Option<u64> {
    let kind = pdu.get(0)?.as_u64()?;
    (kind::HELLO..=kind::PROOF).contains(&kind).then_some(kind)
}

/// RP-3 §2: two bytes of length, big-endian, then the PDU's deterministic encoding.
///
/// # Errors
/// A PDU that does not encode, is empty or is over 65 535 bytes.
pub fn frame(pdu: &Value) -> Result<Vec<u8>, ErrorCode> {
    let raw = pdu.encode().map_err(|_| ErrorCode::Malformed)?;
    let n = u16::try_from(raw.len()).map_err(|_| ErrorCode::TooLarge)?;
    if n == 0 {
        return Err(ErrorCode::Malformed);
    }
    let mut out = n.to_be_bytes().to_vec();
    out.extend(raw);
    Ok(out)
}

/// RP-3 §2: one whole frame back to its PDU.
///
/// # Errors
/// `malformed` for a length that is zero or not the frame's, bytes that are not deterministic
/// CBOR, or a PDU of no known type.
pub fn unframe(data: &[u8]) -> Result<Value, ErrorCode> {
    let (length, rest) = data.split_first_chunk::<2>().ok_or(ErrorCode::Malformed)?;
    let n = usize::from(u16::from_be_bytes(*length));
    if n == 0 || rest.len() != n {
        return Err(ErrorCode::Malformed);
    }
    let pdu = cbor::decode(rest).map_err(|_| ErrorCode::Malformed)?;
    kind_of(&pdu).ok_or(ErrorCode::Malformed)?;
    Ok(pdu)
}

/// Splits a byte stream into frames as they arrive (RP-3 §2): bytes go in, whole frames come out.
#[derive(Clone, Debug, Default)]
pub struct Deframer {
    pending: Vec<u8>,
}

impl Deframer {
    /// A deframer with nothing held.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Take more bytes.
    pub fn push(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
    }

    /// The next whole frame's PDU, if one has arrived.
    ///
    /// # Errors
    /// `malformed` for a frame that is not a PDU; `too-large` for a length over `max_pdu`. The
    /// stream cannot be trusted past either.
    pub fn next(&mut self, max_pdu: usize) -> Option<Result<Value, ErrorCode>> {
        let length = self.pending.first_chunk::<2>()?;
        let n = usize::from(u16::from_be_bytes(*length));
        if n > max_pdu {
            return Some(Err(ErrorCode::TooLarge));
        }
        if self.pending.len() < 2 + n {
            return None;
        }
        let frame: Vec<u8> = self.pending.drain(..2 + n).collect();
        Some(unframe(&frame))
    }

    /// Bytes held that do not yet make a whole frame.
    #[must_use]
    pub fn held(&self) -> usize {
        self.pending.len()
    }
}
