// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-0 §5 and §6.3: hashes, key IDs, content IDs, signatures with a context, and COSE_Sign1.

use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use sha2::{Digest as _, Sha256};

use crate::cbor::Value;

/// RP-0 §9.2: the signature contexts.
pub mod context {
    /// A credential's payload, as COSE external data (RP-1 §4).
    pub const CREDENTIAL: &str = "RadioPost-credential-v1";
    /// A session statement (RP-1 §6).
    pub const SESSION: &str = "RadioPost-session-v1";
    /// A post office API login (RP-5 §3.2).
    pub const API_LOGIN: &str = "RadioPost-api-login-v1";
    /// A new device's enrollment (RP-5 §6.1).
    pub const API_ENROLL: &str = "RadioPost-api-enroll-v1";
    /// A gateway's registration (RP-5 §6.4).
    pub const API_GATEWAY: &str = "RadioPost-api-gateway-v1";
    /// An application for an account, from a new device (RP-5 §6.7).
    pub const API_APPLY: &str = "RadioPost-api-apply-v1";
    /// A post office's placement, as COSE external data (RP-7 §4.2).
    pub const PLACEMENT: &str = "RadioPost-placement-v1";
}

/// SHA-256.
#[must_use]
pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// RP-0 §5: a key's ID, the first 4 bytes of SHA-256 of its 32-byte public key.
#[must_use]
pub fn kid(public: &[u8; 32]) -> [u8; 4] {
    let h = sha256(public);
    [h[0], h[1], h[2], h[3]]
}

/// A content ID: the first 12 bytes of SHA-256 of a canonical encoding (RP-2 §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentId(pub [u8; 12]);

const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

impl ContentId {
    /// The content ID of a canonical encoding.
    #[must_use]
    pub fn of(canonical: &[u8]) -> Self {
        let h = sha256(canonical);
        let mut id = [0u8; 12];
        id.copy_from_slice(&h[..12]);
        ContentId(id)
    }

    /// RP-0 §6.3: base32, lowercase, RFC 4648 alphabet, no padding (20 characters).
    #[must_use]
    pub fn text(&self) -> String {
        let mut bits: u32 = 0;
        let mut held = 0;
        let mut out = String::with_capacity(20);
        for &b in &self.0 {
            bits = (bits << 8) | u32::from(b);
            held += 8;
            while held >= 5 {
                held -= 5;
                out.push(char::from(BASE32[((bits >> held) & 31) as usize]));
            }
        }
        if held > 0 {
            out.push(char::from(BASE32[((bits << (5 - held)) & 31) as usize]));
        }
        out
    }

    /// A content ID from its text, or `None` when the text is not exactly one.
    #[must_use]
    pub fn from_text(text: &str) -> Option<Self> {
        if text.len() != 20 {
            return None;
        }
        let mut bits: u32 = 0;
        let mut held = 0;
        let mut out = Vec::with_capacity(12);
        for c in text.bytes() {
            let v = BASE32.iter().position(|&a| a == c)?;
            bits = (bits << 5) | u32::try_from(v).ok()?;
            held += 5;
            if held >= 8 {
                held -= 8;
                out.push(u8::try_from((bits >> held) & 0xff).ok()?);
            }
        }
        let id = ContentId(out.try_into().ok()?);
        // Unused low bits must be zero, so that each ID has one text.
        (id.text() == text).then_some(id)
    }

    /// The bytes as a slice.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// A content ID from 12 bytes held in a slice.
    #[must_use]
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        Some(ContentId(bytes.try_into().ok()?))
    }
}

/// RP-0 §5: the bytes a context signature is made over: the context, a zero byte, the data.
#[must_use]
pub fn signed_input(context: &str, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(context.len() + 1 + data.len());
    out.extend_from_slice(context.as_bytes());
    out.push(0);
    out.extend_from_slice(data);
    out
}

/// An Ed25519 signature over `data` with a context (RP-0 §5).
#[must_use]
pub fn sign(context: &str, data: &[u8], key: &SigningKey) -> [u8; 64] {
    key.sign(&signed_input(context, data)).to_bytes()
}

