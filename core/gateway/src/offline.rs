// SPDX-License-Identifier: MIT OR Apache-2.0
//! A session while the caller's post office cannot be reached (RP-3 §9, RP-7 §1): the gateway's
//! engine is given this [`Upstream`] in place of the post office's, and it answers from the
//! gateway's own store.
//!
//! * It checks the credential and the statement itself (RP-1 §5, §6.3), with the keys and
//!   revocation lists it holds ([`crate::keys`]) and its own replay cache.
//! * It lists the copies placed ahead it holds for the caller (RP-5 §5.8; RP-3 §5.2 flag `copy`,
//!   `held` when it has no air form) and serves them; the engine applies RP-6 §5.1.
//! * It takes the caller's messages into its queue as a session record (RP-7 §3.1), written
//!   before they are receipted (RP-3 §5.4), and records the copies picked up. An offer already in
//!   the queue, or passed on in the last 30 days, is declined `already-have` (RP-7 §3.6).
//! * It says why in a `NOTICE` (RP-3 §5.10), and the engine's `WELCOME` says `online` false.
//!
//! Everything is reported when the post office can be reached again ([`crate::upkeep`]).

use radiopost::credential::{self, flags};
use radiopost::crypto::ContentId;
use radiopost::session::gateway::Upstream;
use radiopost::statement::{self, Freshness, Statement};
use radiopost::{ErrorCode, base64url};
use serde_json::{Value as Json, json};

use crate::store::{Record, Shared};

/// The gateway's own answer to a session, while offline.
pub struct Offline {
    store: Shared,
    callsign: String,
    blocked: Vec<String>,
    only: Option<Vec<String>>,
    clock: u64,
    /// The record this session builds, by its index in the store: `None` until the session opens.
    record: Option<(String, u64)>,
    address: String,
}

/// What the `NOTICE` of an offline session says.
#[must_use]
pub fn notice(held: usize) -> String {
    let copies = match held {
        0 => "it holds no copies of your mail".to_owned(),
        1 => "it serves the one copy of your mail it holds".to_owned(),
        n => format!("it serves the {n} copies of your mail it holds"),
    };
    format!(
        "This gateway cannot reach your post office now: {copies}, and what you send waits in its \
         queue until it can. Your client offers it again."
    )
}

impl Offline {
    /// An offline answer for the gateway `callsign` at `clock`, from `store`, with its operator's
    /// choice of post offices (RP-1 §7.3).
    #[must_use]
    pub fn new(
        store: Shared,
        callsign: &str,
        blocked: Vec<String>,
        only: Option<Vec<String>>,
        clock: u64,
    ) -> Offline {
        Offline {
            store,
            callsign: callsign.to_owned(),
            blocked,
            only,
            clock,
            record: None,
            address: String::new(),
        }
    }

    fn check(&self, body: &Json) -> Result<(Statement, Vec<u8>, Vec<u8>), ErrorCode> {
        let field = |k: &str| {
            body.get(k)
                .and_then(Json::as_str)
                .and_then(base64url::decode)
        };
        let (Some(raw), Some(signature), Some(cred)) =
            (field("statement"), field("signature"), field("credential"))
        else {
            return Err(ErrorCode::Malformed);
        };
        let stmt = Statement::decode(&raw).ok_or(ErrorCode::Malformed)?;
        // RP-1 §6.3 step 1: the credential by RP-1 §5, with the keys and revocations it holds
        let need = flags::RECEIVE
            | if stmt.messages.is_empty() {
                0
            } else {
                flags::SEND
            };
        let store = self.store.lock();
        let trust = store.trust(&self.blocked, self.only.as_deref());
        let cbody = credential::verify(&cred, &trust, self.clock, need).map_err(|r| r.code)?;
        // steps 2 to 6; the engine built a challenge-form statement over its own challenge
        let seen: Vec<(String, [u8; 8])> = store
            .replay
            .iter()
            .filter_map(|(a, n, _)| {
                let bytes: [u8; 8] = crate::station::unhex(n)?.try_into().ok()?;
                Some((a.clone(), bytes))
            })
            .collect();
        let fresh = Freshness {
            now: self.clock,
            seen: &seen,
            challenge: stmt.challenge(),
            sent_nonce: None,
        };
        statement::verify(&stmt, &signature, &cbody, &self.callsign, &fresh)?;
        Ok((stmt, raw, cred))
    }
}

