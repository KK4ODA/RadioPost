// SPDX-License-Identifier: MIT OR Apache-2.0
//! The library against the test vectors in `vectors/` (vectors/README.md, "Using them"): make
//! each object and compare the bytes, check each case and compare the verdict, refuse each
//! `invalid` item. A mismatch is a bug in this library, never a reason to edit a vector.

use std::collections::HashMap;
use std::path::PathBuf;

use ed25519_dalek::SigningKey;
use radiopost::cbor::{self, hex};
use radiopost::credential::{self, Body, Credential, PostOfficeKey, Revocations, Trust, flags};
use radiopost::crypto::{self, ContentId, context};
use radiopost::message::{Compression, Message};
use radiopost::pdu;
use radiopost::statement::{self, Freshness, Statement};
use serde_json::Value as Json;

fn vectors(name: &str) -> Json {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vectors")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("a vector file is JSON")
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

fn text<'a>(j: &'a Json, key: &str) -> &'a str {
    j[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key} is text in {j}"))
}

fn bytes(j: &Json, key: &str) -> Vec<u8> {
    unhex(text(j, key))
}

fn array<const N: usize>(v: &[u8]) -> [u8; N] {
    v.try_into().expect("the right length")
}

struct Keys {
    seeds: HashMap<String, [u8; 32]>,
    trust: Vec<(String, PostOfficeKey)>,
}

fn keys() -> Keys {
    let file = vectors("keys.json");
    let mut seeds = HashMap::new();
    let mut trust = Vec::new();
    for k in file["keys"].as_array().expect("keys") {
        let label = text(k, "label").to_owned();
        let seed = array(&bytes(k, "seed_hex"));
        let public = SigningKey::from_bytes(&seed).verifying_key().to_bytes();
        assert_eq!(
            public.to_vec(),
            bytes(k, "public_hex"),
            "{label}: public key from seed"
        );
        assert_eq!(
            crypto::kid(&public).to_vec(),
            bytes(k, "kid_hex"),
            "{label}: kid"
        );
        if let Some(domain) = label.strip_prefix("po:") {
            let domain = domain.split(':').next().unwrap_or(domain).to_owned();
            let not_before = k["not_before"].as_u64().expect("validity");
            let not_after = k["not_after"].as_u64().expect("validity");
            trust.push((
                domain,
                PostOfficeKey {
                    public,
                    not_before,
                    not_after,
                },
            ));
        }
        seeds.insert(label, seed);
    }
    Keys { seeds, trust }
}

impl Keys {
    fn signing(&self, label: &str) -> SigningKey {
        SigningKey::from_bytes(&self.seeds[label])
    }

    fn trust(&self, known: &[&str], revoked: &Json) -> Trust {
        let mut trust = Trust::default();
        for (domain, key) in &self.trust {
            if known.contains(&domain.as_str()) {
                trust
                    .keys
                    .entry(domain.clone())
                    .or_default()
                    .push(key.clone());
            }
        }
        for (domain, r) in revoked.as_object().into_iter().flatten() {
            let serials = r["serials"].as_array().into_iter().flatten();
            let kids = r["kids"].as_array().into_iter().flatten();
            trust.revoked.insert(
                domain.clone(),
                Revocations {
                    serials: serials.filter_map(Json::as_u64).collect(),
                    kids: kids
                        .map(|k| array(&unhex(k.as_str().expect("kid"))))
                        .collect(),
                },
            );
        }
        trust
    }
}

#[test]
fn keys_derive_as_published() {
    let k = keys();
    assert_eq!(k.seeds.len(), 12);
}

#[test]
fn cbor_encodes_and_refuses_as_rp0_says() {
    let file = vectors("cbor.json");
    for v in file["valid"].as_array().expect("valid") {
        let raw = bytes(v, "hex");
        let value = cbor::decode(&raw).unwrap_or_else(|e| panic!("{}: {e}", text(v, "what")));
        assert_eq!(value.encode().expect("encodes"), raw, "{}", text(v, "what"));
        assert_eq!(
            value.diagnostic(),
            text(v, "diagnostic"),
            "{}",
            text(v, "what")
        );
    }
    for v in file["invalid"].as_array().expect("invalid") {
        assert!(
            cbor::decode(&bytes(v, "hex")).is_err(),
            "took invalid CBOR: {}",
            text(v, "why")
        );
    }
}

