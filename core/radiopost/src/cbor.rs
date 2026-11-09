// SPDX-License-Identifier: MIT OR Apache-2.0
//! Deterministic CBOR, as RP-0 §4.1 uses it (RFC 8949 §4.2.1, core deterministic encoding).
//!
//! Only what RadioPost carries: unsigned and negative integers, byte strings, text strings, arrays,
//! maps, and `true`, `false` and `null`. No floats, no tags, no indefinite lengths. The decoder is
//! strict: it refuses anything not already in deterministic form, because a station that
//! re-encoded a message differently would compute a different content ID (RP-2 §3).

use std::fmt::{self, Write as _};

/// A CBOR value in RadioPost's subset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// An unsigned integer (major type 0).
    Uint(u64),
    /// A negative integer `-1 - n` (major type 1), holding `n`.
    Nint(u64),
    /// A byte string.
    Bytes(Vec<u8>),
    /// A UTF-8 text string.
    Text(String),
    /// An array.
    Array(Vec<Value>),
    /// A map. Encoding sorts it; keys are integers or text.
    Map(Vec<(Value, Value)>),
    /// `true` or `false`.
    Bool(bool),
    /// `null`.
    Null,
}

/// Why bytes are not one item of RadioPost's deterministic CBOR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotCanonical(pub &'static str);

impl fmt::Display for NotCanonical {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not deterministic CBOR: {}", self.0)
    }
}

impl std::error::Error for NotCanonical {}

/// A map that cannot be encoded: two of its keys are the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DuplicateKey;

impl fmt::Display for DuplicateKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a map key appears twice")
    }
}

impl std::error::Error for DuplicateKey {}

impl Value {
    /// An integer, positive or negative.
    #[must_use]
    pub fn int(n: i64) -> Self {
        match u64::try_from(n) {
            Ok(u) => Value::Uint(u),
            Err(_) => Value::Nint(n.unsigned_abs() - 1),
        }
    }

    /// A text string.
    #[must_use]
    pub fn text(s: &str) -> Self {
        Value::Text(s.to_owned())
    }

    /// A byte string.
    #[must_use]
    pub fn bytes(b: &[u8]) -> Self {
        Value::Bytes(b.to_vec())
    }

    /// A map with integer keys, in any order.
    #[must_use]
    pub fn int_map(entries: Vec<(i64, Value)>) -> Self {
        Value::Map(
            entries
                .into_iter()
                .map(|(k, v)| (Value::int(k), v))
                .collect(),
        )
    }

    /// The value under integer key `key` in a map.
    #[must_use]
    pub fn get(&self, key: i64) -> Option<&Value> {
        let wanted = Value::int(key);
        match self {
            Value::Map(entries) => entries.iter().find(|(k, _)| *k == wanted).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The unsigned integer this is.
    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Uint(n) => Some(*n),
            _ => None,
        }
    }

