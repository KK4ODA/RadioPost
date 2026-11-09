// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the tests share: the vector files, and RP-3's engines as the client in session-01 and the
//! two stations of session-03, each behind RPMI's [`Engine`].

#![allow(dead_code, reason = "each test file uses part of it")]

use std::collections::HashMap;
use std::path::PathBuf;

use ed25519_dalek::SigningKey;
use radiopost::credential::{PostOfficeKey, Trust};
use radiopost::message::{Compression, Message};
use radiopost::session::called::{Called, CalledConfig};
use radiopost::session::client::{Client, ClientConfig};
use radiopost::session::{Caps, Event, Outgoing};
use rpmi::host::Engine;
use serde_json::Value as Json;

pub fn vectors(name: &str) -> Json {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vectors")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).expect("a vector file")).expect("JSON")
}

pub fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

/// Keys, credentials and messages from the other vector files.
pub struct World {
    seeds: HashMap<String, [u8; 32]>,
    pub trust: Trust,
    pub credentials: HashMap<String, Vec<u8>>,
    pub messages: HashMap<String, Message>,
}

pub fn world() -> World {
    let mut seeds = HashMap::new();
    let mut trust = Trust::default();
    for k in vectors("keys.json")["keys"].as_array().expect("keys") {
        let label = k["label"].as_str().expect("label").to_owned();
        let seed: [u8; 32] = unhex(k["seed_hex"].as_str().expect("seed"))
            .try_into()
            .expect("32");
        if let Some(rest) = label.strip_prefix("po:") {
            let domain = rest.split(':').next().unwrap_or(rest).to_owned();
            trust.keys.entry(domain).or_default().push(PostOfficeKey {
                public: SigningKey::from_bytes(&seed).verifying_key().to_bytes(),
                not_before: k["not_before"].as_u64().expect("validity"),
                not_after: k["not_after"].as_u64().expect("validity"),
            });
        }
        seeds.insert(label, seed);
    }
    let credentials = vectors("credentials.json")["credentials"]
        .as_array()
        .expect("credentials")
        .iter()
        .map(|c| {
            (
                c["name"].as_str().expect("name").to_owned(),
                unhex(c["credential_hex"].as_str().expect("hex")),
            )
        })
        .collect();
    let messages = vectors("messages.json")["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|m| {
            let canonical = unhex(m["canonical_hex"].as_str().expect("hex"));
            (
                m["name"].as_str().expect("name").to_owned(),
                Message::from_canonical(&canonical).expect("a message"),
            )
        })
        .collect();
    World {
        seeds,
        trust,
        credentials,
        messages,
    }
}

impl World {
    pub fn device(&self, label: &str) -> SigningKey {
        SigningKey::from_bytes(&self.seeds[label])
    }

    /// The session vectors send uncompressed, so that whole sessions are byte for byte.
    pub fn out(&self, name: &str) -> Outgoing {
        Outgoing::new(self.messages[name].clone(), Compression::None)
    }
}

fn caps() -> Caps {
    Caps {
        compression: vec![0, 1, 2],
        max_pdu: 4096,
        renew: false,
    }
}

/// The transmissions of a session vector, in order: who wrote each, and its bytes.
pub fn transmissions(session: &Json) -> Vec<(String, Vec<u8>)> {
    session["transmissions"]
        .as_array()
        .expect("transmissions")
        .iter()
        .map(|t| {
            (
                t["from"].as_str().expect("from").to_owned(),
                unhex(t["bytes_hex"].as_str().expect("hex")),
            )
        })
        .collect()
}

fn nonce_of(hello: &[u8]) -> [u8; 8] {
    let pdu = radiopost::pdu::unframe(hello).expect("a HELLO");
    pdu.get(7)
        .and_then(radiopost::cbor::Value::as_bytes)
        .expect("a nonce")
        .try_into()
        .expect("8 bytes")
}

