// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-6 against the vectors: the door's answers (`email-door.json`), emails made into messages,
//! a bounce and the message part between post offices (`email-in.json`), and messages made into
//! emails (`email-out.json`). A mismatch is a bug here, never a reason to change a vector.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use ed25519_dalek::SigningKey;
use radiopost::cbor::Value;
use radiopost::credential::{PostOfficeKey, Trust};
use radiopost::crypto::ContentId;
use radiopost::message::Message;
use radiopost::statement::{Form, Statement};
use radiopost_postoffice::mail::{self, Arrival, Bounce, Carried, Door, Leaving, Verdicts};
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

fn strings(v: &Json) -> Vec<String> {
    v.as_array()
        .expect("a list")
        .iter()
        .map(|s| s.as_str().expect("text").to_owned())
        .collect()
}

/// `sent_by_this_post_office`: content IDs by `Message-ID`.
fn sent_ids(v: &Json) -> HashMap<String, [u8; 12]> {
    v.as_object()
        .expect("a map")
        .iter()
        .map(|(k, h)| {
            (
                k.clone(),
                unhex(h.as_str().expect("hex"))
                    .try_into()
                    .expect("12 bytes"),
            )
        })
        .collect()
}

fn trust() -> Trust {
    let mut trust = Trust::default();
    for k in vectors("keys.json")["keys"].as_array().expect("keys") {
        let label = k["label"].as_str().expect("label");
        if let Some(domain) = label.strip_prefix("po:") {
            let domain = domain.split(':').next().unwrap_or(domain).to_owned();
            let seed: [u8; 32] = unhex(k["seed_hex"].as_str().expect("seed"))
                .try_into()
                .expect("32");
            trust.keys.entry(domain).or_default().push(PostOfficeKey {
                public: SigningKey::from_bytes(&seed).verifying_key().to_bytes(),
                not_before: k["not_before"].as_u64().expect("validity"),
                not_after: k["not_after"].as_u64().expect("validity"),
            });
        }
    }
    trust
}

