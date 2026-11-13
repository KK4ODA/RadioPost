// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the gateway holds for when the Internet is down (Phase 3), kept in `store.json` in its
//! directory and written whole after each change:
//!
//! * **copies placed ahead** (RP-5 §5.8): each user's waiting mail its post office copied here,
//!   with the air form when it fitted, dropped when picked up elsewhere or when it expires; and
//!   the **forward items** for cut-off gateways (RP-7 §4.1), kept for when partners collect them;
//! * **the post offices it polls**, each with its cursor and whether it places items here;
//! * **the queue** (RP-7 §3.1): a session record for each session it carried while it could not
//!   reach the caller's post office, stored before its `RECEIPT` (RP-3 §5.4), until it is
//!   reported (RP-5 §5.6); and the content IDs it passed on, for 30 days (RP-7 §3.5);
//! * **the key cache** (RP-1 §7.3): each post office's keys and revocation list, and the replay
//!   cache of statements checked offline (RP-1 §6.3).
//!
//! The answering loop and the threads beside it share one [`Shared`] store; the panel reads the
//! file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use radiopost::credential::{PostOfficeKey, Revocations, Trust};
use radiopost_postoffice::known::Found;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::station::write;

/// How long a record not reported is kept, and how long a content ID passed on is remembered
/// (RP-7 §3.5, §7).
pub const KEEP_S: u64 = 30 * 86_400;
/// How long a statement checked offline is remembered against replay (RP-1 §6.3).
pub const REPLAY_S: u64 = radiopost::statement::REPLAY_S;

/// A copy placed ahead (RP-5 §5.8).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Copy {
    /// The user it is for.
    pub address: String,
    /// Its serial in the user's mailbox (RP-3 §6.2).
    pub serial: u64,
    /// Its content ID, base32.
    pub id: String,
    /// The sender.
    pub from: String,
    /// The subject.
    pub subject: String,
    /// Its air form's size.
    pub size: u64,
    /// Its priority.
    pub priority: u64,
    /// Its sender is not shown to be licensed (RP-6 §5.1).
    pub third_party: bool,
    /// Its air form, base64url, when it fitted the gateway's `max_air_bytes`.
    pub air: Option<String>,
    /// When it expires.
    pub expires: u64,
    /// The post office that placed it.
    pub by: String,
    /// Picked up here while offline: no longer listed, waiting for the report and the drop.
    pub picked: bool,
}

impl Copy {
    /// The entry an offline session lists (RP-3 §5.2): `held` when there is no air form.
    #[must_use]
    pub fn entry(&self) -> Json {
        json!({"serial": self.serial, "id": self.id, "from": self.from, "subject": self.subject,
               "size": self.size, "priority": self.priority, "held": self.air.is_none(),
               "third_party": self.third_party, "copy": true})
    }
}

/// A post office the gateway polls for copies (RP-5 §5.8).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Poll {
    /// Its API's base URL.
    pub api: String,
    /// The cursor of the last answer held.
    pub cursor: String,
    /// Its last answer's `placing`.
    pub placing: bool,
    /// When it was last asked, Unix seconds.
    pub asked: u64,
    /// Why the last poll failed, when it did.
    pub error: Option<String>,
}

/// A session carried while the caller's post office could not be reached (RP-7 §3.1), as RP-5
/// §5.6 reports it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    /// The post office it is reported to: the domain of the statement's address.
    pub domain: String,
    /// The account.
    pub address: String,
    /// The caller's callsign.
    pub operator: String,
    /// The statement's encoding, base64url.
    pub statement: String,
    /// Its signature, base64url.
    pub signature: String,
    /// The caller's credential, base64url.
    pub credential: String,
    /// When the session started, on the gateway's clock.
    pub started: u64,
    /// The gateway's session ID, hex.
    pub gateway_session: String,
    /// The caller's cursor.
    pub cursor: u64,
    /// The messages the caller sent: content ID and air form, base64url.
    pub messages: Vec<(String, String)>,
    /// The serials it picked up from copies.
    pub pickups: Vec<u64>,
    /// The session is over: the record is whole and may be reported.
    pub done: bool,
    /// Tries at reporting it that failed, and why the last did.
    pub tries: u32,
    /// Why the last try failed.
    pub error: Option<String>,
}

