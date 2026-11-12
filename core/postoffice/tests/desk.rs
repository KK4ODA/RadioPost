// SPDX-License-Identifier: MIT OR Apache-2.0
//! The operator's desk over HTTP: the operator enrolls and logs in, opens applications from the
//! client, an applicant applies and is approved, collects the credential and logs in; the desk
//! refuses everyone else; and the applications and their opening survive a restart.

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use radiopost::base64url;
use radiopost::crypto::{self, context};
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
) -> (u16, Json) {
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
    let body = answer.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    (status, serde_json::from_str(body).unwrap_or(Json::Null))
}

fn settings() -> Settings {
    Settings {
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
        hooks: None,
        hook_token: None,
        smtp: None,
        operators: Vec::new(),
    }
}

fn start(dir: &std::path::Path) -> (String, String) {
    let running = Arc::new(Running::load(dir, serve::system_draw()).expect("loaded"));
    serve::serve(running, "127.0.0.1:0", "127.0.0.1:0").expect("serving")
}

fn login(api: &str, credential: &str, device: &SigningKey) -> Json {
    let (_, c) = http(api, "POST", "/rp/v1/auth/challenge", None, None);
    let nonce = c["nonce"].as_str().expect("a nonce").to_owned();
    let mut signed = base64url::decode(&nonce).expect("base64url");
    signed.extend_from_slice(BASE.as_bytes());
    let body = json!({"credential": credential, "nonce": nonce,
        "signature": base64url::encode(&crypto::sign(context::API_LOGIN, &signed, device))});
    let (status, answer) = http(api, "POST", "/rp/v1/auth/token", None, Some(&body));
    assert_eq!(status, 200, "{answer}");
    answer
}

fn application(device: &SigningKey) -> Json {
    let signed = format!("jose@test.example\nN5CALL\n{BASE}");
    json!({"address": "jose@test.example", "operator": "N5CALL", "name": "José Pérez",
           "contact": "jose@example.net",
           "device": base64url::encode(&device.verifying_key().to_bytes()),
           "signature": base64url::encode(&crypto::sign(context::API_APPLY, signed.as_bytes(), device))})
}

#[test]
#[allow(clippy::too_many_lines, reason = "one story, told in order")]
fn the_operator_runs_applications_from_the_client() {
    let dir = std::env::temp_dir().join(format!("radiopost-desk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    let files = Files::new(&dir);
    files.write("key.hex", &"07".repeat(32)).expect("written");
    files
        .write(
            "config.json",
            &serde_json::to_string(&settings()).expect("JSON"),
        )
        .expect("written");
    let (api, admin) = start(&dir);

    // the operator's laptop enrolls with a code made on the server, and logs in
    let (_, made) = http(
        &admin,
        "POST",
        "/code",
        None,
        Some(&json!({"address": "kk4oda@test.example", "operator": "KK4ODA"})),
    );
    let code = made["code"].as_str().expect("a code").to_owned();
    let laptop = SigningKey::from_bytes(&[3; 32]);
    let signed = format!("{code}{BASE}");
    let enroll = json!({"address": "kk4oda@test.example", "operator": "KK4ODA", "code": code,
        "device": base64url::encode(&laptop.verifying_key().to_bytes()),
        "signature": base64url::encode(&crypto::sign(context::API_ENROLL, signed.as_bytes(), &laptop))});
    let (status, enrolled) = http(&api, "POST", "/rp/v1/enroll", None, Some(&enroll));
    assert_eq!(status, 200, "{enrolled}");
    let op_credential = enrolled["credential"]
        .as_str()
        .expect("a credential")
        .to_owned();
    let op = login(&api, &op_credential, &laptop);
    assert_eq!(op["office"], json!(true));
    let op = op["token"].as_str().expect("a token").to_owned();

    // applications are closed until the operator opens them from the client
    let jose = SigningKey::from_bytes(&[5; 32]);
    let (status, _) = http(
        &api,
        "POST",
        "/rp/v1/applications",
        None,
        Some(&application(&jose)),
    );
    assert_eq!(status, 403);
    let (status, s) = http(
        &api,
        "PUT",
        "/rp/v1/office/settings",
        Some(&op),
        Some(&json!({"applications": "open"})),
    );
    assert_eq!((status, s["applications"].as_str()), (200, Some("open")));
    let (status, applied) = http(
        &api,
        "POST",
        "/rp/v1/applications",
        None,
        Some(&application(&jose)),
    );
    assert_eq!(status, 202, "{applied}");
    let id = applied["application"].as_str().expect("an ID").to_owned();

    // the status carries RP-5 §7.2's members and the service's own
    let (status, s) = http(&api, "GET", "/rp/v1/office/status", Some(&op), None);
    assert_eq!(status, 200);
    assert_eq!(s["applications"], json!(1));
    assert_eq!(s["applications_open"], json!(true));
    assert_eq!(s["operators"], json!(["KK4ODA"]));
    assert_eq!(s["version"], json!(env!("CARGO_PKG_VERSION")));

    // approved from the client; the applicant collects the credential and logs in
    let (status, entry) = http(
        &api,
        "POST",
        &format!("/rp/v1/office/applications/{id}"),
        Some(&op),
        Some(&json!({"decision": "approve"})),
    );
    assert_eq!(
        (status, entry["status"].as_str()),
        (200, Some("approved")),
        "{entry}"
    );
    let (_, got) = http(
        &api,
        "GET",
        &format!("/rp/v1/applications/{id}"),
        None,
        None,
    );
    let credential = got["credential"].as_str().expect("a credential").to_owned();
    let jose_login = login(&api, &credential, &jose);
    assert_eq!(jose_login.get("office"), None);
    let jose_token = jose_login["token"].as_str().expect("a token").to_owned();

    // the desk answers its operators only
    for path in [
        "/rp/v1/office/outgoing",
        "/rp/v1/office/settings",
        "/rp/v1/office/status",
    ] {
        assert_eq!(
            http(&api, "GET", path, Some(&jose_token), None).0,
            403,
            "{path}"
        );
        assert_eq!(http(&api, "GET", path, None, None).0, 401, "{path}");
    }
    let (status, named) = http(
        &api,
        "POST",
        "/rp/v1/office/name",
        Some(&op),
        Some(&json!({"address": "jose@test.example", "name": "José"})),
    );
    assert_eq!((status, named["name"].as_str()), (200, Some("José")));
    let (status, accounts) = http(&api, "GET", "/rp/v1/office/accounts", Some(&op), None);
    assert_eq!(status, 200);
    let jose_account = accounts["accounts"]
        .as_array()
        .expect("accounts")
        .iter()
        .find(|a| a["address"] == "jose@test.example")
        .expect("José's account")
        .clone();
    assert_eq!(jose_account["name"], json!("José"));
    assert_eq!(jose_account["credentials"][0]["operator"], json!("N5CALL"));

    // the server's own commands see the same
    let (_, listed) = http(&admin, "GET", "/applications", None, None);
    assert_eq!(listed["applications"][0]["status"], json!("approved"));

    // a restart keeps the applications and their opening
    let (api, _) = start(&dir);
    let op = login(&api, &op_credential, &laptop)["token"]
        .as_str()
        .expect("a token")
        .to_owned();
    let (_, list) = http(&api, "GET", "/rp/v1/office/applications", Some(&op), None);
    assert_eq!(
        list["applications"][0]["address"],
        json!("jose@test.example")
    );
    let (_, s) = http(&api, "GET", "/rp/v1/office/settings", Some(&op), None);
    assert_eq!(s["applications"], json!("open"));
    let _ = std::fs::remove_dir_all(&dir);
}