#[test]
fn messages_have_their_ids_and_air_forms() {
    let file = vectors("messages.json");
    for m in file["messages"].as_array().expect("messages") {
        let name = text(m, "name");
        let canonical = bytes(m, "canonical_hex");
        let message = Message::from_canonical(&canonical).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(message.map().diagnostic(), text(m, "diagnostic"), "{name}");
        assert_eq!(
            message.content_id().0.to_vec(),
            bytes(m, "content_id_hex"),
            "{name}"
        );
        assert_eq!(
            message.content_id().text(),
            text(m, "content_id_text"),
            "{name}"
        );
        assert_eq!(
            ContentId::from_text(text(m, "content_id_text")),
            Some(message.content_id())
        );
        for air in m["air"].as_array().expect("air forms") {
            let raw = bytes(air, "hex");
            let back = Message::from_air(&raw).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(
                back.canonical(),
                canonical.as_slice(),
                "{name}: air form decodes"
            );
            match air["compression"].as_u64() {
                Some(0) => assert_eq!(message.air_form(Compression::None), raw, "{name}"),
                Some(1) => {
                    // Inexact (vectors/README.md): any valid stream; ours must decode too.
                    let ours = message.air_form(Compression::Deflate);
                    assert_eq!(Message::from_air(&ours).expect("ours decodes"), message);
                }
                other => panic!("{name}: compression {other:?}"),
            }
        }
    }
    for bad in file["invalid"].as_array().expect("invalid") {
        assert!(
            Message::from_air(&bytes(bad, "air_hex")).is_err(),
            "took: {}",
            text(bad, "why")
        );
    }
    for bad in file["invalid_ids"].as_array().expect("invalid_ids") {
        assert_eq!(
            ContentId::from_text(text(bad, "text")),
            None,
            "took: {}",
            text(bad, "why")
        );
    }
}

fn need(j: &Json) -> u64 {
    j["need"]
        .as_array()
        .expect("need")
        .iter()
        .fold(0, |acc, f| {
            acc | match f.as_str() {
                Some("send") => flags::SEND,
                Some("receive") => flags::RECEIVE,
                Some("role") => flags::ROLE,
                Some("gateway") => flags::GATEWAY,
                other => panic!("flag {other:?}"),
            }
        })
}

fn verdict<T, E: std::fmt::Display>(result: &Result<T, E>) -> String {
    match result {
        Ok(_) => "ok".to_owned(),
        Err(e) => e.to_string(),
    }
}

#[test]
fn credentials_are_made_and_checked_as_rp1_says() {
    let k = keys();
    let file = vectors("credentials.json");
    let mut by_name = HashMap::new();
    for c in file["credentials"].as_array().expect("credentials") {
        let name = text(c, "name");
        let b = &c["body"];
        let body = Body {
            v: b["v"].as_u64().expect("v"),
            serial: b["serial"].as_u64().expect("serial"),
            address: text(b, "address").to_owned(),
            operator: text(b, "operator").to_owned(),
            device: array(&bytes(b, "device_hex")),
            issued: b["issued"].as_u64().expect("issued"),
            expires: b["expires"].as_u64().expect("expires"),
            flags: b["flags"].as_u64().expect("flags"),
        };
        let made = Credential::issue(&body, &k.signing(text(c, "signed_by")));
        assert_eq!(made.sign1.kid.to_vec(), bytes(c, "kid_hex"), "{name}");
        assert_eq!(made.sign1.payload, bytes(c, "payload_hex"), "{name}");
        assert_eq!(made.sign1.protected, bytes(c, "protected_hex"), "{name}");
        let signed = crypto::to_be_signed(
            context::CREDENTIAL,
            &made.sign1.protected,
            &made.sign1.payload,
        );
        assert_eq!(signed, bytes(c, "to_be_signed_hex"), "{name}");
        assert_eq!(
            made.sign1.signature.to_vec(),
            bytes(c, "signature_hex"),
            "{name}"
        );
        let encoded = made.encode();
        assert_eq!(encoded, bytes(c, "credential_hex"), "{name}");
        assert_eq!(
            credential::hash(&encoded).to_vec(),
            bytes(c, "credential_hash_hex"),
            "{name}"
        );
        assert_eq!(
            Credential::decode(&encoded),
            Some(made),
            "{name}: decodes as made"
        );
        by_name.insert(name.to_owned(), encoded);
    }
    for case in file["verification"].as_array().expect("verification") {
        let what = text(case, "what");
        let raw = match case["credential_hex"].as_str() {
            Some(h) => unhex(h),
            None => by_name[text(case, "credential")].clone(),
        };
        let known: Vec<&str> = case["known_post_offices"]
            .as_array()
            .expect("known")
            .iter()
            .filter_map(Json::as_str)
            .collect();
        let trust = k.trust(&known, &case["revoked"]);
        let now = case["now"].as_u64().expect("now");
        let result = credential::verify(&raw, &trust, now, need(case)).map_err(|r| r.code);
        assert_eq!(verdict(&result), text(case, "expect"), "{what}");
    }
}