/// Whether `signature` is `public`'s over `data` with a context (RP-0 §5).
#[must_use]
pub fn verify(context: &str, data: &[u8], signature: &[u8], public: &[u8; 32]) -> bool {
    verify_raw(&signed_input(context, data), signature, public)
}

fn verify_raw(message: &[u8], signature: &[u8], public: &[u8; 32]) -> bool {
    let Ok(signature) = Signature::from_slice(signature) else {
        return false;
    };
    VerifyingKey::from_bytes(public).is_ok_and(|key| key.verify_strict(message, &signature).is_ok())
}

/// The COSE algorithm identifier of Ed25519 (RFC 9864).
pub const COSE_ED25519: i64 = -19;

/// RP-0 §5: a COSE_Sign1's protected header, exactly `{1: -19, 4: kid}`.
#[must_use]
pub fn protected_header(kid: &[u8; 4]) -> Vec<u8> {
    let header = Value::int_map(vec![(1, Value::int(COSE_ED25519)), (4, Value::bytes(kid))]);
    header.encode().unwrap_or_default()
}

/// RP-0 §5: COSE's `Sig_structure` (RFC 9052 §4.4), the context as external data.
#[must_use]
pub fn to_be_signed(context: &str, protected: &[u8], payload: &[u8]) -> Vec<u8> {
    let structure = Value::Array(vec![
        Value::text("Signature1"),
        Value::bytes(protected),
        Value::bytes(context.as_bytes()),
        Value::bytes(payload),
    ]);
    structure.encode().unwrap_or_default()
}

/// An untagged COSE_Sign1 in RadioPost's form (RP-0 §5), made or opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sign1 {
    /// The signing key's ID, from the protected header.
    pub kid: [u8; 4],
    /// The protected header's bytes.
    pub protected: Vec<u8>,
    /// The payload's bytes.
    pub payload: Vec<u8>,
    /// The signature.
    pub signature: [u8; 64],
}

impl Sign1 {
    /// Sign `payload` with `key`, under `context` (RP-0 §5).
    #[must_use]
    pub fn sign(context: &str, payload: &[u8], key: &SigningKey) -> Self {
        let kid = kid(&key.verifying_key().to_bytes());
        let protected = protected_header(&kid);
        let signature = key
            .sign(&to_be_signed(context, &protected, payload))
            .to_bytes();
        Sign1 {
            kid,
            protected,
            payload: payload.to_vec(),
            signature,
        }
    }

    /// The object's encoding: `[protected, {}, payload, signature]`.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        Value::Array(vec![
            Value::bytes(&self.protected),
            Value::Map(vec![]),
            Value::bytes(&self.payload),
            Value::bytes(&self.signature),
        ])
        .encode()
        .unwrap_or_default()
    }

    /// Open an object, or `None` if it is not in RadioPost's form: any other algorithm or header,
    /// an unprotected header that is not empty, a tag, or bytes that are not deterministic CBOR.
    #[must_use]
    pub fn open(object: &[u8]) -> Option<Self> {
        let outer = crate::cbor::decode(object).ok()?;
        let [protected, unprotected, payload, signature] = outer.as_array()? else {
            return None;
        };
        if !unprotected.as_map()?.is_empty() {
            return None;
        }
        let protected = protected.as_bytes()?.to_vec();
        let header = crate::cbor::decode(&protected).ok()?;
        let entries = header.as_map()?;
        if entries.len() != 2 || header.get(1)?.as_i64()? != COSE_ED25519 {
            return None;
        }
        let kid: [u8; 4] = header.get(4)?.as_bytes()?.try_into().ok()?;
        Some(Sign1 {
            kid,
            protected,
            payload: payload.as_bytes()?.to_vec(),
            signature: signature.as_bytes()?.try_into().ok()?,
        })
    }

    /// Whether the signature is `public`'s, under `context`.
    #[must_use]
    pub fn verify(&self, context: &str, public: &[u8; 32]) -> bool {
        verify_raw(
            &to_be_signed(context, &self.protected, &self.payload),
            &self.signature,
            public,
        )
    }
}
