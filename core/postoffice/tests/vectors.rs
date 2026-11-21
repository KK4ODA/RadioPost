// SPDX-License-Identifier: MIT OR Apache-2.0
//! The post office against the vectors: every exchange of `api-exchanges.json`, `api-account.json`,
//! `api-office.json` and `direct.json`, replayed in order from the state each one starts from, with the vectors'
//! published randomness, and each answer compared as a JSON value (a problem's `title` and a
//! result's `text` aside). A mismatch is a bug here, never a reason to change a vector.

use std::collections::HashMap;
use std::path::PathBuf;

use ed25519_dalek::SigningKey;
use radiopost::base64url;
use radiopost::credential::{PostOfficeKey, Trust};
use radiopost::crypto;
use radiopost::message::{Compression, Message, key};
use radiopost_postoffice::documents::Info;
use radiopost_postoffice::office::Listed;
use radiopost_postoffice::{Config, PostOffice, Request, State};
use serde_json::{Value as Json, json};
use sha2::{Digest as _, Sha256};

const PO_RADIOPOST: &str = "https://po.radiopost.org/rp/v1";
const T_ISSUED: u64 = 1_793_491_200;
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

/// make.py's `stream`: SHA-256 of the label and a counter, concatenated.
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
    credentials: HashMap<String, (Vec<u8>, String, u64)>,
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
                    c["body"]["serial"].as_u64().expect("serial"),
                ),
            )
        })
        .collect();
    let messages = vectors("messages.json")["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|m| {
            let canonical = unhex(m["canonical_hex"].as_str().expect("hex"));
            let mut air = vec![0];
            air.extend(&canonical);
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

/// radiopost.org as the vectors find it: its key, the post offices it knows, Facundo's tablet's
/// enrollment code, and the next credential serial.
fn post_office(w: &World, next_serial: u64) -> PostOffice {
    let config = Config {
        domains: vec!["radiopost.org".into()],
        base_url: PO_RADIOPOST.into(),
        key: SigningKey::from_bytes(&w.seeds["po:radiopost.org"]),
        trust: w.trust.clone(),
        operators: Vec::new(),
        applications_open: false,
        // the vectors' air forms are uncompressed, so they replay byte for byte (RP-5 §5.3 asks
        // only SHOULD)
        compress: false,
    };
    let state = State {
        next_serial,
        ..State::default()
    };
    let mut office = PostOffice::new(config, state, Box::new(stream));
    office.at(T_ISSUED);
    office.add_code("7Q4F-AZ9K", "facundo@radiopost.org", "KK4ODA", 2);
    office
}

/// Mail from outside RadioPost's post offices was made from an email (RP-6 §5.1).
fn third_party(from: &str) -> bool {
    !matches!(
        from.rsplit_once('@').map(|(_, d)| d),
        Some("radiopost.org" | "hampost.org")
    )
}

/// One message arrives for `address`, at its date.
fn deliver(office: &mut PostOffice, w: &World, address: &str, serial: u64, name: &str) {
    let date = w.messages[name]
        .map()
        .get(key::DATE)
        .and_then(radiopost::cbor::Value::as_u64)
        .expect("a date");
    deliver_at(office, w, address, serial, name, date);
}

/// Mail arrives for `address` at `at`, rather than at its own date.
fn deliver_at(office: &mut PostOffice, w: &World, address: &str, serial: u64, name: &str, at: u64) {
    let m = &w.messages[name];
    let field = |k| m.map().get(k).cloned();
    office.at(at);
    let from = m.from().expect("a sender").to_owned();
    office.deliver(
        address,
        serial,
        &m.air_form(Compression::None),
        &from,
        field(key::SUBJECT)
            .and_then(|s| s.as_text().map(str::to_owned))
            .as_deref()
            .unwrap_or(""),
        field(key::PRIORITY).and_then(|p| p.as_u64()).unwrap_or(0),
        third_party(&from),
    );
}

const WAITING: [(u64, &str); 3] = [(419, "maria-419"), (420, "eoc-420"), (421, "maria-421")];

/// A login nobody records: a challenge and a token, as the vectors' setup does.
fn login(office: &mut PostOffice, w: &World, at: u64, credential: &str) -> String {
    let (cred, device, _) = &w.credentials[credential];
    let challenge = office.handle(at, &request("POST", "/auth/challenge", None, None));
    let nonce = challenge.body.as_ref().expect("a body")["nonce"]
        .as_str()
        .expect("a nonce")
        .to_owned();
    let mut signed = radiopost::base64url::decode(&nonce).expect("base64url");
    signed.extend_from_slice(PO_RADIOPOST.as_bytes());
    let key = SigningKey::from_bytes(&w.seeds[device]);
    let signature = crypto::sign(crypto::context::API_LOGIN, &signed, &key);
    let body = serde_json::json!({"credential": radiopost::base64url::encode(cred),
        "nonce": nonce, "signature": radiopost::base64url::encode(&signature)});
    let answer = office.handle(at, &request("POST", "/auth/token", None, Some(body)));
    answer.body.expect("a token")["token"]
        .as_str()
        .expect("a token")
        .to_owned()
}

fn request(method: &str, path: &str, token: Option<&str>, body: Option<Json>) -> Request {
    Request {
        method: method.into(),
        path: path.into(),
        token: token.map(str::to_owned),
        body,
    }
}

/// An answer as RP-5 compares it: a problem's `title` and each result's `text` left out.
fn comparable(body: Option<&Json>) -> Json {
    let mut body = body.cloned().unwrap_or(Json::Null);
    if let Some(o) = body.as_object_mut()
        && o.contains_key("type")
    {
        o.remove("title");
    }
    strip_text(&mut body);
    body
}

fn strip_text(v: &mut Json) {
    match v {
        Json::Object(o) => {
            if o.get("status").and_then(Json::as_str).is_some() {
                o.remove("text");
            }
            for x in o.values_mut() {
                strip_text(x);
            }
        }
        Json::Array(a) => a.iter_mut().for_each(strip_text),
        _ => {}
    }
}

/// An `smtp` entry (RP-6 §2.1): the door's answers for the accept lists as they stand.
fn door(office: &PostOffice, smtp: &Json, what: &str) {
    use radiopost_postoffice::mail;
    let accounts = office.door_accounts();
    let known = office.known_post_offices();
    let none = |_: &str| false;
    let dkim: Vec<String> = smtp["dkim_pass"]
        .as_array()
        .expect("dkim_pass")
        .iter()
        .map(|d| d.as_str().expect("a domain").to_owned())
        .collect();
    let (rcpt, data) = mail::door(
        smtp["mail_from"].as_str().expect("from"),
        smtp["rcpt_to"].as_str().expect("to"),
        smtp["email"].as_str().expect("email").as_bytes(),
        &mail::Verdicts {
            dmarc: smtp["dmarc"].as_str().expect("dmarc"),
            dkim_pass: &dkim,
        },
        &mail::Door {
            accounts: &accounts,
            known_post_offices: &known,
            max_email_bytes: 10_485_760,
            sent: &none,
        },
    );
    assert_eq!(rcpt, smtp["expect"]["rcpt"], "{what}: RCPT TO");
    assert_eq!(
        data.map_or(Json::Null, Json::from),
        smtp["expect"]["data"],
        "{what}: DATA"
    );
}

/// Replays exchanges against `office`: each request at its clock, each answer as the vector's.
/// A `deliver` hook handles the notes that say mail arrives.
fn replay(
    office: &mut PostOffice,
    steps: &[Json],
    what: &str,
    mut note: impl FnMut(&mut PostOffice, &str),
) {
    for (i, step) in steps.iter().enumerate() {
        if let Some(n) = step.get("note").and_then(Json::as_str) {
            note(office, n);
        }
        if let Some(smtp) = step.get("smtp") {
            door(office, smtp, &format!("{what}, step {i}"));
        }
        // a public document as the post office serves it then (RP-5 §4.6's status list)
        if let Some(path) = step.get("document").and_then(Json::as_str) {
            let at = step["at"].as_u64().expect("at");
            let got = office.document(&info(), path, at).expect("a document");
            assert_eq!(got, step["payload"], "{what}, step {i}: {path}");
            continue;
        }
        let Some(r) = step.get("request").filter(|r| r.is_object()) else {
            continue;
        };
        let token = r
            .get("authorization")
            .and_then(Json::as_str)
            .and_then(|a| a.strip_prefix("Bearer "));
        let got = office.handle(
            step["at"].as_u64().expect("at"),
            &request(
                r["method"].as_str().expect("method"),
                r["path"].as_str().expect("path"),
                token,
                r.get("body").cloned(),
            ),
        );
        let want = &step["response"];
        assert_eq!(
            u64::from(got.status),
            want["status"].as_u64().expect("status"),
            "{what}, step {i}: {} {}\n got {:?}",
            r["method"],
            r["path"],
            got.body
        );
        assert_eq!(
            comparable(got.body.as_ref()),
            comparable(want.get("body")),
            "{what}, step {i}: {} {}",
            r["method"],
            r["path"]
        );
    }
}

/// What the post office says of itself in its info document, for the documents a replay reads.
fn info() -> Info {
    Info {
        operator: "KK4ODA".into(),
        contact: "postmaster@radiopost.org".into(),
        country: "US".into(),
        max_air_bytes: 65_536,
        max_email_bytes: 100_000,
        gateways: "any".into(),
        mail: json!({}),
        updated: 0,
        not_before: 0,
        not_after: u64::MAX,
        post_offices: Vec::new(),
        applications: None,
    }
}

/// The notes that say mail arrives for Facundo, as the story's.
fn arrivals(w: &World) -> impl FnMut(&mut PostOffice, &str) + '_ {
    move |office, note| {
        if let Some(rest) = note.strip_prefix("Mail arrives for facundo@radiopost.org: #") {
            let serial: u64 = rest
                .split(',')
                .next()
                .and_then(|s| s.parse().ok())
                .expect("a serial");
            let name = WAITING
                .iter()
                .find(|(s, _)| *s == serial)
                .expect("a known serial")
                .1;
            deliver(office, w, "facundo@radiopost.org", serial, name);
        }
    }
}

