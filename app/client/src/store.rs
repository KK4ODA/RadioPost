// SPDX-License-Identifier: MIT OR Apache-2.0
//! The client's home: the device key and credential (RP-1), the post offices it trusts (RP-1 §7),
//! its messages (RP-2) and where each stands, and what it keeps between sessions (RP-3).
//!
//! ```text
//! <home>/device.key      the device's Ed25519 seed, hex
//! <home>/credential      the credential, as the post office issued it (COSE_Sign1)
//! <home>/trust.json      post office keys by domain
//! <home>/state.json      the folders and the trash, the next serial, the replay cache, what each
//!                        gateway holds, the station-to-station sessions still to report (RP-5
//!                        §6.6), the gateways the post office lists, and what waited at the last
//!                        call
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

/// A folder a message can be deleted from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Folder {
    /// Messages received.
    Inbox,
    /// Messages to send.
    Outbox,
    /// Messages sent.
    Sent,
}

impl Folder {
    /// Its name, as the panel and the state file write it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Folder::Inbox => "inbox",
            Folder::Outbox => "outbox",
            Folder::Sent => "sent",
        }
    }

    /// The folder of that name.
    ///
    /// # Errors
    ///
    /// No folder messages are deleted from has that name.
    pub fn named(name: &str) -> Result<Folder, Error> {
        match name {
            "inbox" => Ok(Folder::Inbox),
            "outbox" => Ok(Folder::Outbox),
            "sent" => Ok(Folder::Sent),
            _ => Err(format!("no folder {name} to delete from")),
        }
    }
}

/// Where a message in the trash was, with what it needs to go back there as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Was {
    /// The inbox, with its arrival.
    Inbox(Arrived),
    /// The outbox, with the serial it is offered under.
    Outbox(Waiting),
    /// Sent.
    Sent(ContentId),
}

impl Was {
    /// The folder it was in.
    #[must_use]
    pub fn folder(&self) -> Folder {
        match self {
            Was::Inbox(_) => Folder::Inbox,
            Was::Outbox(_) => Folder::Outbox,
            Was::Sent(_) => Folder::Sent,
        }
    }

    /// The message.
    #[must_use]
    pub fn id(&self) -> ContentId {
        match self {
            Was::Inbox(a) => a.id,
            Was::Outbox(w) => w.id,
            Was::Sent(id) => *id,
        }
    }
}

/// A message in the trash: deleted here only, never at the post office or another station.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trashed {
    /// Where it was.
    pub was: Was,
    /// When it was deleted.
    pub at: u64,
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
    /// Messages deleted, in the order they were: they go back, or go for good from here.
    pub trash: Vec<Trashed>,
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
    /// The gateways the post office lists (RP-5 §4.4), kept for when there is no Internet:
    /// `{"domain", "fetched", "gateways": […]}`.
    pub gateway_list: Option<Value>,
    /// The operator's Maidenhead grid square, to rank gateways by distance.
    pub grid: Option<String>,
    /// What the post office listed at the last call to a gateway, less what came: `{"via", "at",
    /// "entries": […]}`, each entry `serial`, `id`, `from`, `subject`, `size`, `priority`, `held`,
    /// `third_party`.
    pub waiting: Option<Value>,
    /// Serials to ask for on the next call to a gateway, whatever the fetch rule says.
    pub fetch_next: Vec<u64>,
}

/// A trash entry as the state file writes it: `id`, `was` (the folder), `deleted`, and the
/// inbox's `station`, `at` and `serial` or the outbox's `serial`.
fn trashed_json(t: &Trashed) -> Value {
    let mut v = json!({"id": t.was.id().text(), "was": t.was.folder().name(), "deleted": t.at});
    match &t.was {
        Was::Inbox(a) => {
            v["station"] = a.station.clone().into();
            v["at"] = a.at.into();
            v["serial"] = a.serial.into();
        }
        Was::Outbox(w) => v["serial"] = w.serial.into(),
        Was::Sent(_) => {}
    }
    v
}

