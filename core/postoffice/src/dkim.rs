// SPDX-License-Identifier: MIT OR Apache-2.0
//! The DKIM keys rotated by hand, guided (design §9 stage 1; the post office guide §5.5).
//!
//! Stalwart rotates DKIM keys by itself only when it also manages the domain's DNS, and the
//! pilot's DNS is edited by hand (the guide §5.2: an automatic one would keep a key on the server
//! that can change every domain of the DNS account). Stalwart's own rotation (its documentation,
//! *DKIM key rotation*) is followed step by step, with the operator editing DNS in between and the
//! post office checking each step in DNS before it takes the next:
//!
//! 1. **Start**: for each algorithm signing now, a new key is made here and given to Stalwart in
//!    the stage `pending`, which signs nothing; the desk shows the records to publish.
//! 2. **Switch**, once every new record is visible in DNS: the new keys `active`, the old ones
//!    `retiring`. Mail is never signed with a key whose record is not published.
//! 3. **Removed**, at least [`RETIRE_AFTER_S`] later, once the old records are gone from DNS: the
//!    old keys `retired`. Mail still on its way when the keys changed was verified meanwhile.
//!
//! The private key is made in memory and handed to Stalwart over its JMAP management API with the
//! mailboxes key (`x:DkimSignature/set`); the post office keeps none. A key Stalwart makes `active`
//! at once, whatever stage it was given, is destroyed again and the rotation refused, before any
//! mail is signed with an unpublished key. Health turns amber when the keys are older than
//! [`ROTATE_AFTER_DAYS`], and while a rotation waits on the operator.

use std::path::Path;

use rsa::pkcs8::{EncodePrivateKey as _, EncodePublicKey as _};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::mailboxes::{self, MailboxSettings};

/// Keys older than this are due to be rotated: Stalwart's own automatic rotation uses 90 days; by
/// hand, with two DNS edits each time, twice a year.
pub const ROTATE_AFTER_DAYS: u64 = 180;

/// How long the old records stay published after the switch: Stalwart's `retireAfter` default.
pub const RETIRE_AFTER_S: u64 = 7 * 86_400;

/// The file, in the post office's directory.
const FILE: &str = "dkim-rotation.json";

/// The algorithms rotated, as `(name in selectors, Stalwart's type)`.
const ALGORITHMS: [(&str, &str); 2] =
    [("rsa", "Dkim1RsaSha256"), ("ed25519", "Dkim1Ed25519Sha256")];

/// One DKIM key in Stalwart.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    /// Stalwart's ID of it.
    pub id: String,
    /// `rsa` or `ed25519`.
    pub algorithm: String,
    /// Its selector.
    pub selector: String,
    /// `pending`, `active`, `retiring` or `retired`.
    pub stage: String,
    /// When Stalwart made it, as Stalwart gives it.
    pub created: String,
    /// The public key, base64, the DNS record's `p=`.
    pub public: String,
}

impl Key {
    /// The TXT record that publishes it, as Stalwart's zone file writes one.
    #[must_use]
    pub fn record(&self) -> String {
        format!("v=DKIM1; k={}; h=sha256; p={}", self.algorithm, self.public)
    }
}

/// A rotation under way.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rotation {
    /// The domain whose keys rotate.
    pub domain: String,
    /// When it started, Unix seconds.
    pub started: u64,
    /// The new keys, and the keys each replaces.
    pub new: Vec<Key>,
    /// The keys they replace.
    pub old: Vec<Key>,
    /// When the new keys started signing.
    #[serde(default)]
    pub switched: Option<u64>,
}