#[test]
fn statements_are_signed_and_checked_as_rp1_says() {
    let k = keys();
    let creds = vectors("credentials.json");
    let device_of: HashMap<&str, &str> = creds["credentials"]
        .as_array()
        .expect("credentials")
        .iter()
        .map(|c| (text(c, "name"), text(c, "device")))
        .collect();
    let body_of = |name: &str| -> Body {
        let c = creds["credentials"]
            .as_array()
            .expect("credentials")
            .iter()
            .find(|c| text(c, "name") == name);
        Credential::decode(&bytes(c.expect("a credential"), "credential_hex"))
            .expect("decodes")
            .body
    };
    let file = vectors("statements.json");
    for s in file["statements"].as_array().expect("statements") {
        let name = text(s, "name");
        let raw = bytes(s, "statement_hex");
        let st = Statement::decode(&raw).unwrap_or_else(|| panic!("{name}: decodes"));
        assert_eq!(st.encode(), raw, "{name}");
        assert_eq!(st.to_value().diagnostic(), text(s, "diagnostic"), "{name}");
        assert_eq!(
            crypto::signed_input(context::SESSION, &raw),
            bytes(s, "signed_input_hex"),
            "{name}"
        );
        let device = k.signing(device_of[text(s, "credential")]);
        assert_eq!(
            st.sign(&device).to_vec(),
            bytes(s, "signature_hex"),
            "{name}"
        );
    }
    for case in file["verification"].as_array().expect("verification") {
        let what = text(case, "what");
        let st = Statement::decode(&bytes(case, "statement_hex")).expect("a statement");
        let seen: Vec<(String, [u8; 8])> = case["seen"]
            .as_array()
            .expect("seen")
            .iter()
            .map(|p| {
                (
                    p[0].as_str().expect("address").to_owned(),
                    array(&unhex(p[1].as_str().expect("nonce"))),
                )
            })
            .collect();
        let freshness = Freshness {
            now: case["now"].as_u64().expect("now"),
            seen: &seen,
            challenge: case["challenge_hex"].as_str().map(|h| array(&unhex(h))),
            sent_nonce: case["sent_nonce_hex"].as_str().map(|h| array(&unhex(h))),
        };
        let body = body_of(text(case, "credential"));
        let result = statement::verify(
            &st,
            &bytes(case, "signature_hex"),
            &body,
            text(case, "verifier"),
            &freshness,
        );
        assert_eq!(verdict(&result), text(case, "expect"), "{what}");
    }
}

#[test]
fn pdus_frame_and_refuse_as_rp3_says() {
    let file = vectors("pdus.json");
    for p in file["pdus"].as_array().expect("pdus") {
        let name = text(p, "name");
        let value = cbor::decode(&bytes(p, "pdu_hex")).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(value.diagnostic(), text(p, "diagnostic"), "{name}");
        let framed = bytes(p, "frame_hex");
        assert_eq!(pdu::frame(&value).expect("frames"), framed, "{name}");
        assert_eq!(pdu::unframe(&framed).expect("unframes"), value, "{name}");
        let mut stream = pdu::Deframer::new();
        for b in &framed {
            stream.push(&[*b]);
        }
        assert_eq!(
            stream.next(65535),
            Some(Ok(value)),
            "{name}: one byte at a time"
        );
    }
    for bad in file["invalid"].as_array().expect("invalid") {
        let frame = bytes(bad, "frame_hex");
        let got = match bad["max_pdu"].as_u64() {
            // a receiver with a limit reads the frame as it arrives (RP-3 §2)
            Some(max) => {
                let mut stream = pdu::Deframer::new();
                stream.push(&frame);
                stream
                    .next(usize::try_from(max).expect("a size"))
                    .expect("the length is known from the first two bytes")
            }
            None => pdu::unframe(&frame),
        };
        assert_eq!(verdict(&got), text(bad, "expect"), "{}", text(bad, "why"));
    }
    assert_eq!(hex(&[0x0a, 0xff]), "0aff");
}

