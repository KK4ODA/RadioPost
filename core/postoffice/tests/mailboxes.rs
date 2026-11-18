// SPDX-License-Identifier: MIT OR Apache-2.0
//! Every account with its mailbox at the mail server, against a stand-in Stalwart that keeps the
//! accounts it makes: a code for a new address makes the mailbox and shows its password once; a
//! code for an account that has one makes none; Health names an account the mail server lacks,
//! and Make mailbox (the operator's port and the desk) puts it right; an account approved by
//! postcard gets its mailbox, and its welcome with the password, when it enrolls with the card's
//! code. A mail server that refuses email for a user before the door is asked (RP-6 §2.1) was how
//! an account made by a code lost every email from outside.

use std::collections::BTreeSet;
use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use radiopost::base64url;
use radiopost::crypto::{self, context};
use radiopost_postoffice::check::Verdict;
use radiopost_postoffice::documents::Info;
use radiopost_postoffice::mailboxes::MailboxSettings;
use radiopost_postoffice::serve::{self, Files, Running, Settings};
use radiopost_tester::mailserver::{MailServer, decoded_body, split};
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

/// The accounts a stand-in Stalwart holds, by address.
type Held = Arc<Mutex<BTreeSet<String>>>;

/// A stand-in Stalwart with the domain `test.example` (ID `d1`) and the accounts it holds, by
/// address: it makes one at `x:Account/set` and lists them at `x:Account/get`, with the post
/// office's own `radiopost@` among them as on a real server. Also how many it was asked to make.
fn stalwart() -> (String, Held, Arc<Mutex<usize>>) {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("a port");
    let at = format!("http://{}", server.server_addr());
    let held: Held = Arc::new(Mutex::new(BTreeSet::from([
        "radiopost@test.example".to_owned()
    ])));
    let asked = Arc::new(Mutex::new(0));
    let (keep, count) = (Arc::clone(&held), Arc::clone(&asked));
    std::thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            let body: Json = serde_json::from_str(&body).unwrap_or(Json::Null);
            let call = &body["methodCalls"][0];
            let answer = match call[0].as_str() {
                Some("x:Domain/get") => json!({"methodResponses": [
                    ["x:Domain/get", {"list": [{"id": "d1", "name": "test.example"}]}, "d"]]}),
                Some("x:Account/set") => {
                    *count.lock().expect("lock") += 1;
                    let new = &call[1]["create"]["new"];
                    assert_eq!(new["domainId"], "d1", "{new}");
                    let address = format!("{}@test.example", new["name"].as_str().unwrap_or(""));
                    if keep.lock().expect("lock").insert(address.clone()) {
                        json!({"methodResponses": [
                            ["x:Account/set", {"created": {"new": {"id": address}}}, "a"]]})
                    } else {
                        json!({"methodResponses": [["x:Account/set", {"notCreated": {"new":
                            {"type": "alreadyExists", "description": address}}}, "a"]]})
                    }
                }
                Some("x:Account/get") => {
                    let list: Vec<Json> = keep
                        .lock()
                        .expect("lock")
                        .iter()
                        .map(|a| json!({"id": a, "emailAddress": a}))
                        .collect();
                    json!({"methodResponses": [["x:Account/get", {"list": list}, "a"]]})
                }
                _ => json!({"methodResponses": [["error", {"type": "unknownMethod"}, "x"]]}),
            };
            let _ = request.respond(tiny_http::Response::from_string(answer.to_string()));
        }
    });
    (at, held, asked)
}