/// The rotation under way, if any.
#[must_use]
pub fn rotation(dir: &Path) -> Option<Rotation> {
    std::fs::read_to_string(dir.join(FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

fn keep(dir: &Path, rotation: Option<&Rotation>) -> Result<(), String> {
    let path = dir.join(FILE);
    match rotation {
        // public keys and Stalwart's IDs only: nothing secret
        Some(r) => std::fs::write(&path, serde_json::to_string_pretty(r).unwrap_or_default())
            .map_err(|e| format!("{}: {e}", path.display())),
        None => match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        },
    }
}

/// A new private key, PEM (PKCS #8), and its public key as DKIM publishes it (RFC 6376 §3.6.1,
/// RFC 8463 §4.2): RSA 2048 bits, or Ed25519.
///
/// # Errors
///
/// No random numbers, or an algorithm not rotated here.
pub fn generate(algorithm: &str) -> Result<(String, String), String> {
    match algorithm {
        "rsa" => {
            let key = rsa::RsaPrivateKey::new(&mut rand_core::OsRng, 2048)
                .map_err(|e| format!("an RSA key: {e}"))?;
            let private = key.to_pkcs8_der().map_err(|e| format!("an RSA key: {e}"))?;
            let public = key
                .to_public_key()
                .to_public_key_der()
                .map_err(|e| format!("an RSA key: {e}"))?;
            Ok((pem(private.as_bytes()), standard_base64(public.as_bytes())))
        }
        "ed25519" => {
            let mut seed = [0u8; 32];
            getrandom::fill(&mut seed).map_err(|e| e.to_string())?;
            // RFC 8410 §7: a PKCS #8 OneAsymmetricKey for Ed25519 is this prefix and the seed
            let mut der = vec![
                0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22,
                0x04, 0x20,
            ];
            der.extend_from_slice(&seed);
            let public = ed25519_dalek::SigningKey::from_bytes(&seed)
                .verifying_key()
                .to_bytes();
            Ok((pem(&der), standard_base64(&public)))
        }
        other => Err(format!(
            "DKIM keys of the algorithm {other} are not rotated here"
        )),
    }
}

fn standard_base64(data: &[u8]) -> String {
    crate::mail::base64_lines(data).replace("\r\n", "")
}

fn pem(der: &[u8]) -> String {
    let body = standard_base64(der);
    let lines: Vec<&str> = body
        .as_bytes()
        .chunks(64)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect();
    format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
        lines.join("\n")
    )
}

/// The `p=` of a DKIM record (whitespace inside it removed), if it has a key.
#[must_use]
pub fn public_of(txt: &[String]) -> Option<String> {
    txt.iter().find_map(|record| {
        record
            .split(';')
            .filter_map(|tag| tag.split_once('='))
            .find(|(k, _)| k.trim().eq_ignore_ascii_case("p"))
            .map(|(_, v)| v.chars().filter(|c| !c.is_whitespace()).collect::<String>())
            .filter(|p| !p.is_empty())
    })
}

/// Whether `key`'s record is published for `domain`, by the resolver at `resolver`.
fn published(resolver: &str, domain: &str, key: &Key) -> Result<bool, String> {
    let name = format!("{}._domainkey.{domain}", key.selector);
    let txt = radiopost_api::query(resolver, &name, "TXT")?;
    Ok(public_of(&txt).as_deref() == Some(key.public.as_str()))
}

const ADMIN: &[&str] = &["urn:ietf:params:jmap:core", "urn:stalwart:jmap"];

/// The domain's DKIM keys in Stalwart.
///
/// # Errors
///
/// Stalwart could not be asked, or does not have the domain.
pub fn keys(settings: &MailboxSettings, domain: &str) -> Result<Vec<Key>, String> {
    let domain_id = mailboxes::stalwart_domain(settings, domain)?;
    let responses = mailboxes::jmap_using(
        settings,
        ADMIN,
        &json!([
            ["x:DkimSignature/query", {"filter": {"domainId": domain_id}}, "q"],
            ["x:DkimSignature/get", {"#ids": {"resultOf": "q", "name": "x:DkimSignature/query",
                                              "path": "/ids"}}, "g"]
        ]),
    )?;
    Ok(responses
        .iter()
        .filter(|r| r[0] == "x:DkimSignature/get")
        .flat_map(|r| r[1]["list"].as_array().cloned().unwrap_or_default())
        .filter_map(|k| {
            let algorithm = ALGORITHMS
                .iter()
                .find(|(_, t)| k["@type"] == *t)
                .map(|(a, _)| (*a).to_owned())?;
            Some(Key {
                id: k["id"].as_str()?.to_owned(),
                algorithm,
                selector: k["selector"].as_str()?.to_owned(),
                stage: k["stage"].as_str().unwrap_or("active").to_owned(),
                created: k["createdAt"].as_str().unwrap_or("").to_owned(),
                public: k["publicKey"].as_str().unwrap_or("").to_owned(),
            })
        })
        .collect())
}

/// Sets keys' stages, and reads them back: an error unless each took.
fn set_stages(
    settings: &MailboxSettings,
    domain: &str,
    stages: &[(&str, &str)],
) -> Result<(), String> {
    let update: serde_json::Map<String, Json> = stages
        .iter()
        .map(|(id, stage)| ((*id).to_owned(), json!({"stage": stage})))
        .collect();
    let responses = mailboxes::jmap_using(
        settings,
        ADMIN,
        &json!([["x:DkimSignature/set", {"update": update}, "u"]]),
    )?;
    refusal(&responses, "notUpdated")?;
    let now = keys(settings, domain)?;
    for (id, stage) in stages {
        let took = now.iter().any(|k| k.id == *id && k.stage == *stage);
        if !took {
            return Err(format!(
                "Stalwart did not put the DKIM key {id} in the stage {stage}"
            ));
        }
    }
    Ok(())
}

fn refusal(responses: &[Json], member: &str) -> Result<(), String> {
    let refused = responses
        .iter()
        .filter_map(|r| r[1][member].as_object())
        .flat_map(|m| m.values())
        .next();
    match refused {
        Some(e) => Err(format!(
            "Stalwart refused the DKIM keys: {} {}",
            e["type"].as_str().unwrap_or("error"),
            e["description"].as_str().unwrap_or("")
        )),
        None => Ok(()),
    }
}

/// A selector as Stalwart's own template makes one (`v1-rsa-20261115`), not one in use.
fn selector(algorithm: &str, day: &str, taken: &[Key]) -> String {
    let base = format!("v1-{algorithm}-{}", day.replace('-', ""));
    let mut name = base.clone();
    let mut n = 2;
    while taken.iter().any(|k| k.selector == name) {
        name = format!("{base}-{n}");
        n += 1;
    }
    name
}

/// Starts a rotation: a new key for each algorithm signing now (both, when none is), given to
/// Stalwart as `pending`. The rotation, with the records to publish.
///
/// # Errors
///
/// A rotation is under way, Stalwart could not be asked, or it made a new key `active` at once
/// (destroyed again).
pub fn start(
    dir: &Path,
    settings: &MailboxSettings,
    domain: &str,
    now: u64,
) -> Result<Rotation, String> {
    if let Some(r) = rotation(dir) {
        return Err(format!(
            "a rotation of {}'s keys is under way since {}",
            r.domain,
            crate::mail::iso(r.started)
        ));
    }
    let domain_id = mailboxes::stalwart_domain(settings, domain)?;
    let existing = keys(settings, domain)?;
    let active: Vec<Key> = existing
        .iter()
        .filter(|k| k.stage == "active")
        .cloned()
        .collect();
    let algorithms: Vec<&str> = ALGORITHMS
        .iter()
        .map(|(a, _)| *a)
        .filter(|a| active.is_empty() || active.iter().any(|k| k.algorithm == *a))
        .collect();
    let day = crate::lookups::day(now);
    let mut create = serde_json::Map::new();
    for algorithm in &algorithms {
        let (private, _) = generate(algorithm)?;
        let kind = ALGORITHMS
            .iter()
            .find(|(a, _)| a == algorithm)
            .map_or("", |(_, t)| *t);
        create.insert(
            (*algorithm).to_owned(),
            json!({"@type": kind, "domainId": domain_id, "selector": selector(algorithm, &day, &existing),
                   "privateKey": {"@type": "Text", "secret": private}, "stage": "pending"}),
        );
    }
    let responses = mailboxes::jmap_using(
        settings,
        ADMIN,
        &json!([["x:DkimSignature/set", {"create": create}, "c"]]),
    )?;
    refusal(&responses, "notCreated")?;
    let made: Vec<String> = responses
        .iter()
        .filter_map(|r| r[1]["created"].as_object())
        .flat_map(|m| m.values())
        .filter_map(|k| k["id"].as_str().map(str::to_owned))
        .collect();
    let now_keys = keys(settings, domain)?;
    let new: Vec<Key> = now_keys
        .iter()
        .filter(|k| made.contains(&k.id))
        .cloned()
        .collect();
    // never sign with a key whose record is not published: one made active at once goes again
    if new.len() != made.len() || new.iter().any(|k| k.stage != "pending") {
        let _ = mailboxes::jmap_using(
            settings,
            ADMIN,
            &json!([["x:DkimSignature/set", {"destroy": made}, "d"]]),
        );
        return Err(
            "Stalwart did not keep the new DKIM keys pending (signing nothing until their records \
             are published), so they were removed again and nothing changed: rotate them in \
             Stalwart's administration instead"
                .into(),
        );
    }
    let rotation = Rotation {
        domain: domain.to_owned(),
        started: now,
        old: active
            .into_iter()
            .filter(|k| algorithms.contains(&k.algorithm.as_str()))
            .collect(),
        new,
        switched: None,
    };
    keep(dir, Some(&rotation))?;
    Ok(rotation)
}

/// Switches to the new keys once every new record is visible in DNS: the new keys `active`, the
/// old ones `retiring`. The selectors the hourly check reads become the old and the new.
///
/// # Errors
///
/// No rotation waiting to switch, a record not visible yet (named), or Stalwart refused.
pub fn switch(
    dir: &Path,
    settings: &MailboxSettings,
    resolver: &str,
    now: u64,
) -> Result<Rotation, String> {
    let mut r = rotation(dir).ok_or("no rotation of the DKIM keys is under way")?;
    if r.switched.is_some() {
        return Err("the new DKIM keys sign already".into());
    }
    let mut missing = Vec::new();
    for k in &r.new {
        if !published(resolver, &r.domain, k)? {
            missing.push(format!("{}._domainkey.{}", k.selector, r.domain));
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "not visible in DNS yet: {}. A new record can take some minutes to show; try again",
            missing.join(", ")
        ));
    }
    let mut stages: Vec<(&str, &str)> = r.new.iter().map(|k| (k.id.as_str(), "active")).collect();
    stages.extend(r.old.iter().map(|k| (k.id.as_str(), "retiring")));
    set_stages(settings, &r.domain, &stages)?;
    r.switched = Some(now);
    keep(dir, Some(&r))?;
    Ok(r)
}