#[test]
fn public_documents_are_signed_and_checked_as_rp0_and_rp1_say() {
    let keys = keys();
    let file = vectors("jws.json");
    for d in file["documents"].as_array().expect("documents") {
        let key = SigningKey::from_bytes(&keys.seeds[text(d, "signed_by")]);
        let payload = radiopost::base64url::decode(text(d, "payload_b64url")).expect("base64url");
        assert_eq!(
            radiopost::jws::sign(&payload, &key),
            text(d, "jws"),
            "{}",
            text(d, "name")
        );
    }
    let publics: HashMap<String, [u8; 32]> = keys
        .seeds
        .values()
        .map(|s| {
            let public = SigningKey::from_bytes(s).verifying_key().to_bytes();
            (hex(&crypto::kid(&public)), public)
        })
        .collect();
    for c in file["verification"].as_array().expect("verification") {
        let dns: HashMap<String, [u8; 32]> = c["dns_kids"]
            .as_array()
            .expect("dns_kids")
            .iter()
            .map(|k| {
                let k = k.as_str().expect("a kid");
                (k.to_owned(), publics[k])
            })
            .collect();
        let accepted = radiopost::jws::accepted_keys(&dns, c["info_jws"].as_str());
        let got = match radiopost::jws::verify(text(c, "jws"), &accepted) {
            Ok(_) => "ok".to_owned(),
            Err(e) => e.to_string(),
        };
        assert_eq!(got, text(c, "expect"), "{}", text(c, "what"));
    }
}

#[test]
fn third_party_mail_is_listed_only_where_rp6_5_1_allows() {
    let file = vectors("third-party.json");
    let country = text(&file["context"], "gateway_country");
    for c in file["callsign_cases"].as_array().expect("callsign cases") {
        let call = text(c, "callsign");
        assert_eq!(
            radiopost::thirdparty::station_designator(call).as_deref(),
            c["where"].as_str(),
            "{}",
            text(c, "what")
        );
        assert_eq!(
            radiopost::thirdparty::may_hand(call, country),
            c["expect"].as_bool().expect("expect"),
            "{}",
            text(c, "what")
        );
    }
    let flagged: Vec<bool> = file["waiting"]
        .as_array()
        .expect("waiting")
        .iter()
        .map(|w| w["third_party"].as_bool().expect("a flag"))
        .collect();
    for c in file["listing_cases"].as_array().expect("listing cases") {
        let may = radiopost::thirdparty::may_hand(text(c, "operator"), country);
        let held_back = flagged.iter().filter(|f| **f && !may).count() as u64;
        assert_eq!(
            Some(held_back),
            c["held_back"].as_u64(),
            "{}",
            text(c, "operator")
        );
    }
    // the table's own prefixes as written
    assert_eq!(radiopost::thirdparty::prefixes("LO–LW").len(), 9);
}

#[test]
fn dns_records_are_read_and_post_offices_trusted_as_rp1_7_says() {
    use radiopost::dns::{Found, parse, trusts};
    let file = vectors("dns.json");
    for r in file["records"].as_array().expect("records") {
        let got = parse(text(r, "txt")).map(|t| serde_json::to_value(t).expect("JSON"));
        let want = (!r["result"].is_null()).then(|| r["result"].clone());
        assert_eq!(got, want, "{}", text(r, "what"));
    }
    let found: std::collections::BTreeMap<String, Found> = file["found"]
        .as_object()
        .expect("found")
        .iter()
        .map(|(d, how)| {
            let how = how.as_str().expect("how");
            let f = how
                .strip_prefix("list:")
                .map_or(Found::Dns, |by| Found::ListOf(by.to_owned()));
            (d.clone(), f)
        })
        .collect();
    let list = |v: &Json| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    for c in file["trust_cases"].as_array().expect("trust cases") {
        let only = (!c["only"].is_null()).then(|| list(&c["only"]));
        assert_eq!(
            trusts(
                text(c, "domain"),
                &found,
                &list(&c["blocked"]),
                only.as_deref()
            ),
            c["expect"].as_bool().expect("expect"),
            "{}",
            text(c, "what")
        );
    }
}
