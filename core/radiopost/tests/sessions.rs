// SPDX-License-Identifier: MIT OR Apache-2.0
//! The session engines against the session vectors (vectors/README.md, "Replay"): each engine
//! plays one side, is fed the other side's bytes from the file, and must write its own side's
//! bytes exactly. The inputs are the vectors' own: the clock, the nonce, the outbox, the cursor,
//! `auto`, and what the user chose to ask for.

use std::collections::HashMap;
use std::path::PathBuf;

use ed25519_dalek::SigningKey;
use radiopost::cbor::{self, Value};
use radiopost::credential::{PostOfficeKey, Trust};
use radiopost::message::{Compression, Message};
use radiopost::session::called::{Called, CalledConfig};
use radiopost::session::client::{Client, ClientConfig};
use radiopost::session::{Caps, Event, Outgoing, Partial};
use radiopost::{ContentId, ErrorCode};
use serde_json::Value as Json;

fn vectors(name: &str) -> Json {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vectors")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).expect("a vector file")).expect("JSON")
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

/// What the replays need from the other vector files: keys, credentials, messages.
struct World {
    seeds: HashMap<String, [u8; 32]>,
    trust: Trust,
    credentials: HashMap<String, Vec<u8>>,
    messages: HashMap<String, Message>,
}

fn world() -> World {
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
    fn device(&self, label: &str) -> SigningKey {
        SigningKey::from_bytes(&self.seeds[label])
    }

    /// The session vectors send uncompressed, so that whole sessions are byte for byte.
    fn out(&self, name: &str) -> Outgoing {
        Outgoing::new(self.messages[name].clone(), Compression::None)
    }
}

/// `CLIENT_CAPS` of the vectors: compressions 0, 1, 2; PDUs up to 4 096 bytes.
fn client_caps() -> Caps {
    Caps {
        compression: vec![0, 1, 2],
        max_pdu: 4096,
        renew: false,
    }
}

const AUTO: u64 = 1000; // the client's `auto` in every session vector

/// A transmission: who wrote it, its bytes, and how many the other side received.
struct Turn {
    from: String,
    bytes: Vec<u8>,
    received: usize,
}

fn turns(session: &Json) -> Vec<Turn> {
    session["transmissions"]
        .as_array()
        .expect("transmissions")
        .iter()
        .map(|t| {
            let bytes = unhex(t["bytes_hex"].as_str().expect("hex"));
            let received = t["broken"]["received_bytes"]
                .as_u64()
                .map_or(bytes.len(), |n| usize::try_from(n).expect("a length"));
            Turn {
                from: t["from"].as_str().expect("from").to_owned(),
                bytes,
                received,
            }
        })
        .collect()
}

/// The nonce in a session's `HELLO`: a random input, taken as the vector drew it.
fn nonce_of(first: &Turn) -> [u8; 8] {
    let hello = radiopost::pdu::unframe(&first.bytes).expect("a HELLO");
    hello
        .get(7)
        .and_then(Value::as_bytes)
        .expect("a nonce")
        .try_into()
        .expect("8 bytes")
}

/// Plays the client's side of `session`: writes `HELLO`, then each of its transmissions, and is
/// fed the other side's. Returns every event.
fn replay_client(client: &mut Client, session: &Json, me: &str) -> Vec<Event> {
    let mut events = Vec::new();
    for (i, turn) in turns(session).iter().enumerate() {
        if turn.from == me {
            let written = if i == 0 {
                client.hello()
            } else {
                client.transmit().expect("a transmission due")
            };
            assert_eq!(
                cbor::hex(&written),
                cbor::hex(&turn.bytes),
                "transmission {}",
                i + 1
            );
        } else {
            // A modem delivers bytes in any pieces: one at a time, nothing may fall due early.
            let got = &turn.bytes[..turn.received];
            for (j, b) in got.iter().enumerate() {
                events.extend(client.receive(std::slice::from_ref(b)));
                if j + 1 < got.len() {
                    assert!(
                        client.transmit().is_none(),
                        "answered early, in transmission {}",
                        i + 1
                    );
                }
            }
        }
    }
    events
}

fn delivered(events: &[Event]) -> Vec<ContentId> {
    events
        .iter()
        .filter_map(|e| {
            if let Event::Delivered(id) = e {
                Some(*id)
            } else {
                None
            }
        })
        .collect()
}

fn received(events: &[Event]) -> Vec<(Option<u64>, ContentId)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Received { serial, message } => Some((*serial, message.content_id())),
            _ => None,
        })
        .collect()
}

