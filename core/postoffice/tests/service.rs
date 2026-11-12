// SPDX-License-Identifier: MIT OR Apache-2.0
//! The post office as a service, over HTTP: a device enrolls with a code its operator made, logs
//! in, changes its settings; the public documents are signed with the post office's key; and the
//! state survives a restart.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use radiopost::base64url;
use radiopost::crypto;
use radiopost_postoffice::documents::Info;
use radiopost_postoffice::serve::{self, Files, Running, Settings};
use serde_json::{Value as Json, json};

const BASE: &str = "https://po.test.example/rp/v1";

fn http(
    at: &str,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&Json>,
) -> (u16, String) {
    let mut s = TcpStream::connect(at).expect("the post office");
    let text = body.map(Json::to_string).unwrap_or_default();
    let auth = token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: {at}\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    )
    .expect("sent");
    let mut answer = String::new();
    s.read_to_string(&mut answer).expect("an answer");
    let status = answer
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .expect("a status");
    let body = answer
        .split_once("\r\n\r\n")
        .map_or("", |(_, b)| b)
        .to_owned();
    (status, body)
}

fn make(dir: &std::path::Path) -> (String, String, SigningKey) {
    let files = Files::new(dir);
    let key = SigningKey::from_bytes(&[7; 32]);
    files.write("key.hex", &"07".repeat(32)).expect("written");
    let settings = Settings {
        domains: vec!["test.example".into()],
        base_url: BASE.into(),
        listen: "127.0.0.1:0".into(),
        admin: "127.0.0.1:0".into(),
        info: Info {
            operator: "KK4ODA".into(),
            contact: "postmaster@test.example".into(),
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
        known: Vec::new(),
        blocked: Vec::new(),
        resolver: None,
        hooks: None,
        hook_token: None,
        smtp: None,
        operators: Vec::new(),
        lookups: radiopost_postoffice::lookups::LookupSettings::default(),
        mailboxes: radiopost_postoffice::mailboxes::MailboxSettings::default(),
        health: radiopost_postoffice::serve::HealthSettings::default(),
    };
    files
        .write(
            "config.json",
            &serde_json::to_string(&settings).expect("JSON"),
        )
        .expect("written");
    let running = Arc::new(Running::load(dir, serve::system_draw()).expect("loaded"));
    let (api, admin) = serve::serve(running, "127.0.0.1:0", "127.0.0.1:0").expect("serving");
    (api, admin, key)
}

#[test]
#[allow(clippy::too_many_lines, reason = "one story, told in order")]
fn a_device_enrolls_logs_in_and_the_documents_are_signed() {
    let dir = std::env::temp_dir().join(format!("radiopost-po-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    let (api, admin, po_key) = make(&dir);

    // the operator makes a code; a code it did not make is refused
    let (_, made) = http(
        &admin,
        "POST",
        "/code",
        None,
        Some(
            &json!({"address": "ana@test.example", "operator": "N0CALL", "flags": ["send", "receive"]}),
        ),
    );
    let code = serde_json::from_str::<Json>(&made).expect("JSON")["code"]
        .as_str()
        .expect("a code")
        .to_owned();
    let device = SigningKey::from_bytes(&[9; 32]);
    let enroll = |code: &str| {
        let mut signed = code.as_bytes().to_vec();
        signed.extend_from_slice(BASE.as_bytes());
        json!({"address": "ana@test.example", "operator": "N0CALL", "code": code,
               "device": base64url::encode(&device.verifying_key().to_bytes()),
               "signature": base64url::encode(&crypto::sign(crypto::context::API_ENROLL, &signed, &device))})
    };
    assert_eq!(
        http(
            &api,
            "POST",
            "/rp/v1/enroll",
            None,
            Some(&enroll("AAAA-AAAA"))
        )
        .0,
        403
    );
    let (status, body) = http(&api, "POST", "/rp/v1/enroll", None, Some(&enroll(&code)));
    assert_eq!(status, 200, "{body}");
    let credential = serde_json::from_str::<Json>(&body).expect("JSON")["credential"]
        .as_str()
        .expect("a credential")
        .to_owned();
    // a code is used once
    assert_eq!(
        http(&api, "POST", "/rp/v1/enroll", None, Some(&enroll(&code))).0,
        403
    );

    // login, then settings
    let (_, c) = http(&api, "POST", "/rp/v1/auth/challenge", None, None);
    let nonce = serde_json::from_str::<Json>(&c).expect("JSON")["nonce"]
        .as_str()
        .expect("a nonce")
        .to_owned();
    let mut signed = base64url::decode(&nonce).expect("base64url");
    signed.extend_from_slice(BASE.as_bytes());
    let login = json!({"credential": credential, "nonce": nonce,
        "signature": base64url::encode(&crypto::sign(crypto::context::API_LOGIN, &signed, &device))});
    let (status, body) = http(&api, "POST", "/rp/v1/auth/token", None, Some(&login));
    assert_eq!(status, 200, "{body}");
    let token = serde_json::from_str::<Json>(&body).expect("JSON")["token"]
        .as_str()
        .expect("a token")
        .to_owned();
    let settings = json!({"usual_gateways": ["KK4ODA-1"], "auto_bytes": 1000});
    assert_eq!(
        http(
            &api,
            "PUT",
            "/rp/v1/settings",
            Some(&token),
            Some(&settings)
        )
        .0,
        204
    );
    let (status, body) = http(&api, "GET", "/rp/v1/settings", Some(&token), None);
    assert_eq!(
        (status, serde_json::from_str::<Json>(&body).expect("JSON")),
        (200, settings)
    );
    assert_eq!(
        http(&api, "GET", "/rp/v1/settings", Some("tok_nonsense"), None).0,
        401
    );
    assert_eq!(
        http(
            &api,
            "POST",
            "/rp/v1/sessions",
            Some(&token),
            Some(&json!({}))
        )
        .0,
        403
    );

    // the documents, signed with the post office's key
    let public = po_key.verifying_key().to_bytes();
    let kid = radiopost::cbor::hex(&crypto::kid(&public));
    let keys = HashMap::from([(kid, public)]);
    for path in [
        "/info",
        "/revocations",
        "/post-offices",
        "/gateways",
        "/routes",
    ] {
        let (status, jws) = http(&api, "GET", &format!("/rp/v1{path}"), None, None);
        assert_eq!(status, 200, "{path}");
        let doc = radiopost::jws::verify(&jws, &keys).expect("signed by the post office");
        if path == "/info" {
            assert_eq!(doc["domains"], json!(["test.example"]));
        }
    }

    // the state survives a restart: the token still works
    let running = Arc::new(Running::load(&dir, serve::system_draw()).expect("loaded again"));
    let (api2, _) = serve::serve(running, "127.0.0.1:0", "127.0.0.1:0").expect("serving");
    assert_eq!(
        http(&api2, "GET", "/rp/v1/settings", Some(&token), None).0,
        200
    );
    let _ = std::fs::remove_dir_all(&dir);
}
