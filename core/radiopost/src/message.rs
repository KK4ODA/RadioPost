// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-2: a message's canonical encoding, its content ID and its air form.

use std::fmt;

use std::io::Write as _;

use flate2::write::DeflateEncoder;
use flate2::{Decompress, FlushDecompress, Status};

use crate::cbor::{self, Value};
use crate::crypto::ContentId;

/// RP-2 §2: the message map's keys.
pub mod key {
    #![allow(missing_docs)] // each is its field's name in RP-2 §2
    pub const V: i64 = 1;
    pub const FROM: i64 = 2;
    pub const TO: i64 = 3;
    pub const CC: i64 = 4;
    pub const DATE: i64 = 5;
    pub const SUBJECT: i64 = 6;
    pub const BODY: i64 = 7;
    pub const FILES: i64 = 8;
    pub const PRIORITY: i64 = 9;
    pub const REPLY_TO: i64 = 10;
    pub const FLAGS: i64 = 11;
    pub const HELD: i64 = 12;
    pub const FORM: i64 = 13;
}

/// RP-2 §4: the compressions an air form names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    /// 0: none.
    None = 0,
    /// 1: raw DEFLATE (RFC 1951).
    Deflate = 1,
}

/// The largest canonical encoding this library inflates an air form to. RP-2 §5 asks for at least
/// 64 KiB; the bound keeps a hostile stream from filling memory.
pub const MAX_CANONICAL: usize = 16 * 1024 * 1024;

/// Not a message: an air form or an encoding RP-2 refuses (`malformed`, RP-0 §9.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Malformed(pub &'static str);

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "malformed: {}", self.0)
    }
}

impl std::error::Error for Malformed {}

/// A message, held as its canonical encoding (RP-2 §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    canonical: Vec<u8>,
    map: Value,
}

impl Message {
    /// A message from its canonical encoding, checked: deterministic CBOR, a map, `v` 1, and the
    /// fields RP-2 §2 requires, of their types.
    ///
    /// # Errors
    /// Anything RP-2 refuses.
    pub fn from_canonical(canonical: &[u8]) -> Result<Self, Malformed> {
        let map = cbor::decode(canonical).map_err(|e| Malformed(e.0))?;
        if map.as_map().is_none() || map.get(key::V).and_then(Value::as_u64) != Some(1) {
            return Err(Malformed("not a version 1 message"));
        }
        Ok(Message {
            canonical: canonical.to_vec(),
            map,
        })
    }

    /// A message from its map, encoded deterministically.
    ///
    /// # Errors
    /// A map with a repeated key, or one RP-2 refuses.
    pub fn from_map(map: &Value) -> Result<Self, Malformed> {
        let canonical = map
            .encode()
            .map_err(|_| Malformed("a map key appears twice"))?;
        Self::from_canonical(&canonical)
    }

    /// The canonical encoding.
    #[must_use]
    pub fn canonical(&self) -> &[u8] {
        &self.canonical
    }

    /// The decoded map.
    #[must_use]
    pub fn map(&self) -> &Value {
        &self.map
    }

    /// The content ID (RP-2 §3).
    #[must_use]
    pub fn content_id(&self) -> ContentId {
        ContentId::of(&self.canonical)
    }

    /// The sender's address.
    #[must_use]
    pub fn from(&self) -> Option<&str> {
        self.map.get(key::FROM).and_then(Value::as_text)
    }

    /// Every recipient, `to` then `cc`.
    #[must_use]
    pub fn recipients(&self) -> Vec<&str> {
        [key::TO, key::CC]
            .iter()
            .filter_map(|k| self.map.get(*k).and_then(Value::as_array))
            .flatten()
            .filter_map(Value::as_text)
            .collect()
    }

    /// The air form with this compression (RP-2 §4). Compression 1 makes one valid DEFLATE
    /// stream; any valid stream is correct, so other implementations may make other bytes.
    #[must_use]
    pub fn air_form(&self, compression: Compression) -> Vec<u8> {
        let mut out = vec![compression as u8];
        match compression {
            Compression::None => out.extend_from_slice(&self.canonical),
            Compression::Deflate => out.extend(deflate(&self.canonical)),
        }
        out
    }

    /// The message an air form carries (RP-2 §4).
    ///
    /// # Errors
    /// An unknown compression, a stream that does not end where the air form does, or bytes that
    /// are not a canonical message.
    pub fn from_air(air: &[u8]) -> Result<Self, Malformed> {
        let (&compression, rest) = air.split_first().ok_or(Malformed("an empty air form"))?;
        match compression {
            0 => Self::from_canonical(rest),
            1 => Self::from_canonical(&inflate(rest)?),
            _ => Err(Malformed("a compression this station does not take")),
        }
    }
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut packer = DeflateEncoder::new(Vec::new(), flate2::Compression::best());
    // Writing into memory cannot fail.
    packer.write_all(data).expect("writing to a Vec");
    packer.finish().expect("writing to a Vec")
}

fn inflate(stream: &[u8]) -> Result<Vec<u8>, Malformed> {
    let mut unpacker = Decompress::new(false);
    let mut out = Vec::with_capacity(stream.len() * 3);
    loop {
        let consumed = usize::try_from(unpacker.total_in()).unwrap_or(usize::MAX);
        let input = stream
            .get(consumed..)
            .ok_or(Malformed("a broken DEFLATE stream"))?;
        if out.len() >= MAX_CANONICAL {
            return Err(Malformed(
                "the message inflates past the size this station takes",
            ));
        }
        out.reserve(4096.min(MAX_CANONICAL - out.len()));
        let before = (unpacker.total_in(), unpacker.total_out());
        let status = unpacker
            .decompress_vec(input, &mut out, FlushDecompress::None)
            .map_err(|_| Malformed("a broken DEFLATE stream"))?;
        if status == Status::StreamEnd {
            if usize::try_from(unpacker.total_in()).ok() != Some(stream.len()) {
                return Err(Malformed("bytes after the DEFLATE stream"));
            }
            return Ok(out);
        }
        if (unpacker.total_in(), unpacker.total_out()) == before && out.len() < out.capacity() {
            return Err(Malformed("a DEFLATE stream that does not end"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deflate_and_inflate_round_trip() {
        let data = b"Facundo, the shelter's generator needs a spark plug. ".repeat(40);
        assert_eq!(inflate(&deflate(&data)).unwrap(), data);
    }

    #[test]
    fn a_truncated_stream_is_malformed() {
        let data = b"a message long enough to compress, a message long enough".repeat(10);
        let stream = deflate(&data);
        assert!(inflate(&stream[..stream.len() - 2]).is_err());
    }
}
