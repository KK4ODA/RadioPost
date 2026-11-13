// SPDX-License-Identifier: MIT OR Apache-2.0
//! The key cache (design §5.2, RP-1 §7.3): for each post office whose users call, its keys, info
//! document and revocation list, fetched while online, refreshed every six hours, and kept for
//! checking callers while offline (RP-1 §5, §6.3).
//!
//! A post office is learned from DNS when one of its users calls (RP-1 §7.1, §7.2): its records,
//! its info document signed by a key there, its revocation list signed by an accepted key; the
//! check is the post office's own ([`radiopost_postoffice::known::assemble`]). The post offices its
//! list names (RP-5 §4.3), signed by its keys, are kept too, with the keys the list gives and the
//! revocation list each publishes: RP-1 §7.3 trusts them while offline, one hop only
//! ([`crate::store::Store::trust`]).
//!
//! A gateway that reaches its own post office at a URL its settings name, without DNS, anchors
//! that post office on the key that signed its own credential, which it got over HTTPS when it
//! registered: an info document signed by that key adds the rest, as DNS would.

use std::collections::{BTreeMap, HashMap};

use radiopost::credential::{self, Credential, PostOfficeKey, Trust};
use radiopost::{base64url, crypto, jws};
use radiopost_postoffice::known::{self, Found, Key};
use serde_json::Value as Json;

use crate::store::Keyed;

/// How often a post office's documents are fetched again (RP-1 §7.3).
pub const REFRESH_S: u64 = 6 * 3600;

/// Where a post office's documents are fetched, and with what anchor.
pub struct Source<'a> {
    /// The DNS-over-HTTPS resolver; empty, DNS is not asked.
    pub resolver: &'a str,
    /// The API URL the gateway's settings name for the domain, when they name one.
    pub url: Option<&'a str>,
    /// The gateway's own credential, the anchor for its own post office without DNS.
    pub own_credential: Option<&'a [u8]>,
}

fn get(api: &str, path: &str) -> Option<String> {
    let http = radiopost_api::Http::anonymous(api);
    match http.send_text("GET", path, None, None) {
        Ok((200, text)) => Some(text.trim().to_owned()),
        _ => None,
    }
}

/// The payload of a JWS, read without checking it: only to find which of its keys to try.
fn unchecked_payload(jws: &str) -> Option<Json> {
    let body = jws.split('.').nth(1)?;
    serde_json::from_slice(&base64url::decode(body)?).ok()
}

/// A DNS key record as RP-1 §7.1 writes one, for the anchor.
fn key_record(public: &[u8; 32]) -> BTreeMap<String, String> {
    let b64 = base64url::encode(public)
        .replace('-', "+")
        .replace('_', "/");
    let pad = "=".repeat((4 - b64.len() % 4) % 4);
    BTreeMap::from([
        ("v".to_owned(), "RP1".to_owned()),
        ("t".to_owned(), "key".to_owned()),
        ("p".to_owned(), format!("{b64}{pad}")),
    ])
}

/// The key, among those an info document lists, that signed `credential`.
fn anchor(domain: &str, info: &str, credential: &[u8], at: u64) -> Option<[u8; 32]> {
    let payload = unchecked_payload(info)?;
    payload["keys"]
        .as_array()?
        .iter()
        .filter_map(|k| {
            base64url::decode(k["public"].as_str()?).and_then(|p| <[u8; 32]>::try_from(p).ok())
        })
        .find(|public| {
            let mut trust = Trust::default();
            trust.keys.insert(
                domain.to_owned(),
                vec![PostOfficeKey {
                    public: *public,
                    not_before: 0,
                    not_after: u64::MAX,
                }],
            );
            let body = Credential::decode(credential).map(|c| c.body);
            let when = body.map_or(at, |b| b.issued);
            credential::verify(credential, &trust, when, 0).is_ok()
        })
}