/// make.py's `case_office`: Facundo's mail waiting, and N1CALL-10, KK4ODA-1 and Facundo logged in.
fn case_office(w: &World) -> PostOffice {
    let mut office = post_office(w, 1050);
    for (serial, name) in WAITING {
        deliver(&mut office, w, "facundo@radiopost.org", serial, name);
    }
    for credential in ["gateway-n1call-10", "gateway-kk4oda-1", "facundo"] {
        login(&mut office, w, T_SESSION - 600, credential);
    }
    office
}

#[test]
fn the_api_story_replays_exactly() {
    let w = world();
    let file = vectors("api-exchanges.json");
    let mut office = post_office(&w, 1050);
    let story = file["story"].as_array().expect("story");
    replay(&mut office, story, "story", arrivals(&w));
    // the gateway list says when KK4ODA-1 last reached the post office: its last login or poll
    // that was let in, to the minute (RP-5 §4.4, `seen`)
    let last = story
        .iter()
        .filter(|e| e["who"].as_str().is_some_and(|w| w.starts_with("KK4ODA-1")))
        .filter(|e| e["response"]["status"] == 200)
        .filter(|e| {
            let path = e["request"]["path"].as_str().unwrap_or("");
            path == "/auth/token" || path.starts_with("/copies?")
        })
        .filter_map(|e| e["at"].as_u64())
        .max()
        .expect("KK4ODA-1 reached the post office");
    let served = office
        .document(&info(), "/gateways", last + 600)
        .expect("a gateway list");
    let kk = served["gateways"]
        .as_array()
        .expect("gateways")
        .iter()
        .find(|g| g["callsign"] == "KK4ODA-1")
        .expect("KK4ODA-1 listed");
    assert_eq!(kk["seen"], json!(last / 60 * 60), "{kk}");
}