impl Upstream for Offline {
    fn online(&self) -> bool {
        false
    }

    fn open(&mut self, body: &Json) -> Result<Json, ErrorCode> {
        let (stmt, raw, cred) = self.check(body)?;
        let started = body.get("started").and_then(Json::as_u64).unwrap_or(0);
        let session = body
            .get("gateway_session")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_owned();
        let domain = stmt
            .address
            .rsplit_once('@')
            .map_or(String::new(), |(_, d)| d.to_lowercase());
        self.address = stmt.address.to_lowercase();
        let now = self.clock;
        let answer = self
            .store
            .change(|s| {
                s.expire(now);
                if let radiopost::statement::Form::Time { time, nonce } = &stmt.form {
                    s.replay
                        .push((stmt.address.clone(), radiopost::cbor::hex(nonce), *time));
                }
                let copies = s.copies_for(&stmt.address);
                let (mut accept, mut decline) = (Vec::new(), Vec::new());
                for id in &stmt.messages {
                    if s.has(&id.text(), now) {
                        decline.push(json!({"id": id.text(), "code": 14, "name": "already-have"}));
                    } else {
                        accept.push(json!({"id": id.text(), "offset": 0}));
                    }
                }
                s.records.push(Record {
                    domain,
                    address: stmt.address.clone(),
                    operator: stmt.operator.clone(),
                    statement: base64url::encode(&raw),
                    signature: body
                        .get("signature")
                        .and_then(Json::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    credential: base64url::encode(&cred),
                    started,
                    gateway_session: session.clone(),
                    cursor: body.get("cursor").and_then(Json::as_u64).unwrap_or(0),
                    ..Record::default()
                });
                json!({
                    "session": format!("offline-{session}"),
                    "waiting": copies.iter().map(crate::store::Copy::entry).collect::<Vec<_>>(),
                    "more": 0, "accept": accept, "decline": decline,
                    "notice": notice(copies.len()),
                })
            })
            .map_err(|_| ErrorCode::Internal)?;
        self.record = Some((session, started));
        Ok(answer)
    }

    fn fetch(&mut self, serial: u64) -> Result<Vec<u8>, ErrorCode> {
        let store = self.store.lock();
        store
            .copies
            .iter()
            .find(|c| c.address == self.address && c.serial == serial)
            .and_then(|c| c.air.as_deref())
            .and_then(base64url::decode)
            .ok_or(ErrorCode::UnknownMessage)
    }

    fn hand_in(
        &mut self,
        messages: &[(ContentId, Vec<u8>)],
    ) -> Result<Vec<Result<(), ErrorCode>>, ErrorCode> {
        let Some(key) = self.record.clone() else {
            return Err(ErrorCode::Internal);
        };
        // RP-3 §5.4: stored durably before the RECEIPT lists them
        self.store
            .change(|s| {
                let Some(r) = s
                    .records
                    .iter_mut()
                    .find(|r| (r.gateway_session.clone(), r.started) == key)
                else {
                    return Err(ErrorCode::Internal);
                };
                for (id, air) in messages {
                    if !r.messages.iter().any(|(m, _)| *m == id.text()) {
                        r.messages.push((id.text(), base64url::encode(air)));
                    }
                }
                Ok(messages.iter().map(|_| Ok(())).collect())
            })
            .map_err(|_| ErrorCode::Internal)?
    }

    fn picked_up(&mut self, serials: &[u64]) -> Result<(), ErrorCode> {
        let Some(key) = self.record.clone() else {
            return Err(ErrorCode::Internal);
        };
        let address = self.address.clone();
        self.store
            .change(|s| {
                for c in s
                    .copies
                    .iter_mut()
                    .filter(|c| c.address == address && serials.contains(&c.serial))
                {
                    c.picked = true;
                }
                if let Some(r) = s
                    .records
                    .iter_mut()
                    .find(|r| (r.gateway_session.clone(), r.started) == key)
                {
                    for serial in serials {
                        if !r.pickups.contains(serial) {
                            r.pickups.push(*serial);
                        }
                    }
                }
            })
            .map_err(|_| ErrorCode::Internal)
    }

    fn files(&mut self, _asked: &[(u64, Vec<u8>)]) -> Result<(), ErrorCode> {
        Err(ErrorCode::PostOfficeUnreachable)
    }

    fn close(&mut self) {
        let Some(key) = self.record.take() else {
            return;
        };
        // the record is whole: reported when the post office can be reached, or dropped when it
        // holds nothing to report
        let _ = self.store.change(|s| {
            if let Some(i) = s
                .records
                .iter()
                .position(|r| (r.gateway_session.clone(), r.started) == key)
            {
                if s.records[i].is_empty() {
                    s.records.remove(i);
                } else {
                    s.records[i].done = true;
                }
            }
        });
    }
}

/// The post office over HTTPS while it answers, and the gateway's own answer from the moment it
/// cannot be reached: decided when the session opens (RP-3 §9), for the whole session.
pub struct Fallback {
    online: Option<Box<dyn Upstream + Send>>,
    offline: Offline,
    cut_off: bool,
}

/// What says the post office cannot be reached now, rather than that it refused.
fn unreachable(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::PostOfficeUnreachable
            | ErrorCode::Timeout
            | ErrorCode::Internal
            | ErrorCode::Busy
    )
}

