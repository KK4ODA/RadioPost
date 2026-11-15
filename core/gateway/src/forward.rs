// SPDX-License-Identifier: MIT OR Apache-2.0
//! Forwarding by radio between gateways (RP-7 §2, §3), the gateway program's side both ways.
//!
//! * [`Partner`] is what a partner answers a forwarding session from: its own store. It checks
//!   the calling gateway's credential (`gateway`, and `send` when it offers) and statement with the
//!   keys it holds (RP-1 §5, §6.3), declines what it already has (RP-7 §3.6), lists the forward
//!   items it holds for the caller with their forwarding serials (RP-7 §4.3), and keeps each
//!   record the engine took with its own callsign added to the path (RP-7 §3.4). The upkeep
//!   reports it to its post office with that path (RP-5 §5.6); a partner cut off too forwards it
//!   in turn, as its own.
//! * [`outbox`] and [`Outcome::apply`] are a cut-off gateway calling a partner: the records it
//!   sends and their messages, and what the session's events do to its store (RP-7 §3.5): a
//!   message listed `complete`, or declined `already-have`, is passed on; a record listed in
//!   `records` is let go and its credential remembered as the partner's; a forward item is held
//!   as a copy placed ahead for its user.

use std::collections::HashMap;

use radiopost::credential::flags;
use radiopost::crypto::ContentId;
use radiopost::forwarding::{Forwarded, Proof};
use radiopost::message::{self, Message};
use radiopost::session::gateway::Upstream;
use radiopost::session::{Event, Outgoing, flags as listed};
use radiopost::statement::Statement;
use radiopost::{ErrorCode, base64url, cbor};
use serde_json::{Value as Json, json};

use crate::offline::check_caller;
use crate::store::{Copy, Record, Shared, Store};

/// A partner's answer to a forwarding session from one gateway (RP-7 §2).
pub struct Partner {
    store: Shared,
    me: String,
    caller: String,
    blocked: Vec<String>,
    only: Option<Vec<String>>,
    clock: u64,
    online: bool,
}

impl Partner {
    /// The gateway `me`, answering a forwarding session from the gateway `caller` at `clock` from
    /// `store`, with its operator's choice of post offices (RP-1 §7.3); `online` is whether it
    /// reaches the Internet now (`WELCOME` `online`).
    #[must_use]
    pub fn new(
        store: Shared,
        me: &str,
        caller: &str,
        (blocked, only): (Vec<String>, Option<Vec<String>>),
        clock: u64,
        online: bool,
    ) -> Partner {
        Partner {
            store,
            me: me.to_owned(),
            caller: caller.to_owned(),
            blocked,
            only,
            clock,
            online,
        }
    }

    /// The forward item for the caller with this forwarding serial.
    fn item(&self, fserial: u64) -> Option<Json> {
        self.store
            .lock()
            .forwards
            .iter()
            .find(|f| {
                f["gateway"].as_str() == Some(self.caller.as_str())
                    && f["fserial"].as_u64() == Some(fserial)
            })
            .cloned()
    }
}

impl Upstream for Partner {
    fn online(&self) -> bool {
        self.online
    }

    fn open(&mut self, body: &Json) -> Result<Json, ErrorCode> {
        // RP-7 §2: a gateway credential, with `send` when it offers; no `receive`
        let need = |stmt: &Statement| {
            flags::GATEWAY
                | if stmt.messages.is_empty() {
                    0
                } else {
                    flags::SEND
                }
        };
        let (stmt, _, _, cbody) = check_caller(
            &self.store,
            body,
            &self.me,
            (&self.blocked, self.only.as_deref()),
            self.clock,
            need,
        )?;
        if !cbody.operator.eq_ignore_ascii_case(&self.caller) {
            return Err(ErrorCode::NotAuthorized);
        }
        let cursor = body.get("cursor").and_then(Json::as_u64).unwrap_or(0);
        let now = self.clock;
        let caller = self.caller.clone();
        let session = body
            .get("gateway_session")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_owned();
        self.store
            .change(|s| {
                s.expire(now);
                if let radiopost::statement::Form::Time { time, nonce } = &stmt.form {
                    s.replay
                        .push((stmt.address.clone(), cbor::hex(nonce), *time));
                }
                let waiting: Vec<Json> = s
                    .forwards_for(&caller, cursor)
                    .iter()
                    .map(|f| {
                        json!({"serial": f["fserial"], "id": f["id"], "from": f["from"],
                               "subject": f["subject"], "size": f["size"],
                               "priority": f["priority"], "held": f["air"].is_null(),
                               "third_party": f["third_party"].as_bool().unwrap_or(false)})
                    })
                    .collect();
                let (mut accept, mut decline) = (Vec::new(), Vec::new());
                for id in &stmt.messages {
                    if s.has(&id.text(), now) {
                        decline.push(json!({"id": id.text(), "code": 14, "name": "already-have"}));
                    } else {
                        accept.push(json!({"id": id.text(), "offset": 0}));
                    }
                }
                json!({"session": format!("forwarding-{session}"), "waiting": waiting, "more": 0,
                       "accept": accept, "decline": decline})
            })
            .map_err(|_| ErrorCode::Internal)
    }