fn message(name: &str) -> Message {
    let m = vectors("messages.json")["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|m| m["name"] == name)
        .and_then(|m| m["canonical_hex"].as_str())
        .map(unhex)
        .expect("a message");
    Message::from_canonical(&m).expect("canonical")
}

#[test]
fn the_door_answers_as_rp6_2_1_says() {
    let file = vectors("email-door.json");
    let ctx = &file["context"];
    let accounts: BTreeMap<String, Vec<String>> = ctx["accounts"]
        .as_object()
        .expect("accounts")
        .iter()
        .map(|(a, l)| (a.clone(), strings(l)))
        .collect();
    let known = strings(&ctx["known_post_offices"]);
    let sent = sent_ids(&ctx["sent_by_this_post_office"]);
    let is_sent = |id: &str| sent.contains_key(id);
    for case in file["cases"].as_array().expect("cases") {
        let what = case["what"].as_str().expect("what");
        let door = Door {
            accounts: &accounts,
            known_post_offices: &known,
            max_email_bytes: case["max_email_bytes"]
                .as_u64()
                .or_else(|| ctx["max_email_bytes"].as_u64())
                .expect("a limit"),
            sent: &is_sent,
        };
        let verdicts = Verdicts {
            dmarc: case["dmarc"].as_str().expect("dmarc"),
            dkim_pass: &strings(&case["dkim_pass"]),
        };
        let (rcpt, data) = mail::door(
            case["mail_from"].as_str().expect("from"),
            case["rcpt_to"].as_str().expect("to"),
            case["email"].as_str().expect("email").as_bytes(),
            &verdicts,
            &door,
        );
        assert_eq!(rcpt, case["expect"]["rcpt"], "{what}: RCPT TO");
        assert_eq!(
            data.map_or(Json::Null, Json::from),
            case["expect"]["data"],
            "{what}: DATA"
        );
    }
}

/// Everything but the text, for a message whose text came from HTML.
fn without_body(m: &Message) -> Vec<(Value, Value)> {
    m.map()
        .as_map()
        .expect("a map")
        .iter()
        .filter(|(k, _)| k.as_i64() != Some(radiopost::message::key::BODY))
        .cloned()
        .collect()
}

#[test]
fn emails_become_the_messages_rp6_2_4_says() {
    let file = vectors("email-in.json");
    let ctx = &file["context"];
    let sent = sent_ids(&ctx["sent_by_this_post_office"]);
    let lookup = |id: &str| sent.get(id).copied();
    for e in file["emails"].as_array().expect("emails") {
        let name = e["name"].as_str().expect("name");
        let held_refs = e["context"]["held_refs"]
            .as_array()
            .map(|r| {
                r.iter()
                    .map(|h| unhex(h.as_str().expect("hex")).try_into().expect("8 bytes"))
                    .collect()
            })
            .unwrap_or_default();
        let arrival = Arrival {
            arrival: ctx["arrival"].as_u64().expect("arrival"),
            attach_bytes: ctx["attach_bytes"].as_u64().expect("attach_bytes"),
            max_air_bytes: ctx["max_air_bytes"].as_u64().expect("max_air_bytes"),
            sent: &lookup,
            held_refs,
        };
        let made = mail::to_message(e["email"].as_str().expect("email").as_bytes(), &arrival)
            .unwrap_or_else(|why| panic!("{name}: {why}"));
        let want = Message::from_canonical(&unhex(e["canonical_hex"].as_str().expect("hex")))
            .expect("canonical");
        assert_eq!(made.from_html, e["exact"] == false, "{name}: from HTML");
        if e["exact"] == true {
            assert_eq!(
                made.message.map().diagnostic(),
                want.map().diagnostic(),
                "{name}"
            );
            assert_eq!(
                made.message.content_id().text(),
                e["content_id_text"],
                "{name}"
            );
        } else {
            assert_eq!(without_body(&made.message), without_body(&want), "{name}");
        }
    }

    let b = &file["bounce"];
    let made = mail::bounce(
        b["email"].as_str().expect("email").as_bytes(),
        &Bounce {
            domain: "radiopost.org",
            sender: "facundo@radiopost.org",
            arrival: b["arrival"].as_u64().expect("arrival"),
            sent: &lookup,
        },
    )
    .expect("a bounce");
    assert_eq!(
        radiopost::cbor::hex(made.canonical()),
        b["canonical_hex"].as_str().expect("hex"),
        "{}",
        made.map().diagnostic()
    );

    let trust = trust();
    for case in file["between"].as_array().expect("between") {
        let what = case["what"].as_str().expect("what");
        let raw = case["email"].as_str().expect("email").as_bytes();
        let recipient = case["recipient"].as_str().expect("recipient");
        let part = mail::find_part(raw).expect("a part");
        let checked = mail::check_part(&part, recipient, &trust);
        assert_eq!(
            checked.as_ref().map_or_else(|c| c.name(), |_| "ok"),
            case["part_check"],
            "{what}"
        );
        let used = checked.unwrap_or_else(|_| {
            let arrival = Arrival {
                arrival: ctx["arrival"].as_u64().expect("arrival"),
                attach_bytes: ctx["attach_bytes"].as_u64().expect("attach_bytes"),
                max_air_bytes: ctx["max_air_bytes"].as_u64().expect("max_air_bytes"),
                sent: &|_| None,
                held_refs: Vec::new(),
            };
            mail::to_message(raw, &arrival).expect("converted").message
        });
        assert_eq!(
            radiopost::cbor::hex(used.canonical()),
            case["canonical_hex"].as_str().expect("hex"),
            "{what}: {}",
            used.map().diagnostic()
        );
        assert_eq!(used.content_id().text(), case["content_id_text"], "{what}");
    }
}

/// A header's value as a reader sees it: unfolded, RFC 2047 decoded.
fn decoded(value: &str) -> String {
    let raw = format!("Subject: {value}\r\n\r\n");
    mail::parse(raw.as_bytes())
        .and_then(|m| m.subject().map(str::to_owned))
        .unwrap_or_default()
}

/// Session-01's statement and signature, rebuilt from its `HELLO` as a gateway does.
fn session_01_proof() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let file = vectors("session-01.json");
    let first = unhex(file["transmissions"][0]["bytes_hex"].as_str().expect("hex"));
    let hello = radiopost::pdu::unframe(&first).expect("HELLO");
    let text = |k| {
        hello
            .get(k)
            .and_then(Value::as_text)
            .expect("text")
            .to_owned()
    };
    let stmt = Statement {
        address: text(3),
        operator: text(4),
        gateway: text(5),
        messages: hello
            .get(8)
            .and_then(Value::as_array)
            .expect("offers")
            .iter()
            .filter_map(|o| ContentId::from_slice(o.as_array()?.first()?.as_bytes()?))
            .collect(),
        form: Form::Time {
            time: hello.get(6).and_then(Value::as_u64).expect("time"),
            nonce: hello
                .get(7)
                .and_then(Value::as_bytes)
                .and_then(|n| n.try_into().ok())
                .expect("nonce"),
        },
    };
    let credential = vectors("credentials.json")["credentials"]
        .as_array()
        .expect("credentials")
        .iter()
        .find(|c| c["name"] == "facundo")
        .and_then(|c| c["credential_hex"].as_str())
        .map(unhex)
        .expect("Facundo's credential");
    let signature = hello
        .get(15)
        .and_then(Value::as_bytes)
        .expect("signature")
        .to_vec();
    (credential, stmt.encode(), signature)
}

