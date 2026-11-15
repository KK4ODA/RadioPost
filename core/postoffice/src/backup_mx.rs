// SPDX-License-Identifier: MIT OR Apache-2.0
//! The backup mail server (wishlist W38): a second mail server, listed in DNS as a lower
//! priority MX for the post office's domains, that takes mail while the post office's own mail
//! server is down and hands it on when it is back.
//!
//! **It refuses as the door does** (RP-6 §2.1). A backup that took everything would take mail the
//! door refuses, and later bounce it to senders who may be forged (D15). So it runs a door of its
//! own, `radiopost-po backup-door`, called by its mail server's MTA hook like the post office's,
//! and judging with the same rules (`mail::rcpt_why`, `mail::data_why`) over a copy of what the
//! door judges with: the accounts and their accept lists, the post offices known,
//! `max_email_bytes`, and the `Message-ID`s of the mail sent from RadioPost (a [`Snapshot`]). The
//! copy is fetched from the post office every five minutes, with a token the post office's
//! operator made for this backup, and kept on disk for when the post office cannot be reached.
//! The backup checks the sender itself (SPF, DKIM, DMARC: `auth`).
//!
//! **Its verdicts travel with the message.** Mail the backup takes reaches the post office from
//! the backup's address, where the sender's SPF cannot pass. So the backup's door adds a header,
//! [`HEADER`], carrying its DMARC and DKIM verdicts, sealed with an HMAC under a key derived from
//! the token, over the envelope sender, the message's body and the time ([`seal`]). The post
//! office's door takes those verdicts when the seal checks with the key of one of its backups
//! ([`verified`]); a header anyone else wrote, or one moved onto another message, does not check,
//! and the door then makes its own checks as before.
//!
//! The post office's operator makes a backup's token with `radiopost-po backup-mx add NAME`
//! (shown once), lists them with `backup-mx`, and takes one away with `backup-mx remove NAME`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ring::{digest, hmac};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::auth::Checker;
use crate::email::{self, Hook};
use crate::mail::{self, Door};

/// The header the backup adds to mail it takes.
pub const HEADER: &str = "X-RadioPost-Door";

/// Where the post office serves the snapshot, under its API's base URL.
pub const SNAPSHOT_PATH: &str = "/door/snapshot";

/// How often the backup fetches the snapshot.
pub const FETCH_EVERY: Duration = Duration::from_secs(300);

/// How old a sealed verdict the post office still takes: mail may wait at the backup for days
/// (RFC 5321 §4.5.4.1 suggests trying for four or five).
pub const SEAL_KEPT_S: u64 = 7 * 86_400;

/// A backup mail server the post office knows: its name, the hash of its token (what a snapshot
/// request is checked against) and the key its verdicts are sealed with.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupMx {
    /// The operator's name for it.
    pub name: String,
    /// SHA-256 of its token, hex.
    pub token_hash: String,
    /// The key its verdicts are sealed with, hex ([`key_of`]).
    pub key: String,
    /// When it was made, Unix seconds.
    pub added: u64,
}

/// What the door judges with, as the backup keeps a copy of it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// When the post office made it.
    pub issued: u64,
    /// The post office's domains.
    pub domains: Vec<String>,
    /// Its `max_email_bytes` (RP-5 §4.1).
    pub max_email_bytes: u64,
    /// Its accounts' accept lists, by address in lowercase.
    pub accounts: BTreeMap<String, Vec<String>>,
    /// The RadioPost post offices it knows, by domain.
    pub known_post_offices: Vec<String>,
    /// The `Message-ID`s of the emails it sent from RadioPost, with their angle brackets.
    pub sent: BTreeSet<String>,
}

fn hex(bytes: &[u8]) -> String {
    radiopost::cbor::hex(bytes)
}

/// SHA-256 of a token, hex: what the post office keeps to check a snapshot request.
#[must_use]
pub fn token_hash(token: &str) -> String {
    hex(digest::digest(&digest::SHA256, token.as_bytes()).as_ref())
}