/// Ends a rotation once the old records are gone from DNS, at least [`RETIRE_AFTER_S`] after the
/// switch: the old keys `retired`.
///
/// # Errors
///
/// No rotation switched, too soon, an old record still published (named), or Stalwart refused.
pub fn removed(
    dir: &Path,
    settings: &MailboxSettings,
    resolver: &str,
    now: u64,
) -> Result<Rotation, String> {
    let r = rotation(dir).ok_or("no rotation of the DKIM keys is under way")?;
    let Some(switched) = r.switched else {
        return Err("switch to the new keys first".into());
    };
    if now < switched + RETIRE_AFTER_S {
        return Err(format!(
            "keep the old records published until {}: mail signed with the old keys may still be \
             on its way",
            crate::mail::iso(switched + RETIRE_AFTER_S)
        ));
    }
    let mut still = Vec::new();
    for k in &r.old {
        if published(resolver, &r.domain, k)? {
            still.push(format!("{}._domainkey.{}", k.selector, r.domain));
        }
    }
    if !still.is_empty() {
        return Err(format!(
            "still in DNS: {}. Remove them, then try again",
            still.join(", ")
        ));
    }
    let stages: Vec<(&str, &str)> = r.old.iter().map(|k| (k.id.as_str(), "retired")).collect();
    set_stages(settings, &r.domain, &stages)?;
    keep(dir, None)?;
    Ok(r)
}