impl Fallback {
    /// The post office at `online`, when a URL for it was found, and the gateway's own answer.
    #[must_use]
    pub fn new(online: Option<Box<dyn Upstream + Send>>, offline: Offline) -> Fallback {
        Fallback {
            cut_off: online.is_none(),
            online,
            offline,
        }
    }

    fn active(&mut self) -> &mut dyn Upstream {
        match (&mut self.online, self.cut_off) {
            (Some(up), false) => up.as_mut(),
            _ => &mut self.offline,
        }
    }
}

impl Upstream for Fallback {
    fn online(&self) -> bool {
        !self.cut_off
    }

    fn open(&mut self, body: &Json) -> Result<Json, ErrorCode> {
        let mut why = None;
        if let (Some(up), false) = (&mut self.online, self.cut_off) {
            match up.open(body) {
                Err(code) if unreachable(code) => {
                    self.cut_off = true;
                    why = Some(code);
                }
                other => return other,
            }
        }
        match (self.offline.open(body), why) {
            // a post office it found but cannot reach, and holds no keys of: it says so
            (Err(ErrorCode::UnknownPostOffice), Some(code)) => Err(code),
            (answer, _) => answer,
        }
    }

    fn fetch(&mut self, serial: u64) -> Result<Vec<u8>, ErrorCode> {
        self.active().fetch(serial)
    }

    fn hand_in(
        &mut self,
        messages: &[(ContentId, Vec<u8>)],
    ) -> Result<Vec<Result<(), ErrorCode>>, ErrorCode> {
        self.active().hand_in(messages)
    }

    fn picked_up(&mut self, serials: &[u64]) -> Result<(), ErrorCode> {
        self.active().picked_up(serials)
    }

    fn files(&mut self, asked: &[(u64, Vec<u8>)]) -> Result<(), ErrorCode> {
        self.active().files(asked)
    }

    fn close(&mut self) {
        self.active().close();
    }
}
