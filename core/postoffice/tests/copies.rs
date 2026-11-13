// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-5 §4.4, §5.8, D37: copies placed ahead at a gateway registered with another post office.
//! `other.example` serves its documents over HTTP; `test.example` knows it by a typed key, fetches
//! and checks its gateway list, places a copy of a user's mail at N1CALL-10, registered there,
//! and serves it only to N1CALL-10's credential from `other.example`. Forgotten, `other.example`'s
//! gateways get nothing more.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use radiopost::base64url;
use radiopost::credential::flags;
use radiopost::crypto::{self, context};
use radiopost_postoffice::documents::Info;
use radiopost_postoffice::serve::{self, Files, Running, Settings};
use serde_json::{Value as Json, json};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("radiopost-copies-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    dir
}

fn settings(domain: &str, base: &str, listen: &str, known: Vec<Json>) -> Settings {
    Settings {
        domains: vec![domain.into()],
        base_url: base.into(),
        listen: listen.into(),
        admin: "127.0.0.1:0".into(),
        info: Info {
            operator: "KK4ODA".into(),
            contact: format!("postmaster@{domain}"),
            country: "US".into(),
            max_air_bytes: 65_536,
            max_email_bytes: 10_485_760,
            gateways: "any".into(),
            mail: json!({}),
            updated: 0,
            not_before: 0,
            not_after: u64::MAX,
            post_offices: Vec::new(),
            applications: None,
        },
        known,
        blocked: Vec::new(),
        // nothing listens here: DNS fails, and the typed key serves
        resolver: Some("http://127.0.0.1:9/resolve".into()),
        hooks: None,
        hook_token: None,
        smtp: None,
        operators: Vec::new(),
        lookups: radiopost_postoffice::lookups::LookupSettings::default(),
        mailboxes: radiopost_postoffice::mailboxes::MailboxSettings::default(),
        health: radiopost_postoffice::serve::HealthSettings::default(),
        compress: true,
    }
}

fn make(dir: &Path, s: &Settings, seed: u8) -> Running {
    std::fs::create_dir_all(dir).expect("a directory");
    let files = Files::new(dir);
    files
        .write("key.hex", &format!("{seed:02x}").repeat(32))
        .expect("written");
    files
        .write("config.json", &serde_json::to_string(s).expect("JSON"))
        .expect("written");
    Running::load(dir, serve::system_draw()).expect("loaded")
}

fn call(r: &Running, method: &str, path: &str, token: Option<&str>, body: &Json) -> (u16, Json) {
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        body.to_string().into_bytes()
    };
    let (status, _, text) = r.api(method, &format!("/rp/v1{path}"), token, &bytes);
    (status, serde_json::from_str(&text).unwrap_or(Json::Null))
}

/// A token at `here` (base `base`) for a credential and its device key.
fn login(here: &Running, base: &str, credential: &[u8], device: &SigningKey) -> String {
    let (_, c) = call(here, "POST", "/auth/challenge", None, &Json::Null);
    let nonce = c["nonce"].as_str().expect("a nonce").to_owned();
    let mut signed = base64url::decode(&nonce).expect("base64url");
    signed.extend_from_slice(base.as_bytes());
    let body = json!({"credential": base64url::encode(credential), "nonce": nonce,
        "signature": base64url::encode(&crypto::sign(context::API_LOGIN, &signed, device))});
    let (status, answer) = call(here, "POST", "/auth/token", None, &body);
    assert_eq!(status, 200, "{answer}");
    answer["token"].as_str().expect("a token").to_owned()
}