/// Stops a rotation not switched yet: its new keys removed from Stalwart, nothing else changed.
///
/// # Errors
///
/// No rotation, one switched already, or Stalwart refused.
pub fn cancel(dir: &Path, settings: &MailboxSettings) -> Result<(), String> {
    let r = rotation(dir).ok_or("no rotation of the DKIM keys is under way")?;
    if r.switched.is_some() {
        return Err("the new keys sign already: remove the old records, then end it".into());
    }
    let ids: Vec<&str> = r.new.iter().map(|k| k.id.as_str()).collect();
    let responses = mailboxes::jmap_using(
        settings,
        ADMIN,
        &json!([["x:DkimSignature/set", {"destroy": ids}, "d"]]),
    )?;
    refusal(&responses, "notDestroyed")?;
    keep(dir, None)
}

/// The selectors the hourly check should read during and after a rotation: the old and new while
/// both are published, the new once it ends.
#[must_use]
pub fn selectors(r: &Rotation) -> Vec<String> {
    r.old
        .iter()
        .chain(&r.new)
        .map(|k| k.selector.clone())
        .collect()
}

/// The age in days of the newest key among `selectors`, by the date in its name (Stalwart's
/// `v1-rsa-20261110`), on `day` (`2026-11-15`).
#[must_use]
pub fn age_days(selectors: &[String], day: &str) -> Option<u64> {
    let today = days(&day.replace('-', ""))?;
    selectors
        .iter()
        .filter_map(|s| {
            s.split('-')
                .find(|p| p.len() == 8 && p.chars().all(|c| c.is_ascii_digit()))
        })
        .filter_map(days)
        .max()
        .map(|newest| today.saturating_sub(newest))
}

