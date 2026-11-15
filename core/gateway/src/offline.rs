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
//! * Local delivery (RP-7 §4.4): a message that names a local user (a copy placed ahead here in
//!   the last 30 days) is kept for that user too, listed to them with serial 0 and the flag
//!   `local`, sent by content ID, and its pickup kept in their record as a local pickup.
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
    /// The `NOTICE` text, when not [`notice`]'s.
    notice: Option<String>,
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
         queue until it can, mail for its own users handed over here. Your client offers it again."
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
            notice: None,
        }
    }

    /// The same, saying `text` in its `NOTICE` (RP-3 §5.10) rather than [`notice`]'s words.
    #[must_use]
    pub fn with_notice(mut self, text: &str) -> Offline {
        self.notice = Some(text.to_owned());
        self
    }

    fn check(&self, body: &Json) -> Result<(Statement, Vec<u8>, Vec<u8>), ErrorCode> {
        // RP-1 §6.3 step 1: `receive`, and `send` when the statement lists messages
        let need = |stmt: &Statement| {
            flags::RECEIVE
                | if stmt.messages.is_empty() {
                    0
                } else {
                    flags::SEND
                }
        };
        let (stmt, raw, cred, _) = check_caller(
            &self.store,
            body,
            &self.callsign,
            (&self.blocked, self.only.as_deref()),
            self.clock,
            need,
        )?;
        Ok((stmt, raw, cred))
    }
}

/// A caller's credential and statement checked as RP-1 §5 and §6.3 say, by a gateway that answers
/// from what it holds: the gateway `me` at `clock`, with the keys and revocation lists in `store`
/// its operator's choices (`blocked`, `only`) let it trust, its replay cache, and the flags `need`
/// says the statement requires. The statement, its encoding, the credential and its body.
///
/// # Errors
///
/// The code RP-1 §5 or §6.3 refuses with.
pub(crate) fn check_caller(
    store: &Shared,
    body: &Json,
    me: &str,
    (blocked, only): (&[String], Option<&[String]>),
    clock: u64,
    need: impl Fn(&Statement) -> u64,
) -> Result<(Statement, Vec<u8>, Vec<u8>, radiopost::credential::Body), ErrorCode> {
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
    let store = store.lock();
    let trust = store.trust(blocked, only);
    let cbody = credential::verify(&cred, &trust, clock, need(&stmt)).map_err(|r| r.code)?;
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
        now: clock,
        seen: &seen,
        challenge: stmt.challenge(),
        sent_nonce: None,
    };
    statement::verify(&stmt, &signature, &cbody, me, &fresh)?;
    Ok((stmt, raw, cred, cbody))
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
                let local = s.local_for(&stmt.address);
                // RP-3 §6.1's order, the local messages after the copies of the same priority
                let mut waiting: Vec<(u64, u8, Json)> = copies
                    .iter()
                    .map(|c| (c.priority, 0, c.entry()))
                    .chain(local.iter().map(|l| (l.priority, 1, l.entry())))
                    .collect();
                waiting.sort_by_key(|(priority, kind, _)| (std::cmp::Reverse(*priority), *kind));
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
                    "waiting": waiting.into_iter().map(|(_, _, e)| e).collect::<Vec<_>>(),
                    "more": 0, "accept": accept, "decline": decline,
                    "notice": self.notice.clone().unwrap_or_else(|| notice(copies.len())),
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
        let now = self.clock;
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
                // RP-7 §4.4: kept for each local user it names as well
                for (id, air) in messages {
                    let Ok(message) = radiopost::message::Message::from_air(air) else {
                        continue;
                    };
                    let local = radiopost::session::gateway::local_recipients(
                        &message,
                        &s.copies_seen,
                        now,
                    );
                    for address in local {
                        if s.local
                            .iter()
                            .any(|l| l.address == address && l.id == id.text())
                        {
                            continue;
                        }
                        s.local.push(crate::store::Local {
                            address,
                            id: id.text(),
                            air: base64url::encode(air),
                            from: message.from().unwrap_or("").to_owned(),
                            subject: message
                                .map()
                                .get(radiopost::message::key::SUBJECT)
                                .and_then(radiopost::cbor::Value::as_text)
                                .unwrap_or("")
                                .to_owned(),
                            size: air.len() as u64,
                            priority: message
                                .map()
                                .get(radiopost::message::key::PRIORITY)
                                .and_then(radiopost::cbor::Value::as_u64)
                                .unwrap_or(0),
                            kept: now,
                            picked: false,
                        });
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

    fn fetch_local(&mut self, id: &ContentId) -> Result<Vec<u8>, ErrorCode> {
        let store = self.store.lock();
        store
            .local
            .iter()
            .find(|l| l.address.eq_ignore_ascii_case(&self.address) && l.id == id.text())
            .and_then(|l| base64url::decode(&l.air))
            .ok_or(ErrorCode::UnknownMessage)
    }

    fn picked_up_local(&mut self, ids: &[ContentId]) -> Result<(), ErrorCode> {
        let Some(key) = self.record.clone() else {
            return Err(ErrorCode::Internal);
        };
        let address = self.address.clone();
        let ids: Vec<String> = ids.iter().map(ContentId::text).collect();
        self.store
            .change(|s| {
                for l in s
                    .local
                    .iter_mut()
                    .filter(|l| l.address.eq_ignore_ascii_case(&address) && ids.contains(&l.id))
                {
                    l.picked = true;
                }
                // the post office learns of it with the record (RP-5 §5.6 `local_pickups`)
                if let Some(r) = s
                    .records
                    .iter_mut()
                    .find(|r| (r.gateway_session.clone(), r.started) == key)
                {
                    for id in &ids {
                        if !r.local_pickups.contains(id) {
                            r.local_pickups.push(id.clone());
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

    fn fetch_local(&mut self, id: &ContentId) -> Result<Vec<u8>, ErrorCode> {
        self.active().fetch_local(id)
    }

    fn picked_up_local(&mut self, ids: &[ContentId]) -> Result<(), ErrorCode> {
        self.active().picked_up_local(ids)
    }

    fn files(&mut self, asked: &[(u64, Vec<u8>)]) -> Result<(), ErrorCode> {
        self.active().files(asked)
    }

    fn close(&mut self) {
        self.active().close();
    }
}