#[test]
fn each_api_case_is_answered_as_rp5_says() {
    let w = world();
    let file = vectors("api-exchanges.json");
    for case in file["cases"].as_array().expect("cases") {
        let what = case["what"].as_str().expect("what");
        let mut office = case_office(&w);
        if what == "the user's credential revoked" {
            office.state.revoked.push(w.credentials["facundo"].2);
        }
        // RP-6 §2.5: the attachments radiopost.org held, each its data's stream label and size
        if let Some(held) = case.get("held").and_then(Json::as_array) {
            let data: HashMap<Vec<u8>, Vec<u8>> = held
                .iter()
                .map(|h| {
                    let r = base64url::decode(h["ref"].as_str().expect("ref")).expect("b64");
                    let label = h["data"][0].as_str().expect("label");
                    let n = usize::try_from(h["data"][1].as_u64().expect("size")).expect("size");
                    (r, stream(label, n))
                })
                .collect();
            office.keep_held(Box::new(move |r| data.get(r).cloned()), 65_536);
        }
        // RP-5 §4.4, §5.8: the gateway lists, keys and deliveries the case's setup gives
        let setup = &case["setup"];
        let lists = |office: &mut PostOffice, lists: &Json| {
            for (domain, l) in lists.as_object().into_iter().flatten() {
                office.state.listed.insert(
                    domain.clone(),
                    Listed {
                        api: l["api"].as_str().expect("api").to_owned(),
                        gateways: l["gateways"].as_array().expect("gateways").clone(),
                        fetched: 0,
                    },
                );
            }
        };
        lists(&mut office, &setup["gateway_lists"]);
        replay(
            &mut office,
            case["exchanges"].as_array().expect("exchanges"),
            what,
            |office, note| {
                let arrives = note
                    .strip_prefix("Mail arrives for facundo@radiopost.org: #")
                    .and_then(|n| n.get(..3))
                    .and_then(|n| n.parse::<u64>().ok());
                if let Some(d) = setup["deliveries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|d| d["serial"].as_u64() == arrives)
                {
                    let at = d["at"].as_u64().expect("at");
                    let name = d["message"].as_str().expect("message");
                    deliver_at(
                        office,
                        &w,
                        "facundo@radiopost.org",
                        arrives.unwrap_or(0),
                        name,
                        at,
                    );
                } else if arrives == Some(422) {
                    deliver(office, &w, "facundo@radiopost.org", 422, "maria-house");
                }
                if note.starts_with("qrp-mail.example, a post office radiopost.org knows") {
                    for (domain, k) in setup["later"]["keys"].as_object().into_iter().flatten() {
                        let public = unhex(k["public_hex"].as_str().expect("hex"));
                        office.config.trust.keys.insert(
                            domain.clone(),
                            vec![PostOfficeKey {
                                public: public.try_into().expect("32 bytes"),
                                not_before: 0,
                                not_after: k["not_after"].as_u64().expect("not_after"),
                            }],
                        );
                    }
                    lists(office, &setup["later"]["gateway_lists"]);
                }
            },
        );
    }
}

#[test]
fn an_offline_gateways_report_is_taken_session_by_session() {
    let w = world();
    let file = vectors("api-exchanges.json");
    let mut office = case_office(&w);
    replay(
        &mut office,
        file["report"].as_array().expect("report"),
        "report",
        |_, _| {},
    );
}

const T_ACCOUNT: u64 = T_SESSION + 2 * 86_400;
const T_EXPIRES: u64 = T_ISSUED + 90 * 86_400;

#[test]
fn renewal_over_the_internet_and_by_radio() {
    let w = world();
    let file = vectors("api-account.json");
    let mut office = post_office(&w, 1101);
    replay(
        &mut office,
        file["renewal"].as_array().expect("renewal"),
        "renewal",
        |_, _| {},
    );
    for case in file["renewal_cases"].as_array().expect("cases") {
        let what = case["what"].as_str().expect("what");
        let mut office = post_office(&w, 1050);
        if what.starts_with("a revoked") {
            office.state.revoked.push(w.credentials["facundo"].2);
        }
        if what.starts_with("renewal by radio") {
            office = post_office(&w, 1101);
            for (serial, name) in WAITING {
                deliver(&mut office, &w, "facundo@radiopost.org", serial, name);
            }
        }
        replay(
            &mut office,
            case["exchanges"].as_array().expect("exchanges"),
            what,
            |_, _| {},
        );
    }
    // the expired case logs in at the credential's expiry
    assert_eq!(
        file["renewal_cases"][0]["exchanges"][0]["at"].as_u64(),
        Some(T_EXPIRES)
    );
}

#[test]
fn the_accept_list_read_changed_shared_and_filled_by_mail_sent() {
    let w = world();
    let file = vectors("api-account.json");
    let mut office = post_office(&w, 1050);
    for (address, entries) in file["context"]["accept_lists_at_start"]
        .as_object()
        .expect("lists")
    {
        let entries = entries
            .as_array()
            .expect("entries")
            .iter()
            .map(|e| e.as_str().expect("an entry").to_owned())
            .collect();
        office.state.accept.insert(address.clone(), entries);
    }
    replay(
        &mut office,
        file["accept_list"].as_array().expect("steps"),
        "accept list",
        |_, _| {},
    );
    assert_eq!(file["accept_list"][1]["at"].as_u64(), Some(T_ACCOUNT));
}

#[test]
fn station_to_station_sessions_are_reported_as_rp5_6_6_says() {
    let w = world();
    let file = vectors("direct.json");
    let with_mail = || {
        let mut office = post_office(&w, 1050);
        for (serial, name) in WAITING {
            deliver(&mut office, &w, "facundo@radiopost.org", serial, name);
        }
        office
    };
    let mut office = with_mail();
    replay(
        &mut office,
        file["radiopost"].as_array().expect("radiopost"),
        "radiopost.org",
        |_, _| {},
    );
    let unpicked: Vec<u64> = office.state.mailboxes["facundo@radiopost.org"]
        .iter()
        .filter(|m| !m.picked)
        .map(|m| m.serial)
        .collect();
    assert_eq!(
        unpicked,
        [419, 420, 421],
        "received mail is filed as picked up"
    );
    assert_eq!(
        office
            .state
            .outgoing
            .iter()
            .map(|o| o.to.clone())
            .collect::<Vec<_>>(),
        [["maria@example.com".to_owned()]],
        "sent on to Maria, not to Ana, who has it"
    );

    let config = Config {
        domains: vec!["hampost.org".into()],
        base_url: "https://po.hampost.org/rp/v1".into(),
        key: SigningKey::from_bytes(&w.seeds["po:hampost.org"]),
        trust: w.trust.clone(),
        operators: Vec::new(),
        applications_open: false,
        // the vectors' air forms are uncompressed, so they replay byte for byte (RP-5 §5.3 asks
        // only SHOULD)
        compress: false,
    };
    let state = State {
        next_serial: 1,
        ..State::default()
    };
    let mut hampost = PostOffice::new(config, state, Box::new(stream));
    replay(
        &mut hampost,
        file["hampost"].as_array().expect("hampost"),
        "hampost.org",
        |_, _| {},
    );

    let t_direct = file["radiopost"][1]["at"].as_u64().expect("at");
    for case in file["cases"].as_array().expect("cases") {
        let what = case["what"].as_str().expect("what");
        let by = case["by"].as_str().expect("by");
        let credential = if by.starts_with("Ana") {
            "ana"
        } else {
            "facundo"
        };
        let mut office = with_mail();
        let token = login(&mut office, &w, t_direct, credential);
        let body = serde_json::json!({"sessions": [case["session"].clone()]});
        let got = office.handle(
            t_direct + 5,
            &request("POST", "/direct", Some(&token), Some(body)),
        );
        let got = comparable(got.body.as_ref());
        assert_eq!(
            got["sessions"][0],
            comparable(Some(&case["expect"])),
            "{what}"
        );
    }
}

/// make.py's `office_post_office`: radiopost.org taking applications, run by `operators`.
fn office_post_office(w: &World, operators: &[&str], open: bool) -> PostOffice {
    let mut office = post_office(w, 1050);
    office.config.operators = operators.iter().map(|o| (*o).to_owned()).collect();
    office.config.applications_open = open;
    office
}

#[test]
fn applications_and_the_operator_endpoints_as_rp5_says() {
    let w = world();
    let file = vectors("api-office.json");
    let mut office = office_post_office(&w, &["KK4ODA"], true);
    replay(
        &mut office,
        file["story"].as_array().expect("story"),
        "office story",
        |_, _| {},
    );
    let cases = file["cases"].as_array().expect("cases");
    assert_eq!(cases.len(), 23);
    for case in cases {
        let what = case["what"].as_str().expect("what");
        let mut office = match what {
            "a post office that takes no applications" => {
                office_post_office(&w, &["KK4ODA"], false)
            }
            "a credential from another post office, with an operator's callsign" => {
                office_post_office(&w, &["KK4ODA", "N0CALL"], true)
            }
            "a gateway's credential, with an operator's callsign" => {
                office_post_office(&w, &["KK4ODA", "KK4ODA-1"], true)
            }
            "a post office that offers no operator endpoints" => office_post_office(&w, &[], true),
            _ => office_post_office(&w, &["KK4ODA"], true),
        };
        replay(
            &mut office,
            case["exchanges"].as_array().expect("exchanges"),
            what,
            |office, note| {
                // make.py's OPERATOR_SINCE: a callsign joins the operators mid-case
                if note == "N5CALL becomes one of the post office's operators (RP-5 §7.1)." {
                    office.config.operators.push("N5CALL".into());
                }
            },
        );
    }
}

const T_WINDOW: u64 = 1_794_492_900; // session-04: N1CALL-10's window opens
const T_CUT: u64 = T_WINDOW - 6 * 3600; // Atlanta loses the Internet
const PO_HAMPOST: &str = "https://po.hampost.org/rp/v1";

/// make.py's `cut_off_office`: radiopost.org on the day Atlanta loses the Internet. N2CALL-5 holds
/// copies; N1CALL-10 is hampost.org's gateway, its record from hampost.org's list; the route sends
/// N2CALL-5's users' mail to N1CALL-10 while it is cut off (RP-7 §4.1, §6.1).
fn cut_off_office(w: &World) -> PostOffice {
    use radiopost_postoffice::office::RadioRoute;
    let mut office = post_office(w, 1050);
    let window = vectors("forwarding.json")["context"]["window"].clone();
    office.state.gateways.insert(
        "N2CALL-5".into(),
        serde_json::json!({"callsign": "N2CALL-5", "operator": "N2CALL",
                           "copies": {"users": 50, "days": 7, "max_air_bytes": 16384}}),
    );
    office.state.listed.insert(
        "hampost.org".into(),
        Listed {
            api: PO_HAMPOST.into(),
            gateways: vec![
                serde_json::json!({"callsign": "N1CALL-10", "operator": "N1CALL",
                                              "forwarding": [window]}),
            ],
            fetched: 0,
        },
    );
    office.state.routes = vec![RadioRoute {
        users_at: vec!["N2CALL-5".into(), "KK4ODA-1".into()],
        via: "N1CALL-10".into(),
    }];
    for (address, usual) in [
        ("facundo@radiopost.org", vec!["KK4ODA-1", "N2CALL-5"]),
        ("shelter3@radiopost.org", vec!["N2CALL-5"]),
    ] {
        office.state.settings.insert(
            address.into(),
            serde_json::json!({ "usual_gateways": usual }),
        );
    }
    office
}

/// The notes that say mail arrives, `(address, serial) -> (message, when)`.
fn arriving<'a>(
    w: &'a World,
    when: &'a [(&'a str, u64, &'a str, u64)],
) -> impl FnMut(&mut PostOffice, &str) + 'a {
    move |office, note| {
        for (address, serial, name, at) in when {
            if note.starts_with(&format!("Mail arrives for {address}: #{serial},")) {
                deliver_at(office, w, address, *serial, name, *at);
            }
        }
    }
}

