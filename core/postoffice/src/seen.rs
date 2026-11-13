// SPDX-License-Identifier: MIT OR Apache-2.0
//! `radio_skips_read` (RP-5 §6.3): mail a user has already read in a mail program is not offered
//! by radio. The post office takes it as picked up, so it leaves the waiting list and its copies
//! placed ahead are dropped (RP-5 §5.5, §5.8): a gateway holding a copy of mail its user has read
//! would offer it again while offline.
//!
//! What was read is learned in the background, never while a gateway waits on a session (RP-3 §9
//! gives it about a second). Every minute, for each account with the setting on and mail not yet
//! picked up, the service asks the mail server which of the account's messages carry `$seen`,
//! over JMAP (RFC 8621), and matches them to the radio's messages by `Message-ID`: the one the
//! email came with (RP-6 §2.4 step 8), or the one the post office gave a RadioPost message it
//! handed to the mail server (RP-6 §3.1). It reads Stalwart with the `mailboxes` key: an account
//! with `impersonate` names another account's mail by its JMAP `accountId`. No user's password is
//! ever kept.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use radiopost::crypto::ContentId;
use serde_json::{Value as Json, json};

use crate::mailboxes::{self, MailboxSettings};
use crate::office::{PostOffice, domain_of};
use crate::serve::{Running, now};

/// How often the service asks the mail server what was read.
pub const EVERY_S: u64 = 60;

/// How many of an account's most recently received read messages are looked at each time: the
/// mail the radio still offers is recent.
pub const NEWEST: usize = 500;

/// The messages the radio still offers to accounts that skip what they read: by address, each
/// message's serial by the `Message-ID` its email has, angle brackets kept.
#[must_use]
pub fn unread_by_radio(office: &PostOffice) -> BTreeMap<String, BTreeMap<String, u64>> {
    let mut out = BTreeMap::new();
    for (address, settings) in &office.state.settings {
        if settings.get("radio_skips_read").and_then(Json::as_bool) != Some(true) {
            continue;
        }
        let waiting: BTreeMap<String, u64> = office
            .state
            .mailboxes
            .get(address)
            .into_iter()
            .flatten()
            .filter(|w| !w.picked)
            .filter_map(|w| Some((message_id_of(office, &w.id, &w.from)?, w.serial)))
            .collect();
        if !waiting.is_empty() {
            out.insert(address.clone(), waiting);
        }
    }
    out
}

/// The `Message-ID` a message has in the mail server: the email's it was made of (RP-6 §2.4
/// step 8), or, for a RadioPost message, the one RP-6 §3.1 gives its email.
fn message_id_of(office: &PostOffice, id: &str, from: &str) -> Option<String> {
    let cid = ContentId::from_text(id)?;
    if let Some(m) = office.email_message_id(&cid.0) {
        return Some(m);
    }
    let domain = domain_of(from);
    (!domain.is_empty()).then(|| format!("<{}@{domain}>", cid.text()))
}

/// The serials of `waiting` whose `Message-ID` is among those read (`read`, as JMAP gives them,
/// without angle brackets).
#[must_use]
pub fn read_serials(waiting: &BTreeMap<String, u64>, read: &BTreeSet<String>) -> Vec<u64> {
    waiting
        .iter()
        .filter(|(m, _)| read.contains(m.trim_start_matches('<').trim_end_matches('>')))
        .map(|(_, s)| *s)
        .collect()
}

/// Each account's JMAP `accountId` in Stalwart, by address in lowercase.
fn account_ids(settings: &MailboxSettings) -> Result<BTreeMap<String, String>, String> {
    let responses = mailboxes::jmap_using(
        settings,
        &["urn:ietf:params:jmap:core", "urn:stalwart:jmap"],
        &json!([["x:Account/get", {"ids": null, "properties": ["emailAddress"]}, "a"]]),
    )?;
    Ok(responses
        .iter()
        .filter(|r| r[0] == "x:Account/get")
        .flat_map(|r| r[1]["list"].as_array().cloned().unwrap_or_default())
        .filter_map(|a| {
            Some((
                a["emailAddress"].as_str()?.to_lowercase(),
                a["id"].as_str()?.to_owned(),
            ))
        })
        .collect())
}

/// The `Message-ID`s of an account's newest messages that carry `$seen`.
fn read_in(settings: &MailboxSettings, account: &str) -> Result<BTreeSet<String>, String> {
    let responses = mailboxes::jmap_using(
        settings,
        &["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
        &json!([
            ["Email/query", {"accountId": account, "filter": {"hasKeyword": "$seen"},
                             "sort": [{"property": "receivedAt", "isAscending": false}],
                             "limit": NEWEST}, "q"],
            ["Email/get", {"accountId": account, "properties": ["messageId"],
                           "#ids": {"resultOf": "q", "name": "Email/query", "path": "/ids"}}, "g"]
        ]),
    )?;
    Ok(responses
        .iter()
        .filter(|r| r[0] == "Email/get")
        .flat_map(|r| r[1]["list"].as_array().cloned().unwrap_or_default())
        .flat_map(|e| e["messageId"].as_array().cloned().unwrap_or_default())
        .filter_map(|m| m.as_str().map(str::to_owned))
        .collect())
}

impl Running {
    /// One look at what users read in their mail programs (RP-5 §6.3): each message the radio
    /// still offers to an account with `radio_skips_read` on, and read there, is taken as picked
    /// up. Returns how many.
    ///
    /// # Errors
    ///
    /// The mail server could not be asked: nothing is changed.
    pub fn learn_read(&self) -> Result<usize, String> {
        let wanted = unread_by_radio(&self.office());
        if wanted.is_empty() {
            return Ok(0);
        }
        let settings = self.settings().mailboxes;
        if settings.kind != "stalwart" || settings.url.is_empty() {
            return Err(format!(
                "radio_skips_read is on for {} accounts, and the post office cannot read the mail \
                 server: set mailboxes to Stalwart with its key (post office guide §8)",
                wanted.len()
            ));
        }
        let ids = account_ids(&settings)?;
        let mut found = BTreeMap::new();
        for (address, waiting) in &wanted {
            let Some(account) = ids.get(address) else {
                continue;
            };
            let read = read_in(&settings, account)?;
            let serials = read_serials(waiting, &read);
            if !serials.is_empty() {
                found.insert(address.clone(), serials);
            }
        }
        let mut office = self.office();
        office.at(now());
        let mut taken = 0;
        for (address, serials) in &found {
            taken += office.read_in_mail_program(address, serials);
        }
        if taken > 0 {
            self.save(&office);
        }
        Ok(taken)
    }
}

/// Starts the look at what was read, every minute, on a thread of its own. A failure is said
/// once until it changes.
pub fn start(running: &Arc<Running>) {
    let r = Arc::clone(running);
    std::thread::spawn(move || {
        let mut last = String::new();
        loop {
            match r.learn_read() {
                Ok(0) => {}
                Ok(n) => {
                    eprintln!(
                        "radiopost-po: {n} messages read in mail programs, not offered by radio"
                    );
                    last.clear();
                }
                Err(e) if e != last => {
                    eprintln!("radiopost-po: what was read in mail programs: {e}");
                    last = e;
                }
                Err(_) => {}
            }
            std::thread::sleep(Duration::from_secs(EVERY_S));
        }
    });
}