    fn fetch(&mut self, serial: u64) -> Result<Vec<u8>, ErrorCode> {
        self.item(serial)
            .and_then(|f| f["air"].as_str().and_then(base64url::decode))
            .ok_or(ErrorCode::UnknownMessage)
    }

    fn placement(&mut self, serial: u64) -> Option<Vec<u8>> {
        self.item(serial)
            .and_then(|f| f["placement"].as_str().and_then(base64url::decode))
    }

    fn trust(&mut self) -> radiopost::credential::Trust {
        self.store.lock().trust(&self.blocked, self.only.as_deref())
    }

    fn take_records(
        &mut self,
        records: &[Forwarded],
    ) -> Result<Vec<Result<(), ErrorCode>>, ErrorCode> {
        let me = self.me.clone();
        // RP-3 §5.4: stored before the RECEIPT lists them
        self.store
            .change(|s| {
                records
                    .iter()
                    .map(|f| {
                        keep_record(s, f, &me);
                        Ok(())
                    })
                    .collect()
            })
            .map_err(|_| ErrorCode::Internal)
    }

    fn hand_in(
        &mut self,
        _messages: &[(ContentId, Vec<u8>)],
    ) -> Result<Vec<Result<(), ErrorCode>>, ErrorCode> {
        // forwarded mail comes with its records (RP-7 §2)
        Err(ErrorCode::RefusedByPolicy)
    }

    fn picked_up(&mut self, serials: &[u64]) -> Result<(), ErrorCode> {
        // RP-7 §4.3: a forward item goes once the caller lists it `complete`
        let caller = self.caller.clone();
        self.store
            .change(|s| {
                s.forwards.retain(|f| {
                    f["gateway"].as_str() != Some(caller.as_str())
                        || !f["fserial"].as_u64().is_some_and(|n| serials.contains(&n))
                });
            })
            .map_err(|_| ErrorCode::Internal)
    }

    fn files(&mut self, _asked: &[(u64, Vec<u8>)]) -> Result<(), ErrorCode> {
        Err(ErrorCode::RefusedByPolicy)
    }

    fn close(&mut self) {}
}

/// RP-7 §3.4: a record a partner took, kept with its callsign `me` added to the path, to report or
/// to forward in turn. A record it holds already (the same statement) is kept once.
fn keep_record(s: &mut Store, f: &Forwarded, me: &str) {
    let statement = base64url::encode(&f.record.proof.statement);
    if s.records.iter().any(|r| r.statement == statement) {
        return;
    }
    let address = f.record.statement.address.clone();
    let mut path = f.record.proof.path.clone();
    path.push(me.to_owned());
    s.records.push(Record {
        domain: address
            .rsplit_once('@')
            .map_or(String::new(), |(_, d)| d.to_lowercase()),
        address,
        operator: f.record.statement.operator.clone(),
        statement,
        signature: base64url::encode(&f.record.proof.signature),
        credential: base64url::encode(&f.record.credential),
        started: f.record.proof.started,
        gateway_session: cbor::hex(&f.record.proof.session),
        messages: f
            .messages
            .iter()
            .map(|(id, air)| (id.text(), base64url::encode(air)))
            .collect(),
        pickups: f.record.proof.pickups.clone(),
        local_pickups: f.record.proof.local.iter().map(ContentId::text).collect(),
        done: true,
        path,
        ..Record::default()
    });
}

/// The hash a partner holding a credential knows it by, hex (RP-1 §4.3).
fn hash_hex(credential: &str) -> Option<String> {
    base64url::decode(credential).map(|c| cbor::hex(&radiopost::credential::hash(&c)))
}

/// What the cut-off gateway `me` sends the partner `partner` (RP-7 §3.2): each whole record whose
/// path does not already hold the partner, oldest first, by its credential's hash when the partner
/// holds that credential; and the messages of those records, to offer in `HELLO`.
#[must_use]
pub fn outbox(s: &Store, me: &str, partner: &str) -> (Vec<Proof>, Vec<Outgoing>) {
    let holds = s
        .partners
        .get(partner)
        .map(|p| p.holds.clone())
        .unwrap_or_default();
    let mut records = Vec::new();
    let mut messages: Vec<Outgoing> = Vec::new();
    for r in &s.records {
        if !r.done || r.is_empty() || r.path.iter().any(|c| c.eq_ignore_ascii_case(partner)) {
            continue;
        }
        let hashed = hash_hex(&r.credential).is_some_and(|h| holds.contains(&h));
        let Some(proof) = r.proof(me, hashed) else {
            continue;
        };
        records.push(proof);
        for (id, air) in &r.messages {
            let Some(air) = base64url::decode(air) else {
                continue;
            };
            let Ok(message) = Message::from_air(&air) else {
                continue;
            };
            if message.content_id().text() == *id
                && !messages.iter().any(|m| m.id() == message.content_id())
            {
                messages.push(Outgoing { message, air });
            }
        }
    }
    (records, messages)
}