/// The date a message was written.
fn date_of(w: &World, name: &str) -> u64 {
    w.messages[name]
        .map()
        .get(key::DATE)
        .and_then(radiopost::cbor::Value::as_u64)
        .expect("a date")
}

/// The forwarding story's steps until the Internet comes back to Atlanta, and after.
fn forwarding_story() -> (Vec<Json>, Vec<Json>) {
    let steps = vectors("forwarding.json")["post_office"]
        .as_array()
        .expect("steps")
        .clone();
    let back = steps
        .iter()
        .position(|s| {
            s["note"]
                .as_str()
                .is_some_and(|n| n.starts_with("Atlanta's Internet returns"))
        })
        .expect("the Internet returns");
    (steps[..back].to_vec(), steps[back..].to_vec())
}

/// RP-7 §4.1, §4.2, RP-5 §5.8: `forwarding.json`'s post office side, replayed exactly. N2CALL-5
/// polls, then falls silent; 15 minutes on it is cut off, and mail for Shelter 3 goes to
/// N1CALL-10 as a forward item with radiopost.org's placement. N1CALL-10 reports session-04's
/// records with their path; a path that does not end at it is refused. When N2CALL-5 polls
/// again, the forward item is dropped at N1CALL-10.
#[test]
fn a_cut_off_gateways_mail_goes_by_its_route() {
    let w = world();
    let mut office = cut_off_office(&w);
    let arrivals = [
        (
            "facundo@radiopost.org",
            421,
            "maria-421",
            date_of(&w, "maria-421"),
        ),
        (
            "shelter3@radiopost.org",
            501,
            "ana-to-shelter3",
            date_of(&w, "ana-to-shelter3") + 600,
        ),
    ];
    let (before, after) = forwarding_story();
    replay(&mut office, &before, "forwarding", arriving(&w, &arrivals));
    replay(
        &mut office,
        &after,
        "forwarding, back",
        arriving(&w, &arrivals),
    );
    // the placement N1CALL-10 was given is the vector's, byte for byte
    let placed = &vectors("forwarding.json")["placement"]["placement_hex"];
    let item = office.state.copies["N1CALL-10"]
        .iter()
        .find(|i| i["kind"] == "forward")
        .expect("the forward item");
    let bytes = base64url::decode(item["placement"].as_str().expect("text")).expect("base64url");
    assert_eq!(&radiopost::cbor::hex(&bytes), placed.as_str().expect("hex"));
}

