// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-5 §4.3 and §4.4: a gateway list read from the home's list and the list of every post office
//! the home names in `/post-offices`. A client reads it so; a post office reads it for the
//! gateways its users name (RP-5 §5.8). No input or output here: the reader fetches.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;

use crate::base64url;
use crate::crypto;
use crate::error::ErrorCode;
use crate::jws;

/// One entry of a `/post-offices` document (RP-5 §4.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The domain it serves.
    pub domain: String,
    /// Its API's base URL.
    pub api: String,
    /// Its keys by ID in hex, each kept only when the ID is that key's (RP-0 §5).
    pub keys: HashMap<String, [u8; 32]>,
}

/// The entries of a `/post-offices` payload, in its order; one with no `api` is left out.
#[must_use]
pub fn entries(payload: &Value) -> Vec<Entry> {
    payload["post_offices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let domain = e["domain"].as_str()?.to_lowercase();
            let api = e["api"].as_str().filter(|a| !a.is_empty())?.to_owned();
            let keys = e["keys"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|k| {
                    let public: [u8; 32] =
                        base64url::decode(k["public"].as_str()?)?.try_into().ok()?;
                    let kid = k["kid"].as_str()?;
                    (crate::cbor::hex(&crypto::kid(&public)) == kid)
                        .then(|| (kid.to_owned(), public))
                })
                .collect();
            Some(Entry { domain, api, keys })
        })
        .collect()
}

/// Each API once, with the first entry that names it: one post office serving several domains is
/// asked once, and its list counted once.
#[must_use]
pub fn by_api(entries: &[Entry]) -> Vec<&Entry> {
    let mut seen = HashSet::new();
    entries
        .iter()
        .filter(|e| seen.insert(e.api.clone()))
        .collect()
}

/// A post office's `/gateways` checked with its entry's keys and those its info document adds,
/// signed by one of them (RP-1 §7.2): its records.
///
/// # Errors
///
/// The list is not a JWS those keys verify (RP-0 §5's codes).
pub fn verify_list(entry: &Entry, info: Option<&str>, list: &str) -> Result<Vec<Value>, ErrorCode> {
    let keys = jws::accepted_keys(&entry.keys, info);
    let payload = jws::verify(list, &keys)?;
    Ok(payload["gateways"].as_array().cloned().unwrap_or_default())
}

/// The list a reader makes: every record, each with `post_office`, and the callsigns listed by
/// two post offices neither of which is the home.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Merged {
    /// The home's records first, then each other list's, in order.
    pub gateways: Vec<Value>,
    /// Callsigns in more than one list other than the home's (RP-5 §4.4): a client keeps each
    /// record, and a post office places no copies at them.
    pub ambiguous: Vec<String>,
}

fn tagged(record: &Value, domain: &str) -> Value {
    let mut r = record.clone();
    if let Some(o) = r.as_object_mut() {
        o.insert("post_office".into(), Value::String(domain.to_owned()));
    }
    r
}

/// RP-5 §4.4: the home's records, and those of the `others` lists (each with its post office's
/// domain, as checked by [`verify_list`]) for callsigns the home does not list. A reader whose
/// home says `gateways: "listed"` passes no others.
#[must_use]
pub fn merge(home: &str, home_gateways: &[Value], others: &[(String, Vec<Value>)]) -> Merged {
    let callsign = |r: &Value| r["callsign"].as_str().unwrap_or("").to_owned();
    let home_calls: HashSet<String> = home_gateways.iter().map(callsign).collect();
    let mut gateways: Vec<Value> = home_gateways.iter().map(|r| tagged(r, home)).collect();
    let mut counted: BTreeMap<String, usize> = BTreeMap::new();
    for (domain, records) in others {
        for r in records {
            let c = callsign(r);
            if home_calls.contains(&c) {
                continue; // the home's record wins
            }
            *counted.entry(c).or_default() += 1;
            gateways.push(tagged(r, domain));
        }
    }
    Merged {
        gateways,
        ambiguous: counted
            .into_iter()
            .filter(|(_, n)| *n > 1)
            .map(|(c, _)| c)
            .collect(),
    }
}