fn trashed_from(v: &Value) -> Option<Trashed> {
    let id = ContentId::from_text(v["id"].as_str()?)?;
    let was = match Folder::named(v["was"].as_str()?).ok()? {
        Folder::Inbox => Was::Inbox(Arrived {
            id,
            station: v["station"].as_str()?.to_owned(),
            at: v["at"].as_u64()?,
            serial: v["serial"].as_u64(),
        }),
        Folder::Outbox => Was::Outbox(Waiting {
            id,
            serial: v["serial"].as_u64()?,
        }),
        Folder::Sent => Was::Sent(id),
    };
    Some(Trashed {
        was,
        at: v["deleted"].as_u64()?,
    })
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
            trash: Vec::new(),
            next_serial: 1,
            seen: Vec::new(),
            gateways: serde_json::Map::new(),
            reports: Vec::new(),
            post_office: None,
            files: Vec::new(),
            gateway_list: None,
            grid: None,
            waiting: None,
            fetch_next: Vec::new(),
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
        store.trash = state["trash"]
            .as_array()
            .map(|a| a.iter().filter_map(trashed_from).collect())
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
        store.gateway_list = Some(state["gateway_list"].clone()).filter(Value::is_object);
        store.grid = state["grid"].as_str().map(str::to_owned);
        store.waiting = Some(state["waiting"].clone()).filter(Value::is_object);
        store.fetch_next = state["fetch_next"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_u64).collect())
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
            "trash": self.trash.iter().map(trashed_json).collect::<Vec<_>>(),
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
            "gateway_list": self.gateway_list,
            "grid": self.grid,
            "waiting": self.waiting,
            "fetch_next": self.fetch_next,
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

    // -- the trash --------------------------------------------------------------------------------

    /// Moves a message from a folder to the trash, at `now`. It is deleted here only: the post
    /// office, a gateway and the other stations keep what they hold.
    ///
    /// # Errors
    ///
    /// It is not in that folder, or the state cannot be written.
    pub fn delete(&mut self, folder: Folder, id: &ContentId, now: u64) -> Result<(), Error> {
        let missing = || format!("that message is not in the {}", folder.name());
        let was = match folder {
            Folder::Inbox => {
                let at = self
                    .inbox
                    .iter()
                    .position(|a| a.id == *id)
                    .ok_or_else(missing)?;
                Was::Inbox(self.inbox.remove(at))
            }
            Folder::Outbox => {
                let at = self
                    .outbox
                    .iter()
                    .position(|w| w.id == *id)
                    .ok_or_else(missing)?;
                Was::Outbox(self.outbox.remove(at))
            }
            Folder::Sent => {
                let at = self.sent.iter().position(|s| s == id).ok_or_else(missing)?;
                Was::Sent(self.sent.remove(at))
            }
        };
        self.trash.push(Trashed { was, at: now });
        self.save()
    }

    fn trashed_at(&self, id: &ContentId, folder: Folder) -> Result<usize, Error> {
        self.trash
            .iter()
            .position(|t| t.was.id() == *id && t.was.folder() == folder)
            .ok_or_else(|| {
                format!(
                    "that message from the {} is not in the trash",
                    folder.name()
                )
            })
    }

    /// Puts a message in the trash back in the folder it was deleted from, where it was in it: an
    /// outbox message keeps its serial and its place in the order messages are offered.
    ///
    /// # Errors
    ///
    /// It is not in the trash from that folder, or the state cannot be written.
    pub fn restore(&mut self, id: &ContentId, folder: Folder) -> Result<(), Error> {
        let at = self.trashed_at(id, folder)?;
        match self.trash.remove(at).was {
            Was::Inbox(a) => {
                if !self.inbox.iter().any(|x| x.id == a.id) {
                    let place = self.inbox.partition_point(|x| x.at <= a.at);
                    self.inbox.insert(place, a);
                }
            }
            Was::Outbox(w) => {
                if !self.outbox.iter().any(|x| x.id == w.id) {
                    let place = self.outbox.partition_point(|x| x.serial < w.serial);
                    self.outbox.insert(place, w);
                }
            }
            Was::Sent(id) => {
                if !self.sent.contains(&id) {
                    self.sent.push(id);
                }
            }
        }
        self.save()
    }

    /// Deletes a message in the trash for good: its file goes once no folder holds it, and what
    /// was asked for its held attachments is no longer asked.
    ///
    /// # Errors
    ///
    /// It is not in the trash from that folder, or the state cannot be written.
    pub fn purge(&mut self, id: &ContentId, folder: Folder) -> Result<(), Error> {
        let at = self.trashed_at(id, folder)?;
        let gone = self.trash.remove(at);
        self.forget(&gone);
        self.save()
    }

    /// Empties the trash: how many messages it held.
    ///
    /// # Errors
    ///
    /// The state cannot be written.
    pub fn empty_trash(&mut self) -> Result<usize, Error> {
        let gone = std::mem::take(&mut self.trash);
        for t in &gone {
            self.forget(t);
        }
        self.save()?;
        Ok(gone.len())
    }

    /// What a message deleted for good leaves behind: its file, when nothing else holds it, and
    /// any request for its held attachments.
    fn forget(&mut self, gone: &Trashed) {
        let id = gone.was.id();
        if let Was::Inbox(Arrived {
            serial: Some(serial),
            ..
        }) = gone.was
            && !self.inbox.iter().any(|a| a.serial == Some(serial))
        {
            self.files.retain(|(s, _)| *s != serial);
        }
        let held = self.inbox.iter().any(|a| a.id == id)
            || self.outbox.iter().any(|w| w.id == id)
            || self.sent.contains(&id)
            || self.trash.iter().any(|t| t.was.id() == id);
        if !held {
            let _ = fs::remove_file(self.home.join("messages").join(id.text()));
        }
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

    // -- gateways and what waits ---------------------------------------------------------------

    /// The gateways kept from the post office's list, each with `km`, its distance from the
    /// operator's grid square when both are known, nearest first; the others after, as listed.
    #[must_use]
    pub fn gateways_ranked(&self) -> Vec<Value> {
        let mut list: Vec<Value> = self
            .gateway_list
            .as_ref()
            .and_then(|l| l["gateways"].as_array())
            .cloned()
            .unwrap_or_default();
        for g in &mut list {
            let km = match (&self.grid, g["grid"].as_str()) {
                (Some(mine), Some(theirs)) => crate::grid::km(mine, theirs),
                _ => None,
            };
            g["km"] = km.map_or(Value::Null, |k| json!(k.round()));
        }
        list.sort_by(|a, b| match (a["km"].as_f64(), b["km"].as_f64()) {
            (Some(x), Some(y)) => x.total_cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        list
    }

    /// Sets the operator's grid square, or clears it.
    ///
    /// # Errors
    ///
    /// It is not a grid square of four or six characters, or the state cannot be written.
    pub fn set_grid(&mut self, grid: Option<&str>) -> Result<(), Error> {
        self.grid = match grid.map(str::trim).filter(|g| !g.is_empty()) {
            None => None,
            Some(g) if crate::grid::centre(g).is_some() => {
                let (field, rest) = g.split_at(2);
                let (square, sub) = rest.split_at(2);
                Some(format!(
                    "{}{square}{}",
                    field.to_uppercase(),
                    sub.to_lowercase()
                ))
            }
            Some(g) => return Err(format!("{g} is not a grid square, such as EM73 or EM73tt")),
        };
        self.save()
    }

    /// What waited at the last call to a gateway, each entry with `fetch` set when it is asked
    /// for on the next call.
    #[must_use]
    pub fn waiting_entries(&self) -> Vec<Value> {
        self.waiting
            .as_ref()
            .and_then(|w| w["entries"].as_array())
            .into_iter()
            .flatten()
            .map(|e| {
                let mut e = e.clone();
                let marked = e["serial"]
                    .as_u64()
                    .is_some_and(|s| self.fetch_next.contains(&s));
                e["fetch"] = marked.into();
                e
            })
            .collect()
    }

    /// Asks, or stops asking, for a message that waited at the last call, on the next call to a
    /// gateway, whatever the fetch rule says.
    ///
    /// # Errors
    ///
    /// No such message waited, or the state cannot be written.
    pub fn fetch_on_next_call(&mut self, serial: u64, on: bool) -> Result<(), Error> {
        if !self
            .waiting_entries()
            .iter()
            .any(|e| e["serial"].as_u64() == Some(serial))
        {
            return Err(format!("no message #{serial} waited at the last call"));
        }
        self.fetch_next.retain(|s| *s != serial);
        if on {
            self.fetch_next.push(serial);
        }
        self.save()
    }

    /// What a call to `gateway` at `at` found waiting (RP-5 §5.2's entries), less what came in
    /// it: kept as the waiting list, and the serials that came no longer asked for.
    pub fn listed(
        &mut self,
        gateway: &str,
        at: u64,
        entries: &[radiopost::session::WaitingEntry],
        came: &[u64],
    ) {
        use radiopost::session::flags;
        let left: Vec<Value> = entries
            .iter()
            .filter(|e| !came.contains(&e.serial))
            .map(|e| {
                json!({
                    "serial": e.serial, "id": e.id.text(), "from": e.from, "subject": e.subject,
                    "size": e.size, "priority": e.priority,
                    "held": e.flags & flags::HELD != 0,
                    "third_party": e.flags & flags::THIRD_PARTY != 0,
                })
            })
            .collect();
        let still: Vec<u64> = left.iter().filter_map(|e| e["serial"].as_u64()).collect();
        self.fetch_next.retain(|s| still.contains(s));
        self.waiting = Some(json!({"via": gateway, "at": at, "entries": left}));
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::Store;

    /// The gateways nearest first from the operator's grid square; those with no grid after, in
    /// the post office's order; and the grid kept in its usual form.
    #[test]
    fn gateways_nearest_first() {
        let home = std::env::temp_dir().join(format!("radiopost-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let mut store = Store::open(&home).expect("a home");
        store.gateway_list = Some(
            json!({"domain": "radiopost.org", "fetched": 1, "gateways": [
                {"callsign": "N1FAR-1", "grid": "IO91"},
                {"callsign": "N2NONE-1"},
                {"callsign": "KK4ODA-1", "grid": "EM73"},
                {"callsign": "W4MID-1", "grid": "FM18"},
            ]}),
        );
        let order = |s: &Store| -> Vec<String> {
            s.gateways_ranked()
                .iter()
                .map(|g| g["callsign"].as_str().expect("a callsign").to_owned())
                .collect()
        };
        assert_eq!(
            order(&store),
            ["N1FAR-1", "N2NONE-1", "KK4ODA-1", "W4MID-1"],
            "no grid: as listed"
        );
        store.set_grid(Some("em73TT")).expect("a grid square");
        assert_eq!(store.grid.as_deref(), Some("EM73tt"));
        assert_eq!(
            order(&store),
            ["KK4ODA-1", "W4MID-1", "N1FAR-1", "N2NONE-1"]
        );
        let ranked = store.gateways_ranked();
        assert!(
            ranked[0]["km"].as_f64().is_some_and(|k| k < 150.0),
            "{ranked:?}"
        );
        assert!(ranked[3]["km"].is_null());
        assert!(store.set_grid(Some("XX99")).is_err());
        let again = Store::open(&home).expect("the home");
        assert_eq!(again.grid.as_deref(), Some("EM73tt"), "kept");
        store.set_grid(None).expect("cleared");
        assert_eq!(store.grid, None);
        let _ = std::fs::remove_dir_all(&home);
    }

    fn message(subject: &str, date: u64) -> radiopost::message::Message {
        crate::compose::Draft {
            from: "ana@hampost.org".into(),
            to: vec!["facundo@radiopost.org".into()],
            cc: Vec::new(),
            date,
            subject: subject.into(),
            body: "73".into(),
            files: Vec::new(),
            priority: 0,
            reply_to: None,
        }
        .message()
        .expect("a message")
    }

    /// A message deleted from any folder goes to the trash and back to its place; deleted for
    /// good, its file goes only when no folder holds it, with the request for its held
    /// attachments; and the trash outlives the program.
    #[test]
    fn the_trash() {
        use super::Folder;
        let home = std::env::temp_dir().join(format!("radiopost-trash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let mut store = Store::open(&home).expect("a home");
        let (a, b, c) = (message("a", 1), message("b", 2), message("c", 3));
        let ids: Vec<_> = [&a, &b, &c]
            .iter()
            .map(|m| store.queue(m).expect("queued"))
            .collect();
        store
            .received(&a, "KK4ODA-1", 10, Some(7))
            .expect("received");
        store
            .received(&b, "KK4ODA-1", 20, Some(8))
            .expect("received");
        store.files.push((7, vec![1]));
        store.delivered(&ids[2]);
        let file = |id: &radiopost::ContentId| home.join("messages").join(id.text());

        // the outbox's first message goes and comes back in its place, first, with its serial
        store.delete(Folder::Outbox, &ids[0], 100).expect("deleted");
        assert_eq!(store.outbox.len(), 1);
        assert!(
            store.delete(Folder::Outbox, &ids[0], 100).is_err(),
            "gone from there"
        );
        store.restore(&ids[0], Folder::Outbox).expect("restored");
        let serials: Vec<u64> = store.outbox.iter().map(|w| w.serial).collect();
        assert_eq!(serials, [1, 2], "back in its place");

        // the inbox's first, the same message as the outbox's first: its file stays while the
        // outbox holds it, and the attachments asked for its serial are no longer asked
        store.delete(Folder::Inbox, &ids[0], 101).expect("deleted");
        store.delete(Folder::Sent, &ids[2], 102).expect("deleted");
        let again = Store::open(&home).expect("the home");
        assert_eq!(again.trash, store.trash, "kept in the state file");
        assert_eq!(again.trash[0].at, 101);
        store.purge(&ids[0], Folder::Inbox).expect("purged");
        assert!(file(&ids[0]).exists(), "the outbox still holds it");
        assert!(
            store.files.is_empty(),
            "nothing asked for a message deleted for good"
        );
        assert!(
            store.restore(&ids[0], Folder::Inbox).is_err(),
            "gone for good"
        );

        // restored to the inbox, a message goes back by when it came
        store.delete(Folder::Inbox, &ids[1], 103).expect("deleted");
        store.restore(&ids[1], Folder::Inbox).expect("restored");
        assert_eq!(store.inbox.iter().map(|x| x.at).collect::<Vec<_>>(), [20]);

        // emptied: the sent message held nowhere else, its file goes
        assert_eq!(store.empty_trash().expect("emptied"), 1);
        assert_eq!(store.trash.len(), 0, "{:?}", store.trash);
        assert!(!file(&ids[2]).exists(), "held by no folder");
        assert!(file(&ids[1]).exists());
        let _ = std::fs::remove_dir_all(&home);
    }
}
