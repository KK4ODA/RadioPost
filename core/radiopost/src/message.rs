// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-2: a message's canonical encoding, its content ID and its air form.

use std::fmt;

use std::io::Write as _;

use flate2::write::DeflateEncoder;
use flate2::{Decompress, FlushDecompress, Status};

use crate::cbor::{self, Value};
use crate::crypto::ContentId;
use crate::error::ErrorCode;

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

/// The largest canonical encoding this library inflates an air form to: the 16 MiB every station
/// takes (RP-2 §5). Past it a hostile stream could fill memory.
pub const MAX_CANONICAL: usize = 16 * 1024 * 1024;

/// Not a message: an air form or an encoding RP-2 refuses (`malformed`, RP-0 §9.3; or
/// [`TOO_LARGE`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Malformed(pub &'static str);

/// An air form that inflates past [`MAX_CANONICAL`]: refused `too-large` (RP-2 §5).
pub const TOO_LARGE: Malformed = Malformed("the message inflates past 16 MiB");

impl Malformed {
    /// The RP-0 §9.3 code a refusal is answered with: `too-large` past RP-2 §5's bound,
    /// `malformed` otherwise.
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        if *self == TOO_LARGE {
            ErrorCode::TooLarge
        } else {
            ErrorCode::Malformed
        }
    }
}

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
    /// A message from its canonical encoding, checked: deterministic CBOR, a map, `v` 1, and every
    /// key RP-2 §2 requires, each key it defines of its type. Nothing more: a message with no
    /// recipient, a `priority` above 2 or text not in NFC is taken as it is.
    ///
    /// # Errors
    /// Anything RP-2 refuses.
    pub fn from_canonical(canonical: &[u8]) -> Result<Self, Malformed> {
        let map = cbor::decode(canonical).map_err(|e| Malformed(e.0))?;
        if map.as_map().is_none() || map.get(key::V).and_then(Value::as_u64) != Some(1) {
            return Err(Malformed("not a version 1 message"));
        }
        for &(k, required, kind) in KINDS {
            match map.get(k) {
                None if required => return Err(Malformed("a key RP-2 §2 requires is missing")),
                Some(v) if !kind.fits(v) => {
                    return Err(Malformed("a key is not of the type RP-2 §2 gives it"));
                }
                _ => {}
            }
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

/// What a key of the message holds (RP-2 §2's table).
#[derive(Clone, Copy)]
enum Kind {
    Text,
    Texts,
    Uint,
    /// `files`: maps of name, type and data.
    Files,
    /// `held`: maps of name, type, size and ref.
    Held,
    /// `reply_to`: a content ID, 12 bytes.
    Id,
    /// `form`: the template's 8-byte fingerprint and a map of values by field number (§8.2).
    Form,
}

/// RP-2 §2: each key, whether it is required, and what it holds.
const KINDS: &[(i64, bool, Kind)] = &[
    (key::FROM, true, Kind::Text),
    (key::TO, true, Kind::Texts),
    (key::CC, false, Kind::Texts),
    (key::DATE, true, Kind::Uint),
    (key::SUBJECT, true, Kind::Text),
    (key::BODY, true, Kind::Text),
    (key::FILES, false, Kind::Files),
    (key::PRIORITY, false, Kind::Uint),
    (key::REPLY_TO, false, Kind::Id),
    (key::FLAGS, false, Kind::Uint),
    (key::HELD, false, Kind::Held),
    (key::FORM, false, Kind::Form),
];

impl Kind {
    fn fits(self, v: &Value) -> bool {
        let text = |v: Option<&Value>| v.and_then(Value::as_text).is_some();
        let bytes = |v: Option<&Value>| v.and_then(Value::as_bytes).is_some();
        let uint = |v: Option<&Value>| v.and_then(Value::as_u64).is_some();
        let maps = |v: &Value, ok: &dyn Fn(&Value) -> bool| {
            v.as_array()
                .is_some_and(|a| a.iter().all(|m| m.as_map().is_some() && ok(m)))
        };
        match self {
            Kind::Text => v.as_text().is_some(),
            Kind::Texts => v
                .as_array()
                .is_some_and(|a| a.iter().all(|t| t.as_text().is_some())),
            Kind::Uint => v.as_u64().is_some(),
            Kind::Files => maps(v, &|m| text(m.get(1)) && text(m.get(2)) && bytes(m.get(3))),
            Kind::Held => maps(v, &|m| {
                text(m.get(1)) && text(m.get(2)) && uint(m.get(3)) && bytes(m.get(4))
            }),
            Kind::Id => v.as_bytes().is_some_and(|b| b.len() == 12),
            Kind::Form => {
                v.get(1)
                    .and_then(Value::as_bytes)
                    .is_some_and(|b| b.len() == 8)
                    && v.get(2).and_then(Value::as_map).is_some_and(|values| {
                        values.iter().all(|(k, _)| matches!(k, Value::Uint(_)))
                    })
            }
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
        if out.len() > MAX_CANONICAL {
            return Err(TOO_LARGE);
        }
        // room for one byte past the bound at most, so that passing it is seen
        if out.capacity() == out.len() {
            out.reserve_exact(65_536.min(MAX_CANONICAL + 1 - out.len()));
        }
        let before = (unpacker.total_in(), unpacker.total_out());
        let status = unpacker
            .decompress_vec(input, &mut out, FlushDecompress::None)
            .map_err(|_| Malformed("a broken DEFLATE stream"))?;
        if status == Status::StreamEnd {
            if out.len() > MAX_CANONICAL {
                return Err(TOO_LARGE);
            }
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
