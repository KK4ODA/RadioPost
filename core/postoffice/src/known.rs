// SPDX-License-Identifier: MIT OR Apache-2.0
//! The other post offices (RP-1 §7.5, D38). Those its operators accepted are **known**: their DNS
//! records, info documents and revocation lists are fetched and checked here, refreshed every six
//! hours, and what was last fetched whole is kept when a refresh fails. The domains it meets (at
//! the door, in mail its users send, on a credential it is shown, in a known post office's list)
//! are looked up on a thread of their own, and those with a RadioPost API record are kept as
//! **seen**, for the operators to accept or block from the desk.
//!
//! [`assemble`] is the check, with no input or output; [`fetch`] asks DNS and the post office.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use radiopost::credential::{PostOfficeKey, Revocations};
use radiopost::dns::{self, Meets};
use radiopost::{base64url, crypto, jws};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::serve::{Running, Settings, now};

/// How often a known post office is fetched again (RP-1 §7.5).
pub const REFRESH_EVERY_S: u64 = 6 * 3600;
/// How long a seen post office's records stand before meeting it again looks them up again.
const SEEN_FOR_S: u64 = 86_400;
/// How long a domain met with no RadioPost record is not asked about again.
const NONE_FOR_S: u64 = 86_400;
/// How long a domain whose lookup failed is not asked about again.
const FAILED_FOR_S: u64 = 3600;

/// A post office key as it was found: its ID and public key, and its validity (RP-1 §7.2).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Key {
    /// The key's ID, hex.
    pub kid: String,
    /// The public key, base64url.
    pub public: String,
    /// Valid from.
    pub not_before: u64,
    /// Valid until (not at).
    pub not_after: u64,
}

/// What was found of a post office, checked: its API and keys in DNS, the keys its info
/// document adds, and its revocation list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Found {
    /// Its API's base URL, from its `t=api` record (RP-1 §7.1).
    pub api: String,
    /// The keys RP-1 §7.2 accepts: those in DNS, and those its info document adds.
    pub keys: Vec<Key>,
    /// Whether the resolver validated the records with DNSSEC.
    pub dnssec: bool,
    /// The info document's payload (RP-5 §4.1), when it verified; null otherwise.
    pub info: Json,
    /// Its revoked credentials' serials (RP-1 §8).
    pub serials: Vec<u64>,
    /// Its revoked keys' IDs, hex.
    pub kids: Vec<String>,
    /// When it was found.
    pub at: u64,
}

impl Found {
    /// The keys, for RP-1 §5.
    #[must_use]
    pub fn trust_keys(&self) -> Vec<PostOfficeKey> {
        self.keys
            .iter()
            .filter_map(|k| {
                let public = base64url::decode(&k.public)?.try_into().ok()?;
                Some(PostOfficeKey {
                    public,
                    not_before: k.not_before,
                    not_after: k.not_after,
                })
            })
            .collect()
    }

    /// The revocation list, for RP-1 §5.
    #[must_use]
    pub fn revocations(&self) -> Revocations {
        Revocations {
            serials: self.serials.clone(),
            kids: self
                .kids
                .iter()
                .filter_map(|k| unhex(k)?.try_into().ok())
                .collect(),
        }
    }

    /// The entry for `/post-offices` (RP-5 §4.3).
    #[must_use]
    pub fn entry(&self, domain: &str) -> Json {
        json!({"domain": domain, "api": self.api, "keys": self.keys})
    }
}

/// A known post office: what was last fetched whole, and how the last try went.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Fetched {
    /// The last fetch that was whole and checked.
    pub found: Option<Found>,
    /// When it was last tried.
    pub tried: u64,
    /// Why the last try failed, when it did.
    pub error: Option<String>,
}