/// What a forwarding call came to, for the log and the panel.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    /// The partner's `WELCOME` `online`.
    pub online: Option<bool>,
    /// Messages passed on.
    pub passed: usize,
    /// Records the partner took.
    pub records: usize,
    /// Forward items held as copies placed ahead.
    pub held: usize,
    /// What went wrong, one line each.
    pub problems: Vec<String>,
    /// The flags each listed forward item came with.
    listed: HashMap<ContentId, u64>,
}

impl Outcome {
    /// What one event of a forwarding call to `partner` does to the store at `now` (RP-7 §3.5,
    /// §4.3).
    pub fn apply(&mut self, s: &mut Store, partner: &str, event: &Event, now: u64) {
        match event {
            Event::Welcome { online } => {
                self.online = Some(*online);
                s.partners.entry(partner.to_owned()).or_default().online = Some(*online);
            }
            Event::Listed(entries) => {
                for e in entries {
                    self.listed.insert(e.id, e.flags);
                }
            }
            Event::Delivered(id)
            | Event::NotTaken {
                id,
                code: ErrorCode::AlreadyHave,
            } => {
                let id = id.text();
                for r in &mut s.records {
                    r.messages.retain(|(m, _)| *m != id);
                }
                s.passed_on.insert(id, now);
                self.passed += 1;
            }
            Event::Records(ids) => {
                let state = s.partners.entry(partner.to_owned()).or_default();
                for r in s
                    .records
                    .iter()
                    .filter(|r| r.id().is_some_and(|i| ids.contains(&i)))
                {
                    if let Some(h) = hash_hex(&r.credential)
                        && !state.holds.contains(&h)
                    {
                        state.holds.push(h);
                    }
                }
                let before = s.records.len();
                s.records
                    .retain(|r| !r.id().is_some_and(|i| ids.contains(&i)));
                self.records += before - s.records.len();
            }
            Event::Placed {
                serial,
                placed,
                message,
                air,
            } => {
                let state = s.partners.entry(partner.to_owned()).or_default();
                if let Some(n) = serial {
                    state.cursor = state.cursor.max(*n);
                }
                self.hold(s, placed, message, air, now);
            }
            Event::NotTaken { id, code } => self.problems.push(format!(
                "{partner} did not take {}: {code}",
                &id.text()[..6]
            )),
            Event::Failed { id, code } => self.problems.push(format!(
                "a forward item, {}, was refused: {code}",
                &id.text()[..6]
            )),
            Event::Error { code, text } => self.problems.push(format!(
                "{partner} said {code}{}",
                text.as_deref()
                    .map(|t| format!(": {t}"))
                    .unwrap_or_default()
            )),
            Event::Refused(code) => self.problems.push(format!("refused {partner}: {code}")),
            _ => {}
        }
    }

    /// A forward item, held as a copy placed ahead for its user (RP-7 §4.3, RP-5 §5.8): its user
    /// is local here from now (RP-7 §4.4).
    fn hold(
        &mut self,
        s: &mut Store,
        placed: &radiopost::forwarding::Placed,
        message: &Message,
        air: &[u8],
        now: u64,
    ) {
        let address = placed.address.to_lowercase();
        let id = message.content_id();
        if !s
            .copies
            .iter()
            .any(|c| c.address == address && c.id == id.text())
        {
            let flags = self.listed.get(&id).copied().unwrap_or(0);
            s.copies.push(Copy {
                address: address.clone(),
                serial: placed.serial,
                id: id.text(),
                from: message.from().unwrap_or("").to_owned(),
                subject: message
                    .map()
                    .get(message::key::SUBJECT)
                    .and_then(cbor::Value::as_text)
                    .unwrap_or("")
                    .to_owned(),
                size: air.len() as u64,
                priority: message
                    .map()
                    .get(message::key::PRIORITY)
                    .and_then(cbor::Value::as_u64)
                    .unwrap_or(0),
                third_party: flags & listed::THIRD_PARTY != 0,
                air: Some(base64url::encode(air)),
                expires: placed.expires,
                by: address
                    .rsplit_once('@')
                    .map_or(String::new(), |(_, d)| d.to_owned()),
                picked: false,
            });
        }
        s.copies_seen.insert(address, now);
        self.held += 1;
    }

    /// One line saying what the call did.
    #[must_use]
    pub fn summary(&self, partner: &str) -> String {
        let n =
            |k: usize, one: &str, many: &str| format!("{k} {}", if k == 1 { one } else { many });
        let mut line = format!(
            "forwarded to {partner}: {}, {} passed on, {} held for this gateway's users",
            n(self.records, "record", "records"),
            n(self.passed, "message", "messages"),
            n(self.held, "message", "messages"),
        );
        if self.online == Some(false) {
            line.push_str("; ");
            line.push_str(partner);
            line.push_str(" is cut off too, and forwards them in turn");
        }
        line
    }
}