#[test]
fn messages_become_the_emails_rp6_3_says() {
    let file = vectors("email-out.json");
    let facundo = Carried {
        operator: "KK4ODA".into(),
        gateways: vec!["N1CALL-10".into()],
        started: 1_794_235_240,
        session: "ec7b31841ec47c47".into(),
        proof: Some(session_01_proof()),
    };
    let shelter = Carried {
        operator: "N3CALL".into(),
        gateways: vec!["N2CALL-5".into()],
        started: 1_794_234_060,
        session: "86e5b36c010212ce".into(),
        proof: None,
    };
    let maria_419 = message("maria-419").content_id().0;
    let ids = |id: &[u8]| (id == maria_419).then(|| "<CAF.maria.419@mail.example.com>".to_owned());
    for e in file["emails"].as_array().expect("emails") {
        let name = e["message"].as_str().expect("name");
        let m = message(name);
        let (display, path, with_part) = match name {
            "facundo-reply" => ("Facundo", &facundo, false),
            "facundo-to-ana" => ("Facundo", &facundo, true),
            _ => ("Shelter 3", &shelter, false),
        };
        let (headers, email) = mail::from_message(
            &m,
            &Leaving {
                display_name: display,
                path,
                message_id: &ids,
                with_part,
            },
        );
        let got: HashMap<&str, String> = headers
            .iter()
            .map(|(k, v)| (k.as_str(), decoded(v)))
            .collect();
        for (k, want) in e["headers"].as_object().expect("headers") {
            if k == "Content-Type" && want.as_str().is_some_and(|w| w.starts_with("multipart")) {
                assert!(
                    got[k.as_str()].starts_with("multipart/mixed"),
                    "{name}: {k}"
                );
                continue;
            }
            let want = decoded(want.as_str().expect("text"));
            assert_eq!(got.get(k.as_str()), Some(&want), "{name}: {k}");
        }
        assert_eq!(
            got.len(),
            e["headers"].as_object().expect("headers").len(),
            "{name}"
        );
        // the decoded text is the body and the footer
        let parsed = mail::parse(&email).expect("an email");
        let body = m
            .map()
            .get(radiopost::message::key::BODY)
            .and_then(Value::as_text)
            .expect("a body");
        let text = mail::text(&parsed.body_text(0).expect("a text part"));
        assert_eq!(text, format!("{body}{}", mail::FOOTER), "{name}");
        let part = mail::find_part(&email).map(|p| radiopost::cbor::hex(&p));
        assert_eq!(part.as_deref(), e["part_hex"].as_str(), "{name}");
    }
}
