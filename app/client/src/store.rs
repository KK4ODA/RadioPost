// SPDX-License-Identifier: MIT OR Apache-2.0
//! The client's home: the device key and credential (RP-1), the post offices it trusts (RP-1 §7),
//! its messages (RP-2) and where each stands, and what it keeps between sessions (RP-3).
//!
//! ```text
//! <home>/device.key      the device's Ed25519 seed, hex
//! <home>/credential      the credential, as the post office issued it (COSE_Sign1)
//! <home>/trust.json      post office keys by domain
//! <home>/state.json      the folders, the next serial, the replay cache, what each gateway holds,
//!                        and the station-to-station sessions still to report (RP-5 §6.6)
//! <home>/messages/<id>   each message's canonical encoding, named by its content ID
//! <home>/partials/<id>   the start of a message whose link broke, offered next time (RP-3 §7)
//! ```
//!
//! Every write goes to a temporary file first and is renamed into place, so a received message is
//! on disk before the `RECEIPT` that acknowledges it is sent (RP-3 §5.4).

use std::fs;
use std::path::{Path, PathBuf};

use ed25519_dalek::SigningKey;
use radiopost::ContentId;
use radiopost::credential::{self, Body, PostOfficeKey, Trust};
use radiopost::message::Message;
use radiopost::session::Partial;
use serde_json::{Value, json};

/// What went wrong, for the user.
pub type Error = String;

fn hex(bytes: &[u8]) -> String {
    radiopost::cbor::hex(bytes)
}

/// Bytes from hex, or `None`.
#[must_use]
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Writes a file by way of a temporary one, so it is whole or absent.
fn write(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// The client's home: the one given, else `$RADIOPOST_HOME`, else `.radiopost` in the user's home
/// directory. The `radiopost` program and the desktop application share it.
#[must_use]
pub fn home(given: Option<&str>) -> PathBuf {
    if let Some(h) = given
        .map(str::to_owned)
        .or_else(|| std::env::var("RADIOPOST_HOME").ok())
    {
        return PathBuf::from(h);
    }
    let base = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join(".radiopost")
}

/// A message waiting to be sent, with the serial it is offered under to a station that calls
/// (RP-3 §10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waiting {
    /// The message.
    pub id: ContentId,
    /// Its serial in this client's mail.
    pub serial: u64,
}

/// A message that came in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arrived {
    /// The message.
    pub id: ContentId,
    /// The station it came from.
    pub station: String,
    /// When.
    pub at: u64,
    /// Its serial in the mailbox at the post office, when it came from one: what its held
    /// attachments are asked for by (RP-3 §5.6).
    pub serial: Option<u64>,
}

/// The client's home directory.
#[derive(Debug)]
pub struct Store {
    home: PathBuf,
    /// Messages to send, in the order they are offered.
    pub outbox: Vec<Waiting>,
    /// Messages sent: delivered to a gateway, or to every recipient's station.
    pub sent: Vec<ContentId>,
    /// Messages received, in order.
    pub inbox: Vec<Arrived>,
    /// The serial the next message composed takes.
    pub next_serial: u64,
    /// The station called's replay cache: (address, nonce, when seen) (RP-1 §6.3).
    pub seen: Vec<(String, [u8; 8], u64)>,
    /// What each gateway holds of this client: its cursor, and whether it has the credential.
    pub gateways: serde_json::Map<String, Value>,
    /// Station-to-station sessions to report to the post office, oldest first, each as RP-5
    /// §6.6's `sessions` entry.
    pub reports: Vec<Value>,
    /// The post office's API base URL, when it is not to be found in DNS (RP-1 §7.1).
    pub post_office: Option<String>,
    /// Held attachments to ask for on the next call to a gateway (RP-3 §5.6 `files`): each its
    /// message's serial and its `ref`.
    pub files: Vec<(u64, Vec<u8>)>,
}

fn id_list(v: &Value) -> Vec<ContentId> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().and_then(ContentId::from_text))
                .collect()
        })
        .unwrap_or_default()
}

