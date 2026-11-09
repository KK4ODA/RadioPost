// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-1 §4 and §5: credentials, how a post office makes them and how anyone checks them.

use std::collections::HashMap;

use ed25519_dalek::SigningKey;

use crate::cbor::{self, Value};
use crate::crypto::{self, Sign1, context};
use crate::error::ErrorCode;

/// RP-1 §4.1: what a credential allows.
pub mod flags {
    /// May send mail.
    pub const SEND: u64 = 1;
    /// May pick up mail.
    pub const RECEIVE: u64 = 2;
    /// A role mailbox.
    pub const ROLE: u64 = 4;
    /// May act as a gateway: relay sessions and forward mail.
    pub const GATEWAY: u64 = 8;
}

/// RP-1 §5, step 4: how far ahead of a verifier's clock `issued` may be.
pub const FUTURE_ISSUE_S: u64 = 600;

/// RP-1 §4.1: a credential's body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Body {
    /// Format version: 1.
    pub v: u64,
    /// Unique among the credentials its post office has issued.
    pub serial: u64,
    /// The account (RP-0 §6.2).
    pub address: String,
    /// The operator's callsign.
    pub operator: String,
    /// The device's Ed25519 public key.
    pub device: [u8; 32],
    /// Time of issue.
    pub issued: u64,
    /// Not valid at or after this time.
    pub expires: u64,
    /// Bits of [`flags`].
    pub flags: u64,
}

impl Body {
    /// The body's CBOR map.
    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::int_map(vec![
            (1, Value::Uint(self.v)),
            (2, Value::Uint(self.serial)),
            (3, Value::text(&self.address)),
            (4, Value::text(&self.operator)),
            (5, Value::bytes(&self.device)),
            (6, Value::Uint(self.issued)),
            (7, Value::Uint(self.expires)),
            (9, Value::Uint(self.flags)),
        ])
    }

    /// A body from its map; `None` when a field is missing or of the wrong type.
    #[must_use]
    pub fn from_value(map: &Value) -> Option<Self> {
        let uint = |k| map.get(k).and_then(Value::as_u64);
        let text = |k| map.get(k).and_then(Value::as_text).map(str::to_owned);
        Some(Body {
            v: uint(1)?,
            serial: uint(2)?,
            address: text(3)?,
            operator: text(4)?,
            device: map.get(5)?.as_bytes()?.try_into().ok()?,
            issued: uint(6)?,
            expires: uint(7)?,
            flags: uint(9)?,
        })
    }

    /// The domain of the account's post office, in lowercase.
    #[must_use]
    pub fn domain(&self) -> String {
        self.address
            .rsplit_once('@')
            .map_or("", |(_, d)| d)
            .to_lowercase()
    }
}

/// A credential: the COSE_Sign1 a post office signed, and its body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credential {
    /// The signed object.
    pub sign1: Sign1,
    /// Its body, as the payload holds it.
    pub body: Body,
}

impl Credential {
    /// The post office signs a body (RP-1 §4.1).
    #[must_use]
    pub fn issue(body: &Body, key: &SigningKey) -> Self {
        let payload = body.to_value().encode().unwrap_or_default();
        Credential {
            sign1: Sign1::sign(context::CREDENTIAL, &payload, key),
            body: body.clone(),
        }
    }

    /// The credential's bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        self.sign1.encode()
    }

    /// A credential from its bytes, unverified; `None` when it does not decode as RP-1 §4.1 says.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let sign1 = Sign1::open(bytes)?;
        let body = Body::from_value(&cbor::decode(&sign1.payload).ok()?)?;
        Some(Credential { sign1, body })
    }
}

/// RP-1 §4.3: the first 8 bytes of SHA-256 of a credential's bytes.
#[must_use]
pub fn hash(credential: &[u8]) -> [u8; 8] {
    let h = crypto::sha256(credential);
    [h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]]
}

/// A post office key a verifier holds, with its validity period (RP-1 §7.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostOfficeKey {
    /// The 32-byte public key.
    pub public: [u8; 32],
    /// Valid from this time.
    pub not_before: u64,
    /// Valid until (not at) this time.
    pub not_after: u64,
}

/// What a post office has revoked (RP-1 §8).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Revocations {
    /// Credentials, by serial.
    pub serials: Vec<u64>,
    /// Post office keys, by ID.
    pub kids: Vec<[u8; 4]>,
}

/// The post offices a verifier knows: their keys and revocation lists, by domain.
#[derive(Clone, Debug, Default)]
pub struct Trust {
    /// Keys by domain.
    pub keys: HashMap<String, Vec<PostOfficeKey>>,
    /// Revocations by domain.
    pub revoked: HashMap<String, Revocations>,
}

/// A credential that failed RP-1 §5, with the body when it decoded that far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    /// The error RP-1 §5 names.
    pub code: ErrorCode,
    /// The body, when the credential decoded.
    pub body: Option<Box<Body>>,
}

/// RP-1 §5: check a credential, in order, at `now`, for the flags `need`.
///
/// # Errors
/// The first check that fails, with its code.
pub fn verify(credential: &[u8], trust: &Trust, now: u64, need: u64) -> Result<Body, Refused> {
    let refuse = |code, body: Option<&Body>| Refused {
        code,
        body: body.cloned().map(Box::new),
    };
    let Some(Credential { sign1, body }) = Credential::decode(credential) else {
        return Err(refuse(ErrorCode::CredentialInvalid, None));
    };
    if body.v != 1 {
        return Err(refuse(ErrorCode::CredentialInvalid, Some(&body)));
    }
    let domain = body.domain();
    let Some(keys) = trust.keys.get(&domain).filter(|k| !k.is_empty()) else {
        return Err(refuse(ErrorCode::UnknownPostOffice, Some(&body)));
    };
    let signed = keys.iter().any(|k| {
        crypto::kid(&k.public) == sign1.kid
            && k.not_before <= body.issued
            && body.issued < k.not_after
            && sign1.verify(context::CREDENTIAL, &k.public)
    });
    if !signed {
        return Err(refuse(ErrorCode::CredentialInvalid, Some(&body)));
    }
    if now >= body.expires {
        return Err(refuse(ErrorCode::CredentialExpired, Some(&body)));
    }
    if body.issued > now.saturating_add(FUTURE_ISSUE_S) {
        return Err(refuse(ErrorCode::CredentialInvalid, Some(&body)));
    }
    let gone = trust.revoked.get(&domain);
    if gone.is_some_and(|g| g.serials.contains(&body.serial) || g.kids.contains(&sign1.kid)) {
        return Err(refuse(ErrorCode::CredentialRevoked, Some(&body)));
    }
    if body.flags & need != need {
        return Err(refuse(ErrorCode::NotAuthorized, Some(&body)));
    }
    Ok(body)
}