/// The key a backup's verdicts are sealed with, derived from its token.
#[must_use]
pub fn key_of(token: &str) -> [u8; 32] {
    let k = hmac::Key::new(hmac::HMAC_SHA256, b"RadioPost backup door v1");
    let tag = hmac::sign(&k, token.as_bytes());
    let mut out = [0; 32];
    out.copy_from_slice(tag.as_ref());
    out
}

/// A new backup: its record, and its token, shown to the operator once.
#[must_use]
pub fn make(name: &str, random: &[u8], now: u64) -> (BackupMx, String) {
    let token = radiopost::base64url::encode(random);
    let record = BackupMx {
        name: name.to_owned(),
        token_hash: token_hash(&token),
        key: hex(&key_of(&token)),
        added: now,
    };
    (record, token)
}

/// The message's body: what follows the first blank line, which relaying leaves as it was.
fn body_of(raw: &[u8]) -> &[u8] {
    for sep in [&b"\r\n\r\n"[..], b"\n\n"] {
        if let Some(at) = raw.windows(sep.len()).position(|w| w == sep) {
            return &raw[at + sep.len()..];
        }
    }
    &[]
}

fn mac(key: &[u8; 32], sender: &str, raw: &[u8], at: u64, dmarc: &str, dkim: &[String]) -> String {
    let body = digest::digest(&digest::SHA256, body_of(raw));
    let text = format!(
        "v=1\n{}\n{}\n{at}\n{dmarc}\n{}",
        sender.to_lowercase(),
        hex(body.as_ref()),
        dkim.join(",")
    );
    hex(hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), text.as_bytes()).as_ref())
}

/// [`HEADER`]'s value for a message the backup took at `at`: its verdicts, sealed.
#[must_use]
pub fn seal(
    key: &[u8; 32],
    sender: &str,
    raw: &[u8],
    at: u64,
    dmarc: &str,
    dkim: &[String],
) -> String {
    format!(
        "v=1; t={at}; dmarc={dmarc}; dkim={}; mac={}",
        dkim.join(","),
        mac(key, sender, raw, at, dmarc, dkim)
    )
}

/// The values of every [`HEADER`] in the message's header block, unfolded.
fn header_values(raw: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(raw);
    let head = text
        .split("\r\n\r\n")
        .next()
        .unwrap_or_default()
        .split("\n\n")
        .next()
        .unwrap_or_default()
        .to_owned();
    let mut lines: Vec<String> = Vec::new();
    for line in head.lines() {
        if (line.starts_with(' ') || line.starts_with('\t')) && !lines.is_empty() {
            if let Some(last) = lines.last_mut() {
                last.push(' ');
                last.push_str(line.trim());
            }
        } else {
            lines.push(line.to_owned());
        }
    }
    lines
        .into_iter()
        .filter_map(|l| {
            let (name, value) = l.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case(HEADER)
                .then(|| value.trim().to_owned())
        })
        .collect()
}

/// The verdicts a backup sealed in `raw` for the envelope sender `sender`, when a [`HEADER`] in
/// it checks with one of `keys` and is no older than [`SEAL_KEPT_S`] at `now`: DMARC, and the
/// domains a DKIM signature passed for.
#[must_use]
pub fn verified(
    raw: &[u8],
    sender: &str,
    keys: &[[u8; 32]],
    now: u64,
) -> Option<(String, Vec<String>)> {
    for value in header_values(raw) {
        let fields: BTreeMap<&str, &str> = value
            .split(';')
            .filter_map(|f| f.trim().split_once('='))
            .map(|(k, v)| (k.trim(), v.trim()))
            .collect();
        if fields.get("v") != Some(&"1") {
            continue;
        }
        let Some(at) = fields.get("t").and_then(|t| t.parse::<u64>().ok()) else {
            continue;
        };
        if now.saturating_sub(at) > SEAL_KEPT_S || at > now + 3600 {
            continue;
        }
        let dmarc = fields.get("dmarc").copied().unwrap_or_default();
        let dkim: Vec<String> = fields
            .get("dkim")
            .copied()
            .unwrap_or_default()
            .split(',')
            .filter(|d| !d.is_empty())
            .map(str::to_owned)
            .collect();
        let Some(given) = fields.get("mac") else {
            continue;
        };
        if keys
            .iter()
            .any(|k| mac(k, sender, raw, at, dmarc, &dkim) == *given)
        {
            return Some((dmarc.to_owned(), dkim));
        }
    }
    None
}