impl Store {
    /// Opens a home, making it if it is not there.
    ///
    /// # Errors
    ///
    /// The directory cannot be made or read, or its state is not what this client writes.
    pub fn open(home: &Path) -> Result<Store, Error> {
        for dir in [
            home.to_owned(),
            home.join("messages"),
            home.join("partials"),
        ] {
            fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
        }
        let mut store = Store {
            home: home.to_owned(),
            outbox: Vec::new(),
            sent: Vec::new(),
            inbox: Vec::new(),
            next_serial: 1,
            seen: Vec::new(),
            gateways: serde_json::Map::new(),
            reports: Vec::new(),
            post_office: None,
            files: Vec::new(),
        };
        let path = home.join("state.json");
        if !path.exists() {
            return Ok(store);
        }
        let text = fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let state: Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        store.outbox = state["outbox"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|w| {
                        Some(Waiting {
                            id: ContentId::from_text(w["id"].as_str()?)?,
                            serial: w["serial"].as_u64()?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        store.sent = id_list(&state["sent"]);
        store.inbox = state["inbox"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| {
                        Some(Arrived {
                            id: ContentId::from_text(x["id"].as_str()?)?,
                            station: x["station"].as_str()?.to_owned(),
                            at: x["at"].as_u64()?,
                            serial: x["serial"].as_u64(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        store.next_serial = state["next_serial"].as_u64().unwrap_or(1);
        store.seen = state["seen"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| {
                        let nonce = unhex(x["nonce"].as_str()?)?.try_into().ok()?;
                        Some((x["address"].as_str()?.to_owned(), nonce, x["at"].as_u64()?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        store.gateways = state["gateways"].as_object().cloned().unwrap_or_default();
        store.reports = state["reports"].as_array().cloned().unwrap_or_default();
        store.post_office = state["post_office"].as_str().map(str::to_owned);
        store.files = state["files"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| Some((x["serial"].as_u64()?, unhex(x["ref"].as_str()?)?)))
                    .collect()
            })
            .unwrap_or_default();
        Ok(store)
    }

    /// The home directory.
    #[must_use]
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Writes the folders and what is kept between sessions.
    ///
    /// # Errors
    ///
    /// The file cannot be written.
    pub fn save(&self) -> Result<(), Error> {
        let state = json!({
            "outbox": self.outbox.iter().map(|w| json!({
                "id": w.id.text(), "serial": w.serial,
            })).collect::<Vec<_>>(),
            "sent": self.sent.iter().map(ContentId::text).collect::<Vec<_>>(),
            "inbox": self.inbox.iter().map(|a| json!({
                "id": a.id.text(), "station": a.station, "at": a.at, "serial": a.serial,
            })).collect::<Vec<_>>(),
            "next_serial": self.next_serial,
            "seen": self.seen.iter().map(|(address, nonce, at)| json!({
                "address": address, "nonce": hex(nonce), "at": at,
            })).collect::<Vec<_>>(),
            "gateways": self.gateways,
            "reports": self.reports,
            "post_office": self.post_office,
            "files": self.files.iter().map(|(serial, r)| json!({
                "serial": serial, "ref": hex(r),
            })).collect::<Vec<_>>(),
        });
        let text = serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?;
        write(&self.home.join("state.json"), text.as_bytes())
    }

    // -- identity --------------------------------------------------------------------------------

    /// Keeps the device's seed.
    ///
    /// # Errors
    ///
    /// The file cannot be written.
    pub fn set_device(&self, key: &SigningKey) -> Result<(), Error> {
        write(
            &self.home.join("device.key"),
            hex(&key.to_bytes()).as_bytes(),
        )
    }

    /// The device key.
    ///
    /// # Errors
    ///
    /// No key yet, or not a key.
    pub fn device(&self) -> Result<SigningKey, Error> {
        let path = self.home.join("device.key");
        let text = fs::read_to_string(&path)
            .map_err(|_| "no device key: run `radiopost key` or `radiopost init`".to_owned())?;
        let seed: [u8; 32] = unhex(&text)
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| format!("{} is not a 32-byte seed in hex", path.display()))?;
        Ok(SigningKey::from_bytes(&seed))
    }

    /// Keeps the credential the post office issued.
    ///
    /// # Errors
    ///
    /// It is not a credential, or not for this device's key, or cannot be written.
    pub fn set_credential(&self, bytes: &[u8]) -> Result<Body, Error> {
        let cred = credential::Credential::decode(bytes)
            .ok_or("that file is not a credential (RP-1 §4)")?;
        if let Ok(key) = self.device()
            && key.verifying_key().to_bytes() != cred.body.device
        {
            return Err("that credential is for another device's key".into());
        }
        write(&self.home.join("credential"), bytes)?;
        Ok(cred.body)
    }

    /// The credential's bytes.
    ///
    /// # Errors
    ///
    /// None yet.
    pub fn credential(&self) -> Result<Vec<u8>, Error> {
        fs::read(self.home.join("credential"))
            .map_err(|_| "no credential yet: run `radiopost init`".to_owned())
    }

    /// The credential's body: the account and callsign this client speaks for.
    ///
    /// # Errors
    ///
    /// None yet, or not a credential.
    pub fn me(&self) -> Result<Body, Error> {
        let bytes = self.credential()?;
        credential::Credential::decode(&bytes)
            .map(|c| c.body)
            .ok_or_else(|| "the credential kept is not one".into())
    }

    // -- trust -----------------------------------------------------------------------------------

    /// The post offices this client trusts (RP-1 §7.3: the operator decides).
    ///
    /// # Errors
    ///
    /// The file is not what this client writes.
    pub fn trust(&self) -> Result<Trust, Error> {
        let path = self.home.join("trust.json");
        let mut trust = Trust::default();
        let Ok(text) = fs::read_to_string(&path) else {
            return Ok(trust);
        };
        let v: Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        for (domain, keys) in v.as_object().into_iter().flatten() {
            for k in keys.as_array().into_iter().flatten() {
                let public = k["public"]
                    .as_str()
                    .and_then(unhex)
                    .and_then(|b| b.try_into().ok())
                    .ok_or_else(|| format!("{}: a key that is not 32 bytes", path.display()))?;
                trust
                    .keys
                    .entry(domain.clone())
                    .or_default()
                    .push(PostOfficeKey {
                        public,
                        not_before: k["not_before"].as_u64().unwrap_or(0),
                        not_after: k["not_after"].as_u64().unwrap_or(u64::MAX),
                    });
            }
        }
        Ok(trust)
    }

    /// Trusts a post office key.
    ///
    /// # Errors
    ///
    /// The file cannot be read or written.
    pub fn add_trust(&self, domain: &str, key: &PostOfficeKey) -> Result<(), Error> {
        let path = self.home.join("trust.json");
        let mut v: Value = fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| json!({}));
        let entry = json!({"public": hex(&key.public), "not_before": key.not_before,
                           "not_after": key.not_after});
        let list = v
            .as_object_mut()
            .ok_or("trust.json is not an object")?
            .entry(domain.to_ascii_lowercase())
            .or_insert_with(|| json!([]));
        if let Some(a) = list.as_array_mut()
            && !a.contains(&entry)
        {
            a.push(entry);
        }
        let text = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
        write(&path, text.as_bytes())
    }

    // -- messages --------------------------------------------------------------------------------

    /// Keeps a message's canonical encoding, named by its content ID.
    ///
    /// # Errors
    ///
    /// It cannot be written.
    pub fn keep(&self, message: &Message) -> Result<ContentId, Error> {
        let id = message.content_id();
        let path = self.home.join("messages").join(id.text());
        if !path.exists() {
            write(&path, message.canonical())?;
        }
        Ok(id)
    }

    /// A message kept, by its content ID.
    ///
    /// # Errors
    ///
    /// Not kept, or not a message.
    pub fn message(&self, id: &ContentId) -> Result<Message, Error> {
        let path = self.home.join("messages").join(id.text());
        let bytes = fs::read(&path).map_err(|_| format!("no message {}", id.text()))?;
        Message::from_canonical(&bytes).map_err(|e| format!("{}: {}", path.display(), e.0))
    }

    /// The content ID a prefix of its text names, among the messages kept.
    ///
    /// # Errors
    ///
    /// None, or more than one, starts so.
    pub fn find(&self, prefix: &str) -> Result<ContentId, Error> {
        let prefix = prefix.to_ascii_lowercase();
        let dir = self.home.join("messages");
        let names: Vec<String> = fs::read_dir(&dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.starts_with(&prefix) && !n.contains('.'))
            .collect();
        match names.as_slice() {
            [one] => ContentId::from_text(one).ok_or_else(|| format!("{one} is not a content ID")),
            [] => Err(format!("no message starts {prefix}")),
            _ => Err(format!("more than one message starts {prefix}")),
        }
    }

    /// Puts a message in the outbox, with the next serial.
    ///
    /// # Errors
    ///
    /// It cannot be kept.
    pub fn queue(&mut self, message: &Message) -> Result<ContentId, Error> {
        let id = self.keep(message)?;
        if !self.outbox.iter().any(|w| w.id == id) {
            self.outbox.push(Waiting {
                id,
                serial: self.next_serial,
            });
            self.next_serial += 1;
        }
        self.save()?;
        Ok(id)
    }

    /// A message the other station now holds. Taken by a gateway, it goes on from there; taken by
    /// a station, it reaches the other recipients when the session is reported (RP-5 §6.6, D30),
    /// so it is not sent over the air again. Either way it is sent.
    pub fn delivered(&mut self, id: &ContentId) {
        self.outbox.retain(|w| w.id != *id);
        if !self.sent.contains(id) {
            self.sent.push(*id);
        }
    }

    /// A message received: kept, and put in the inbox.
    ///
    /// # Errors
    ///
    /// It cannot be kept.
    pub fn received(
        &mut self,
        message: &Message,
        station: &str,
        at: u64,
        serial: Option<u64>,
    ) -> Result<(), Error> {
        let id = self.keep(message)?;
        if !self.inbox.iter().any(|a| a.id == id) {
            self.inbox.push(Arrived {
                id,
                station: station.to_owned(),
                at,
                serial,
            });
        }
        self.save()
    }

    /// The attachments a received message lists as held at the post office (RP-2 key 12): each
    /// its name, type, size and `ref`.
    #[must_use]
    pub fn held(message: &Message) -> Vec<(String, String, u64, Vec<u8>)> {
        message
            .map()
            .get(radiopost::message::key::HELD)
            .and_then(radiopost::cbor::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|f| {
                Some((
                    f.get(1)?.as_text()?.to_owned(),
                    f.get(2)
                        .and_then(radiopost::cbor::Value::as_text)
                        .unwrap_or("")
                        .to_owned(),
                    f.get(3)
                        .and_then(radiopost::cbor::Value::as_u64)
                        .unwrap_or(0),
                    f.get(4)?.as_bytes()?.to_vec(),
                ))
            })
            .collect()
    }

    /// Asks, on the next call to a gateway, for the held attachments of the received message
    /// `id` named in `names` (all of them when `names` is empty): the names asked for.
    ///
    /// # Errors
    ///
    /// The message is not in the inbox, came from no post office, lists no such attachment, or
    /// the state cannot be written.
    pub fn ask_for_held(&mut self, id: &ContentId, names: &[String]) -> Result<Vec<String>, Error> {
        let arrived = self
            .inbox
            .iter()
            .find(|a| a.id == *id)
            .ok_or("that message is not in the inbox")?;
        let serial = arrived
            .serial
            .ok_or("that message came from no post office, so it holds nothing for it")?;
        let held = Store::held(&self.message(id)?);
        if held.is_empty() {
            return Err("that message has no attachments held at the post office".into());
        }
        let mut asked = Vec::new();
        for name in names {
            if !held.iter().any(|(n, ..)| n == name) {
                return Err(format!("that message holds no attachment named {name}"));
            }
        }
        for (name, _, _, r) in held {
            if !names.is_empty() && !names.contains(&name) {
                continue;
            }
            if !self.files.iter().any(|(s, x)| *s == serial && *x == r) {
                self.files.push((serial, r));
            }
            asked.push(name);
        }
        self.save()?;
        Ok(asked)
    }

    /// What will be asked for on the next call to a gateway, a line each: the attachment's name,
    /// size and the message it belongs to.
    #[must_use]
    pub fn files_asked_for(&self) -> Vec<String> {
        self.files
            .iter()
            .map(|(serial, r)| {
                let found = self
                    .inbox
                    .iter()
                    .filter(|a| a.serial == Some(*serial))
                    .find_map(|a| {
                        let m = self.message(&a.id).ok()?;
                        let (name, _, size, _) =
                            Store::held(&m).into_iter().find(|(.., x)| x == r)?;
                        Some(format!("{name}, {size} bytes, of {}", &a.id.text()[..6]))
                    });
                found.unwrap_or_else(|| format!("an attachment of message #{serial}"))
            })
            .collect()
    }

    // -- partials --------------------------------------------------------------------------------

    /// Keeps what a broken link left of a message (RP-3 §7).
    ///
    /// # Errors
    ///
    /// It cannot be written.
    pub fn hold(&self, partial: &Partial) -> Result<(), Error> {
        let mut bytes = json!({"total": partial.total, "serial": partial.serial})
            .to_string()
            .into_bytes();
        bytes.push(b'\n');
        bytes.extend_from_slice(&partial.held);
        write(&self.home.join("partials").join(partial.id.text()), &bytes)
    }

    /// Every partial message held, to offer under `partials` (RP-3 §5.1).
    #[must_use]
    pub fn partials(&self) -> Vec<Partial> {
        let dir = self.home.join("partials");
        let Ok(entries) = fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut out: Vec<Partial> = entries
            .filter_map(|e| {
                let e = e.ok()?;
                let id = ContentId::from_text(e.file_name().to_str()?)?;
                let bytes = fs::read(e.path()).ok()?;
                let cut = bytes.iter().position(|&b| b == b'\n')?;
                let head: Value = serde_json::from_slice(&bytes[..cut]).ok()?;
                Some(Partial {
                    id,
                    held: bytes[cut + 1..].to_vec(),
                    total: head["total"].as_u64()?,
                    serial: head["serial"].as_u64(),
                })
            })
            .collect();
        out.sort_by_key(|p| p.id.text());
        out
    }

    /// Lets a partial go: it arrived whole, or will not.
    pub fn drop_partial(&self, id: &ContentId) {
        let _ = fs::remove_file(self.home.join("partials").join(id.text()));
    }

    // -- conformance mode ---------------------------------------------------------------------------

    /// The test adapter, when the client is in conformance mode (the conformance tester's outline,
    /// §2.3): it then reaches no other modem, so the published test keys never reach the air.
    #[must_use]
    pub fn conformance(&self) -> Option<String> {
        fs::read_to_string(self.home.join("conformance"))
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    }

    /// Enters conformance mode with the test adapter at `adapter`, or leaves it with `None`.
    ///
    /// # Errors
    ///
    /// The file cannot be written or removed.
    pub fn set_conformance(&self, adapter: Option<&str>) -> Result<(), Error> {
        let path = self.home.join("conformance");
        match adapter {
            Some(a) => write(&path, a.as_bytes()),
            None => match fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("{}: {e}", path.display())),
            },
        }
    }

    // -- the replay cache and gateways -------------------------------------------------------------

    /// The (address, nonce) pairs seen within RP-1 §6.3's twenty minutes of `now`.
    #[must_use]
    pub fn replay_cache(&self, now: u64) -> Vec<(String, [u8; 8])> {
        self.seen
            .iter()
            .filter(|(_, _, at)| now.saturating_sub(*at) < 20 * 60)
            .map(|(a, n, _)| (a.clone(), *n))
            .collect()
    }

    /// A statement accepted: into the cache, and the cache's old entries out.
    pub fn saw(&mut self, address: &str, nonce: [u8; 8], now: u64) {
        self.seen
            .retain(|(_, _, at)| now.saturating_sub(*at) < 20 * 60);
        self.seen.push((address.to_owned(), nonce, now));
    }

    /// What a gateway holds of this client: its waiting list's cursor, and whether it keeps the
    /// credential, so its hash will do (RP-3 §5.1).
    #[must_use]
    pub fn gateway(&self, callsign: &str) -> (u64, bool) {
        let g = &self.gateways.get(callsign);
        (
            g.and_then(|g| g["cursor"].as_u64()).unwrap_or(0),
            g.and_then(|g| g["credential_cached"].as_bool())
                .unwrap_or(false),
        )
    }

    /// Remembers what a gateway now holds.
    pub fn set_gateway(&mut self, callsign: &str, cursor: u64, credential_cached: bool) {
        self.gateways.insert(
            callsign.to_owned(),
            json!({"cursor": cursor, "credential_cached": credential_cached}),
        );
    }
}