/// Days since 1970 of `YYYYMMDD`.
fn days(date: &str) -> Option<u64> {
    let y: i64 = date.get(0..4)?.parse().ok()?;
    let m: i64 = date.get(4..6)?.parse().ok()?;
    let d: i64 = date.get(6..8)?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Howard Hinnant's days_from_civil
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    u64::try_from(era * 146_097 + doe - 719_468).ok()
}

/// What the desk shows: the domain's keys, the rotation under way, and what to do next.
#[must_use]
pub fn view(dir: &Path, keys: &[Key], now: u64) -> Json {
    let r = rotation(dir);
    let next = match &r {
        None => "start".to_owned(),
        Some(r) if r.switched.is_none() => "publish".to_owned(),
        Some(r) if now < r.switched.unwrap_or(0) + RETIRE_AFTER_S => "wait".to_owned(),
        Some(_) => "remove".to_owned(),
    };
    let records = |ks: &[Key], domain: &str| -> Vec<Json> {
        ks.iter()
            .map(|k| {
                json!({"name": format!("{}._domainkey.{domain}", k.selector),
                            "value": k.record(), "algorithm": k.algorithm})
            })
            .collect()
    };
    json!({
        "keys": keys,
        "rotation": r.as_ref().map(|r| json!({
            "domain": r.domain, "started": r.started, "switched": r.switched,
            "remove_after": r.switched.map(|s| s + RETIRE_AFTER_S),
            "publish": records(&r.new, &r.domain), "unpublish": records(&r.old, &r.domain)})),
        "next": next,
        "rotate_after_days": ROTATE_AFTER_DAYS,
    })
}

impl crate::serve::Running {
    fn dkim_context(&self) -> Result<(MailboxSettings, String, String), String> {
        let settings = self.settings();
        if settings.mailboxes.kind != "stalwart" || settings.mailboxes.url.is_empty() {
            return Err(
                "the DKIM keys are rotated through Stalwart: set mailboxes to Stalwart with its key \
                 (post office guide §8)"
                    .into(),
            );
        }
        let domain = settings
            .domains
            .first()
            .cloned()
            .ok_or("the post office has no domain")?;
        let resolver = settings
            .resolver
            .clone()
            .unwrap_or_else(|| radiopost_api::RESOLVER.to_owned());
        Ok((settings.mailboxes, domain, resolver))
    }

