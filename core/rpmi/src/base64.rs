// SPDX-License-Identifier: MIT OR Apache-2.0
//! Standard base64 with padding (RFC 4648 §4), the spelling of every `data` member (RP-4 §2).
//!
//! RP-4 chose it over RP-0's base64url because adapters are often small scripts and every language
//! has it. Decoding is strict about the alphabet and the padding: base64url, missing padding,
//! whitespace and padding inside the text are all refused. The bits a final group pads with are
//! not checked, as RFC 4648 §3.5 leaves to the decoder.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// The base64 of `data`, padded.
#[must_use]
pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for group in data.chunks(3) {
        let b = [
            group[0],
            group.get(1).copied().unwrap_or(0),
            group.get(2).copied().unwrap_or(0),
        ];
        let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        for i in 0..4 {
            if i <= group.len() {
                let six = (n >> (18 - 6 * i)) & 0x3f;
                out.push(char::from(ALPHABET[six as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn value(c: u8) -> Option<u32> {
    let v = match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    };
    Some(u32::from(v))
}

/// The bytes of `text`, or `None` when it is not standard base64 with padding.
#[must_use]
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let t = text.as_bytes();
    if t.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(t.len() / 4 * 3);
    let groups = t.len() / 4;
    for (g, quad) in t.chunks(4).enumerate() {
        let last = g + 1 == groups;
        let pads = quad.iter().rev().take_while(|&&c| c == b'=').count();
        if pads > 2 || (pads > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for &c in &quad[..4 - pads] {
            n = (n << 6) | value(c)?;
        }
        n <<= 6 * pads;
        let bytes = n.to_be_bytes();
        out.extend_from_slice(&bytes[1..4 - pads]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    #[test]
    fn rfc_4648_section_10() {
        for (plain, coded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode(plain.as_bytes()), coded);
            assert_eq!(decode(coded).as_deref(), Some(plain.as_bytes()));
        }
    }

    #[test]
    fn every_byte_round_trips() {
        let all: Vec<u8> = (0..=255).collect();
        for n in 0..all.len() {
            assert_eq!(decode(&encode(&all[..n])).as_deref(), Some(&all[..n]));
        }
    }

    #[test]
    fn what_is_refused() {
        for bad in [
            "AQ", "-_8=", "AQ=", "A===", "AQ==AQ==", "AQ=A", "AQ= ", " AQ==", "AQ==\n", "====",
        ] {
            assert_eq!(decode(bad), None, "{bad:?}");
        }
    }
}