#[test]
#[allow(clippy::too_many_lines, reason = "one story, told in order")]
fn copies_go_to_a_gateway_of_another_post_office_and_only_to_its_credential() {
    let dir = scratch("two");
    // other.example, serving its documents over HTTP, with N1CALL-10 registered
    let port = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("a free port")
        .port();
    let there_base = format!("http://127.0.0.1:{port}/rp/v1");
    let there = Arc::new(make(
        &dir.join("there"),
        &settings(
            "other.example",
            &there_base,
            &format!("127.0.0.1:{port}"),
            Vec::new(),
        ),
        0x31,
    ));
    let gateway_key = SigningKey::from_bytes(&[0x51; 32]);
    let gateway_credential = {
        let mut office = there.office();
        office.at(serve::now());
        office.state.gateways.insert(
            "N1CALL-10".into(),
            json!({"callsign": "N1CALL-10", "operator": "N1CALL", "grid": "EM10",
                   "place": "Austin, TX", "hours": "always", "internet": true,
                   "copies": {"users": 20, "days": 7, "max_air_bytes": 16384}, "updated": 1}),
        );
        office.issue(
            "n1call@other.example",
            "N1CALL-10",
            gateway_key.verifying_key().to_bytes(),
            flags::SEND | flags::GATEWAY,
        )
    };
    serve::serve(there.clone(), &format!("127.0.0.1:{port}"), "127.0.0.1:0").expect("serving");

    // test.example knows it by a typed key, and fetches its gateway list
    let here_base = "https://po.test.example/rp/v1";
    let there_key = SigningKey::from_bytes(&[0x31; 32])
        .verifying_key()
        .to_bytes();
    let known = vec![json!({"domain": "other.example", "api": there_base,
        "keys": [{"public": base64url::encode(&there_key), "not_before": 0, "not_after": u64::MAX}]})];
    let here = make(
        &dir.join("here"),
        &settings("test.example", here_base, "127.0.0.1:0", known),
        0x2a,
    );
    here.refresh("other.example");
    let (record, issuers) = here
        .office()
        .record_of("N1CALL-10")
        .expect("N1CALL-10's record, from other.example's list");
    assert_eq!(record["place"], "Austin, TX");
    assert_eq!(issuers, ["other.example"]);
    assert_eq!(here.post_offices_view()["known"][0]["gateways"], 1);

    // Facundo names it as usual, and mail arrives for him
    let laptop = SigningKey::from_bytes(&[0x41; 32]);
    let facundo = {
        let mut office = here.office();
        office.at(serve::now());
        office.issue(
            "facundo@test.example",
            "KK4ODA",
            laptop.verifying_key().to_bytes(),
            flags::SEND | flags::RECEIVE,
        )
    };
    let user = login(&here, here_base, &facundo, &laptop);
    let (status, _) = call(
        &here,
        "PUT",
        "/settings",
        Some(&user),
        &json!({"usual_gateways": ["N1CALL-10"]}),
    );
    assert_eq!(status, 204);
    here.email_in(
        "maria@example.com",
        &["facundo@test.example".to_owned()],
        b"From: maria@example.com\r\nTo: facundo@test.example\r\nSubject: Are you safe?\r\n\
          Date: Thu, 12 Nov 2026 10:00:00 +0000\r\nMessage-ID: <1@example.com>\r\n\r\nCall me.\r\n",
    )
    .expect("taken");

    // N1CALL-10, with its credential from other.example, gets the copy
    let gateway = login(&here, here_base, &gateway_credential, &gateway_key);
    let (status, polled) = call(
        &here,
        "GET",
        "/copies?after=&wait=0",
        Some(&gateway),
        &Json::Null,
    );
    assert_eq!(status, 200, "{polled}");
    assert_eq!(polled["placing"], true, "{polled}");
    assert_eq!(polled["items"][0]["kind"], "copy");
    assert_eq!(polled["items"][0]["address"], "facundo@test.example");

    // the same callsign by a credential test.example issued gets nothing
    let stray_credential = {
        let mut office = here.office();
        office.issue(
            "facundo@test.example",
            "N1CALL-10",
            gateway_key.verifying_key().to_bytes(),
            flags::SEND | flags::GATEWAY,
        )
    };
    let stray = login(&here, here_base, &stray_credential, &gateway_key);
    let (_, nothing) = call(
        &here,
        "GET",
        "/copies?after=&wait=0",
        Some(&stray),
        &Json::Null,
    );
    assert_eq!(
        nothing,
        json!({"cursor": "c_0", "placing": false, "items": []})
    );

    // forgotten, other.example's gateways get nothing more
    let (status, _) = here.admin(
        "POST",
        "/post-offices",
        json!({"forget": "other.example"}).to_string().as_bytes(),
    );
    assert_eq!(status, 200);
    assert!(here.office().record_of("N1CALL-10").is_none());
    assert!(here.office().state.listed.is_empty());

    let _ = std::fs::remove_dir_all(dir);
}