/// A key kept in hex, read back.
#[must_use]
pub fn key_from_hex(text: &str) -> Option<[u8; 32]> {
    let bytes: Vec<u8> = (0..text.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect();
    bytes.try_into().ok()
}

// ── the backup's own door ────────────────────────────────────────────────────────────────────

/// The hook's answer that takes the message and adds [`HEADER`], in the request's shape:
/// Stalwart's `modifications`, or the MTA Hooks draft's `add` on `/message/headers`.
#[must_use]
pub fn accept_with_header(draft: bool, value: &str) -> (u16, String) {
    let answer = if draft {
        json!({"add": [{"path": "/message/headers", "value": {"name": HEADER, "value": value}}]})
    } else {
        json!({"action": "accept",
               "modifications": [{"type": "addHeader", "name": HEADER, "value": value}]})
    };
    (200, answer.to_string())
}

/// What the backup's door says to one call of its mail server's hook, judging with `snapshot`,
/// checking senders with `checker`, and sealing the verdicts of what it takes with `key` at
/// `now`. Mail for addresses that are not the post office's accounts is its mail server's to
/// take or refuse; a signed-in submission is passed (a backup has no users of its own).
#[must_use]
pub fn decide(
    body: &[u8],
    snapshot: &Snapshot,
    checker: Result<&Checker, String>,
    key: &[u8; 32],
    now: u64,
) -> (u16, String) {
    let Ok(json) = serde_json::from_slice::<Json>(body) else {
        return (400, json!({"error": "not JSON"}).to_string());
    };
    let mut hook = email::read_hook(&json);
    let ok = if hook.stage == "data" {
        mail::OK_DATA
    } else {
        mail::OK_RCPT
    };
    if hook.login.is_some() {
        return email::hook_answer(hook.draft, ok, "");
    }
    let account = |r: &String| snapshot.accounts.contains_key(&r.to_lowercase());
    let ours: Vec<String> = hook
        .recipients
        .iter()
        .filter(|r| account(r))
        .cloned()
        .collect();
    let max = snapshot.max_email_bytes;
    if hook.stage == "data" && !ours.is_empty() && !hook.given {
        check(&mut hook, checker);
        if hook.dmarc == "temperror" {
            let why = "the sender could not be checked for now";
            log("held back", &hook, &ours, email::TRY_LATER, why);
            return email::hook_answer(
                hook.draft,
                email::TRY_LATER,
                &email::refusal_text(email::TRY_LATER, &ours[0], max),
            );
        }
    }
    let sent = |id: &str| snapshot.sent.contains(id);
    let door = Door {
        accounts: &snapshot.accounts,
        known_post_offices: &snapshot.known_post_offices,
        max_email_bytes: max,
        sent: &sent,
    };
    if let Some(((reply, why), recipient)) = email::first_refusal(&hook, &door, &account, &ours) {
        log(
            "refused",
            &hook,
            std::slice::from_ref(&recipient),
            reply,
            why,
        );
        return email::hook_answer(
            hook.draft,
            reply,
            &email::refusal_text(reply, &recipient, max),
        );
    }
    if hook.stage == "data"
        && !ours.is_empty()
        && let Some(raw) = &hook.raw
    {
        log("took, to hand on", &hook, &ours, mail::OK_DATA, "");
        let value = seal(key, &hook.sender, raw, now, &hook.dmarc, &hook.dkim_pass);
        return accept_with_header(hook.draft, &value);
    }
    email::hook_answer(hook.draft, ok, "")
}

fn check(hook: &mut Hook, checker: Result<&Checker, String>) {
    let (Some((ip, helo)), Some(raw)) = (&hook.client, &hook.raw) else {
        eprintln!(
            "radiopost-po: the mail server gave the backup's door neither verdicts nor client"
        );
        "temperror".clone_into(&mut hook.dmarc);
        return;
    };
    match checker {
        Ok(c) => {
            let found = c.check(*ip, helo, &hook.sender, raw);
            hook.dmarc = found.dmarc;
            hook.dkim_pass = found.dkim_pass;
        }
        Err(e) => {
            eprintln!("radiopost-po: {e}");
            "temperror".clone_into(&mut hook.dmarc);
        }
    }
}

fn log(what: &str, hook: &Hook, recipients: &[String], reply: &str, why: &str) {
    eprintln!(
        "radiopost-po: backup door: {}",
        email::door_log(what, &hook.sender, recipients, &hook.stage, reply, why)
    );
}

/// The backup's settings, in its directory's `backup-door.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    /// The post office's API base URL.
    pub primary: String,
    /// The token the post office's operator made for this backup.
    pub token: String,
    /// Where its mail server's hook reaches it.
    pub listen: String,
    /// The name its mail server receives as, for SPF's `%{r}`.
    pub host: String,
}

