// SPDX-License-Identifier: MIT OR Apache-2.0
//! The forwarding session (RP-7 §2 to §4.3) through the gateway program's own store, both sides of
//! `session-04` byte for byte: N2CALL-5, cut off, sends the two records it keeps and lets them go;
//! N1CALL-10 answers from its store, sends the forward item it polled, and keeps the records with
//! its callsign on their paths, to report exactly as `forwarding.json` has it.

use std::collections::HashMap;
use std::path::PathBuf;

use ed25519_dalek::SigningKey;
use radiopost::base64url;
use radiopost::message::{Compression, Message};
use radiopost::session::client::{Client, ClientConfig, Forwarding};
use radiopost::session::gateway::{Gateway, GatewayConfig, Upstream};
use radiopost::session::{Caps, Outgoing};
use radiopost_gateway::forward::{Outcome, Partner, outbox};
use radiopost_gateway::store::{Keyed, Record, Shared};
use radiopost_postoffice::known::{Found, Key};
use serde_json::Value as Json;

fn vectors(name: &str) -> Json {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vectors")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(&path).expect("a vector file")).expect("JSON")
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

fn seed(label: &str) -> [u8; 32] {
    let k = vectors("keys.json")["keys"]
        .as_array()
        .expect("keys")
        .iter()
        .find(|k| k["label"] == label)
        .cloned()
        .expect("the key");
    unhex(k["seed_hex"].as_str().expect("seed"))
        .try_into()
        .expect("32")
}

fn credential(name: &str) -> Vec<u8> {
    let c = vectors("credentials.json")["credentials"]
        .as_array()
        .expect("credentials")
        .iter()
        .find(|c| c["name"] == name)
        .cloned()
        .expect("the credential");
    unhex(c["credential_hex"].as_str().expect("hex"))
}

fn message(name: &str) -> Message {
    let m = vectors("messages.json")["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|m| m["name"] == name)
        .cloned()
        .expect("the message");
    Message::from_canonical(&unhex(m["canonical_hex"].as_str().expect("hex"))).expect("a message")
}

/// radiopost.org's keys, as a gateway holds them from DNS.
fn radiopost_keys() -> Keyed {
    let keys = vectors("keys.json")["keys"]
        .as_array()
        .expect("keys")
        .iter()
        .filter(|k| {
            k["label"]
                .as_str()
                .is_some_and(|l| l.starts_with("po:radiopost.org"))
        })
        .map(|k| {
            let seed: [u8; 32] = unhex(k["seed_hex"].as_str().expect("seed"))
                .try_into()
                .expect("32");
            Key {
                kid: k["label"].as_str().expect("label").to_owned(),
                public: base64url::encode(
                    &SigningKey::from_bytes(&seed).verifying_key().to_bytes(),
                ),
                not_before: k["not_before"].as_u64().expect("validity"),
                not_after: k["not_after"].as_u64().expect("validity"),
            }
        })
        .collect();
    Keyed {
        found: Found {
            keys,
            ..Found::default()
        },
        how: "dns".into(),
        ..Keyed::default()
    }
}

fn store(name: &str) -> Shared {
    let dir =
        std::env::temp_dir().join(format!("radiopost-session04-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    let shared = Shared::open(&dir);
    shared
        .change(|s| {
            s.keys.insert("radiopost.org".into(), radiopost_keys());
        })
        .expect("written");
    shared
}

/// What N1CALL-10 reported for each record, from `forwarding.json`'s post office exchanges.
fn reported() -> Vec<Json> {
    vectors("forwarding.json")["post_office"]
        .as_array()
        .expect("exchanges")
        .iter()
        .find(|e| e["request"]["path"] == "/reports")
        .expect("the report")["request"]["body"]["sessions"]
        .as_array()
        .expect("sessions")
        .clone()
}

/// A transmission's bytes and who wrote it.
fn turns() -> Vec<(String, Vec<u8>)> {
    vectors("session-04.json")["transmissions"]
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

/// The first PDU of a transmission.
fn first_pdu(bytes: &[u8]) -> radiopost::cbor::Value {
    let length = usize::from(u16::from_be_bytes([bytes[0], bytes[1]]));
    radiopost::pdu::unframe(&bytes[..2 + length]).expect("a PDU")
}

/// RP-7 §2–§4.3: N1CALL-10 answers N2CALL-5 from its own store: it holds the forward item its
/// poll brought (forwarding serial 1), checks both `PROOF`s and Shelter 3's message, and keeps the
/// two records with its callsign on their paths. They are what it reports to radiopost.org, word
/// for word, and the forward item goes once N2CALL-5 lists it `complete`.
#[test]
fn n1call_10_answers_from_its_store() {
    let file = vectors("session-04.json");
    let clock = file["context"]["partner_clock"].as_u64().expect("clock");
    let store = store("partner");
    let polled: Vec<Json> = vectors("forwarding.json")["post_office"]
        .as_array()
        .expect("exchanges")
        .iter()
        .filter(|e| e["who"] == "N1CALL-10")
        .flat_map(|e| {
            e["response"]["body"]["items"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter(|i| i["kind"] == "forward")
        .collect();
    assert_eq!(polled.len(), 1, "Ana's message for N2CALL-5");
    store
        .change(|s| s.apply_at("radiopost.org", &polled, None, clock))
        .expect("written");
    let turns = turns();
    let session_id: [u8; 8] = first_pdu(&turns[1].1)
        .get(2)
        .and_then(radiopost::cbor::Value::as_bytes)
        .expect("a session ID")
        .try_into()
        .expect("8 bytes");
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
    let held = store.clone();
    let mut gateway = Gateway::new(
        cfg,
        Box::new(|_| panic!("forwarded mail is not handed to a post office in the caller's name")),
        Box::new(|_| true),
    )
    .with_partner(Box::new(move |caller: &str| {
        assert_eq!(caller, "N2CALL-5");
        Ok(Box::new(Partner::new(
            held.clone(),
            "N1CALL-10",
            caller,
            (Vec::new(), None),
            clock,
            true,
        )) as Box<dyn Upstream + Send>)
    }));
    for (n, (from, bytes)) in turns.iter().enumerate() {
        if from == "caller" {
            gateway.receive(bytes);
        } else {
            let wrote = gateway.transmit().expect("a transmission due");
            assert_eq!(
                radiopost::cbor::hex(&wrote),
                radiopost::cbor::hex(bytes),
                "transmission {}",
                n + 1
            );
        }
    }
    assert_eq!(gateway.transmit(), None);
    gateway.link_closed();
    let s = store.lock();
    let reports: Vec<Json> = s.records.iter().map(Record::report).collect();
    assert_eq!(
        reports,
        reported(),
        "reported to radiopost.org as the vector has it"
    );
    assert!(s.forwards.is_empty(), "the forward item, receipted, goes");
}

/// N2CALL-5's two records as it kept them while cut off: what N1CALL-10 reports, without the path.
fn n2call_5_records() -> Vec<Record> {
    reported()
        .iter()
        .map(|r| {
            let statement =
                base64url::decode(r["statement"].as_str().expect("statement")).expect("base64url");
            let stmt = radiopost::statement::Statement::decode(&statement).expect("a statement");
            Record {
                domain: "radiopost.org".into(),
                address: stmt.address.clone(),
                operator: stmt.operator.clone(),
                statement: r["statement"].as_str().expect("text").to_owned(),
                signature: r["signature"].as_str().expect("text").to_owned(),
                credential: r["credential"].as_str().expect("text").to_owned(),
                started: r["started"].as_u64().expect("started"),
                gateway_session: r["gateway_session"].as_str().expect("text").to_owned(),
                messages: r["messages"]
                    .as_array()
                    .expect("messages")
                    .iter()
                    .map(|m| {
                        (
                            m["id"].as_str().expect("id").to_owned(),
                            m["air"].as_str().expect("air").to_owned(),
                        )
                    })
                    .collect(),
                pickups: r["pickups"]
                    .as_array()
                    .expect("pickups")
                    .iter()
                    .filter_map(Json::as_u64)
                    .collect(),
                done: true,
                ..Record::default()
            }
        })
        .collect()
}

/// RP-7 §3.2, §3.5, §4.3: N2CALL-5 builds its call from its own store (both records, the first
/// with Shelter 3's message) and writes session-04's caller side byte for byte. What the session
/// says goes back into the store: the message passed on, both records let go and their
/// credentials remembered as N1CALL-10's, and Ana's message held as a copy for Shelter 3.
#[test]
fn n2call_5_forwards_from_its_store() {
    let file = vectors("session-04.json");
    let clock = file["context"]["caller_clock"].as_u64().expect("clock");
    let store = store("caller");
    store
        .change(|s| s.records = n2call_5_records())
        .expect("written");
    let (records, messages) = outbox(&store.lock(), "N2CALL-5", "N1CALL-10");
    let shelter = Outgoing::new(message("shelter3-to-ana"), Compression::None);
    assert_eq!(messages, std::slice::from_ref(&shelter));
    let turns = turns();
    let nonce: [u8; 8] = first_pdu(&turns[0].1)
        .get(7)
        .and_then(radiopost::cbor::Value::as_bytes)
        .expect("a nonce")
        .try_into()
        .expect("8 bytes");
    let cfg = ClientConfig {
        credential: credential("gateway-n2call-5"),
        device: SigningKey::from_bytes(&seed("device:gateway-n2call-5")),
        called: "N1CALL-10".into(),
        time: clock,
        nonce,
        outbox: messages,
        cursor: 0,
        auto: 65_536,
        partials: Vec::new(),
        caps: Caps {
            compression: vec![0, 1, 2],
            max_pdu: 4096,
            renew: false,
        },
        credential_cached: false,
        piece: 4096,
        station: None,
    };
    let trust = store.lock().trust(&[], None);
    let mut client = Client::new(cfg, Box::new(|_| Vec::new()))
        .expect("a client")
        .forwarding(Forwarding { records, trust });
    let mut outcome = Outcome::default();
    for (n, (from, bytes)) in turns.iter().enumerate() {
        if from == "caller" {
            let wrote = if n == 0 {
                client.hello()
            } else {
                client.transmit().expect("a transmission due")
            };
            assert_eq!(
                radiopost::cbor::hex(&wrote),
                radiopost::cbor::hex(bytes),
                "transmission {}",
                n + 1
            );
        } else {
            let events = client.receive(bytes);
            store
                .change(|s| {
                    for e in &events {
                        outcome.apply(s, "N1CALL-10", e, clock);
                    }
                })
                .expect("written");
        }
    }
    assert!(client.is_over());
    assert_eq!((outcome.records, outcome.passed, outcome.held), (2, 1, 1));
    let s = store.lock();
    assert!(s.records.is_empty(), "both records let go (RP-7 §3.5)");
    assert!(s.passed_on.contains_key(&shelter.id().text()));
    let copy = s
        .copies
        .iter()
        .find(|c| c.address == "shelter3@radiopost.org")
        .expect("held for Shelter 3");
    assert_eq!(copy.serial, 501);
    assert_eq!(copy.id, message("ana-to-shelter3").content_id().text());
    assert_eq!(copy.expires, 1_795_087_200);
    assert!(copy.air.is_some());
    let partner = &s.partners["N1CALL-10"];
    assert_eq!(partner.cursor, 1, "the forwarding serial received");
    assert_eq!(partner.online, Some(true));
    for name in ["shelter3-n3call", "facundo"] {
        let h = radiopost::cbor::hex(&radiopost::credential::hash(&credential(name)));
        assert!(
            partner.holds.contains(&h),
            "{name}'s credential, held there now"
        );
    }
    // the next call names those credentials by their hashes
    drop(s);
    store
        .change(|s| {
            let mut again = n2call_5_records();
            again.truncate(1);
            s.records = again;
        })
        .expect("written");
    let (again, _) = outbox(&store.lock(), "N2CALL-5", "N1CALL-10");
    assert!(again[0].credential.is_none() && again[0].credential_hash.is_some());
}