/// Waits until `check` holds, or fails after five seconds.
fn until(what: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..100 {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{what}: not in five seconds");
}

fn enroll(api: &str, address: &str, operator: &str, code: &str, device: &SigningKey) -> String {
    let signed = format!("{code}{BASE}");
    let body = json!({"address": address, "operator": operator, "code": code,
        "device": base64url::encode(&device.verifying_key().to_bytes()),
        "signature": base64url::encode(&crypto::sign(context::API_ENROLL, signed.as_bytes(), device))});
    let (status, enrolled) = http(api, "POST", "/rp/v1/enroll", None, Some(&body));
    assert_eq!(status, 200, "{enrolled}");
    enrolled["credential"]
        .as_str()
        .expect("a credential")
        .to_owned()
}

fn login(api: &str, credential: &str, device: &SigningKey) -> String {
    let (_, c) = http(api, "POST", "/rp/v1/auth/challenge", None, None);
    let nonce = c["nonce"].as_str().expect("a nonce").to_owned();
    let mut signed = base64url::decode(&nonce).expect("base64url");
    signed.extend_from_slice(BASE.as_bytes());
    let body = json!({"credential": credential, "nonce": nonce,
        "signature": base64url::encode(&crypto::sign(context::API_LOGIN, &signed, device))});
    let (status, answer) = http(api, "POST", "/rp/v1/auth/token", None, Some(&body));
    assert_eq!(status, 200, "{answer}");
    answer["token"].as_str().expect("a token").to_owned()
}

fn settings(stalwart: String, smtp: &MailServer) -> Settings {
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
            mail: json!({"imap": "po.test.example:993"}),
            updated: 0,
            not_before: 0,
            not_after: u64::MAX,
            post_offices: Vec::new(),
            applications: Some("open".into()),
        },
        known: Vec::new(),
        blocked: Vec::new(),
        resolver: None,
        hooks: None,
        hook_token: None,
        smtp: Some(format!("smtp://{}", smtp.at)),
        operators: Vec::new(),
        lookups: radiopost_postoffice::lookups::LookupSettings::default(),
        mailboxes: MailboxSettings {
            kind: "stalwart".into(),
            url: stalwart,
            api_key: Some("stalwart-key".into()),
            ..MailboxSettings::default()
        },
        health: radiopost_postoffice::serve::HealthSettings::default(),
        compress: true,
    }
}