/// Facundo's client in session-01: he asks for #420 and leaves the photo waiting.
pub fn session_01_client(w: &World) -> Client {
    let file = vectors("session-01.json");
    let cfg = ClientConfig {
        credential: w.credentials["facundo"].clone(),
        device: w.device("device:facundo"),
        called: "N1CALL-10".into(),
        time: file["context"]["client_clock"].as_u64().expect("clock"),
        nonce: nonce_of(&transmissions(&file)[0].1),
        outbox: vec![w.out("facundo-reply"), w.out("facundo-to-ana")],
        cursor: 418,
        auto: 1000,
        partials: vec![],
        caps: caps(),
        credential_cached: true,
        piece: 4096,
        station: None,
    };
    Client::new(cfg, Box::new(|_| vec![420])).expect("a client")
}

/// Facundo calling Ana's station in session-03.
pub fn session_03_caller(w: &World) -> Client {
    let file = vectors("session-03.json");
    let cfg = ClientConfig {
        credential: w.credentials["facundo"].clone(),
        device: w.device("device:facundo"),
        called: "N0CALL".into(),
        time: file["context"]["caller_clock"].as_u64().expect("clock"),
        nonce: nonce_of(&transmissions(&file)[0].1),
        outbox: vec![w.out("facundo-direct"), w.out("facundo-to-eoc")],
        cursor: 0,
        auto: 1000,
        partials: vec![],
        caps: caps(),
        credential_cached: false,
        piece: 4096,
        station: Some(w.trust.clone()),
    };
    Client::new(cfg, Box::new(|_| vec![])).expect("a client")
}

/// Ana's station in session-03, with a note for Facundo.
pub fn session_03_called(w: &World) -> Called {
    let file = vectors("session-03.json");
    let cfg = CalledConfig {
        credential: w.credentials["ana"].clone(),
        device: w.device("device:ana"),
        time: file["context"]["called_clock"].as_u64().expect("clock"),
        session_id: unhex(file["context"]["session_id_hex"].as_str().expect("id"))
            .try_into()
            .expect("8"),
        challenge: [7; 16],
        trust: w.trust.clone(),
        seen: vec![],
        caps: caps(),
        piece: 4096,
    };
    let note = w.out("ana-to-facundo");
    let mail_for = Box::new(move |address: &str| {
        if address == "facundo@radiopost.org" {
            vec![(1, note.clone())]
        } else {
            vec![]
        }
    });
    Called::new(cfg, mail_for).expect("a station")
}

/// The caller's engine on a link: it speaks first.
pub struct Calling(pub Client);

impl Engine for Calling {
    type Event = Event;
    fn opening(&mut self) -> Option<Vec<u8>> {
        Some(self.0.hello())
    }
    fn receive(&mut self, bytes: &[u8]) -> Vec<Event> {
        self.0.receive(bytes)
    }
    fn transmit(&mut self) -> Option<Vec<u8>> {
        self.0.transmit()
    }
    fn is_over(&self) -> bool {
        self.0.is_over()
    }
    fn link_lost(&mut self) -> Vec<Event> {
        self.0.link_broken()
    }
}

/// The station called: it waits for `HELLO`, and the caller hangs up.
pub struct Answering(pub Called);

impl Engine for Answering {
    type Event = Event;
    fn opening(&mut self) -> Option<Vec<u8>> {
        None
    }
    fn receive(&mut self, bytes: &[u8]) -> Vec<Event> {
        self.0.receive(bytes)
    }
    fn transmit(&mut self) -> Option<Vec<u8>> {
        self.0.transmit()
    }
    fn is_over(&self) -> bool {
        false
    }
    fn link_lost(&mut self) -> Vec<Event> {
        self.0.link_closed()
    }
}

/// Content IDs of the messages an engine received and delivered.
pub fn received(events: &[Event]) -> Vec<radiopost::ContentId> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Received { message, .. } => Some(message.content_id()),
            _ => None,
        })
        .collect()
}

pub fn delivered(events: &[Event]) -> Vec<radiopost::ContentId> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Delivered(id) => Some(*id),
            _ => None,
        })
        .collect()
}