/// Fetches and checks `domain`'s documents: what to keep of it, and of each post office its list
/// names (one hop), or why it could not be fetched whole.
///
/// # Errors
///
/// No API record, no answer, or documents that do not verify.
pub fn fetch(domain: &str, source: &Source<'_>, at: u64) -> Result<Vec<(String, Keyed)>, String> {
    let mut how = "dns";
    let mut records = if source.resolver.is_empty() {
        Vec::new()
    } else {
        radiopost_api::records(source.resolver, domain)
    };
    let api = records
        .iter()
        .find(|t| t.get("t").map(String::as_str) == Some("api"))
        .and_then(|t| t.get("url").cloned());
    let api = match (api, source.url, source.own_credential) {
        (Some(api), _, _) => api,
        (None, Some(url), Some(credential)) => {
            // its own post office, named in the settings: anchored on the gateway's credential
            let info = get(url, "/info").ok_or_else(|| format!("{url}: no info document"))?;
            let public = anchor(domain, &info, credential, at).ok_or_else(|| {
                format!(
                    "{domain}'s info document lists no key that signed this gateway's credential"
                )
            })?;
            how = "credential";
            records = vec![
                BTreeMap::from([
                    ("v".to_owned(), "RP1".to_owned()),
                    ("t".to_owned(), "api".to_owned()),
                    ("url".to_owned(), url.to_owned()),
                ]),
                key_record(&public),
            ];
            url.to_owned()
        }
        _ => return Err(format!("{domain}: no API record found")),
    };
    let info = get(&api, "/info");
    let revocations = get(&api, "/revocations");
    let (found, error) = known::assemble(
        domain,
        &records,
        false,
        info.as_deref(),
        revocations.as_deref(),
        at,
    )
    .ok_or_else(|| format!("{domain}: no API record"))?;
    if let Some(e) = error {
        return Err(format!("{domain}: {e}"));
    }
    let mut out = vec![(
        domain.to_owned(),
        Keyed {
            found: found.clone(),
            how: how.to_owned(),
            fetched: at,
            error: None,
        },
    )];
    if let Some(list) = get(&api, "/post-offices") {
        out.extend(listed(domain, &found, &list, at));
    }
    Ok(out)
}

/// The post offices `domain`'s list names (RP-5 §4.3), when its keys signed it: each with the keys
/// the list gives, and its revocation list when one signed by those keys could be fetched.
fn listed(domain: &str, found: &Found, list: &str, at: u64) -> Vec<(String, Keyed)> {
    let keys: HashMap<String, [u8; 32]> = found
        .trust_keys()
        .iter()
        .map(|k| (radiopost::cbor::hex(&crypto::kid(&k.public)), k.public))
        .collect();
    let Ok(payload) = jws::verify(list, &keys) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in payload["post_offices"].as_array().into_iter().flatten() {
        let (Some(other), Some(api)) = (entry["domain"].as_str(), entry["api"].as_str()) else {
            continue;
        };
        let other = other.to_lowercase();
        if other == domain {
            continue;
        }
        let entry_keys: Vec<Key> = entry["keys"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|k| {
                let public: [u8; 32] = base64url::decode(k["public"].as_str()?)?.try_into().ok()?;
                Some(Key {
                    kid: radiopost::cbor::hex(&crypto::kid(&public)),
                    public: base64url::encode(&public),
                    not_before: k["not_before"].as_u64().unwrap_or(0),
                    not_after: k["not_after"].as_u64().unwrap_or(u64::MAX),
                })
            })
            .collect();
        let mut keyed = Found {
            api: api.to_owned(),
            keys: entry_keys,
            at,
            ..Found::default()
        };
        let by_kid: HashMap<String, [u8; 32]> = keyed
            .trust_keys()
            .iter()
            .map(|k| (radiopost::cbor::hex(&crypto::kid(&k.public)), k.public))
            .collect();
        let mut error = None;
        match get(api, "/revocations").map(|r| jws::verify(&r, &by_kid)) {
            Some(Ok(r)) => {
                keyed.serials = r["serials"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Json::as_u64)
                    .collect();
                keyed.kids = r["kids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|k| k.as_str().map(str::to_owned))
                    .collect();
            }
            _ => error = Some("its revocation list could not be fetched".to_owned()),
        }
        out.push((
            other,
            Keyed {
                found: keyed,
                how: format!("list:{domain}"),
                fetched: at,
                error,
            },
        ));
    }
    out
}

/// Keeps what [`fetch`] found: a post office learned from DNS is never replaced by one hop of a
/// list.
pub fn keep(cache: &mut BTreeMap<String, Keyed>, fetched: Vec<(String, Keyed)>) {
    for (domain, keyed) in fetched {
        let from_list = keyed.how.starts_with("list:");
        if from_list
            && cache
                .get(&domain)
                .is_some_and(|k| !k.how.starts_with("list:"))
        {
            continue;
        }
        cache.insert(domain, keyed);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use radiopost_postoffice::known::Found;

    use super::keep;
    use crate::store::Keyed;

    #[test]
    fn a_list_never_replaces_dns() {
        let keyed = |how: &str| Keyed {
            found: Found::default(),
            how: how.into(),
            fetched: 1,
            error: None,
        };
        let mut cache = BTreeMap::new();
        keep(&mut cache, vec![("hampost.org".into(), keyed("dns"))]);
        keep(
            &mut cache,
            vec![("hampost.org".into(), keyed("list:radiopost.org"))],
        );
        assert_eq!(cache["hampost.org"].how, "dns");
        keep(
            &mut cache,
            vec![("other.org".into(), keyed("list:radiopost.org"))],
        );
        assert_eq!(cache["other.org"].how, "list:radiopost.org");
    }
}
