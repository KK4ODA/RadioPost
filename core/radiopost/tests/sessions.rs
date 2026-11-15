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
            Event::Received {
                serial, message, ..
            } => Some((*serial, message.content_id())),
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
    let mut client =
        Client::new(cfg, radiopost::session::client::by_serials(vec![420])).expect("a client");
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
    // what crossed: both of his, whole; the two that came, and not the photo left waiting
    let size = |n: &str| w.out(n).air.len() as u64;
    let sent = size("facundo-reply") + size("facundo-to-ana");
    let came = size("maria-419") + size("eoc-420");
    let p = client.progress();
    assert_eq!(p.sent_bytes(0), (sent, sent));
    assert_eq!(
        p.sent_bytes(u64::MAX),
        (sent, sent),
        "delivered: the modem's count is past"
    );
    assert_eq!(p.received_bytes(), (came, came));
    assert_eq!(p.receiving.len(), 2, "{:?}", p.receiving);
    assert!(p.sending.iter().chain(&p.receiving).all(|c| c.whole));
}

/// The client of `session-01` with held attachments to ask for.
fn asking_client(w: &World, file: &Json) -> Client {
    let cfg = ClientConfig {
        credential: w.credentials["facundo"].clone(),
        device: w.device("device:facundo"),
        called: "N1CALL-10".into(),
        time: file["context"]["client_clock"].as_u64().expect("clock"),
        nonce: nonce_of(&turns(file)[0]),
        outbox: vec![w.out("facundo-reply"), w.out("facundo-to-ana")],
        cursor: 418,
        auto: AUTO,
        partials: vec![],
        caps: client_caps(),
        credential_cached: true,
        piece: 4096,
        station: None,
    };
    let mut client =
        Client::new(cfg, radiopost::session::client::by_serials(vec![420])).expect("a client");
    client.ask_for_files(vec![(417, b"held-ref".to_vec())]);
    client
}

/// The PDUs of a transmission.
fn pdus_of(bytes: &[u8]) -> Vec<Value> {
    let mut d = radiopost::pdu::Deframer::new();
    d.push(bytes);
    std::iter::from_fn(|| d.next(65_536).map(|p| p.expect("a PDU"))).collect()
}

/// RP-3 §5.6: held attachments go in the caller's `REQUEST` as key 2, beside the serials it asks
/// for; the gateway's `DONE` after it is the answer. A link broken before that leaves them to ask
/// for again.
#[test]
fn held_attachments_ride_in_the_request() {
    let w = world();
    let file = vectors("session-01.json");
    let t = turns(&file);
    let reply = |client: &mut Client| {
        assert_eq!(cbor::hex(&client.hello()), cbor::hex(&t[0].bytes));
        client.receive(&t[1].bytes);
        client.transmit().expect("the client's turn")
    };

    let mut client = asking_client(&w, &file);
    let third = pdus_of(&reply(&mut client));
    let request = third.last().expect("a REQUEST");
    assert_eq!(
        request.diagnostic(),
        "{0: 6, 1: [420], 2: [[417, h'68656c642d726566']]}"
    );
    assert!(!client.files_answered(), "not answered yet");
    client.receive(&t[3].bytes);
    assert!(client.files_answered(), "the gateway's DONE answers it");

    let mut broken = asking_client(&w, &file);
    reply(&mut broken);
    broken.link_broken();
    assert!(!broken.files_answered(), "broken before the answer");
}