fn snapshot_file(dir: &Path) -> PathBuf {
    dir.join("snapshot.json")
}

/// The snapshot kept on disk, if any.
#[must_use]
pub fn kept(dir: &Path) -> Option<Snapshot> {
    serde_json::from_str(&std::fs::read_to_string(snapshot_file(dir)).ok()?).ok()
}

/// Fetches the snapshot from the post office.
///
/// # Errors
///
/// It cannot be reached, refuses the token, or answers something else.
pub fn fetch(primary: &str, token: &str) -> Result<Snapshot, String> {
    let url = format!("{}{SNAPSHOT_PATH}", primary.trim_end_matches('/'));
    let mut answer = ureq::get(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .config()
        .timeout_global(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|e| format!("{url}: {e}"))?;
    let status = answer.status().as_u16();
    let text = answer
        .body_mut()
        .with_config()
        .limit(256 << 20)
        .read_to_string()
        .map_err(|e| format!("{url}: {e}"))?;
    if status != 200 {
        return Err(format!(
            "{url}: {status}, the token refused or not served there"
        ));
    }
    serde_json::from_str(&text).map_err(|e| format!("{url}: not a snapshot: {e}"))
}

/// Runs the backup's door in `dir`: the snapshot fetched every [`FETCH_EVERY`] and kept, and the
/// hook answered at `settings.listen`, until the process ends.
///
/// # Errors
///
/// The listening address cannot be taken.
pub fn run(dir: &Path, settings: &Settings) -> Result<(), String> {
    let server = tiny_http::Server::http(&settings.listen)
        .map_err(|e| format!("cannot listen at {}: {e}", settings.listen))?;
    let snapshot = Arc::new(Mutex::new(kept(dir).unwrap_or_default()));
    let key = key_of(&settings.token);
    {
        let (snapshot, dir, settings) = (Arc::clone(&snapshot), dir.to_owned(), settings.clone());
        std::thread::spawn(move || {
            let mut said = String::new();
            loop {
                let line = match fetch(&settings.primary, &settings.token) {
                    Ok(s) => {
                        let text = serde_json::to_string_pretty(&s).unwrap_or_default();
                        let _ = std::fs::write(snapshot_file(&dir), text);
                        let line = format!(
                            "the post office's accounts fetched: {} accounts",
                            s.accounts.len()
                        );
                        *snapshot.lock().unwrap_or_else(PoisonError::into_inner) = s;
                        line
                    }
                    Err(e) => format!("the post office could not be reached, the copy kept: {e}"),
                };
                if line != said {
                    eprintln!("radiopost-po: backup door: {line}");
                    said = line;
                }
                std::thread::sleep(FETCH_EVERY);
            }
        });
    }
    let checker = Checker::new(&settings.host);
    let json_type = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
        .map_err(|()| "a Content-Type header".to_owned())?;
    eprintln!("radiopost-po: backup door at http://{}/", settings.listen);
    for mut request in server.incoming_requests() {
        let mut body = Vec::new();
        let _ = std::io::Read::read_to_end(
            &mut std::io::Read::take(request.as_reader(), 64 << 20),
            &mut body,
        );
        let snap = snapshot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let (status, text) = if snap.domains.is_empty() {
            // nothing fetched yet, nor kept: nothing is taken until the door knows the accounts
            email::hook_answer(
                false,
                email::TRY_LATER,
                "the backup has no copy of the accounts",
            )
        } else {
            decide(
                &body,
                &snap,
                checker.as_ref().map_err(Clone::clone),
                &key,
                crate::serve::now(),
            )
        };
        let response = tiny_http::Response::from_string(text)
            .with_status_code(status)
            .with_header(json_type.clone());
        let _ = request.respond(response);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{HEADER, Snapshot, key_of, make, seal, token_hash, verified};

    const RAW: &[u8] = b"From: ana@example.com\r\nTo: n1call@radiopost.org\r\nSubject: Hi\r\n\
        \r\nThe body.\r\n";

    /// A token's hash and key are stable and different; a made backup carries both.
    #[test]
    fn tokens_and_keys() {
        let (record, token) = make("mx2", &[7; 32], 100);
        assert_eq!(record.token_hash, token_hash(&token));
        assert_eq!(record.key.len(), 64);
        assert_ne!(key_of(&token), key_of("another"));
        assert_ne!(record.token_hash, record.key);
    }

    /// A seal checks with its key, for its sender and its body, whatever headers relaying adds
    /// above it; not with another key, another sender, another body, nor once too old.
    #[test]
    fn sealed_verdicts() {
        let key = key_of("token");
        let dkim = vec!["example.com".to_owned()];
        let value = seal(&key, "ana@example.com", RAW, 1000, "pass", &dkim);
        let relayed = [
            format!(
                "Received: from mx2 by po\r\n\tfor <n1call@radiopost.org>\r\n{HEADER}: {value}\r\n"
            )
            .as_bytes(),
            RAW,
        ]
        .concat();
        assert_eq!(
            verified(&relayed, "ANA@example.com", &[key_of("other"), key], 2000),
            Some(("pass".to_owned(), dkim.clone()))
        );
        assert_eq!(
            verified(&relayed, "ana@example.com", &[key_of("other")], 2000),
            None
        );
        assert_eq!(verified(&relayed, "eve@example.com", &[key], 2000), None);
        let changed = [
            format!("{HEADER}: {value}\r\n").as_bytes(),
            &RAW[..RAW.len() - 3],
            b"!\r\n",
        ]
        .concat();
        assert_eq!(verified(&changed, "ana@example.com", &[key], 2000), None);
        assert_eq!(
            verified(
                &relayed,
                "ana@example.com",
                &[key],
                1000 + super::SEAL_KEPT_S + 1
            ),
            None
        );
        let forged = format!("{HEADER}: v=1; t=1000; dmarc=pass; dkim=; mac=00\r\n");
        assert_eq!(
            verified(
                &[forged.as_bytes(), RAW].concat(),
                "ana@example.com",
                &[key],
                2000
            ),
            None
        );
        assert_eq!(Snapshot::default().accounts.len(), 0);
    }
}
