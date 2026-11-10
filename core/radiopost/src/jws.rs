// SPDX-License-Identifier: MIT OR Apache-2.0
//! JWS compact serialisation with `EdDSA` (RFC 7515, RFC 8037), as RP-0 §5 and RP-5 §1 use it for a
//! post office's public documents.
//!
//! The signature is plain JWS, over the signing input with no RadioPost context: that input is
//! base64url text and a dot, so it never holds the zero byte that ends every RadioPost context and
//! cannot be taken for another RadioPost signature (RP-0 §5).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::hash::BuildHasher;

use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use serde_json::{Value, json};

use crate::base64url;
use crate::crypto;
use crate::error::ErrorCode;

/// A JWS over `payload`, its header naming the key by its ID in hex (RP-5 §1).
#[must_use]
pub fn sign(payload: &[u8], key: &SigningKey) -> String {
    let kid = crypto::kid(&key.verifying_key().to_bytes());
    let header = json!({"alg": "EdDSA", "kid": hex(&kid)}).to_string();
    let input = format!(
        "{}.{}",
        base64url::encode(header.as_bytes()),
        base64url::encode(payload)
    );
    let signature = key.sign(input.as_bytes()).to_bytes();
    format!("{input}.{}", base64url::encode(&signature))
}

/// A JWS's payload as JSON, when one of `keys` (key ID in hex, public key) signed it.
///
/// # Errors
///
/// `malformed` when it is not three base64url parts with a JSON object header and payload;
/// `signature-invalid` when the algorithm is not `EdDSA`, the header is critical, or no key given
/// signed it.
pub fn verify<S: BuildHasher>(
    jws: &str,
    keys: &HashMap<String, [u8; 32], S>,
) -> Result<Value, ErrorCode> {
    let parts: Vec<&str> = jws.split('.').collect();
    let [head, body, sig] = parts[..] else {
        return Err(ErrorCode::Malformed);
    };
    let (Some(header), Some(payload), Some(signature)) = (
        base64url::decode(head),
        base64url::decode(body),
        base64url::decode(sig),
    ) else {
        return Err(ErrorCode::Malformed);
    };
    let header: Value = serde_json::from_slice(&header).map_err(|_| ErrorCode::Malformed)?;
    let document: Value = serde_json::from_slice(&payload).map_err(|_| ErrorCode::Malformed)?;
    if !header.is_object() || !document.is_object() {
        return Err(ErrorCode::Malformed);
    }
    if header.get("alg").and_then(Value::as_str) != Some("EdDSA") || header.get("crit").is_some() {
        return Err(ErrorCode::SignatureInvalid);
    }
    let public = header
        .get("kid")
        .and_then(Value::as_str)
        .and_then(|k| keys.get(k))
        .ok_or(ErrorCode::SignatureInvalid)?;
    let input = format!("{head}.{body}");
    let good = VerifyingKey::from_bytes(public).is_ok_and(|vk| {
        Signature::from_slice(&signature)
            .is_ok_and(|s| vk.verify_strict(input.as_bytes(), &s).is_ok())
    });
    if good {
        Ok(document)
    } else {
        Err(ErrorCode::SignatureInvalid)
    }
}

/// RP-1 §7.2: the keys a verifier accepts, by ID in hex: those in DNS, and those an info document
/// signed by a DNS key lists, each only when its ID is that key's.
#[must_use]
pub fn accepted_keys<S: BuildHasher + Clone>(
    dns: &HashMap<String, [u8; 32], S>,
    info: Option<&str>,
) -> HashMap<String, [u8; 32], S> {
    let mut keys = dns.clone();
    if let Some(Ok(document)) = info.map(|i| verify(i, dns)) {
        for k in document
            .get("keys")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let public = k
                .get("public")
                .and_then(Value::as_str)
                .and_then(base64url::decode)
                .and_then(|p| <[u8; 32]>::try_from(p).ok());
            let kid = k.get("kid").and_then(Value::as_str);
            if let (Some(public), Some(kid)) = (public, kid)
                && hex(&crypto::kid(&public)) == kid
            {
                keys.insert(kid.to_owned(), public);
            }
        }
    }
    keys
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}