impl Record {
    /// Its entry in `POST /reports` (RP-5 §5.6).
    #[must_use]
    pub fn report(&self) -> Json {
        json!({
            "statement": self.statement, "signature": self.signature,
            "credential": self.credential, "started": self.started,
            "gateway_session": self.gateway_session, "cursor": self.cursor,
            "messages": self.messages.iter()
                .map(|(id, air)| json!({"id": id, "air": air})).collect::<Vec<_>>(),
            "pickups": self.pickups,
        })
    }

    /// Nothing to report: no message and no pickup.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty() && self.pickups.is_empty()
    }
}

/// What the gateway knows of a post office's keys (RP-1 §7.3).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keyed {
    /// Its keys and revocation list, checked.
    pub found: Found,
    /// How it was learned: `dns`, `credential` (the post office of the gateway's own credential,
    /// named in its settings), or `list:<domain>` (named in that post office's list, one hop).
    pub how: String,
    /// When it was last fetched whole.
    pub fetched: u64,
    /// Why the last refresh failed, when it did.
    pub error: Option<String>,
}

/// One report sent, for the panel.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reported {
    /// When.
    pub at: u64,
    /// The account.
    pub address: String,
    /// Messages the post office took (`accepted` or `duplicate`).
    pub taken: usize,
    /// Messages it refused.
    pub refused: usize,
    /// Pickups reported.
    pub pickups: usize,
    /// The session refused whole, with this code's name.
    pub code: Option<String>,
}

/// What the gateway holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    /// Copies placed ahead.
    pub copies: Vec<Copy>,
    /// Forward items for cut-off gateways (RP-7 §4.1), held for Phase 4.
    pub forwards: Vec<Json>,
    /// The post offices it polls or has asked, by domain.
    pub polls: BTreeMap<String, Poll>,
    /// Session records not yet reported, oldest first.
    pub records: Vec<Record>,
    /// Content IDs passed on to a post office, and when (RP-7 §3.5).
    pub passed_on: BTreeMap<String, u64>,
    /// Statements checked offline: (address, nonce hex, `time`) (RP-1 §6.3).
    pub replay: Vec<(String, String, u64)>,
    /// The key cache, by domain.
    pub keys: BTreeMap<String, Keyed>,
    /// The last reports sent, newest last.
    pub reported: Vec<Reported>,
    /// Post offices whose users called since the upkeep last looked: their documents fetched when
    /// not held, and asked whether they place copies here (RP-1 §7.3, RP-5 §5.8).
    pub learn: Vec<String>,
}

/// Reports kept for the panel.
pub const REPORTED_KEPT: usize = 50;

/// The store, shared between the answering loop and the threads beside it.
#[derive(Clone)]
pub struct Shared {
    dir: PathBuf,
    store: Arc<Mutex<Store>>,
}