/// A call to another station asks for no held attachments: there is no post office. The session
/// is `session-03`'s, byte for byte.
#[test]
fn a_station_call_asks_for_no_held_attachments() {
    let w = world();
    let file = vectors("session-03.json");
    let mut client = station_caller(&w, &file);
    client.ask_for_files(vec![(1, b"held-ref".to_vec())]);
    replay_client(&mut client, &file, "caller");
    assert!(client.is_over());
    assert!(!client.files_answered());
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
    let mut a = Client::new(
        base(&sessions[0], "N1CALL-10"),
        radiopost::session::client::by_serials(vec![421]),
    )
    .expect("a client");
    let events = replay_client(&mut a, &sessions[0], "client");
    assert_eq!(
        delivered(&events),
        Vec::<ContentId>::new(),
        "nothing delivered"
    );
    assert!(
        a.link_broken().iter().all(|e| !matches!(e, Event::Held(_))),
        "nothing received"
    );

    // b: the gateway holds 1 920 bytes; he sends the rest; the photo breaks after 4 800 bytes.
    let mut b = Client::new(
        base(&sessions[1], "N1CALL-10"),
        radiopost::session::client::by_serials(vec![421]),
    )
    .expect("a client");
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
    // the report crossed from the 1 920 bytes the gateway held; the photo stopped at 4 800
    let rest = w.out("facundo-report").air.len() as u64 - 1920;
    assert_eq!(b.progress().sent_bytes(0), (rest, rest));
    let photo_size = held[0].total;
    assert_eq!(b.progress().received_bytes(), (photo_size, 4800));

    // c: through another gateway, a first visit: the rest of the photo, from the bytes held.
    let mut cfg = base(&sessions[2], "N2CALL-5");
    cfg.outbox = vec![];
    cfg.partials = held;
    cfg.credential_cached = false;
    let mut c = Client::new(cfg, radiopost::session::client::by_serials(vec![])).expect("a client");
    assert_eq!(
        c.progress().received_bytes(),
        (0, 0),
        "nothing known before WELCOME"
    );
    let events = replay_client(&mut c, &sessions[2], "client");
    assert!(c.is_over());
    assert_eq!(received(&events), vec![(Some(421), photo)]);
    // only the rest crossed
    let rest = photo_size - 4800;
    assert_eq!(c.progress().received_bytes(), (rest, rest));
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
    Client::new(cfg, radiopost::session::client::by_serials(vec![])).expect("a client")
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
    let mut seen_received = 0;
    for (i, turn) in turns(&file).iter().enumerate() {
        let (_, now) = station.progress().received_bytes();
        assert!(now >= seen_received, "what has arrived only grows");
        seen_received = now;
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
    let hello = radiopost::pdu::unframe(&turns(&file)[0].bytes).expect("a HELLO");
    let nonce: [u8; 8] = hello
        .get(7)
        .and_then(Value::as_bytes)
        .expect("a nonce")
        .try_into()
        .expect("8 bytes");
    assert_eq!(
        station.seen(),
        Some(&("facundo@radiopost.org".to_owned(), nonce)),
        "the replay cache's new entry"
    );
    assert_eq!(
        received(&events),
        vec![(None, w.messages["facundo-direct"].content_id())]
    );
    assert_eq!(
        delivered(&events),
        vec![w.messages["ana-to-facundo"].content_id()]
    );
    // what crossed each way, whole; what the caller offered for another account is not counted
    let size = |n: &str| w.out(n).air.len() as u64;
    let p = station.progress();
    let (direct, note) = (size("facundo-direct"), size("ana-to-facundo"));
    assert_eq!(p.received_bytes(), (direct, direct), "{:?}", p.receiving);
    assert_eq!(p.sent_bytes(0), (note, note));
}

fn unbase64url(text: &str) -> Vec<u8> {
    let value = |c: u8| -> u32 {
        u32::from(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => panic!("not base64url: {text}"),
        })
    };
    let mut out = Vec::new();
    for group in text.as_bytes().chunks(4) {
        let n = group.iter().fold(0u32, |n, &c| (n << 6) | value(c)) << (6 * (4 - group.len()));
        out.extend_from_slice(&n.to_be_bytes()[1..group.len()]);
    }
    out
}

/// A proof as `direct.json` writes it (RP-5 §6.6).
fn proof_of(v: &Json) -> radiopost::session::Proof {
    radiopost::session::Proof {
        statement: unbase64url(v["statement"].as_str().expect("statement")),
        signature: unbase64url(v["signature"].as_str().expect("signature")),
        credential: unbase64url(v["credential"].as_str().expect("credential")),
    }
}

