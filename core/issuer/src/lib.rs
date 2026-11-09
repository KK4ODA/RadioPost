// SPDX-License-Identifier: MIT OR Apache-2.0
//! A post office's signing key, and the credentials it issues (RP-1 §4), for on-air tests before
//! the post office service (Phase 2) exists.
//!
//! The vectors' test keys are published, so they must never reach the air (the conformance
//! tester's outline, §2.3). For a test on the air, the post office's operator makes a key of its
//! own, issues each tester a credential for the tester's device key, and gives every tester the
//! key to trust (RP-1 §7.3: the operator decides). The key file is the post office's secret.

use std::fs;
use std::path::Path;

use ed25519_dalek::SigningKey;
use radiopost::credential::{Body, Credential, flags};
use radiopost::crypto::kid;
use serde_json::{Value, json};

/// What went wrong, for the operator.
pub type Error = String;

/// The longest a credential lasts (RP-1 §4.2: a post office SHOULD issue no more than 90 days).
pub const MAX_DAYS: u64 = 90;

fn hex(bytes: &[u8]) -> String {
    radiopost::cbor::hex(bytes)
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// A post office key and what goes with it, kept in one JSON file.
#[derive(Clone, Debug)]
pub struct PostOffice {
    /// The domain whose accounts it issues credentials for.
    pub domain: String,
    /// The signing key.
    pub key: SigningKey,
    /// The key's validity: clients trust it from and until these times.
    pub not_before: u64,
    /// See `not_before`.
    pub not_after: u64,
    /// The serial the next credential takes (RP-1 §4.1: unique to this post office).
    pub next_serial: u64,
}

impl PostOffice {
    /// A new key for `domain`, valid from `now` for `days`.
    ///
    /// # Errors
    ///
    /// The system's random numbers failed.
    pub fn new(domain: &str, now: u64, days: u64) -> Result<PostOffice, Error> {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|e| e.to_string())?;
        Ok(PostOffice {
            domain: domain.trim().to_ascii_lowercase(),
            key: SigningKey::from_bytes(&seed),
            not_before: now,
            not_after: now + days * 86_400,
            next_serial: 1,
        })
    }

    /// The key's public half.
    #[must_use]
    pub fn public(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    /// What a client runs to trust this key (RP-1 §7.3).
    #[must_use]
    pub fn trust_command(&self) -> String {
        format!(
            "radiopost trust {} {} {} {}",
            self.domain,
            hex(&self.public()),
            self.not_before,
            self.not_after
        )
    }

    /// Reads a key file.
    ///
    /// # Errors
    ///
    /// It cannot be read, or is not a key file.
    pub fn load(path: &Path) -> Result<PostOffice, Error> {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let v: Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let bad = || format!("{} is not a post office key file", path.display());
        let seed: [u8; 32] = v["seed"]
            .as_str()
            .and_then(unhex)
            .and_then(|b| b.try_into().ok())
            .ok_or_else(bad)?;
        Ok(PostOffice {
            domain: v["domain"].as_str().ok_or_else(bad)?.to_owned(),
            key: SigningKey::from_bytes(&seed),
            not_before: v["not_before"].as_u64().ok_or_else(bad)?,
            not_after: v["not_after"].as_u64().ok_or_else(bad)?,
            next_serial: v["next_serial"].as_u64().ok_or_else(bad)?,
        })
    }

    /// Writes the key file, whole or not at all.
    ///
    /// # Errors
    ///
    /// It cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        let v = json!({
            "domain": self.domain,
            "seed": hex(&self.key.to_bytes()),
            "public": hex(&self.public()),
            "kid": hex(&kid(&self.public())),
            "not_before": self.not_before,
            "not_after": self.not_after,
            "next_serial": self.next_serial,
        });
        let text = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Issues a credential (RP-1 §4.1) for `address`, its operator's callsign and the device's
    /// public key, valid from `now` for `days`, with the next serial.
    ///
    /// # Errors
    ///
    /// The address is not of this post office's domain, the key is not valid then, the lifetime is
    /// over RP-1 §4.2's 90 days, or the device key is not 32 bytes.
    pub fn issue(
        &mut self,
        address: &str,
        operator: &str,
        device_hex: &str,
        now: u64,
        days: u64,
        flag_bits: u64,
    ) -> Result<Vec<u8>, Error> {
        let address = address.trim();
        let Some((local, domain)) = address.rsplit_once('@') else {
            return Err(format!("{address} is not an address"));
        };
        if !domain.eq_ignore_ascii_case(&self.domain) || local.is_empty() {
            return Err(format!("{address} is not an account of {}", self.domain));
        }
        if days == 0 || days > MAX_DAYS {
            return Err(format!(
                "a credential lasts 1 to {MAX_DAYS} days (RP-1 §4.2)"
            ));
        }
        let expires = now + days * 86_400;
        if now < self.not_before || expires > self.not_after {
            return Err("the post office key is not valid for that long: make a new key".into());
        }
        let device: [u8; 32] = unhex(device_hex)
            .and_then(|b| b.try_into().ok())
            .ok_or("the device key is 32 bytes in hex, as `radiopost key` prints it")?;
        let operator = operator.trim().to_ascii_uppercase();
        if operator.is_empty() {
            return Err("the operator's callsign".into());
        }
        let body = Body {
            v: 1,
            serial: self.next_serial,
            address: format!("{local}@{}", domain.to_ascii_lowercase()),
            operator,
            device,
            issued: now,
            expires,
            flags: flag_bits,
        };
        self.next_serial += 1;
        Ok(Credential::issue(&body, &self.key).encode())
    }
}

/// Credential flags by name (RP-1 §4.1): `send`, `receive`, `role`, `gateway`, joined by commas.
///
/// # Errors
///
/// A name RP-1 does not give.
pub fn flags_of(names: &str) -> Result<u64, Error> {
    names
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .try_fold(0, |bits, name| {
            Ok(bits
                | match name {
                    "send" => flags::SEND,
                    "receive" => flags::RECEIVE,
                    "role" => flags::ROLE,
                    "gateway" => flags::GATEWAY,
                    other => return Err(format!("no flag {other}: send, receive, role, gateway")),
                })
        })
}
