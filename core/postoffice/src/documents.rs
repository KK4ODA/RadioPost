// SPDX-License-Identifier: MIT OR Apache-2.0
//! A post office's public documents (RP-5 §4, RP-1 §7, §8), each a JWS signed with its key.

use radiopost::base64url;
use radiopost::crypto;
use radiopost::jws;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::office::PostOffice;

/// How long a revocation list says it holds before the next (RP-1 §8).
pub const REVOCATIONS_EVERY_S: u64 = 6 * 3600;

/// What an operator says of its post office in the info document (RP-5 §4.1).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Info {
    /// The operator's callsign.
    pub operator: String,
    /// Where to write to the operator.
    pub contact: String,
    /// The country the post office is in (RP-6 §5.1).
    pub country: String,
    /// The largest air form it sends or takes by radio (RP-2 §5).
    pub max_air_bytes: u64,
    /// The largest ordinary email it accepts (RP-6).
    pub max_email_bytes: u64,
    /// `any` or `listed` (RP-5 §4.1).
    pub gateways: String,
    /// Where its users' mail programs connect: `jmap`, `imap`.
    pub mail: Json,
    /// When the operator last changed what this says.
    pub updated: u64,
    /// Its keys' validity, for the info document's `keys` (RP-1 §7.2).
    pub not_before: u64,
    /// The end of its key's validity.
    pub not_after: u64,
    /// The post offices it knows and trusts, as `/post-offices` lists them (RP-5 §4.3).
    #[serde(default)]
    pub post_offices: Vec<Json>,
}

fn kid_hex(public: &[u8; 32]) -> String {
    radiopost::cbor::hex(&crypto::kid(public))
}

impl PostOffice {
    /// The payload of a public document at `path`, at `now`, if there is one (RP-5 §4).
    #[must_use]
    pub fn document(&self, info: &Info, path: &str, now: u64) -> Option<Json> {
        let public = self.config.key.verifying_key().to_bytes();
        Some(match path {
            "/info" => json!({
                "version": 1,
                "domains": self.config.domains,
                "operator": info.operator,
                "contact": info.contact,
                "country": info.country,
                "keys": [{"kid": kid_hex(&public), "public": base64url::encode(&public),
                          "not_before": info.not_before, "not_after": info.not_after}],
                "limits": {"max_air_bytes": info.max_air_bytes,
                           "max_email_bytes": info.max_email_bytes},
                "gateways": info.gateways,
                "mail": info.mail,
                "updated": info.updated,
            }),
            "/revocations" => json!({
                "issued": now,
                "next": now + REVOCATIONS_EVERY_S,
                "serials": self.state.revoked,
                "kids": [],
            }),
            "/post-offices" => json!({"issued": now, "post_offices": info.post_offices}),
            "/gateways" => json!({
                "issued": now,
                "gateways": self.state.gateways.values().collect::<Vec<_>>(),
            }),
            "/routes" => json!({"issued": now, "routes": []}),
            _ => return None,
        })
    }

    /// The document at `path`, signed (RP-5 §1, §4).
    #[must_use]
    pub fn signed_document(&self, info: &Info, path: &str, now: u64) -> Option<String> {
        let payload = self.document(info, path, now)?;
        Some(jws::sign(payload.to_string().as_bytes(), &self.config.key))
    }

    /// The DNS records that publish this post office (RP-1 §7.1), one line each, for a zone file.
    #[must_use]
    pub fn dns_records(&self) -> Vec<String> {
        let public = self.config.key.verifying_key().to_bytes();
        let standard = {
            let mut s = base64url::encode(&public)
                .replace('-', "+")
                .replace('_', "/");
            while s.len() % 4 != 0 {
                s.push('=');
            }
            s
        };
        self.config
            .domains
            .iter()
            .flat_map(|d| {
                [
                    format!(
                        "_radiopost.{d}. TXT \"v=RP1; t=api; url={}\"",
                        self.config.base_url
                    ),
                    format!(
                        "_radiopost.{d}. TXT \"v=RP1; t=key; kid={}; p={standard}\"",
                        kid_hex(&public)
                    ),
                ]
            })
            .collect()
    }
}
