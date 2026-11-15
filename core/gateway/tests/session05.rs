// SPDX-License-Identifier: MIT OR Apache-2.0
//! Local delivery (RP-7 §4.4): the gateway program, cut off, writes `session-05`'s gateway side
//! byte for byte. Its engine answers through the gateway's own store (`offline`), set up as the
//! vector says: copies held for Shelter 3 (Ana's #501, unpicked) and for Facundo (#421, picked
//! up in session-04), so both are local users.

use std::collections::HashMap;
use std::path::PathBuf;

use ed25519_dalek::SigningKey;
use radiopost::base64url;
use radiopost::message::{Compression, Message};
use radiopost::session::Caps;
use radiopost::session::gateway::{Gateway, GatewayConfig, Upstreams};
use radiopost_gateway::offline::Offline;
use radiopost_gateway::store::{Copy, Keyed, Shared};
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

/// radiopost.org's keys as the gateway holds them, fetched before the cut.
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

/// N2CALL-5's store after session-04: Ana's #501 waits for Shelter 3, Facundo picked up #421.
fn store() -> Shared {
    let dir = std::env::temp_dir().join(format!("radiopost-session05-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    let shared = Shared::open(&dir);
    let ana = message("ana-to-shelter3");
    let air = ana.air_form(Compression::None);
    let context = vectors("local-delivery.json")["context"].clone();
    shared
        .change(|s| {
            s.keys.insert("radiopost.org".into(), radiopost_keys());
            s.copies.push(Copy {
                address: "shelter3@radiopost.org".into(),
                serial: 501,
                id: ana.content_id().text(),
                from: "ana@hampost.org".into(),
                subject: "Water truck".into(),
                size: air.len() as u64,
                priority: 0,
                third_party: false,
                air: Some(base64url::encode(&air)),
                expires: u64::MAX,
                by: "radiopost.org".into(),
                picked: false,
            });
            let maria = message("maria-421");
            s.copies.push(Copy {
                address: "facundo@radiopost.org".into(),
                serial: 421,
                id: maria.content_id().text(),
                from: "maria@example.com".into(),
                subject: "Photo".into(),
                size: 10,
                priority: 0,
                third_party: true,
                air: None,
                expires: u64::MAX,
                by: "radiopost.org".into(),
                picked: true,
            });
            for (address, at) in context["copies_seen"].as_object().expect("seen") {
                s.copies_seen
                    .insert(address.clone(), at.as_u64().expect("a time"));
            }
        })
        .expect("written");
    shared
}

/// One session through N2CALL-5's engine and its own store, every gateway transmission compared.
fn replay(session: &Json, store: &Shared, notice: &str) {
    let clock = session["gateway_clock"].as_u64().expect("clock");
    let transmissions = session["transmissions"].as_array().expect("transmissions");
    // the WELCOME, the first frame of the gateway's first transmission: its session ID was drawn
    let first = unhex(transmissions[1]["bytes_hex"].as_str().expect("hex"));
    let length = usize::from(u16::from_be_bytes([first[0], first[1]]));
    let welcome = radiopost::pdu::unframe(&first[..2 + length]).expect("a WELCOME");
    let session_id: [u8; 8] = welcome
        .get(2)
        .and_then(radiopost::cbor::Value::as_bytes)
        .expect("a session ID")
        .try_into()
        .expect("8 bytes");
    let cfg = GatewayConfig {
        callsign: "N2CALL-5".into(),
        time: clock,
        session_id,
        challenge: [0; 16],
        caps: Caps {
            compression: vec![0, 1],
            max_pdu: 4096,
            renew: false,
        },
        piece: 960,
        credentials: vec![credential("shelter3-n3call"), credential("facundo")],
        uploads: HashMap::new(),
        refused: Vec::new(),
    };
    let (store, notice) = (store.clone(), notice.to_owned());
    let upstreams: Upstreams = Box::new(move |domain| {
        assert_eq!(domain, "radiopost.org");
        Ok(Box::new(
            Offline::new(store.clone(), "N2CALL-5", Vec::new(), None, clock).with_notice(&notice),
        ))
    });
    let mut gateway = Gateway::new(cfg, upstreams, Box::new(|_| true));
    for (n, t) in transmissions.iter().enumerate() {
        let bytes = unhex(t["bytes_hex"].as_str().expect("hex"));
        if t["from"] == "client" {
            gateway.receive(&bytes);
        } else {
            let wrote = gateway.transmit().expect("a transmission due");
            assert_eq!(
                radiopost::cbor::hex(&wrote),
                radiopost::cbor::hex(&bytes),
                "session {}, transmission {}",
                session["session"],
                n + 1
            );
        }
    }
    assert_eq!(
        gateway.transmit(),
        None,
        "nothing after the client's last turn"
    );
    gateway.link_closed();
}

/// RP-7 §4.4: Shelter 3's message for Facundo is kept for him as well as in her record; in his
/// session it is listed with serial 0 and the flag `local`, sent without a serial when he asks for
/// it by content ID, and his record keeps it as a local pickup.
#[test]
fn a_cut_off_gateway_hands_mail_between_its_users_as_session_05_says() {
    let file = vectors("session-05.json");
    let sessions = file["sessions"].as_array().expect("sessions");
    let notice = "No Internet here since 08:15 UTC. Your mail waits in my queue; mail for this \
                  gateway's own users is handed over here.";
    let store = store();
    let spark = message("shelter3-to-facundo").content_id().text();

    replay(&sessions[0], &store, notice);
    {
        let s = store.lock();
        let shelter = s
            .records
            .iter()
            .find(|r| r.address == "shelter3@radiopost.org")
            .expect("Shelter 3's record");
        assert_eq!(shelter.messages.len(), 1);
        assert_eq!(shelter.messages[0].0, spark);
        assert_eq!(shelter.pickups, [501]);
        assert!(shelter.done);
        let local: Vec<(&str, &str)> = s
            .local
            .iter()
            .map(|l| (l.address.as_str(), l.id.as_str()))
            .collect();
        assert_eq!(local, [("facundo@radiopost.org", spark.as_str())]);
    }

    replay(&sessions[1], &store, notice);
    let s = store.lock();
    let facundo = s
        .records
        .iter()
        .find(|r| r.address == "facundo@radiopost.org")
        .expect("Facundo's record");
    assert_eq!(facundo.local_pickups, std::slice::from_ref(&spark));
    assert_eq!(
        facundo.report()["local_pickups"],
        serde_json::json!([spark])
    );
    assert!(s.local.iter().all(|l| l.picked), "no longer listed to him");
    assert_eq!(s.local_for("facundo@radiopost.org").len(), 0);
}
