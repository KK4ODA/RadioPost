// SPDX-License-Identifier: MIT OR Apache-2.0
//! Addresses in lowercase (RP-0 1.5.0 §6.2): a code typed with capitals issues a credential
//! whose address is in lowercase, mail for the account is found under it however it was
//! addressed, and a state kept before the rule, holding two spellings of one account, becomes
//! one account when the post office starts.

use ed25519_dalek::SigningKey;
use radiopost::base64url;
use radiopost::credential::{Credential, Trust};
use radiopost::crypto::{self, context};
use radiopost_postoffice::office::{Config, PostOffice, Request, State, Waiting};
use serde_json::{Value as Json, json};

const BASE: &str = "https://po.radiopost.org/rp/v1";

fn office(state: State) -> PostOffice {
    let config = Config {
        domains: vec!["radiopost.org".into()],
        base_url: BASE.into(),
        key: SigningKey::from_bytes(&[7; 32]),
        trust: Trust::default(),
        operators: Vec::new(),
        applications_open: false,
        compress: false,
    };
    let mut n = 0u8;
    let draw = Box::new(move |_: &str, len: usize| {
        n = n.wrapping_add(1);
        vec![n; len]
    });
    let mut po = PostOffice::new(config, state, draw);
    po.at(1_794_000_000);
    po
}

fn waiting(serial: u64, id: &str) -> Waiting {
    Waiting {
        serial,
        air: String::new(),
        id: id.into(),
        from: "maria@example.com".into(),
        subject: String::new(),
        priority: 0,
        picked: false,
        third_party: true,
    }
}

#[test]
fn a_code_typed_in_capitals_gives_an_account_in_lowercase() {
    let mut po = office(State::default());
    po.add_code("7Q4F-AZ9K", "Ana.Perez@RadioPost.ORG", "N0CALL", 3);
    let device = SigningKey::from_bytes(&[9; 32]);
    let mut signed = b"7Q4F-AZ9K".to_vec();
    signed.extend_from_slice(BASE.as_bytes());
    let signature = crypto::sign(context::API_ENROLL, &signed, &device);
    let answer = po.handle(
        1_794_000_060,
        &Request {
            method: "POST".into(),
            path: "/enroll".into(),
            token: None,
            // the user typed it with other capitals: one address
            body: Some(json!({
                "code": "7Q4F-AZ9K", "address": "ana.perez@radiopost.org", "operator": "N0CALL",
                "device": base64url::encode(device.verifying_key().as_bytes()),
                "signature": base64url::encode(&signature),
            })),
        },
    );
    assert_eq!(answer.status, 200, "{:?}", answer.body);
    let body = answer.body.unwrap_or(Json::Null);
    let credential = base64url::decode(body["credential"].as_str().unwrap_or("")).expect("bytes");
    let issued = Credential::decode(&credential).expect("a credential");
    assert_eq!(issued.body.address, "ana.perez@radiopost.org");
    // mail addressed with capitals goes in the account's one mailbox
    po.deliver(
        "ANA.PEREZ@radiopost.org",
        1,
        b"",
        "maria@example.com",
        "Hola",
        0,
        true,
    );
    assert_eq!(po.next_mailbox_serial("Ana.Perez@RadioPost.org"), 2);
    assert_eq!(
        po.state.mailboxes.keys().collect::<Vec<_>>(),
        ["ana.perez@radiopost.org"]
    );
}

#[test]
fn two_spellings_kept_before_become_one_account() {
    let mut state = State::default();
    state.mailboxes.insert(
        "Ana@RadioPost.org".into(),
        vec![waiting(1, "a"), waiting(2, "b")],
    );
    state
        .mailboxes
        .insert("ana@radiopost.org".into(), vec![waiting(1, "c")]);
    state
        .accept
        .insert("Ana@RadioPost.org".into(), vec!["maria@example.com".into()]);
    state
        .accept
        .insert("ana@radiopost.org".into(), vec!["@hampost.org".into()]);
    state
        .settings
        .insert("ana@radiopost.org".into(), json!({"attach_bytes": 1000}));
    state
        .settings
        .insert("Ana@RadioPost.org".into(), json!({"attach_bytes": 5}));
    state.names.insert("Ana@RadioPost.org".into(), "Ana".into());
    let po = office(state);
    let s = &po.state;
    assert_eq!(
        s.mailboxes.keys().collect::<Vec<_>>(),
        ["ana@radiopost.org"]
    );
    let mut kept: Vec<(u64, &str)> = s.mailboxes["ana@radiopost.org"]
        .iter()
        .map(|w| (w.serial, w.id.as_str()))
        .collect();
    kept.sort_unstable();
    // the lowercase spelling's serial 1 stays; the other's 1 and 2 follow it, none taken twice
    assert_eq!(kept, [(1, "c"), (2, "a"), (3, "b")]);
    assert_eq!(
        s.accept["ana@radiopost.org"],
        ["@hampost.org", "maria@example.com"]
    );
    assert_eq!(s.settings["ana@radiopost.org"]["attach_bytes"], 1000);
    assert_eq!(s.names["ana@radiopost.org"], "Ana");
    assert_eq!(s.accept.len() + s.settings.len() + s.names.len(), 3);
}