/// A post office met and not known (RP-1 §7.5).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Seen {
    /// Where it was met: `door`, `outgoing`, `credential`, `list:<domain>`.
    pub how: BTreeSet<String>,
    /// When it was first met.
    pub first: u64,
    /// When it was last met.
    pub last: u64,
    /// What was found of it, perhaps not all.
    pub found: Found,
    /// What could not be found or checked, when something could not.
    pub error: Option<String>,
}

/// What a lookup of a domain gave.
#[derive(Clone, Debug)]
pub enum Lookup {
    /// DNS could not be asked.
    Failed(String),
    /// The domain has no RadioPost API record.
    NoRecord,
    /// Its records, what was found, what could not be found or checked, and the domains its
    /// `/post-offices` names when that verified.
    Found {
        /// Its `_radiopost` records, as RP-1 §7.1 reads them.
        records: Vec<BTreeMap<String, String>>,
        /// What was found.
        found: Box<Found>,
        /// What failed, when the fetch was not whole.
        error: Option<String>,
        /// The domains its list of post offices names.
        listed: Vec<String>,
    },
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

/// RP-1 §7.1, §7.2 and §8: what `domain`'s records and documents say, checked. `None` when its
/// records hold no API record; otherwise what was found, and why it is not whole when it is not:
/// no key in DNS, an info document missing, not signed by a DNS key or not naming the domain, a
/// revocation list missing or not signed by an accepted key.
#[must_use]
pub fn assemble(
    domain: &str,
    records: &[BTreeMap<String, String>],
    dnssec: bool,
    info: Option<&str>,
    revocations: Option<&str>,
    at: u64,
) -> Option<(Found, Option<String>)> {
    let kind = |t: &BTreeMap<String, String>, k: &str| t.get("t").map(String::as_str) == Some(k);
    let api = records.iter().find(|t| kind(t, "api"))?.get("url")?.clone();
    let mut found = Found {
        api,
        dnssec,
        info: Json::Null,
        at,
        ..Found::default()
    };
    let in_dns: HashMap<String, [u8; 32]> = records
        .iter()
        .filter(|t| kind(t, "key"))
        .filter_map(dns::key_of)
        .map(|k| (radiopost::cbor::hex(&crypto::kid(&k)), k))
        .collect();
    let keys_of = |m: &HashMap<String, [u8; 32]>| -> Vec<Key> {
        let mut keys: Vec<Key> = m
            .iter()
            .map(|(kid, public)| Key {
                kid: kid.clone(),
                public: base64url::encode(public),
                not_before: 0,
                not_after: u64::MAX,
            })
            .collect();
        keys.sort_by(|a, b| a.kid.cmp(&b.kid));
        keys
    };
    found.keys = keys_of(&in_dns);
    if in_dns.is_empty() {
        return Some((found, Some("no key record in DNS (RP-1 §7.1)".into())));
    }
    let Some(info) = info else {
        return Some((found, Some("its info document could not be fetched".into())));
    };
    let Ok(payload) = jws::verify(info, &in_dns) else {
        return Some((
            found,
            Some("its info document is not signed by a key in DNS (RP-1 §7.2)".into()),
        ));
    };
    let names = payload["domains"]
        .as_array()
        .is_some_and(|d| d.iter().any(|d| d.as_str() == Some(domain)));
    // RP-1 §7.2: keys in DNS, and those the info document adds, with its validity periods
    let accepted = jws::accepted_keys(&in_dns, Some(info));
    found.keys = keys_of(&accepted);
    for k in &mut found.keys {
        if let Some(listed) = payload["keys"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|l| l["kid"].as_str() == Some(&k.kid))
        {
            k.not_before = listed["not_before"].as_u64().unwrap_or(0);
            k.not_after = listed["not_after"].as_u64().unwrap_or(u64::MAX);
        }
    }
    found.info = payload;
    if !names {
        return Some((
            found,
            Some(format!("its info document does not name {domain}")),
        ));
    }
    let Some(revocations) = revocations else {
        return Some((
            found,
            Some("its revocation list could not be fetched".into()),
        ));
    };
    let Ok(list) = jws::verify(revocations, &accepted) else {
        return Some((
            found,
            Some("its revocation list is not signed by its keys (RP-1 §8)".into()),
        ));
    };
    found.serials = list["serials"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_u64)
        .collect();
    found.kids = list["kids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .map(str::to_owned)
        .collect();
    Some((found, None))
}

/// The domains a post office's `/post-offices` (RP-5 §4.3) names, when its keys signed it.
#[must_use]
pub fn listed(found: &Found, list: &str) -> Vec<String> {
    let keys: HashMap<String, [u8; 32]> = found
        .trust_keys()
        .iter()
        .map(|k| (radiopost::cbor::hex(&crypto::kid(&k.public)), k.public))
        .collect();
    jws::verify(list, &keys)
        .map(|p| {
            p["post_offices"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| e["domain"].as_str())
                .map(str::to_lowercase)
                .collect()
        })
        .unwrap_or_default()
}

/// Looks `domain` up: its records by DNS over HTTPS at `resolver`, then its documents.
#[must_use]
pub fn fetch(resolver: &str, domain: &str, at: u64) -> Lookup {
    let (records, dnssec) = match radiopost_api::records_signed(resolver, domain) {
        Ok(r) => r,
        Err(e) => return Lookup::Failed(e),
    };
    let Some(api) = records
        .iter()
        .find(|t| t.get("t").map(String::as_str) == Some("api"))
        .and_then(|t| t.get("url"))
    else {
        return Lookup::NoRecord;
    };
    let http = radiopost_api::Http::anonymous(api);
    let get = |path: &str| match http.send_text("GET", path, None, None) {
        Ok((200, text)) => Some(text.trim().to_owned()),
        _ => None,
    };
    let info = get("/info");
    let revocations = get("/revocations");
    let Some((found, error)) = assemble(
        domain,
        &records,
        dnssec,
        info.as_deref(),
        revocations.as_deref(),
        at,
    ) else {
        return Lookup::NoRecord;
    };
    let listed = if error.is_none() {
        get("/post-offices")
            .map(|l| listed(&found, &l))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    Lookup::Found {
        records,
        found: Box::new(found),
        error,
        listed,
    }
}

/// Whether `text` reads as a domain name: lowercase labels of letters, digits and hyphens.
#[must_use]
pub fn is_domain(text: &str) -> bool {
    text.contains('.')
        && text.len() <= 253
        && text.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

/// The domains its operators accepted (RP-1 §7.5), as `config.json`'s `known` holds them.
#[must_use]
pub fn accepted(settings: &Settings) -> Vec<String> {
    settings
        .known
        .iter()
        .filter_map(|k| k.get("domain").and_then(Json::as_str))
        .map(str::to_lowercase)
        .collect()
}

/// A job for the worker.
enum Job {
    /// Look up what has been met.
    Met,
    /// Fetch one known post office now.
    Refresh(String),
    /// Fetch every known post office now.
    RefreshAll,
}

/// The service's side: the worker's queue, the domains met and not yet looked up, and those
/// recently found to have no record.
#[derive(Default)]
pub struct Known {
    queue: Mutex<Option<Sender<Job>>>,
    waiting: Mutex<BTreeMap<String, BTreeSet<String>>>,
    none: Mutex<HashMap<String, u64>>,
}

impl Known {
    fn send(&self, job: Job) -> bool {
        self.queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|q| q.send(job).is_ok())
    }
}

impl Running {
    fn resolver(&self) -> String {
        self.settings()
            .resolver
            .unwrap_or_else(|| radiopost_api::RESOLVER.into())
    }

    /// Notes that `domain` was met, `how` (RP-1 §7.5), for the worker to look up. Its own
    /// domains, and domains it knows or blocked, are left alone.
    pub fn met(&self, domain: &str, how: &str) {
        let domain = domain.trim().trim_end_matches('.').to_lowercase();
        if !is_domain(&domain) {
            return;
        }
        let settings = self.settings();
        if settings
            .domains
            .iter()
            .any(|d| d.eq_ignore_ascii_case(&domain))
            || settings.blocked.contains(&domain)
            || accepted(&settings).contains(&domain)
        {
            return;
        }
        self.known
            .waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(domain)
            .or_default()
            .insert(how.to_owned());
        self.known.send(Job::Met);
    }

    /// Looks up what has been met: each domain kept as seen when RP-1 §7.5 says so.
    fn look_up_met(&self) {
        let waiting = std::mem::take(
            &mut *self
                .known
                .waiting
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        let resolver = self.resolver();
        for (domain, hows) in waiting {
            let at = now();
            if self
                .known
                .none
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&domain)
                .is_some_and(|until| *until > at)
            {
                continue;
            }
            {
                let mut office = self.office();
                if let Some(s) = office.state.seen.get_mut(&domain)
                    && at.saturating_sub(s.found.at) < SEEN_FOR_S
                {
                    s.last = at;
                    s.how.extend(hows);
                    self.save(&office);
                    continue;
                }
            }
            match fetch(&resolver, &domain, at) {
                Lookup::Failed(e) => {
                    eprintln!("radiopost-po: {domain}, met: DNS did not answer: {e}");
                    self.none_for(&domain, at + FAILED_FOR_S);
                }
                Lookup::NoRecord => self.none_for(&domain, at + NONE_FOR_S),
                Lookup::Found {
                    records,
                    found,
                    error,
                    ..
                } => self.saw(&domain, &records, &hows, *found, error),
            }
        }
    }

    fn none_for(&self, domain: &str, until: u64) {
        self.known
            .none
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(domain.to_owned(), until);
    }

    /// Keeps `domain` as seen, met `hows`, with what was found, when RP-1 §7.5 says so.
    pub fn saw(
        &self,
        domain: &str,
        records: &[BTreeMap<String, String>],
        hows: &BTreeSet<String>,
        found: Found,
        error: Option<String>,
    ) {
        let settings = self.settings();
        let meets = dns::meets(
            domain,
            records,
            &settings.domains,
            &accepted(&settings),
            &settings.blocked,
        );
        if meets != Meets::Seen {
            return;
        }
        let at = now();
        let mut office = self.office();
        let seen = office.state.seen.entry(domain.to_owned()).or_default();
        if seen.first == 0 {
            seen.first = at;
            eprintln!(
                "radiopost-po: {domain} seen ({}): a RadioPost post office its operators may \
                 accept",
                hows.iter().cloned().collect::<Vec<_>>().join(", ")
            );
        }
        seen.last = at;
        seen.how.extend(hows.iter().cloned());
        seen.found = found;
        seen.error = error;
        self.save(&office);
    }

    /// Fetches a known post office now, and keeps what was found when it is whole; otherwise
    /// keeps what it held and says why (RP-1 §7.5). The post offices its list names are met.
    pub fn refresh(&self, domain: &str) {
        let at = now();
        match fetch(&self.resolver(), domain, at) {
            Lookup::Found {
                found,
                error: None,
                listed,
                ..
            } => {
                self.learned(domain, *found);
                for d in listed {
                    self.met(&d, &format!("list:{domain}"));
                }
            }
            Lookup::Found { error: Some(e), .. } => self.not_learned(domain, &e),
            Lookup::NoRecord => self.not_learned(domain, "no API record in DNS (RP-1 §7.1)"),
            Lookup::Failed(e) => self.not_learned(domain, &format!("DNS did not answer: {e}")),
        }
    }

    /// Keeps a known post office's whole fetch: its keys and revocations are checked against from
    /// now on.
    pub fn learned(&self, domain: &str, found: Found) {
        let mut office = self.office();
        let entry = office
            .state
            .post_offices
            .entry(domain.to_owned())
            .or_default();
        entry.tried = found.at;
        entry.error = None;
        entry.found = Some(found);
        self.save(&office);
    }

    fn not_learned(&self, domain: &str, why: &str) {
        eprintln!("radiopost-po: {domain} could not be refreshed, and what it held is kept: {why}");
        let mut office = self.office();
        let entry = office
            .state
            .post_offices
            .entry(domain.to_owned())
            .or_default();
        entry.tried = now();
        entry.error = Some(why.to_owned());
        self.save(&office);
    }

    fn refresh_all(&self) {
        for domain in accepted(&self.settings()) {
            self.refresh(&domain);
        }
    }

    /// The entries `/post-offices` publishes (RP-5 §4.3): each known post office with its API and
    /// keys, as last fetched, or as its operator typed them; one with no API is left out.
    #[must_use]
    pub fn published_post_offices(&self, settings: &Settings) -> Vec<Json> {
        let office = self.office();
        settings
            .known
            .iter()
            .filter_map(|k| {
                let domain = k.get("domain")?.as_str()?.to_lowercase();
                if settings.blocked.contains(&domain) {
                    return None;
                }
                if let Some(found) = office
                    .state
                    .post_offices
                    .get(&domain)
                    .and_then(|f| f.found.as_ref())
                {
                    return Some(found.entry(&domain));
                }
                let api = k.get("api").and_then(Json::as_str).unwrap_or("");
                (!api.is_empty()).then(|| json!({"domain": domain, "api": api, "keys": k["keys"]}))
            })
            .collect()
    }

    /// What the desk shows (`/office/post-offices`): the post offices known, seen and blocked, and
    /// those met and not yet looked up.
    #[must_use]
    pub fn post_offices_view(&self) -> Json {
        let settings = self.settings();
        let office = self.office();
        let about = |f: &Found| {
            json!({"api": f.api, "kids": f.keys.iter().map(|k| k.kid.clone()).collect::<Vec<_>>(),
                   "dnssec": f.dnssec, "operator": f.info["operator"],
                   "contact": f.info["contact"], "country": f.info["country"], "found": f.at})
        };
        let known: Vec<Json> = accepted(&settings)
            .iter()
            .filter(|d| !settings.blocked.contains(d))
            .map(|d| {
                let fetched = office
                    .state
                    .post_offices
                    .get(d)
                    .cloned()
                    .unwrap_or_default();
                let mut entry = fetched.found.as_ref().map_or_else(|| json!({}), about);
                entry["domain"] = json!(d);
                entry["tried"] = json!(fetched.tried);
                entry["error"] = json!(fetched.error);
                entry["typed"] = json!(settings.known.iter().any(|k| {
                    k["domain"].as_str() == Some(d)
                        && k["keys"].as_array().is_some_and(|a| !a.is_empty())
                }));
                entry
            })
            .collect();
        let seen: Vec<Json> = office
            .state
            .seen
            .iter()
            .map(|(d, s)| {
                let mut entry = about(&s.found);
                entry["domain"] = json!(d);
                entry["how"] = json!(s.how);
                entry["first"] = json!(s.first);
                entry["last"] = json!(s.last);
                entry["error"] = json!(s.error);
                entry
            })
            .collect();
        let waiting: Vec<String> = self
            .known
            .waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect();
        json!({"known": known, "seen": seen, "blocked": settings.blocked,
               "looking_up": waiting})
    }

    /// The operators' decision on a post office (RP-1 §7.5): `accept`, `block`, `unblock`,
    /// `forget` (known no more, not blocked), or `refresh` (fetch the known ones now). Returns
    /// what was done, for the log, or 400 for a domain that is not one, or one of its own.
    ///
    /// # Errors
    ///
    /// 400 for a bad request, 500 when `config.json` could not be written.
    pub fn decide_post_office(&self, action: &str, domain: &str) -> Result<String, u16> {
        let domain = domain.trim().to_lowercase();
        if action == "refresh" {
            if !self.known.send(Job::RefreshAll) {
                self.refresh_all();
            }
            return Ok("the known post offices are fetched again".into());
        }
        if !is_domain(&domain)
            || self
                .settings()
                .domains
                .iter()
                .any(|d| d.eq_ignore_ascii_case(&domain))
        {
            return Err(400);
        }
        let entry = json!({"domain": domain});
        let changed = match action {
            "accept" => self.change_settings(|s| {
                s.blocked.retain(|b| b != &domain);
                if !accepted(s).contains(&domain) {
                    s.known.push(entry);
                }
            }),
            "block" => self.change_settings(|s| {
                s.known.retain(|k| {
                    k["domain"].as_str().map(str::to_lowercase) != Some(domain.clone())
                });
                if !s.blocked.contains(&domain) {
                    s.blocked.push(domain.clone());
                }
            }),
            "unblock" => self.change_settings(|s| s.blocked.retain(|b| b != &domain)),
            "forget" => self.change_settings(|s| {
                s.known.retain(|k| {
                    k["domain"].as_str().map(str::to_lowercase) != Some(domain.clone())
                });
            }),
            _ => return Err(400),
        };
        changed.map_err(|e| {
            eprintln!("radiopost-po: cannot save config.json: {e}");
            500u16
        })?;
        let mut office = self.office();
        match action {
            "accept" => {
                // what was seen whole serves at once; the worker fetches it again
                if let Some(seen) = office.state.seen.remove(&domain)
                    && seen.error.is_none()
                    && !seen.found.keys.is_empty()
                {
                    let entry = office.state.post_offices.entry(domain.clone()).or_default();
                    entry.tried = seen.found.at;
                    entry.found = Some(seen.found);
                }
            }
            "block" => {
                office.state.seen.remove(&domain);
                office.state.post_offices.remove(&domain);
            }
            "forget" => {
                office.state.post_offices.remove(&domain);
            }
            _ => {}
        }
        self.save(&office);
        drop(office);
        if action == "accept" {
            self.known.send(Job::Refresh(domain.clone()));
        }
        Ok(format!("{domain}: {action}"))
    }
}

fn work(running: &Arc<Running>, rx: &Receiver<Job>) {
    let mut next = now();
    loop {
        let wait = next.saturating_sub(now()).max(1);
        match rx.recv_timeout(Duration::from_secs(wait)) {
            Ok(Job::Met) => running.look_up_met(),
            Ok(Job::Refresh(d)) => running.refresh(&d),
            Ok(Job::RefreshAll) | Err(RecvTimeoutError::Timeout) => {
                running.refresh_all();
                next = now() + REFRESH_EVERY_S;
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Starts the worker: the known post offices fetched now and every six hours, and what is met
/// looked up as it comes.
pub fn start(running: &Arc<Running>) {
    let (tx, rx) = channel::<Job>();
    let _ = tx.send(Job::Met);
    *running
        .known
        .queue
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(tx);
    let worker = Arc::clone(running);
    std::thread::spawn(move || work(&worker, &rx));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_are_told_from_other_text() {
        assert!(is_domain("hampost.org"));
        assert!(is_domain("po-1.qrp-mail.example"));
        assert!(!is_domain("hampost"));
        assert!(!is_domain("HamPost.org"));
        assert!(!is_domain("-x.org"));
        assert!(!is_domain("a..org"));
        assert!(!is_domain("mail@hampost.org"));
    }

    #[test]
    fn no_api_record_is_nothing_found() {
        let key: BTreeMap<String, String> = [("v", "RP1"), ("t", "key")]
            .into_iter()
            .map(|(a, b)| (a.to_owned(), b.to_owned()))
            .collect();
        assert!(assemble("hampost.org", &[key], false, None, None, 1).is_none());
    }
}