/// RP-7 §4.1: a gateway last answered `placing` false is not counted cut off when it falls
/// silent; one that polled while radiopost.org placed items at it is, and the copy it never
/// acknowledged goes to N1CALL-10 while the one it did does not.
#[test]
fn a_gateway_is_counted_cut_off_only_after_a_poll_that_counts() {
    let w = world();
    let mut office = cut_off_office(&w);
    for s in office.state.settings.values_mut() {
        *s = serde_json::json!({"usual_gateways": []});
    }
    let t0 = T_CUT - 3 * 3600;
    let arrivals = [
        ("shelter3@radiopost.org", 501, "ana-to-shelter3", t0 + 1210),
        ("shelter3@radiopost.org", 502, "emergency-held", t0 + 1700),
    ];
    let mut deliver = arriving(&w, &arrivals);
    replay(
        &mut office,
        vectors("forwarding.json")["placing"]
            .as_array()
            .expect("steps"),
        "placing",
        |office, note| {
            if note.starts_with("Shelter 3 names N2CALL-5 as usual") {
                office.state.settings.insert(
                    "shelter3@radiopost.org".into(),
                    serde_json::json!({"usual_gateways": ["N2CALL-5"]}),
                );
            }
            deliver(office, note);
        },
    );
}

/// RP-7 §4.4: `local-delivery.json`'s post office side, going on from the forwarding story before
/// the Internet comes back: N1CALL-10 reports session-05's records with a local pickup; the
/// message reaches Facundo's mailbox already picked up, its forward item placed and dropped in
/// the same poll, and it is not offered to him again.
#[test]
fn local_pickups_reported_through_a_partner() {
    let w = world();
    let mut office = cut_off_office(&w);
    let arrivals = [
        (
            "facundo@radiopost.org",
            421,
            "maria-421",
            date_of(&w, "maria-421"),
        ),
        (
            "shelter3@radiopost.org",
            501,
            "ana-to-shelter3",
            date_of(&w, "ana-to-shelter3") + 600,
        ),
    ];
    let (before, _) = forwarding_story();
    replay(&mut office, &before, "forwarding", arriving(&w, &arrivals));
    replay(
        &mut office,
        vectors("local-delivery.json")["post_office"]
            .as_array()
            .expect("steps"),
        "local delivery",
        |_, _| {},
    );
}

/// RP-6 §3.1 (gap G17): a reply's thread comes from the email the message was made of, or from
/// the email this post office sent for it; the email that came wins where both are kept.
#[test]
fn a_reply_finds_its_thread_in_the_email_sent_as_well_as_the_one_that_came() {
    let w = world();
    let mut office = post_office(&w, 1);
    office
        .state
        .sent_ids
        .insert("<sent@radiopost.org>".into(), "aa".repeat(12));
    office
        .state
        .email_ids
        .insert("bb".repeat(12), "<came@mail.example.com>".into());
    office
        .state
        .sent_ids
        .insert("<also-sent@radiopost.org>".into(), "bb".repeat(12));
    let ids = office.thread_ids();
    assert_eq!(ids[&"aa".repeat(12)], "<sent@radiopost.org>");
    assert_eq!(ids[&"bb".repeat(12)], "<came@mail.example.com>");
}