impl Store {
    /// `store.json` in `dir`, or an empty store when there is none yet.
    #[must_use]
    pub fn load(dir: &Path) -> Store {
        std::fs::read_to_string(dir.join("store.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Applies one poll's items from the post office of `by` (RP-5 §5.8): copies kept, drops
    /// applied, forward items kept; the copies of at most `users` users. Returns how many copies
    /// were kept and dropped.
    pub fn apply(&mut self, by: &str, items: &[Json], users: Option<u64>) -> (usize, usize) {
        let (mut kept, mut dropped) = (0, 0);
        for item in items {
            let address = item["address"].as_str().unwrap_or("").to_lowercase();
            let serial = item["serial"].as_u64().unwrap_or(0);
            match item["kind"].as_str() {
                Some("copy") => {
                    let id = item["id"].as_str().unwrap_or("").to_owned();
                    if self
                        .copies
                        .iter()
                        .any(|c| c.address == address && c.id == id)
                    {
                        continue;
                    }
                    let holds = self.copies.iter().any(|c| c.address == address);
                    let users_held = self.users().len() as u64;
                    if !holds && users.is_some_and(|n| users_held >= n) {
                        continue; // RP-5 §4.4 `copies.users`: no more users than it said
                    }
                    self.copies.push(Copy {
                        address,
                        serial,
                        id,
                        from: item["from"].as_str().unwrap_or("").to_owned(),
                        subject: item["subject"].as_str().unwrap_or("").to_owned(),
                        size: item["size"].as_u64().unwrap_or(0),
                        priority: item["priority"].as_u64().unwrap_or(0),
                        third_party: item["third_party"].as_bool().unwrap_or(true),
                        air: item["air"].as_str().map(str::to_owned),
                        expires: item["expires"].as_u64().unwrap_or(0),
                        by: by.to_owned(),
                        picked: false,
                    });
                    kept += 1;
                }
                Some("drop") => {
                    let before = self.copies.len();
                    self.copies
                        .retain(|c| !(c.address == address && c.serial == serial));
                    dropped += before - self.copies.len();
                    self.forwards.retain(|f| {
                        !(f["address"].as_str() == Some(address.as_str())
                            && f["serial"].as_u64() == Some(serial))
                    });
                }
                Some("forward") if !self.forwards.contains(item) => {
                    self.forwards.push(item.clone());
                }
                _ => {}
            }
        }
        (kept, dropped)
    }

    /// Drops what has expired at `now`: copies and forward items past their `expires`, records
    /// unreported after 30 days (said in the returned lines), content IDs passed on more than 30
    /// days ago, and the replay cache's old entries.
    pub fn expire(&mut self, now: u64) -> Vec<String> {
        self.copies.retain(|c| c.expires == 0 || c.expires > now);
        self.forwards
            .retain(|f| f["expires"].as_u64().is_none_or(|e| e > now));
        let mut said = Vec::new();
        self.records.retain(|r| {
            let old = now.saturating_sub(r.started) >= KEEP_S;
            if old {
                said.push(format!(
                    "dropped the session record of {} from {}: not reported in 30 days ({})",
                    r.operator,
                    r.started,
                    r.error
                        .as_deref()
                        .unwrap_or("its post office was not reached")
                ));
            }
            !old
        });
        self.passed_on
            .retain(|_, at| now.saturating_sub(*at) < KEEP_S);
        self.replay
            .retain(|(_, _, t)| now.saturating_sub(*t) < REPLAY_S);
        said
    }

    /// The users it holds copies for.
    #[must_use]
    pub fn users(&self) -> Vec<String> {
        let mut users: Vec<String> = self.copies.iter().map(|c| c.address.clone()).collect();
        users.sort();
        users.dedup();
        users
    }

    /// The copies an offline session lists for `address`: not picked up here, oldest serial
    /// first.
    #[must_use]
    pub fn copies_for(&self, address: &str) -> Vec<Copy> {
        let mut out: Vec<Copy> = self
            .copies
            .iter()
            .filter(|c| c.address.eq_ignore_ascii_case(address) && !c.picked)
            .cloned()
            .collect();
        out.sort_by_key(|c| (std::cmp::Reverse(c.priority), c.serial));
        out
    }

    /// Whether a content ID is in the queue, or was passed on in the last 30 days (RP-7 §3.6).
    #[must_use]
    pub fn has(&self, id: &str, now: u64) -> bool {
        self.records
            .iter()
            .any(|r| r.messages.iter().any(|(m, _)| m == id))
            || self
                .passed_on
                .get(id)
                .is_some_and(|at| now.saturating_sub(*at) < KEEP_S)
    }

    /// The post offices' keys and revocation lists it holds that RP-1 §7.3 lets it trust while
    /// offline: those learned from DNS or its own credential, and one hop of their lists; never
    /// one its operator blocked, and only those it named when it named any.
    #[must_use]
    pub fn trust(&self, blocked: &[String], only: Option<&[String]>) -> Trust {
        let found: BTreeMap<String, radiopost::dns::Found> = self
            .keys
            .iter()
            .map(|(d, k)| {
                let how = match k.how.strip_prefix("list:") {
                    Some(by) => radiopost::dns::Found::ListOf(by.to_owned()),
                    None => radiopost::dns::Found::Dns,
                };
                (d.clone(), how)
            })
            .collect();
        let mut trust = Trust::default();
        for (domain, keyed) in &self.keys {
            if !radiopost::dns::trusts(domain, &found, blocked, only) {
                continue;
            }
            let keys: Vec<PostOfficeKey> = keyed.found.trust_keys();
            trust.keys.insert(domain.clone(), keys);
            let r: Revocations = keyed.found.revocations();
            trust.revoked.insert(domain.clone(), r);
        }
        trust
    }

    /// The content IDs of the messages in the queue, base32.
    #[must_use]
    pub fn queued_ids(&self) -> Vec<String> {
        self.records
            .iter()
            .flat_map(|r| r.messages.iter().map(|(id, _)| id.clone()))
            .collect()
    }
}

impl Shared {
    /// The store of the gateway in `dir`, loaded.
    #[must_use]
    pub fn open(dir: &Path) -> Shared {
        Shared {
            dir: dir.to_owned(),
            store: Arc::new(Mutex::new(Store::load(dir))),
        }
    }

    /// The store, locked.
    pub fn lock(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Writes the store whole.
    ///
    /// # Errors
    ///
    /// It could not be written.
    pub fn save(&self, store: &Store) -> Result<(), String> {
        let text = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
        write(&self.dir, "store.json", &text)
    }

    /// Changes the store and writes it, in one lock.
    ///
    /// # Errors
    ///
    /// It could not be written: what changed is kept in memory only.
    pub fn change<T>(&self, f: impl FnOnce(&mut Store) -> T) -> Result<T, String> {
        let mut store = self.lock();
        let out = f(&mut store);
        self.save(&store)?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{Record, Store};

    /// RP-7 §3.6, against `vectors/queued.json`: a cut-off gateway declines an offer
    /// `already-have` while it is in the queue, and for 30 days after it was passed on.
    #[test]
    fn answers_offers_as_the_vectors_say() {
        let v: Value =
            serde_json::from_str(include_str!("../../../vectors/queued.json")).expect("JSON");
        for c in v["answer_cases"].as_array().expect("answer_cases") {
            let text = |hex: &str| {
                let bytes: Vec<u8> = crate::station::unhex(hex).expect("hex");
                radiopost::ContentId::from_slice(&bytes)
                    .expect("an id")
                    .text()
            };
            let mut s = Store::default();
            for q in c["queue_hex"].as_array().expect("queue") {
                s.records.push(Record {
                    messages: vec![(text(q.as_str().expect("hex")), String::new())],
                    ..Record::default()
                });
            }
            for (hex, at) in c["passed_on"].as_object().expect("passed_on") {
                s.passed_on.insert(text(hex), at.as_u64().expect("when"));
            }
            let offer = text(c["offer_hex"].as_str().expect("offer"));
            let got = if s.has(&offer, c["now"].as_u64().expect("now")) {
                "already-have"
            } else {
                "accept"
            };
            assert_eq!(got, c["expect"].as_str().expect("expect"), "{}", c["what"]);
        }
    }

    #[test]
    fn copies_are_kept_dropped_and_limited_to_their_users() {
        let mut s = Store::default();
        let copy = |address: &str, serial: u64, id: &str| {
            json!({"kind": "copy", "address": address, "serial": serial, "id": id,
                   "from": "maria@example.com", "subject": "Hola", "size": 10, "priority": 0,
                   "third_party": true, "expires": 1000})
        };
        let (kept, _) = s.apply(
            "radiopost.org",
            &[
                copy("a@radiopost.org", 1, "aa"),
                copy("b@radiopost.org", 1, "bb"),
                copy("a@radiopost.org", 2, "ac"),
                copy("a@radiopost.org", 2, "ac"),
            ],
            Some(1),
        );
        assert_eq!(
            kept, 2,
            "the second user is over the limit, the repeat is held"
        );
        assert_eq!(s.users(), ["a@radiopost.org"]);
        assert!(s.copies_for("a@radiopost.org")[0].air.is_none());
        assert_eq!(s.copies_for("A@radiopost.org").len(), 2);
        let (_, dropped) = s.apply(
            "radiopost.org",
            &[json!({"kind": "drop", "address": "a@radiopost.org", "serial": 1})],
            Some(1),
        );
        assert_eq!(dropped, 1);
        s.expire(999);
        assert_eq!(s.copies.len(), 1);
        s.expire(1000);
        assert!(s.copies.is_empty(), "expired");
    }
}