    /// The integer this is, if it fits an `i64`.
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Uint(n) => i64::try_from(*n).ok(),
            Value::Nint(n) => i64::try_from(*n).ok().map(|n| -1 - n),
            _ => None,
        }
    }

    /// The bytes this byte string holds.
    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }

    /// The text this text string holds.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }

    /// The items of this array.
    #[must_use]
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The entries of this map.
    #[must_use]
    pub fn as_map(&self) -> Option<&[(Value, Value)]> {
        match self {
            Value::Map(entries) => Some(entries),
            _ => None,
        }
    }

    /// The deterministic encoding: shortest heads, definite lengths, map keys in byte order.
    ///
    /// # Errors
    /// A map with the same key twice.
    pub fn encode(&self) -> Result<Vec<u8>, DuplicateKey> {
        let mut out = Vec::new();
        self.encode_into(&mut out)?;
        Ok(out)
    }

    fn encode_into(&self, out: &mut Vec<u8>) -> Result<(), DuplicateKey> {
        match self {
            Value::Uint(n) => head(out, 0, *n),
            Value::Nint(n) => head(out, 1, *n),
            Value::Bytes(b) => {
                head(out, 2, b.len() as u64);
                out.extend_from_slice(b);
            }
            Value::Text(s) => {
                head(out, 3, s.len() as u64);
                out.extend_from_slice(s.as_bytes());
            }
            Value::Array(items) => {
                head(out, 4, items.len() as u64);
                for item in items {
                    item.encode_into(out)?;
                }
            }
            Value::Map(entries) => {
                let mut pairs = Vec::with_capacity(entries.len());
                for (k, v) in entries {
                    pairs.push((k.encode()?, v.encode()?));
                }
                pairs.sort();
                if pairs.windows(2).any(|w| w[0].0 == w[1].0) {
                    return Err(DuplicateKey);
                }
                head(out, 5, pairs.len() as u64);
                for (k, v) in pairs {
                    out.extend_from_slice(&k);
                    out.extend_from_slice(&v);
                }
            }
            Value::Bool(false) => out.push(0xf4),
            Value::Bool(true) => out.push(0xf5),
            Value::Null => out.push(0xf6),
        }
        Ok(())
    }

    /// RFC 8949 §8 diagnostic notation, as the test vectors write it for people.
    #[must_use]
    pub fn diagnostic(&self) -> String {
        let mut out = String::new();
        self.diagnose(&mut out);
        out
    }

    fn diagnose(&self, out: &mut String) {
        match self {
            Value::Uint(n) => {
                let _ = write!(out, "{n}");
            }
            Value::Nint(n) => {
                let _ = write!(out, "-{}", u128::from(*n) + 1);
            }
            Value::Bytes(b) => {
                out.push_str("h'");
                out.push_str(&hex(b));
                out.push('\'');
            }
            Value::Text(s) => {
                out.push('"');
                for c in s.chars() {
                    match c {
                        '\\' => out.push_str("\\\\"),
                        '"' => out.push_str("\\\""),
                        '\n' => out.push_str("\\n"),
                        _ => out.push(c),
                    }
                }
                out.push('"');
            }
            Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    item.diagnose(out);
                }
                out.push(']');
            }
            Value::Map(entries) => {
                // In encoded order, as the deterministic encoding writes them.
                let mut sorted: Vec<&(Value, Value)> = entries.iter().collect();
                sorted.sort_by_key(|(k, _)| k.encode().unwrap_or_default());
                out.push('{');
                for (i, (k, v)) in sorted.into_iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    k.diagnose(out);
                    out.push_str(": ");
                    v.diagnose(out);
                }
                out.push('}');
            }
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Null => out.push_str("null"),
        }
    }
}

/// Lowercase hexadecimal, as the vectors and diagnostic notation write bytes.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// An item's head: the major type and the argument in its shortest form (RFC 8949 §4.2.1).
fn head(out: &mut Vec<u8>, major: u8, n: u64) {
    let be = n.to_be_bytes();
    // Below 24 the argument is the low byte itself; otherwise 1, 2, 4 or 8 bytes follow.
    let (info, width) = match n {
        0..=23 => (be[7], 0),
        24..=0xff => (0x18, 1),
        0x100..=0xffff => (0x19, 2),
        0x1_0000..=0xffff_ffff => (0x1a, 4),
        _ => (0x1b, 8),
    };
    out.push((major << 5) | info);
    out.extend_from_slice(&be[8 - width..]);
}

/// Decode one item that fills `data` exactly.
///
/// # Errors
/// Anything not in deterministic form, or outside RadioPost's subset (RP-0 §4.1).
pub fn decode(data: &[u8]) -> Result<Value, NotCanonical> {
    let mut reader = Reader {
        data,
        pos: 0,
        depth: 0,
    };
    let value = reader.item()?;
    if reader.pos != data.len() {
        return Err(NotCanonical("bytes after the item"));
    }
    Ok(value)
}

