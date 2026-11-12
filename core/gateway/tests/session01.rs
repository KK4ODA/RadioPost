// SPDX-License-Identifier: MIT OR Apache-2.0
//! The gateway's engine against the session vectors: given the client's side of `session-01` and
//! the post office itself behind it (in process), it writes the gateway's side byte for byte.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ed25519_dalek::SigningKey;
use radiopost::credential::{PostOfficeKey, Trust};
use radiopost::crypto;
use radiopost::message::{Compression, Message, key};
use radiopost::session::Caps;
use radiopost::session::gateway::{Gateway, GatewayConfig, Upstreams};
use radiopost_gateway::upstream;
use radiopost_postoffice::{Config, PostOffice, Request, State};
use serde_json::Value as Json;
use sha2::{Digest as _, Sha256};

const PO: &str = "https://po.radiopost.org/rp/v1";
const T_SESSION: u64 = 1_794_235_200;

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

fn stream(label: &str, n: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while out.len() < n {
        out.extend(Sha256::digest(
            format!("RadioPost test bytes: {label}: {i}").as_bytes(),
        ));
        i += 1;
    }
    out.truncate(n);
    out
}

struct World {
    seeds: HashMap<String, [u8; 32]>,
    trust: Trust,
    credentials: HashMap<String, (Vec<u8>, String)>,
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
        if let Some(domain) = label.strip_prefix("po:") {
            let domain = domain.split(':').next().unwrap_or(domain).to_owned();
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
                (
                    unhex(c["credential_hex"].as_str().expect("hex")),
                    c["device"].as_str().expect("device").to_owned(),
                ),
            )
        })
        .collect();
    let messages = vectors("messages.json")["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|m| {
            let mut air = vec![0];
            air.extend(unhex(m["canonical_hex"].as_str().expect("hex")));
            (
                m["name"].as_str().expect("name").to_owned(),
                Message::from_air(&air).expect("a message"),
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

/// radiopost.org with Facundo's #419-#421 waiting, and N1CALL-10's token.
fn post_office(w: &World) -> (Arc<Mutex<PostOffice>>, String) {
    let (office, mut tokens) = post_office_with(
        w,
        &[(419, "maria-419"), (420, "eoc-420"), (421, "maria-421")],
        &["gateway-n1call-10"],
    );
    (office, tokens.remove(0))
}

/// radiopost.org with these messages waiting for Facundo, and these gateways' tokens.
fn post_office_with(
    w: &World,
    waiting: &[(u64, &str)],
    gateways: &[&str],
) -> (Arc<Mutex<PostOffice>>, Vec<String>) {
    let config = Config {
        domains: vec!["radiopost.org".into()],
        base_url: PO.into(),
        key: SigningKey::from_bytes(&w.seeds["po:radiopost.org"]),
        trust: w.trust.clone(),
        operators: Vec::new(),
        applications_open: false,
    };
    let mut office = PostOffice::new(
        config,
        State {
            next_serial: 1050,
            ..State::default()
        },
        Box::new(stream),
    );
    for &(serial, name) in waiting {
        let m = &w.messages[name];
        let field = |k| m.map().get(k).cloned();
        office.at(field(key::DATE).and_then(|d| d.as_u64()).expect("a date"));
        let from = m.from().expect("a sender").to_owned();
        let third = !from.ends_with("@radiopost.org") && !from.ends_with("@hampost.org");
        office.deliver(
            "facundo@radiopost.org",
            serial,
            &m.air_form(Compression::None),
            &from,
            field(key::SUBJECT)
                .and_then(|s| s.as_text().map(str::to_owned))
                .as_deref()
                .unwrap_or(""),
            field(key::PRIORITY).and_then(|p| p.as_u64()).unwrap_or(0),
            third,
        );
    }
    let mut tokens = Vec::new();
    for gateway in gateways {
        tokens.push(login(&mut office, w, gateway));
    }
    (Arc::new(Mutex::new(office)), tokens)
}

fn login(office: &mut PostOffice, w: &World, credential: &str) -> String {
    login_at(office, w, credential, T_SESSION - 600)
}

fn login_at(office: &mut PostOffice, w: &World, credential: &str, at: u64) -> String {
    let request = |m: &str, p: &str, body: Option<Json>| Request {
        method: m.into(),
        path: p.into(),
        token: None,
        body,
    };
    let c = office.handle(at, &request("POST", "/auth/challenge", None));
    let nonce = c.body.expect("a body")["nonce"]
        .as_str()
        .expect("a nonce")
        .to_owned();
    let (cred, device) = &w.credentials[credential];
    let mut signed = radiopost::base64url::decode(&nonce).expect("base64url");
    signed.extend_from_slice(PO.as_bytes());
    let sig = crypto::sign(
        crypto::context::API_LOGIN,
        &signed,
        &SigningKey::from_bytes(&w.seeds[device]),
    );
    let body = serde_json::json!({"credential": radiopost::base64url::encode(cred), "nonce": nonce,
                                  "signature": radiopost::base64url::encode(&sig)});
    let t = office.handle(at, &request("POST", "/auth/token", Some(body)));
    t.body.expect("a token")["token"]
        .as_str()
        .expect("a token")
        .to_owned()
}

#[test]
fn the_gateway_writes_session_01s_gateway_side_byte_for_byte() {
    let w = world();
    let (office, token) = post_office(&w);
    let time = T_SESSION + 41;
    let cfg = GatewayConfig {
        callsign: "N1CALL-10".into(),
        time,
        session_id: stream("session-01 id", 8).try_into().expect("8 bytes"),
        challenge: [0; 16],
        caps: Caps {
            compression: vec![0, 1],
            max_pdu: 4096,
            renew: false,
        },
        piece: 4096,
        credentials: vec![w.credentials["facundo"].0.clone()],
        uploads: HashMap::new(),
    };
    let upstreams: Upstreams = Box::new(move |domain| {
        assert_eq!(domain, "radiopost.org");
        Ok(upstream::in_process(
            office.clone(),
            token.clone(),
            Arc::new(move || time),
        ))
    });
    let mut gateway = Gateway::new(
        cfg,
        upstreams,
        Box::new(|call| radiopost::thirdparty::may_hand(call, "US")),
    );
    let session = vectors("session-01.json");
    for t in session["transmissions"].as_array().expect("transmissions") {
        let bytes = unhex(t["bytes_hex"].as_str().expect("hex"));
        let n = t["number"].as_u64().expect("number");
        if t["from"] == "client" {
            gateway.receive(&bytes);
        } else {
            let wrote = gateway.transmit().expect("a transmission due");
            assert_eq!(
                radiopost::cbor::hex(&wrote),
                radiopost::cbor::hex(&bytes),
                "transmission {n}"
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

#[test]
#[allow(clippy::too_many_lines, reason = "two sessions, told in order")]
fn uploads_and_downloads_resume_across_gateways_as_session_02_says() {
    let w = world();
    // the report waits to go up; #421 waits to come down (Facundo's cursor is 420)
    let (office, _) = post_office_with(&w, &[(421, "maria-421")], &[]);
    let file = vectors("session-02.json");
    let mut uploads: HashMap<(String, radiopost::crypto::ContentId), (Vec<u8>, u64)> =
        HashMap::new();
    let mut held: HashMap<&str, Vec<Vec<u8>>> = HashMap::from([
        ("N1CALL-10", vec![w.credentials["facundo"].0.clone()]),
        ("N2CALL-5", vec![]),
    ]);
    for s in file["sessions"].as_array().expect("sessions") {
        let name = s["session"].as_str().expect("a name");
        let callsign = if s["gateway"]
            .as_str()
            .expect("gateway")
            .starts_with("N2CALL-5")
        {
            "N2CALL-5"
        } else {
            "N1CALL-10"
        };
        let time: u64 = s["gateway_clock"].as_u64().expect("a clock");
        let credential = if callsign == "N2CALL-5" {
            "gateway-n2call-5"
        } else {
            "gateway-n1call-10"
        };
        let token = login_at(
            &mut office.lock().expect("the office"),
            &w,
            credential,
            time - 60,
        );
        let cfg = GatewayConfig {
            callsign: callsign.into(),
            time,
            session_id: unhex(s["session_id_hex"].as_str().expect("id"))
                .try_into()
                .expect("8 bytes"),
            challenge: [0; 16],
            caps: Caps {
                compression: vec![0, 1],
                max_pdu: 4096,
                renew: false,
            },
            piece: 960,
            credentials: held[callsign].clone(),
            uploads: if callsign == "N1CALL-10" {
                uploads.clone()
            } else {
                HashMap::new()
            },
        };
        let office = office.clone();
        let upstreams: Upstreams = Box::new(move |_| {
            Ok(upstream::in_process(
                office.clone(),
                token.clone(),
                Arc::new(move || time),
            ))
        });
        let mut gateway = Gateway::new(
            cfg,
            upstreams,
            Box::new(|call| radiopost::thirdparty::may_hand(call, "US")),
        );
        for t in s["transmissions"].as_array().expect("transmissions") {
            let bytes = unhex(t["bytes_hex"].as_str().expect("hex"));
            let n = t["number"].as_u64().expect("number");
            let cut = t["broken"]["received_bytes"].as_u64();
            if t["from"] == "client" {
                let got = cut.map_or(bytes.len(), |c| usize::try_from(c).expect("a size"));
                gateway.receive(&bytes[..got]);
            } else {
                let wrote = gateway.transmit().expect("a transmission due");
                assert_eq!(
                    radiopost::cbor::hex(&wrote),
                    radiopost::cbor::hex(&bytes),
                    "session {name}, transmission {n}"
                );
            }
            if cut.is_some() {
                break;
            }
        }
        gateway.link_closed();
        for note in gateway.take_notes() {
            match note {
                radiopost::session::gateway::Note::Upload {
                    address,
                    id,
                    held: bytes,
                    total,
                } if callsign == "N1CALL-10" => {
                    if bytes.is_empty() {
                        uploads.remove(&(address, id));
                    } else {
                        uploads.insert((address, id), (bytes, total));
                    }
                }
                radiopost::session::gateway::Note::Credential(c) => {
                    let list = held.get_mut(callsign).expect("a gateway");
                    if !list.contains(&c) {
                        list.push(c);
                    }
                }
                _ => {}
            }
        }
    }
    // the report reached the post office, and #421 was picked up through N2CALL-5
    let office = office.lock().expect("the office");
    assert_eq!(office.state.had.len(), 1);
    assert!(
        office.state.mailboxes["facundo@radiopost.org"]
            .iter()
            .all(|m| m.picked)
    );
}

/// One call as session-01's client makes it, then `REQUEST` asking for `files` of #422
/// (`maria-house`), held at radiopost.org: the PDUs of the gateway's answer, and the post office.
/// `maria-house`'s held attachments: each its name and ref.
fn held_refs(w: &World) -> Vec<(String, Vec<u8>)> {
    use radiopost::cbor::Value;
    w.messages["maria-house"]
        .map()
        .get(key::HELD)
        .and_then(Value::as_array)
        .expect("held attachments")
        .iter()
        .map(|f| {
            (
                f.get(1)
                    .and_then(Value::as_text)
                    .expect("a name")
                    .to_owned(),
                f.get(4).and_then(Value::as_bytes).expect("a ref").to_vec(),
            )
        })
        .collect()
}

/// `make.py`'s `HELD_FILES`: each of `maria-house`'s held attachments' data, by its ref.
fn held_data(w: &World) -> HashMap<Vec<u8>, Vec<u8>> {
    held_refs(w)
        .iter()
        .map(|(name, r)| {
            let (label, n) = match name.as_str() {
                "house-1.jpg" => ("held photo 1", 40_000),
                "house-2.jpg" => ("held photo 2", 40_000),
                _ => ("held video", 70_000),
            };
            (r.clone(), stream(label, n))
        })
        .collect()
}

fn ask_for_held(
    w: &World,
    names: &[&str],
) -> (Vec<radiopost::cbor::Value>, Arc<Mutex<PostOffice>>) {
    use radiopost::cbor::Value;
    let (office, mut tokens) = post_office_with(
        w,
        &[
            (419, "maria-419"),
            (420, "eoc-420"),
            (421, "maria-421"),
            (422, "maria-house"),
        ],
        &["gateway-n1call-10"],
    );
    let token = tokens.remove(0);
    let data = held_data(w);
    office
        .lock()
        .expect("lock")
        .keep_held(Box::new(move |r| data.get(r).cloned()), 65_536);
    let time = T_SESSION + 41;
    let cfg = GatewayConfig {
        callsign: "N1CALL-10".into(),
        time,
        session_id: stream("session-01 id", 8).try_into().expect("8 bytes"),
        challenge: [0; 16],
        caps: Caps {
            compression: vec![0, 1],
            max_pdu: 4096,
            renew: false,
        },
        piece: 4096,
        credentials: vec![w.credentials["facundo"].0.clone()],
        uploads: HashMap::new(),
    };
    let up = office.clone();
    let upstreams: Upstreams = Box::new(move |_| {
        Ok(upstream::in_process(
            up.clone(),
            token.clone(),
            Arc::new(move || time),
        ))
    });
    let mut gateway = Gateway::new(
        cfg,
        upstreams,
        Box::new(|call| radiopost::thirdparty::may_hand(call, "US")),
    );
    let session = vectors("session-01.json");
    let hello = &session["transmissions"][0];
    assert_eq!(hello["from"], "client");
    gateway.receive(&unhex(hello["bytes_hex"].as_str().expect("hex")));
    gateway.transmit().expect("WELCOME");
    let held = held_refs(w);
    let files: Vec<Value> = names
        .iter()
        .map(|name| {
            let r = &held
                .iter()
                .find(|(n, _)| n == name)
                .expect("a held attachment")
                .1;
            Value::Array(vec![Value::Uint(422), Value::Bytes(r.clone())])
        })
        .collect();
    let request = Value::int_map(vec![
        (0, Value::Uint(radiopost::pdu::kind::REQUEST)),
        (2, Value::Array(files)),
    ]);
    gateway.receive(&radiopost::pdu::frame(&request).expect("a frame"));
    let answer = gateway.transmit().expect("an answer");
    let mut deframer = radiopost::pdu::Deframer::new();
    deframer.push(&answer);
    let mut pdus = Vec::new();
    while let Some(p) = deframer.next(65_536) {
        pdus.push(p.expect("a PDU"));
    }
    gateway.link_closed();
    (pdus, office)
}

/// RP-3 §5.6 `files`: the gateway asks the post office (RP-5 §5.3) and says how it went in a
/// `NOTICE` before its `DONE`.
#[test]
fn held_attachments_asked_for_are_passed_on_with_a_notice() {
    use radiopost::cbor::Value;
    use radiopost::pdu::kind;
    let w = world();
    let said = |pdus: &[Value]| -> Vec<(u64, Option<String>)> {
        pdus.iter()
            .map(|p| {
                (
                    radiopost::pdu::kind_of(p).expect("a kind"),
                    p.get(1).and_then(Value::as_text).map(str::to_owned),
                )
            })
            .collect()
    };

    let (pdus, office) = ask_for_held(&w, &["house-1.jpg", "house-2.jpg"]);
    let ok = radiopost::session::gateway::files_notice(Ok(()));
    assert_eq!(said(&pdus), [(kind::NOTICE, Some(ok)), (kind::DONE, None)]);
    let office = office.lock().expect("lock");
    let new: Vec<(u64, &str)> = office.state.mailboxes["facundo@radiopost.org"]
        .iter()
        .filter(|m| m.serial > 422)
        .map(|m| (m.serial, m.from.as_str()))
        .collect();
    assert_eq!(
        new,
        [
            (423, "postmaster@radiopost.org"),
            (424, "postmaster@radiopost.org")
        ]
    );
    drop(office);

    let (pdus, office) = ask_for_held(&w, &["house.mp4"]);
    let too_large = radiopost::session::gateway::files_notice(Err(radiopost::ErrorCode::TooLarge));
    assert_eq!(
        said(&pdus),
        [(kind::NOTICE, Some(too_large)), (kind::DONE, None)]
    );
    assert_eq!(
        office.lock().expect("lock").state.mailboxes["facundo@radiopost.org"].len(),
        4,
        "nothing sent"
    );
}