#[test]
fn session_01_the_client_side() {
    let w = world();
    let file = vectors("session-01.json");
    let first = &turns(&file)[0];
    let cfg = ClientConfig {
        credential: w.credentials["facundo"].clone(),
        device: w.device("device:facundo"),
        called: "N1CALL-10".into(),
        time: file["context"]["client_clock"].as_u64().expect("clock"),
        nonce: nonce_of(first),
        outbox: vec![w.out("facundo-reply"), w.out("facundo-to-ana")],
        cursor: 418,
        auto: AUTO,
        partials: vec![],
        caps: client_caps(),
        credential_cached: true,
        piece: 4096,
        station: None,
    };
    // Facundo asks for #420 and leaves the photo, #421, waiting.
    let mut client = Client::new(cfg, Box::new(|_| vec![420])).expect("a client");
    let events = replay_client(&mut client, &file, "client");
    assert!(
        client.is_over() && client.transmit().is_none(),
        "hangs up after transmission 5"
    );
    let ids = |names: &[&str]| {
        names
            .iter()
            .map(|n| w.messages[*n].content_id())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        delivered(&events),
        ids(&["facundo-reply", "facundo-to-ana"])
    );
    let got = received(&events);
    assert_eq!(
        got,
        vec![
            (Some(419), ids(&["maria-419"])[0]),
            (Some(420), ids(&["eoc-420"])[0])
        ]
    );
}