#[test]
#[allow(clippy::too_many_lines, reason = "one story, told in order")]
fn every_account_gets_its_mailbox_however_it_began() {
    let dir = std::env::temp_dir().join(format!("radiopost-mailboxes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    let smtp = MailServer::start().expect("the stand-in mail server");
    let (stalwart_at, held, asked) = stalwart();
    let files = Files::new(&dir);
    files.write("key.hex", &"07".repeat(32)).expect("written");
    files
        .write(
            "config.json",
            &serde_json::to_string(&settings(stalwart_at, &smtp)).expect("JSON"),
        )
        .expect("written");
    let running = Arc::new(Running::load(&dir, serve::system_draw()).expect("loaded"));
    let (api, admin) =
        serve::serve(Arc::clone(&running), "127.0.0.1:0", "127.0.0.1:0").expect("serving");
    let has = |a: &str| held.lock().expect("lock").contains(a);

    // a code for a new address, made on the server: the mailbox is made, its password shown once
    let (status, made) = http(
        &admin,
        "POST",
        "/code",
        None,
        Some(&json!({"address": "kk4oda@test.example", "operator": "KK4ODA"})),
    );
    assert_eq!(status, 200, "{made}");
    assert!(has("kk4oda@test.example"));
    assert_eq!(made["mailbox"]["address"], "kk4oda@test.example", "{made}");
    assert_eq!(made["mailbox"]["password"].as_str().map(str::len), Some(20));
    let code = made["code"].as_str().expect("a code").to_owned();

    // the operator enrolls with it; a code for a new device of the account makes no mailbox
    let laptop = SigningKey::from_bytes(&[3; 32]);
    let credential = enroll(&api, "kk4oda@test.example", "KK4ODA", &code, &laptop);
    let (_, again) = http(
        &admin,
        "POST",
        "/code",
        None,
        Some(&json!({"address": "kk4oda@test.example", "operator": "KK4ODA"})),
    );
    assert!(again.get("mailbox").is_none(), "{again}");
    assert_eq!(*asked.lock().expect("lock"), 1);

    // a code from the desk for a new address makes it too
    let op = login(&api, &credential, &laptop);
    let (status, made) = http(
        &api,
        "POST",
        "/rp/v1/office/codes",
        Some(&op),
        Some(&json!({"address": "g@test.example", "operator": "N1CALL", "name": "Georges"})),
    );
    assert_eq!(status, 200, "{made}");
    assert!(made["mailbox"]["password"].is_string(), "{made}");
    assert!(has("g@test.example"));

    // the mail server loses g@'s mailbox: Health says so, and the desk's list marks it
    let line = running.accounts_line().expect("a line");
    assert_eq!(line.verdict, Verdict::Ok, "{}", line.text);
    held.lock().expect("lock").remove("g@test.example");
    let line = running.accounts_line().expect("a line");
    assert_eq!(line.verdict, Verdict::Fail);
    assert!(
        line.text.contains("g@test.example has no mailbox") && !line.text.contains("kk4oda@"),
        "{}",
        line.text
    );
    let (_, accounts) = http(&api, "GET", "/rp/v1/office/accounts", Some(&op), None);
    let mailbox_of = |list: &Json, a: &str| {
        list["accounts"]
            .as_array()
            .expect("accounts")
            .iter()
            .find(|x| x["address"] == a)
            .map(|x| x["mailbox"].clone())
    };
    assert_eq!(mailbox_of(&accounts, "g@test.example"), Some(json!(false)));
    assert_eq!(
        mailbox_of(&accounts, "kk4oda@test.example"),
        Some(json!(true))
    );

    // Make mailbox, from the desk: made again, its password shown once; Health is clear
    let (status, m) = http(
        &api,
        "POST",
        "/rp/v1/office/mailbox",
        Some(&op),
        Some(&json!({"address": "G@test.example"})),
    );
    assert_eq!(status, 200, "{m}");
    assert!(m["password"].is_string() && has("g@test.example"), "{m}");
    assert_eq!(
        running.accounts_line().expect("a line").verdict,
        Verdict::Ok
    );
    // and from the operator's port; an address that is no account is refused
    held.lock().expect("lock").remove("g@test.example");
    let (status, m) = http(
        &admin,
        "POST",
        "/mailbox",
        None,
        Some(&json!({"address": "g@test.example"})),
    );
    assert_eq!(status, 200, "{m}");
    assert!(m["password"].is_string(), "{m}");
    let (status, _) = http(
        &admin,
        "POST",
        "/mailbox",
        None,
        Some(&json!({"address": "nobody@test.example"})),
    );
    assert_eq!(status, 400);
    let (status, _) = http(
        &api,
        "POST",
        "/rp/v1/office/mailbox",
        Some(&op),
        Some(&json!({"address": "nobody@test.example"})),
    );
    assert_eq!(status, 404);

    // José applies and is sent a code by post: no mailbox yet, as his licence is not proved
    let jose = SigningKey::from_bytes(&[5; 32]);
    let signed = format!("jose@test.example\nN5CALL\n{BASE}");
    let application = json!({"address": "jose@test.example", "operator": "N5CALL",
        "name": "José Pérez", "contact": "jose@example.net",
        "device": base64url::encode(&jose.verifying_key().to_bytes()),
        "signature": base64url::encode(&crypto::sign(context::API_APPLY, signed.as_bytes(), &jose))});
    let (status, applied) = http(
        &api,
        "POST",
        "/rp/v1/applications",
        None,
        Some(&application),
    );
    assert_eq!(status, 202, "{applied}");
    let id = applied["application"].as_str().expect("an ID");
    let (_, decided) = http(
        &admin,
        "POST",
        "/decide",
        None,
        Some(&json!({"application": id, "decision": "postcard"})),
    );
    let card = decided["code"]
        .as_str()
        .expect("the card's code")
        .to_owned();
    until("José told of the card", || {
        smtp.took().iter().any(|t| t.to == ["jose@example.net"])
    });
    assert!(!has("jose@test.example"));

    // he enrolls with the card's code: the mailbox is made, and the welcome brings its password
    enroll(&api, "jose@test.example", "N5CALL", &card, &jose);
    until("José welcomed", || {
        smtp.took()
            .iter()
            .filter(|t| t.to == ["jose@example.net"])
            .count()
            == 2
    });
    assert!(has("jose@test.example"));
    let welcome = smtp
        .took()
        .into_iter()
        .rfind(|t| t.to == ["jose@example.net"])
        .expect("sent");
    let (h, body) = split(&welcome.email);
    let text = String::from_utf8(decoded_body(&h, &body)).expect("UTF-8");
    assert!(
        text.contains("jose@test.example is ready")
            && text.contains("password   ")
            && text.contains("po.test.example:993"),
        "{text}"
    );
    let (_, list) = http(&admin, "GET", "/applications", None, None);
    assert!(
        list["applications"][0]["mailbox"]["made"].is_string(),
        "{list}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
