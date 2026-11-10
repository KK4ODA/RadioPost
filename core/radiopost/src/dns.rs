// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-1 §7.1 and §7.3: a post office's DNS records read, and which post offices a gateway trusts.

use std::collections::BTreeMap;

use crate::crypto;

/// A record's tags, by name, or `None` when RP-1 §7.1 says the whole record is to be ignored.
#[must_use]
pub fn parse(txt: &str) -> Option<BTreeMap<String, String>> {
    let mut tags = BTreeMap::new();
    for part in txt.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (name, value) = part.split_once('=')?;
        if tags
            .insert(name.trim().to_owned(), value.trim().to_owned())
            .is_some()
        {
            return None; // a tag twice
        }
    }
    if tags.get("v").map(String::as_str) != Some("RP1") {
        return None;
    }
    match tags.get("t").map(String::as_str) {
        Some("api") => tags.get("url")?.starts_with("https://").then_some(tags),
        Some("key") => {
            let public = standard_base64(tags.get("p")?)?;
            let public: [u8; 32] = public.try_into().ok()?;
            let kid = crate::cbor::hex(&crypto::kid(&public));
            (tags.get("kid") == Some(&kid)).then_some(tags)
        }
        _ => None,
    }
}

/// The 32 bytes of a key record's `p`.
#[must_use]
pub fn key_of(tags: &BTreeMap<String, String>) -> Option<[u8; 32]> {
    standard_base64(tags.get("p")?)?.try_into().ok()
}

fn standard_base64(text: &str) -> Option<Vec<u8>> {
    if text.len() % 4 != 0 {
        return None;
    }
    let url = text
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_");
    crate::base64url::decode(&url)
}

/// How a gateway learned of a post office (RP-1 §7.3): looked up in DNS when its user called, or
/// named in the list of a post office it learned of that way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// Looked up in DNS.
    Dns,
    /// Named in this post office's list.
    ListOf(String),
}

/// RP-1 §7.3: does a gateway trust `domain`, given how it learned of each post office, the
/// post offices its operator blocked, and the operator's limit (`only`), if any?
#[must_use]
pub fn trusts(
    domain: &str,
    found: &BTreeMap<String, Found>,
    blocked: &[String],
    only: Option<&[String]>,
) -> bool {
    if blocked.iter().any(|b| b == domain) || only.is_some_and(|o| !o.iter().any(|d| d == domain)) {
        return false;
    }
    match found.get(domain) {
        Some(Found::Dns) => true,
        // one hop of lists, from a post office looked up in DNS and not blocked
        Some(Found::ListOf(by)) => {
            found.get(by) == Some(&Found::Dns) && !blocked.iter().any(|b| b == by)
        }
        None => false,
    }
}