#[test]
fn session_02_broken_and_resumed() {
    let w = world();
    let file = vectors("session-02.json");
    let sessions = file["sessions"].as_array().expect("sessions");
    let base = |session: &Json, called: &str| ClientConfig {
        credential: w.credentials["facundo"].clone(),
        device: w.device("device:facundo"),
        called: called.into(),
        time: session["client_clock"].as_u64().expect("clock"),
        nonce: unhex(session["nonce_hex"].as_str().expect("nonce"))
            .try_into()
            .expect("8 bytes"),
        outbox: vec![w.out("facundo-report")],
        cursor: 420,
        auto: AUTO,
        partials: vec![],
        caps: client_caps(),
        credential_cached: true,
        piece: 960,
        station: None,
    };
    let report = w.messages["facundo-report"].content_id();
    let photo = w.messages["maria-421"].content_id();

    // a: the link breaks in his third piece; nothing is delivered.
    let mut a =
        Client::new(base(&sessions[0], "N1CALL-10"), Box::new(|_| vec![421])).expect("a client");
    let events = replay_client(&mut a, &sessions[0], "client");
    assert!(delivered(&events).is_empty());
    assert!(
        a.link_broken().iter().all(|e| !matches!(e, Event::Held(_))),
        "nothing received"
    );

    // b: the gateway holds 1 920 bytes; he sends the rest; the photo breaks after 4 800 bytes.
    let mut b =
        Client::new(base(&sessions[1], "N1CALL-10"), Box::new(|_| vec![421])).expect("a client");
    let events = replay_client(&mut b, &sessions[1], "client");
    assert_eq!(delivered(&events), vec![report]);
    let held: Vec<Partial> = b
        .link_broken()
        .into_iter()
        .filter_map(|e| {
            if let Event::Held(p) = e {
                Some(p)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(held.len(), 1);
    assert_eq!(
        (held[0].id, held[0].held.len(), held[0].serial),
        (photo, 4800, Some(421))
    );

    // c: through another gateway, a first visit: the rest of the photo, from the bytes held.
    let mut cfg = base(&sessions[2], "N2CALL-5");
    cfg.outbox = vec![];
    cfg.partials = held;
    cfg.credential_cached = false;
    let mut c = Client::new(cfg, Box::new(|_| vec![])).expect("a client");
    let events = replay_client(&mut c, &sessions[2], "client");
    assert!(c.is_over());
    assert_eq!(received(&events), vec![(Some(421), photo)]);
}

fn station_caller(w: &World, file: &Json) -> Client {
    let cfg = ClientConfig {
        credential: w.credentials["facundo"].clone(),
        device: w.device("device:facundo"),
        called: "N0CALL".into(),
        time: file["context"]["caller_clock"].as_u64().expect("clock"),
        nonce: nonce_of(&turns(file)[0]),
        outbox: vec![w.out("facundo-direct"), w.out("facundo-to-eoc")],
        cursor: 0,
        auto: AUTO,
        partials: vec![],
        caps: client_caps(),
        credential_cached: false,
        piece: 4096,
        station: Some(w.trust.clone()),
    };
    Client::new(cfg, Box::new(|_| vec![])).expect("a client")
}

#[test]
fn session_03_the_caller() {
    let w = world();
    let file = vectors("session-03.json");
    let mut client = station_caller(&w, &file);
    let events = replay_client(&mut client, &file, "caller");
    assert!(
        client.is_over() && client.transmit().is_none(),
        "hangs up after transmission 4"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::StationProved(b) if b.address == "ana@hampost.org"))
    );
    assert_eq!(
        delivered(&events),
        vec![w.messages["facundo-direct"].content_id()]
    );
    assert_eq!(
        received(&events),
        vec![(Some(1), w.messages["ana-to-facundo"].content_id())]
    );
}

fn ana(w: &World, file: &Json) -> Called {
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
        caps: client_caps(),
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

#[test]
fn session_03_the_station_called() {
    let w = world();
    let file = vectors("session-03.json");
    let mut station = ana(&w, &file);
    let mut events = Vec::new();
    for (i, turn) in turns(&file).iter().enumerate() {
        if turn.from == "called" {
            let written = station.transmit().expect("a transmission due");
            assert_eq!(
                cbor::hex(&written),
                cbor::hex(&turn.bytes),
                "transmission {}",
                i + 1
            );
        } else {
            for (j, b) in turn.bytes.iter().enumerate() {
                events.extend(station.receive(std::slice::from_ref(b)));
                if j + 1 < turn.bytes.len() {
                    assert!(
                        station.transmit().is_none(),
                        "answered early, in transmission {}",
                        i + 1
                    );
                }
            }
        }
    }
    assert!(station.transmit().is_none());
    assert_eq!(
        station.caller().map(|b| b.address.as_str()),
        Some("facundo@radiopost.org")
    );
    assert_eq!(
        received(&events),
        vec![(None, w.messages["facundo-direct"].content_id())]
    );
    assert_eq!(
        delivered(&events),
        vec![w.messages["ana-to-facundo"].content_id()]
    );
}

/// The two engines, joined with no vector between them: the same session comes out.
#[test]
fn session_03_engine_to_engine() {
    let w = world();
    let file = vectors("session-03.json");
    let mut caller = station_caller(&w, &file);
    let mut station = ana(&w, &file);
    let mut written = vec![caller.hello()];
    for _ in 0..8 {
        let last = written.last().expect("a transmission").clone();
        if written.len() % 2 == 1 {
            station.receive(&last);
            match station.transmit() {
                Some(t) => written.push(t),
                None => break,
            }
        } else {
            caller.receive(&last);
            match caller.transmit() {
                Some(t) => written.push(t),
                None => break,
            }
        }
    }
    let expected: Vec<Vec<u8>> = turns(&file).into_iter().map(|t| t.bytes).collect();
    assert_eq!(written, expected);
    assert!(caller.is_over());
}

/// A message for someone else that reaches the station called is refused and dropped (RP-3 §10).
#[test]
fn the_station_called_refuses_mail_for_another_account() {
    let w = world();
    let file = vectors("session-03.json");
    let mut station = ana(&w, &file);
    station.receive(&turns(&file)[0].bytes);
    let _welcome = station.transmit().expect("a WELCOME");
    let eoc = w.out("facundo-to-eoc");
    let msg = Value::int_map(vec![
        (0, Value::Uint(4)),
        (1, Value::bytes(eoc.id().as_bytes())),
        (2, Value::Uint(0)),
        (3, Value::Uint(eoc.air.len() as u64)),
        (4, Value::bytes(&eoc.air)),
    ]);
    let done = Value::int_map(vec![(0, Value::Uint(7))]);
    let mut bytes = radiopost::pdu::frame(&msg).expect("framed");
    bytes.extend(radiopost::pdu::frame(&done).expect("framed"));
    let events = station.receive(&bytes);
    assert!(events.contains(&Event::Failed {
        id: eoc.id(),
        code: ErrorCode::RefusedByPolicy
    }));
    let receipt = radiopost::pdu::unframe(&station.transmit().expect("a RECEIPT")).expect("a PDU");
    let failed = receipt.get(3).and_then(Value::as_array).expect("failed");
    assert_eq!(failed.len(), 1);
}

/// A station called that cannot prove itself gets `ERROR`, and the caller hangs up (RP-3 §10).
#[test]
fn the_caller_refuses_a_station_that_cannot_prove_itself() {
    let w = world();
    let file = vectors("session-03.json");
    let mut client = station_caller(&w, &file);
    client.hello();
    let welcome_turn = &turns(&file)[1];
    let mut stream = radiopost::pdu::Deframer::new();
    stream.push(&welcome_turn.bytes);
    let mut pdus = Vec::new();
    while let Some(Ok(pdu)) = stream.next(65535) {
        pdus.push(pdu);
    }
    // Ana's signature, one bit changed.
    let Value::Map(entries) = &mut pdus[0] else {
        panic!("a WELCOME")
    };
    for (k, v) in entries.iter_mut() {
        if *k == Value::Uint(15) {
            let Value::Bytes(sig) = v else {
                panic!("a signature")
            };
            sig[0] ^= 1;
        }
    }
    let bytes: Vec<u8> = pdus
        .iter()
        .flat_map(|p| radiopost::pdu::frame(p).expect("framed"))
        .collect();
    let events = client.receive(&bytes);
    assert!(events.contains(&Event::End));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::Received { .. } | Event::StationProved(_)))
    );
    let error = radiopost::pdu::unframe(&client.transmit().expect("an ERROR")).expect("a PDU");
    assert_eq!(
        error.get(1).and_then(Value::as_u64),
        Some(u64::from(ErrorCode::SignatureInvalid.code()))
    );
}