    /// What the desk shows of the DKIM keys (`GET /office/dkim`).
    ///
    /// # Errors
    ///
    /// No way to reach Stalwart, or it refused.
    pub fn dkim_view(&self) -> Result<Json, String> {
        let (mailboxes, domain, _) = self.dkim_context()?;
        let keys = keys(&mailboxes, &domain)?;
        Ok(view(self.files().dir(), &keys, crate::serve::now()))
    }

    /// One step of a rotation, asked by the operator `who` (`POST /office/dkim`): `start`,
    /// `switch`, `removed` or `cancel`. The selectors the hourly check reads follow it. What the
    /// desk shows after it.
    ///
    /// # Errors
    ///
    /// The step cannot be taken now (why), or Stalwart refused.
    pub fn dkim_step(&self, step: &str, who: &str) -> Result<Json, String> {
        let (mailboxes, domain, resolver) = self.dkim_context()?;
        let dir = self.files().dir().to_path_buf();
        let now = crate::serve::now();
        match step {
            "start" => {
                let r = start(&dir, &mailboxes, &domain, now)?;
                eprintln!(
                    "radiopost-po: {who} started a rotation of {domain}'s DKIM keys: new selectors {}",
                    r.new
                        .iter()
                        .map(|k| k.selector.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            "switch" => {
                let r = switch(&dir, &mailboxes, &resolver, now)?;
                self.change_settings(|s| s.health.dkim = selectors(&r))?;
                eprintln!("radiopost-po: {who} switched {domain}'s DKIM keys to the new ones");
            }
            "removed" => {
                let r = removed(&dir, &mailboxes, &resolver, now)?;
                self.change_settings(|s| {
                    s.health.dkim = r.new.iter().map(|k| k.selector.clone()).collect();
                })?;
                eprintln!("radiopost-po: {who} ended the rotation of {domain}'s DKIM keys");
            }
            "cancel" => {
                cancel(&dir, &mailboxes)?;
                eprintln!("radiopost-po: {who} stopped the rotation of {domain}'s DKIM keys");
            }
            other => {
                return Err(format!(
                    "no step {other:?}: start, switch, removed or cancel"
                ));
            }
        }
        self.dkim_view()
    }
}

#[cfg(test)]
mod tests {
    use super::{Key, age_days, generate, public_of, selector};

    /// Keys made here are what DKIM publishes: an Ed25519 key's `p=` is its 32 bytes, an RSA
    /// key's a `SubjectPublicKeyInfo`; the private keys are PKCS #8 PEM.
    #[test]
    fn keys_are_made_as_dkim_publishes_them() {
        let (private, public) = generate("ed25519").expect("a key");
        assert!(private.starts_with("-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEI"));
        assert_eq!(public.len(), 44, "{public}");
        let (private, public) = generate("rsa").expect("a key");
        assert!(private.starts_with("-----BEGIN PRIVATE KEY-----\nMIIE"));
        assert!(
            public.starts_with("MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA"),
            "{public}"
        );
        assert!(generate("dsa").is_err());
    }

    #[test]
    fn records_selectors_and_ages() {
        let txt = vec!["v=DKIM1; k=rsa; h=sha256; p=MIIB IjAN".to_owned()];
        assert_eq!(public_of(&txt).as_deref(), Some("MIIBIjAN"));
        assert_eq!(public_of(&["v=DKIM1; p=".to_owned()]), None);
        let taken = vec![Key {
            id: "k1".into(),
            algorithm: "rsa".into(),
            selector: "v1-rsa-20261115".into(),
            stage: "active".into(),
            created: String::new(),
            public: String::new(),
        }];
        assert_eq!(selector("rsa", "2026-11-15", &taken), "v1-rsa-20261115-2");
        assert_eq!(
            selector("ed25519", "2026-11-15", &taken),
            "v1-ed25519-20261115"
        );
        let s = [
            "v1-rsa-20261110".to_owned(),
            "v1-ed25519-20261110".to_owned(),
        ];
        assert_eq!(age_days(&s, "2026-11-15"), Some(5));
        assert_eq!(age_days(&s, "2027-05-09"), Some(180));
        assert_eq!(age_days(&["rsa_default".to_owned()], "2026-11-15"), None);
    }
}