/// Deeper nesting than RadioPost uses is refused, so a hostile input cannot exhaust the stack.
const MAX_DEPTH: usize = 32;

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    depth: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], NotCanonical> {
        let end = self.pos.checked_add(n).ok_or(NotCanonical("truncated"))?;
        let slice = self
            .data
            .get(self.pos..end)
            .ok_or(NotCanonical("truncated"))?;
        self.pos = end;
        Ok(slice)
    }

    fn argument(&mut self, info: u8) -> Result<u64, NotCanonical> {
        let n = match info {
            0..=23 => return Ok(u64::from(info)),
            24 => u64::from(self.take(1)?[0]),
            25 => u64::from(u16::from_be_bytes(
                self.take(2)?.try_into().unwrap_or_default(),
            )),
            26 => u64::from(u32::from_be_bytes(
                self.take(4)?.try_into().unwrap_or_default(),
            )),
            27 => u64::from_be_bytes(self.take(8)?.try_into().unwrap_or_default()),
            _ => return Err(NotCanonical("indefinite length or reserved value")),
        };
        let mut shortest = Vec::new();
        head(&mut shortest, 0, n);
        if shortest[0] & 0x1f != info {
            return Err(NotCanonical("a length or integer not in its shortest form"));
        }
        Ok(n)
    }

    fn length(&mut self, info: u8) -> Result<usize, NotCanonical> {
        let n = self.argument(info)?;
        let n = usize::try_from(n).map_err(|_| NotCanonical("truncated"))?;
        if n > self.data.len() - self.pos {
            // Every item takes at least one byte, so a count beyond what is left is truncated.
            return Err(NotCanonical("truncated"));
        }
        Ok(n)
    }

    fn item(&mut self) -> Result<Value, NotCanonical> {
        let initial = *self.take(1)?.first().ok_or(NotCanonical("truncated"))?;
        let (major, info) = (initial >> 5, initial & 0x1f);
        match major {
            0 => Ok(Value::Uint(self.argument(info)?)),
            1 => Ok(Value::Nint(self.argument(info)?)),
            2 => {
                let n = self.length(info)?;
                Ok(Value::Bytes(self.take(n)?.to_vec()))
            }
            3 => {
                let n = self.length(info)?;
                let raw = self.take(n)?;
                let text =
                    std::str::from_utf8(raw).map_err(|_| NotCanonical("text is not UTF-8"))?;
                Ok(Value::Text(text.to_owned()))
            }
            4 => {
                let n = self.length(info)?;
                self.nest()?;
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push(self.item()?);
                }
                self.depth -= 1;
                Ok(Value::Array(items))
            }
            5 => {
                let n = self.length(info)?;
                self.nest()?;
                let mut entries = Vec::with_capacity(n);
                let data = self.data;
                let mut previous: Option<&[u8]> = None;
                for _ in 0..n {
                    let start = self.pos;
                    let key = self.item()?;
                    let key_bytes = &data[start..self.pos];
                    if previous.is_some_and(|p| key_bytes <= p) {
                        return Err(NotCanonical("map keys out of order, or repeated"));
                    }
                    previous = Some(key_bytes);
                    if !matches!(key, Value::Uint(_) | Value::Nint(_) | Value::Text(_)) {
                        return Err(NotCanonical("map keys are integers or text"));
                    }
                    let value = self.item()?;
                    entries.push((key, value));
                }
                self.depth -= 1;
                Ok(Value::Map(entries))
            }
            6 => Err(NotCanonical("tags are not used")),
            _ => match info {
                20 => Ok(Value::Bool(false)),
                21 => Ok(Value::Bool(true)),
                22 => Ok(Value::Null),
                _ => Err(NotCanonical("floats and other simple values are not used")),
            },
        }
    }

    fn nest(&mut self) -> Result<(), NotCanonical> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(NotCanonical("nested too deeply"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_integers_round_trip() {
        for n in [-1_i64, -24, -25, -256, -257, i64::MIN] {
            let v = Value::int(n);
            assert_eq!(v.as_i64(), Some(n));
            assert_eq!(decode(&v.encode().unwrap()).unwrap(), v);
        }
    }

    #[test]
    fn a_duplicate_key_is_not_encoded() {
        let v = Value::int_map(vec![(1, Value::Null), (1, Value::Bool(true))]);
        assert_eq!(v.encode(), Err(DuplicateKey));
    }

    #[test]
    fn deep_nesting_is_refused() {
        let data = vec![0x81; MAX_DEPTH + 1]
            .into_iter()
            .chain([0x00])
            .collect::<Vec<u8>>();
        assert!(decode(&data).is_err());
    }
}