/// RP-5 §6.6: after session-03 each engine holds the proofs its station reports, exactly as
/// `direct.json` has Facundo's and Ana's reports.
#[test]
fn session_03_leaves_the_proofs_each_station_reports() {
    let w = world();
    let file = vectors("session-03.json");
    let direct = vectors("direct.json");
    let case = |what: &str| {
        direct["cases"]
            .as_array()
            .expect("cases")
            .iter()
            .find(|c| c["what"] == what)
            .map(|c| c["session"].clone())
            .expect("a case")
    };
    let (his, hers) = (
        case("as Facundo reports it"),
        case("Ana's own report, sent to radiopost.org"),
    );
    let mut caller = station_caller(&w, &file);
    let mut station = ana(&w, &file);
    assert_eq!(caller.proofs(), None, "nothing proved yet");
    let mut written = vec![caller.hello()];
    for _ in 0..8 {
        let last = written.last().expect("a transmission").clone();
        let next = if written.len() % 2 == 1 {
            station.receive(&last);
            station.transmit()
        } else {
            caller.receive(&last);
            caller.transmit()
        };
        match next {
            Some(t) => written.push(t),
            None => break,
        }
    }
    for (proofs, report) in [
        (caller.proofs().expect("the caller's"), &his),
        (station.proofs().expect("the station's"), &hers),
    ] {
        assert_eq!(Some(proofs.started), report["started"].as_u64());
        assert_eq!(proofs.mine, proof_of(&report["mine"]));
        assert_eq!(proofs.theirs, proof_of(&report["theirs"]));
    }
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

/// A station that trusts no post office refuses the caller with `ERROR` and says so (RP-1 §5).
#[test]
fn the_station_called_says_when_it_refuses() {
    let w = world();
    let file = vectors("session-03.json");
    let mut station = ana(&w, &file);
    let mut lonely = {
        let note = w.out("ana-to-facundo");
        let cfg = CalledConfig {
            credential: w.credentials["ana"].clone(),
            device: w.device("device:ana"),
            time: file["context"]["called_clock"].as_u64().expect("clock"),
            session_id: [1; 8],
            challenge: [7; 16],
            trust: Trust::default(),
            seen: vec![],
            caps: client_caps(),
            piece: 4096,
        };
        Called::new(cfg, Box::new(move |_| vec![(1, note.clone())])).expect("a station")
    };
    let hello = &turns(&file)[0].bytes;
    let events = lonely.receive(hello);
    assert!(
        events.contains(&Event::Refused(ErrorCode::UnknownPostOffice)),
        "{events:?}"
    );
    let answer = lonely.transmit().expect("an ERROR");
    assert_eq!(
        radiopost::pdu::unframe(&answer)
            .expect("a PDU")
            .get(0)
            .and_then(Value::as_u64),
        Some(radiopost::pdu::kind::ERROR)
    );
    // the same HELLO, with the post office trusted, is taken
    assert!(
        !station
            .receive(hello)
            .iter()
            .any(|e| matches!(e, Event::Refused(_))),
        "trusted, it is not refused"
    );
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

/// RP-7 §4.4: `session-05`, the client's side of both sessions, byte for byte. Shelter 3 hands
/// N2CALL-5, cut off, a message for Facundo and takes Ana's copy unasked; Facundo, listing only,
/// finds it listed with serial 0 and the flag `local`, asks for it by content ID and takes it
/// without a serial.
#[test]
fn session_05_local_delivery_the_client_side() {
    let w = world();
    let file = vectors("session-05.json");
    let sessions = file["sessions"].as_array().expect("sessions");
    let to_facundo = w.messages["shelter3-to-facundo"].content_id();
    let from_ana = w.messages["ana-to-shelter3"].content_id();

    let first = turns(&sessions[0]);
    let shelter = ClientConfig {
        credential: w.credentials["shelter3-n3call"].clone(),
        device: w.device("device:shelter3-n3call"),
        called: "N2CALL-5".into(),
        time: sessions[0]["client_clock"].as_u64().expect("clock"),
        nonce: nonce_of(&first[0]),
        outbox: vec![w.out("shelter3-to-facundo")],
        cursor: 0,
        auto: AUTO,
        partials: vec![],
        caps: client_caps(),
        credential_cached: true,
        piece: 960,
        station: None,
    };
    let mut a =
        Client::new(shelter, radiopost::session::client::by_serials(vec![])).expect("a client");
    let events = replay_client(&mut a, &sessions[0], "client");
    assert!(a.is_over());
    assert_eq!(received(&events), vec![(Some(501), from_ana)]);
    assert_eq!(delivered(&events), vec![to_facundo]);

    let second = turns(&sessions[1]);
    let facundo = ClientConfig {
        credential: w.credentials["facundo"].clone(),
        device: w.device("device:facundo"),
        called: "N2CALL-5".into(),
        time: sessions[1]["client_clock"].as_u64().expect("clock"),
        nonce: nonce_of(&second[0]),
        outbox: vec![],
        cursor: 421,
        auto: 0,
        partials: vec![],
        caps: client_caps(),
        credential_cached: true,
        piece: 960,
        station: None,
    };
    // everything listed is fetched: the local message too, by its content ID
    let mut b = Client::new(
        facundo,
        Box::new(|entries: &[radiopost::session::WaitingEntry]| {
            entries.iter().map(|e| e.id).collect()
        }),
    )
    .expect("a client");
    let events = replay_client(&mut b, &sessions[1], "client");
    assert!(b.is_over());
    let listed: Vec<(u64, u64)> = events
        .iter()
        .filter_map(|e| match e {
            Event::Listed(l) => Some(l.iter().map(|w| (w.serial, w.flags)).collect::<Vec<_>>()),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(listed, vec![(0, 4)], "serial 0, flag local");
    assert_eq!(
        received(&events),
        vec![(None, to_facundo)],
        "taken without a serial"
    );
}

/// RP-7 §4.4, `local-delivery.json`: who is a cut-off gateway's local user (a copy placed ahead
/// there in the last 30 days), and the local recipients each case's message is kept for.
#[test]
fn local_users_and_the_mail_kept_for_them() {
    use radiopost::session::gateway::{is_local, local_recipients};
    let file = vectors("local-delivery.json");
    let context = &file["context"];
    let now = context["now"].as_u64().expect("now");
    let seen: std::collections::BTreeMap<String, u64> = context["copies_seen"]
        .as_object()
        .expect("copies seen")
        .iter()
        .map(|(a, t)| (a.clone(), t.as_u64().expect("a time")))
        .collect();
    assert!(
        is_local(&seen, "Ana@HamPost.org", now),
        "29 days ago, any case"
    );
    assert!(!is_local(&seen, "kq4sax@radiopost.org", now), "31 days ago");
    assert!(!is_local(&seen, "maria@example.com", now), "never");
    let cases = file["cases"].as_array().expect("cases");
    assert_eq!(cases.len(), 6);
    for case in cases {
        let message = Message::from_canonical(&unhex(case["message_hex"].as_str().expect("hex")))
            .expect("a message");
        let expect: Vec<String> = case["expect"]
            .as_array()
            .expect("expect")
            .iter()
            .filter_map(|a| a.as_str().map(str::to_owned))
            .collect();
        assert_eq!(
            local_recipients(&message, &seen, now),
            expect,
            "{}",
            case["what"]
        );
    }
}

/// RP-7 §3.1–§3.3, `forwarding.json`: each session record's `PROOF` encodes byte for byte with
/// the ID its statement gives it, and each `PROOF` case is answered as the partner's checks say.
#[test]
fn session_records_and_the_partners_checks_of_a_proof() {
    use radiopost::forwarding::{Proof, check_proof, record_id};
    let w = world();
    let file = vectors("forwarding.json");
    let local = vectors("local-delivery.json");
    let records = file["records"]
        .as_array()
        .expect("records")
        .iter()
        .chain(local["records"].as_array().expect("records"));
    for r in records {
        let bytes = unhex(r["proof"]["pdu_hex"].as_str().expect("hex"));
        let value = cbor::decode(&bytes).expect("CBOR");
        let proof = Proof::from_value(&value).expect("a PROOF");
        assert_eq!(
            cbor::hex(&proof.to_value().encode().expect("encodes")),
            cbor::hex(&bytes)
        );
        assert_eq!(
            cbor::hex(&proof.id()),
            r["record_id_hex"].as_str().expect("an ID")
        );
        assert_eq!(proof.id(), record_id(&proof.statement));
    }
    let cases = file["proof_cases"].as_array().expect("cases");
    assert_eq!(cases.len(), 11);
    for case in cases {
        let value = cbor::decode(&unhex(case["proof_hex"].as_str().expect("hex"))).expect("CBOR");
        let held: Vec<Vec<u8>> = case["held_hex"]
            .as_array()
            .expect("held")
            .iter()
            .map(|h| unhex(h.as_str().expect("hex")))
            .collect();
        let got = check_proof(
            &value,
            case["caller"].as_str().expect("caller"),
            case["me"].as_str().expect("me"),
            &held,
            &w.trust,
        );
        let said = match got {
            Ok(_) => "ok".to_owned(),
            Err(code) => code.name().to_owned(),
        };
        assert_eq!(
            said,
            case["expect"].as_str().expect("expect"),
            "{}",
            case["what"]
        );
    }
}

/// RP-7 §4.2–§4.3, `forwarding.json`: the placement made from its body with radiopost.org's key
/// is the vector's byte for byte, and each placement case is answered as the cut-off gateway's
/// checks say, in their order.
#[test]
fn placements_and_the_cut_off_gateways_checks() {
    use radiopost::forwarding::{Placed, check_placement};
    let w = world();
    let file = vectors("forwarding.json");
    let made = unhex(file["placement"]["placement_hex"].as_str().expect("hex"));
    let sign1 = radiopost::crypto::Sign1::open(&made).expect("a COSE_Sign1");
    let placed = Placed::from_value(&cbor::decode(&sign1.payload).expect("CBOR")).expect("a body");
    let key = w
        .seeds
        .iter()
        .filter(|(label, _)| label.starts_with("po:radiopost.org"))
        .map(|(_, seed)| SigningKey::from_bytes(seed))
        .find(|k| radiopost::crypto::kid(&k.verifying_key().to_bytes()) == sign1.kid)
        .expect("the key that placed it");
    assert_eq!(cbor::hex(&placed.sign(&key)), cbor::hex(&made));
    assert_eq!(
        cbor::hex(&sign1.payload),
        file["placement"]["payload_hex"].as_str().expect("hex")
    );
    let cases = file["placement_cases"].as_array().expect("cases");
    assert_eq!(cases.len(), 8);
    for case in cases {
        let id = ContentId::from_slice(&unhex(case["content_id_hex"].as_str().expect("hex")))
            .expect("an ID");
        let got = check_placement(
            &unhex(case["placement_hex"].as_str().expect("hex")),
            &id,
            case["me"].as_str().expect("me"),
            case["now"].as_u64().expect("now"),
            &w.trust,
        );
        let said = match got {
            Ok(p) => {
                assert_eq!(p, placed, "{}", case["what"]);
                "ok".to_owned()
            }
            Err(code) => code.name().to_owned(),
        };
        assert_eq!(
            said,
            case["expect"].as_str().expect("expect"),
            "{}",
            case["what"]
        );
    }
}

/// The two records N2CALL-5 forwards in `session-04`, Shelter 3's and Facundo's, as
/// `forwarding.json` has their `PROOF`s.
fn session_04_records() -> Vec<radiopost::forwarding::Proof> {
    vectors("forwarding.json")["records"]
        .as_array()
        .expect("records")
        .iter()
        .map(|r| {
            let bytes = unhex(r["proof"]["pdu_hex"].as_str().expect("hex"));
            radiopost::forwarding::Proof::from_value(&cbor::decode(&bytes).expect("CBOR"))
                .expect("a PROOF")
        })
        .collect()
}

/// RP-7 §2–§4.3: `session-04`, the caller's side byte for byte. N2CALL-5, cut off, calls its
/// partner N1CALL-10 as a gateway (role 1), offering Shelter 3's message; it takes Ana's message
/// with its placement unasked, sends both records' `PROOF`s before the message, and lets go of
/// the records the partner lists.
#[test]
fn session_04_the_cut_off_gateway_calls_its_partner() {
    use radiopost::session::client::Forwarding;
    let w = world();
    let file = vectors("session-04.json");
    let first = &turns(&file)[0];
    let cfg = ClientConfig {
        credential: w.credentials["gateway-n2call-5"].clone(),
        device: w.device("device:gateway-n2call-5"),
        called: "N1CALL-10".into(),
        time: file["context"]["caller_clock"].as_u64().expect("clock"),
        nonce: nonce_of(first),
        outbox: vec![w.out("shelter3-to-ana")],
        cursor: 0,
        auto: 65_536,
        partials: Vec::new(),
        caps: client_caps(),
        credential_cached: false,
        piece: 4096,
        station: None,
    };
    let records = session_04_records();
    let mut client = Client::new(cfg, Box::new(|_| Vec::new()))
        .expect("a client")
        .forwarding(Forwarding {
            records: records.clone(),
            trust: w.trust.clone(),
        });
    let events = replay_client(&mut client, &file, "caller");
    assert!(client.is_over(), "it hangs up after transmission 4");
    let ana = w.messages["ana-to-shelter3"].content_id();
    let placed: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::Placed {
                serial,
                placed,
                message,
                ..
            } => Some((*serial, placed.clone(), message.content_id())),
            _ => None,
        })
        .collect();
    assert_eq!(placed.len(), 1);
    let (serial, placement, id) = &placed[0];
    assert_eq!((*serial, *id), (Some(1), ana), "forwarding serial 1");
    assert_eq!(
        (placement.address.as_str(), placement.serial),
        ("shelter3@radiopost.org", 501),
        "held as #501 for Shelter 3"
    );
    let taken: Vec<[u8; 12]> = events
        .iter()
        .filter_map(|e| match e {
            Event::Records(r) => Some(r.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(
        taken,
        records
            .iter()
            .map(radiopost::forwarding::Proof::id)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        delivered(&events),
        [w.messages["shelter3-to-ana"].content_id()]
    );
}

/// What N1CALL-10's store did in `session-04`.
#[derive(Default)]
struct PartnerLog {
    opened: Vec<String>,
    records: Vec<radiopost::forwarding::Forwarded>,
    picked: Vec<u64>,
}

/// N1CALL-10's own store for `session-04`, as a test's: it checks the caller as RP-1 §5 and §6.3
/// say (a gateway credential with `send`), holds Ana's message for N2CALL-5 with its placement as
/// forwarding serial 1, and keeps what it is given.
struct Partner {
    trust: Trust,
    clock: u64,
    item: Option<(Vec<u8>, Vec<u8>)>,
    log: std::sync::Arc<std::sync::Mutex<PartnerLog>>,
}

impl radiopost::session::gateway::Upstream for Partner {
    fn open(&mut self, body: &Json) -> Result<Json, ErrorCode> {
        use radiopost::credential::{self, flags};
        use radiopost::statement::{self, Freshness, Statement};
        let field = |k: &str| radiopost::base64url::decode(body[k].as_str().unwrap_or(""));
        let (Some(raw), Some(sig), Some(cred)) =
            (field("statement"), field("signature"), field("credential"))
        else {
            return Err(ErrorCode::Malformed);
        };
        let stmt = Statement::decode(&raw).ok_or(ErrorCode::Malformed)?;
        let cbody =
            credential::verify(&cred, &self.trust, self.clock, flags::GATEWAY | flags::SEND)
                .map_err(|r| r.code)?;
        let fresh = Freshness {
            now: self.clock,
            seen: &[],
            challenge: stmt.challenge(),
            sent_nonce: None,
        };
        statement::verify(&stmt, &sig, &cbody, "N1CALL-10", &fresh)?;
        self.log.lock().expect("log").opened.push(cbody.operator);
        let waiting: Vec<Json> = self
            .item
            .iter()
            .map(|(air, _)| {
                let ana = Message::from_air(air).expect("a message");
                serde_json::json!({
                    "serial": 1, "id": ana.content_id().text(), "from": ana.from(),
                    "subject": "Water truck", "size": air.len(), "priority": 0,
                })
            })
            .collect();
        Ok(serde_json::json!({
            "session": "partner-1",
            "waiting": waiting,
            "more": 0,
            "accept": stmt.messages.iter().map(|m| serde_json::json!({"id": m.text(), "offset": 0}))
                .collect::<Vec<_>>(),
            "decline": [],
        }))
    }

    fn fetch(&mut self, serial: u64) -> Result<Vec<u8>, ErrorCode> {
        self.item
            .as_ref()
            .filter(|_| serial == 1)
            .map(|(air, _)| air.clone())
            .ok_or(ErrorCode::UnknownMessage)
    }

    fn placement(&mut self, serial: u64) -> Option<Vec<u8>> {
        self.item
            .as_ref()
            .filter(|_| serial == 1)
            .map(|(_, p)| p.clone())
    }

    fn trust(&mut self) -> Trust {
        self.trust.clone()
    }

    fn take_records(
        &mut self,
        records: &[radiopost::forwarding::Forwarded],
    ) -> Result<Vec<Result<(), ErrorCode>>, ErrorCode> {
        self.log
            .lock()
            .expect("log")
            .records
            .extend(records.iter().cloned());
        Ok(vec![Ok(()); records.len()])
    }

    fn hand_in(
        &mut self,
        _messages: &[(ContentId, Vec<u8>)],
    ) -> Result<Vec<Result<(), ErrorCode>>, ErrorCode> {
        panic!("forwarded mail is not handed in in the caller's name (RP-7 §2)")
    }

    fn picked_up(&mut self, serials: &[u64]) -> Result<(), ErrorCode> {
        self.log.lock().expect("log").picked.extend(serials);
        Ok(())
    }

    fn files(&mut self, _asked: &[(u64, Vec<u8>)]) -> Result<(), ErrorCode> {
        Ok(())
    }

    fn close(&mut self) {}
}

/// RP-7 §2–§4.3: `session-04`, the partner's side byte for byte. N1CALL-10 answers N2CALL-5's
/// role-1 call from its own store, sends the forward item with its placement, checks both
/// `PROOF`s and the message against Shelter 3's record, and lists both records in its `RECEIPT`.
#[test]
fn session_04_the_partner_answers() {
    use radiopost::session::gateway::{Gateway, GatewayConfig};
    let w = world();
    let file = vectors("session-04.json");
    let turns = turns(&file);
    let first = &turns[1].bytes;
    let length = usize::from(u16::from_be_bytes([first[0], first[1]]));
    let session_id: [u8; 8] = radiopost::pdu::unframe(&first[..2 + length])
        .expect("a WELCOME")
        .get(2)
        .and_then(Value::as_bytes)
        .expect("a session ID")
        .try_into()
        .expect("8 bytes");
    let clock = file["context"]["partner_clock"].as_u64().expect("clock");
    let cfg = GatewayConfig {
        callsign: "N1CALL-10".into(),
        time: clock,
        session_id,
        challenge: [0; 16],
        caps: Caps {
            compression: vec![0, 1],
            max_pdu: 4096,
            renew: false,
        },
        piece: 960,
        credentials: Vec::new(),
        uploads: HashMap::new(),
        refused: Vec::new(),
    };
    let placement = unhex(
        vectors("forwarding.json")["placement"]["placement_hex"]
            .as_str()
            .expect("hex"),
    );
    let log = std::sync::Arc::new(std::sync::Mutex::new(PartnerLog::default()));
    let partner = {
        let (trust, log) = (w.trust.clone(), log.clone());
        let item = Some((w.out("ana-to-shelter3").air, placement));
        move |caller: &str| -> Result<Box<dyn radiopost::session::gateway::Upstream + Send>, ErrorCode> {
            assert_eq!(caller, "N2CALL-5");
            Ok(Box::new(Partner {
                trust: trust.clone(),
                clock,
                item: item.clone(),
                log: log.clone(),
            }))
        }
    };
    let mut gateway = Gateway::new(
        cfg,
        Box::new(|_| panic!("no post office is asked in the caller's name")),
        Box::new(|_| true),
    )
    .with_partner(Box::new(partner));
    for (n, t) in turns.iter().enumerate() {
        if t.from == "caller" {
            gateway.receive(&t.bytes);
        } else {
            let wrote = gateway.transmit().expect("a transmission due");
            assert_eq!(
                cbor::hex(&wrote),
                cbor::hex(&t.bytes),
                "transmission {}",
                n + 1
            );
        }
    }
    assert_eq!(
        gateway.transmit(),
        None,
        "nothing after the caller's last turn"
    );
    gateway.link_closed();
    let log = log.lock().expect("log");
    assert_eq!(log.opened, ["N2CALL-5"]);
    assert_eq!(log.picked, [1], "the forward item, receipted");
    let ids: Vec<[u8; 12]> = log.records.iter().map(|r| r.record.proof.id()).collect();
    assert_eq!(
        ids,
        session_04_records()
            .iter()
            .map(radiopost::forwarding::Proof::id)
            .collect::<Vec<_>>()
    );
    let shelter = w.messages["shelter3-to-ana"].content_id();
    assert_eq!(
        log.records[0]
            .messages
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        [shelter],
        "Shelter 3's record, with her message"
    );
    assert!(
        log.records[1].messages.is_empty(),
        "Facundo's, a pickup only"
    );
    assert_eq!(log.records[1].record.proof.pickups, [421]);
}

/// What one forwarding session, engine to engine, left on each side.
struct Forwarded {
    caller: Vec<Event>,
    partner: Vec<Event>,
    /// The caller's third transmission's PDUs.
    third: Vec<Value>,
    notes: Vec<radiopost::session::gateway::Note>,
    log: PartnerLog,
}

/// N2CALL-5 forwarding `records` and `outbox` to N1CALL-10, engine to engine, at session-04's
/// clocks; N1CALL-10 holds `item` for it (Ana's message and a placement), and answers forwarding
/// sessions only when `partner` is true.
fn forward(
    w: &World,
    records: Vec<radiopost::forwarding::Proof>,
    outbox: Vec<Outgoing>,
    item: Option<Vec<u8>>,
    partner: bool,
) -> Forwarded {
    use radiopost::session::client::Forwarding;
    use radiopost::session::gateway::{Gateway, GatewayConfig};
    let file = vectors("session-04.json");
    let cfg = ClientConfig {
        credential: w.credentials["gateway-n2call-5"].clone(),
        device: w.device("device:gateway-n2call-5"),
        called: "N1CALL-10".into(),
        time: file["context"]["caller_clock"].as_u64().expect("clock"),
        nonce: [7; 8],
        outbox,
        cursor: 0,
        auto: 65_536,
        partials: Vec::new(),
        caps: client_caps(),
        credential_cached: false,
        piece: 4096,
        station: None,
    };
    let mut client = Client::new(cfg, Box::new(|_| Vec::new()))
        .expect("a client")
        .forwarding(Forwarding {
            records,
            trust: w.trust.clone(),
        });
    let clock = file["context"]["partner_clock"].as_u64().expect("clock");
    let gcfg = GatewayConfig {
        callsign: "N1CALL-10".into(),
        time: clock,
        session_id: [9; 8],
        challenge: [0; 16],
        caps: Caps {
            compression: vec![0, 1],
            max_pdu: 4096,
            renew: false,
        },
        piece: 960,
        credentials: Vec::new(),
        uploads: HashMap::new(),
        refused: Vec::new(),
    };
    let log = std::sync::Arc::new(std::sync::Mutex::new(PartnerLog::default()));
    let mut gateway = Gateway::new(
        gcfg,
        Box::new(|_| panic!("no post office is asked in the caller's name")),
        Box::new(|_| true),
    );
    if partner {
        let (trust, shared) = (w.trust.clone(), log.clone());
        let item = item.map(|p| (w.out("ana-to-shelter3").air, p));
        gateway = gateway.with_partner(Box::new(move |_caller: &str| {
            Ok(Box::new(Partner {
                trust: trust.clone(),
                clock,
                item: item.clone(),
                log: shared.clone(),
            })
                as Box<dyn radiopost::session::gateway::Upstream + Send>)
        }));
    }
    let (mut caller, mut answered, mut third) = (Vec::new(), Vec::new(), Vec::new());
    let mut wire = vec![client.hello()];
    let mut turn = 1;
    while !wire.is_empty() {
        for bytes in std::mem::take(&mut wire) {
            answered.extend(gateway.receive(&bytes));
        }
        let mut back = Vec::new();
        while let Some(bytes) = gateway.transmit() {
            back.push(bytes);
        }
        for bytes in back {
            caller.extend(client.receive(&bytes));
        }
        while let Some(bytes) = client.transmit() {
            turn += 2;
            if turn == 3 {
                third = pdus_of(&bytes);
            }
            wire.push(bytes);
        }
    }
    let notes = gateway.take_notes();
    gateway.link_closed();
    let log = std::mem::take(&mut *log.lock().expect("log"));
    Forwarded {
        caller,
        partner: answered,
        third,
        notes,
        log,
    }
}

fn not_taken(events: &[Event]) -> Vec<(ContentId, ErrorCode)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::NotTaken { id, code } => Some((*id, *code)),
            _ => None,
        })
        .collect()
}

fn records_taken(events: &[Event]) -> Vec<[u8; 12]> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Records(r) => Some(r.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// The record Shelter 3 left with N2CALL-5 at 14:35 (`local-delivery.json`), with her credential
/// whole in place of its hash.
fn shelter_second_record(w: &World) -> radiopost::forwarding::Proof {
    let bytes = unhex(
        vectors("local-delivery.json")["records"][0]["proof"]["pdu_hex"]
            .as_str()
            .expect("hex"),
    );
    let mut record = radiopost::forwarding::Proof::from_value(&cbor::decode(&bytes).expect("CBOR"))
        .expect("a PROOF");
    record.credential = Some(w.credentials["shelter3-n3call"].clone());
    record.credential_hash = None;
    record
}

/// RP-7 §3.2: two records of one user in one transmission carry her credential once, then its
/// hash, and the partner finds it by the hash among the credentials it has just checked; it keeps
/// the credential once, for later windows.
#[test]
fn a_credential_goes_once_then_by_its_hash() {
    let w = world();
    let first = session_04_records().remove(0);
    let second = shelter_second_record(&w);
    let ids = [first.id(), second.id()];
    let got = forward(
        &w,
        vec![first, second],
        vec![w.out("shelter3-to-ana"), w.out("shelter3-to-facundo")],
        None,
        true,
    );
    let proofs: Vec<&Value> = got
        .third
        .iter()
        .filter(|p| p.get(0).and_then(Value::as_u64) == Some(11))
        .collect();
    assert_eq!(proofs.len(), 2);
    assert!(
        proofs[0].get(1).is_some() && proofs[0].get(9).is_none(),
        "whole"
    );
    assert!(
        proofs[1].get(1).is_none(),
        "the second without the credential"
    );
    assert_eq!(
        proofs[1].get(9).and_then(Value::as_bytes),
        Some(&radiopost::credential::hash(&w.credentials["shelter3-n3call"])[..]),
        "but its hash"
    );
    assert_eq!(records_taken(&got.caller), ids);
    assert_eq!(
        delivered(&got.caller),
        [
            w.messages["shelter3-to-ana"].content_id(),
            w.messages["shelter3-to-facundo"].content_id()
        ]
    );
    let kept: Vec<_> = got
        .notes
        .iter()
        .filter(|n| {
            **n == radiopost::session::gateway::Note::Credential(
                w.credentials["shelter3-n3call"].clone(),
            )
        })
        .collect();
    assert_eq!(kept.len(), 1, "Shelter 3's credential, kept once");
    assert_eq!(got.log.records.len(), 2);
}

/// RP-7 §3.3: a record whose path does not end with the caller is refused `refused-by-policy`;
/// its message fails with that code and stays with the caller, the record is not listed, and the
/// other record is taken.
#[test]
fn a_refused_record_fails_its_messages_with_its_code() {
    use radiopost::session::gateway::Note;
    let w = world();
    let mut records = session_04_records();
    records[0].path = vec!["N2CALL-5".into(), "N9ZZZ".into()];
    let refused = records[0].id();
    let facundo = records[1].id();
    let got = forward(&w, records, vec![w.out("shelter3-to-ana")], None, true);
    let shelter = w.messages["shelter3-to-ana"].content_id();
    assert_eq!(
        not_taken(&got.caller),
        [(shelter, ErrorCode::RefusedByPolicy)]
    );
    assert_eq!(records_taken(&got.caller), [facundo]);
    assert_eq!(delivered(&got.caller), Vec::<ContentId>::new());
    assert!(got.notes.contains(&Note::RecordRefused {
        id: Some(refused),
        code: ErrorCode::RefusedByPolicy
    }));
    assert_eq!(got.log.records.len(), 1, "Facundo's alone stored");
}

/// RP-7 §3.3: a message that no record taken covers is `not-authorized`; the records sent are
/// taken all the same.
#[test]
fn a_message_no_record_covers_is_not_authorized() {
    let w = world();
    let facundo = session_04_records().remove(1);
    let id = facundo.id();
    let got = forward(
        &w,
        vec![facundo],
        vec![w.out("shelter3-to-ana")],
        None,
        true,
    );
    assert_eq!(
        not_taken(&got.caller),
        [(
            w.messages["shelter3-to-ana"].content_id(),
            ErrorCode::NotAuthorized
        )]
    );
    assert_eq!(records_taken(&got.caller), [id]);
    assert!(got.log.records.iter().all(|r| r.messages.is_empty()));
}

/// RP-7 §2: a gateway that answers no forwarding sessions refuses `HELLO` role 1.
#[test]
fn a_gateway_without_partners_refuses_a_forwarding_call() {
    let w = world();
    let got = forward(
        &w,
        session_04_records(),
        vec![w.out("shelter3-to-ana")],
        None,
        false,
    );
    assert!(got.caller.iter().any(|e| matches!(
        e,
        Event::Error {
            code: ErrorCode::RefusedByPolicy,
            ..
        }
    )));
    assert!(
        got.partner
            .contains(&Event::Refused(ErrorCode::RefusedByPolicy))
    );
    assert_eq!(got.log.opened, Vec::<String>::new());
}

/// RP-7 §4.3: a forward item whose placement names another gateway is listed failed
/// `not-authorized` and not held; the partner keeps it, since the caller did not list it
/// `complete`.
#[test]
fn a_placement_for_another_gateway_is_not_held() {
    let w = world();
    let file = vectors("forwarding.json");
    let other = file["placement_cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|c| c["what"] == "for another gateway")
        .expect("the case");
    let placement = unhex(other["placement_hex"].as_str().expect("hex"));
    let got = forward(&w, Vec::new(), Vec::new(), Some(placement), true);
    let ana = w.messages["ana-to-shelter3"].content_id();
    assert!(got.caller.contains(&Event::Failed {
        id: ana,
        code: ErrorCode::NotAuthorized
    }));
    assert!(!got.caller.iter().any(|e| matches!(e, Event::Placed { .. })));
    assert!(got.log.picked.is_empty(), "the forward item is kept");
}

/// RP-7 §5, §5.1, `forwarding.json`'s window cases: a window is allowed when its signal lies in
/// one segment of the country's table, ends included; a country with no table allows none.
#[test]
fn windows_only_inside_a_countrys_segments() {
    let file = vectors("forwarding.json");
    let cases = file["window_cases"].as_array().expect("cases");
    assert_eq!(cases.len(), 19);
    for case in cases {
        let o = &case["occupied_hz"];
        let occupied = (o[0].as_u64().expect("low"), o[1].as_u64().expect("high"));
        assert_eq!(
            radiopost::forwarding::window_allowed(
                occupied,
                case["country"].as_str().expect("country")
            ),
            case["expect"].as_bool().expect("expect"),
            "{}",
            case["what"]
        );
    }
    let table: Vec<(u64, u64)> = file["segments"]["US"]
        .as_array()
        .expect("segments")
        .iter()
        .map(|s| (s[0].as_u64().expect("from"), s[1].as_u64().expect("to")))
        .collect();
    assert_eq!(radiopost::forwarding::segments("US"), table.as_slice());
}

/// RP-7 §6.2, `forwarding.json`'s partner cases: in a partner's window, a cut-off gateway sends it
/// its records unless that partner said it is cut off too and another that reaches the Internet
/// opens a window within the hour.
#[test]
fn records_go_to_a_partner_that_reaches_the_post_offices() {
    use radiopost::forwarding::{PartnerView, Window, records_to};
    let file = vectors("forwarding.json");
    let cases = file["partner_cases"].as_array().expect("cases");
    assert_eq!(cases.len(), 8);
    for case in cases {
        let partners: Vec<(String, PartnerView)> = case["partners"]
            .as_object()
            .expect("partners")
            .iter()
            .map(|(c, p)| {
                (
                    c.clone(),
                    PartnerView {
                        window: Window::from_json(&p["window"]).expect("a window"),
                        online: p["online"].as_bool(),
                    },
                )
            })
            .collect();
        assert_eq!(
            records_to(
                case["calling"].as_str().expect("calling"),
                &partners,
                case["now"].as_u64().expect("now")
            ),
            case["expect"].as_bool().expect("expect"),
            "{}",
            case["what"]
        );
    }
    // session-04's window: N1CALL-10's opens at :15 for ten minutes
    let w = Window::from_json(&file["context"]["window"]).expect("a window");
    let t = vectors("session-04.json")["context"]["caller_clock"]
        .as_u64()
        .expect("clock");
    assert!(w.is_open(t), "N2CALL-5 calls inside N1CALL-10's window");
    assert!(!w.is_open(t + 600), "ten minutes on, it has closed");
    assert_eq!(
        w.next(t + 600),
        w.next(t) + 3600,
        "and opens again an hour on"
    );
}
