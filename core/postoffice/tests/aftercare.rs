// SPDX-License-Identifier: MIT OR Apache-2.0
//! What follows an application, over HTTP, against stand-ins: a new application is told to the
//! post office's contact; approved, its mailbox is made in a stand-in Stalwart (JMAP
//! `x:Domain/get`, then `x:Account/set`) and the applicant is sent the welcome with the mailbox's
//! password; declined, the reason; and a mailbox the mail server refuses is said on the desk.

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use radiopost::base64url;
use radiopost::crypto::{self, context};
use radiopost_postoffice::documents::Info;
use radiopost_postoffice::mailboxes::MailboxSettings;
use radiopost_postoffice::serve::{self, Files, Running, Settings};
use radiopost_tester::mailserver::{MailServer, decoded_body, header, split};
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

/// A stand-in Stalwart: its JMAP endpoint, and every request body it took. It has the domain
/// `test.example` (ID `d1`) and makes every account but `taken@`.
fn stalwart() -> (String, Arc<Mutex<Vec<Json>>>) {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("a port");
    let at = format!("http://{}", server.server_addr());
    let took: Arc<Mutex<Vec<Json>>> = Arc::default();
    let keep = Arc::clone(&took);
    std::thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let auth = request
                .headers()
                .iter()
                .find(|h| h.field.equiv("Authorization"))
                .map(|h| h.value.as_str().to_owned());
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            let body: Json = serde_json::from_str(&body).unwrap_or(Json::Null);
            if auth.as_deref() != Some("Bearer stalwart-key") || request.url() != "/jmap/" {
                let _ = request.respond(tiny_http::Response::from_string("").with_status_code(401));
                continue;
            }
            keep.lock().expect("lock").push(body.clone());
            let call = &body["methodCalls"][0];
            let answer = match call[0].as_str() {
                Some("x:Domain/get") => json!({"methodResponses": [
                    ["x:Domain/get", {"list": [{"id": "d0", "name": "other.example"},
                                               {"id": "d1", "name": "test.example"}]}, "d"]]}),
                Some("x:Account/set") if call[1]["create"]["new"]["name"] == "taken" => {
                    json!({"methodResponses": [["x:Account/set", {"notCreated": {"new":
                        {"type": "alreadyExists", "description": "taken@test.example"}}}, "a"]]})
                }
                Some("x:Account/set") => json!({"methodResponses": [
                    ["x:Account/set", {"created": {"new": {"id": "acc7"}}}, "a"]]}),
                _ => json!({"methodResponses": [["error", {"type": "unknownMethod"}, "x"]]}),
            };
            let _ = request.respond(tiny_http::Response::from_string(answer.to_string()));
        }
    });
    (at, took)
}

fn apply(api: &str, local: &str, call: &str, name: &str, seed: u8) -> String {
    let device = SigningKey::from_bytes(&[seed; 32]);
    let address = format!("{local}@test.example");
    let signed = format!("{address}\n{call}\n{BASE}");
    let body = json!({"address": address, "operator": call, "name": name,
        "contact": format!("{local}@example.net"),
        "device": base64url::encode(&device.verifying_key().to_bytes()),
        "signature": base64url::encode(&crypto::sign(context::API_APPLY, signed.as_bytes(), &device))});
    let (status, applied) = http(api, "POST", "/rp/v1/applications", None, Some(&body));
    assert_eq!(status, 202, "{applied}");
    applied["application"].as_str().expect("an ID").to_owned()
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

#[test]
#[allow(clippy::too_many_lines, reason = "one story, told in order")]
fn an_approved_account_gets_its_mailbox_and_its_welcome() {
    let dir = std::env::temp_dir().join(format!("radiopost-aftercare-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    let smtp = MailServer::start().expect("the stand-in mail server");
    let (stalwart_at, stalwart_took) = stalwart();
    let files = Files::new(&dir);
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
            mail: json!({"imap": "po.test.example:993"}),
            updated: 0,
            not_before: 0,
            not_after: u64::MAX,
            post_offices: Vec::new(),
            applications: Some("open".into()),
        },
        known: Vec::new(),
        hooks: None,
        hook_token: None,
        smtp: Some(format!("smtp://{}", smtp.at)),
        operators: Vec::new(),
        lookups: radiopost_postoffice::lookups::LookupSettings::default(),
        mailboxes: MailboxSettings {
            kind: "stalwart".into(),
            url: stalwart_at,
            api_key: Some("stalwart-key".into()),
            ..MailboxSettings::default()
        },
    };
    files
        .write(
            "config.json",
            &serde_json::to_string(&settings).expect("JSON"),
        )
        .expect("written");
    let running = Arc::new(Running::load(&dir, serve::system_draw()).expect("loaded"));
    let (api, admin) = serve::serve(running, "127.0.0.1:0", "127.0.0.1:0").expect("serving");

    // the server's own command checks the way to make mailboxes, without making one
    let (_, checked) = http(&admin, "POST", "/mailboxes/check", None, None);
    assert_eq!(checked["ok"], json!(true), "{checked}");

    // José applies: the contact address is told
    let jose = apply(&api, "jose", "N5CALL", "José Pérez", 5);
    until("the operators told", || {
        smtp.took()
            .iter()
            .any(|t| t.to == ["postmaster@test.example"])
    });
    let notice = smtp
        .took()
        .into_iter()
        .find(|t| t.to == ["postmaster@test.example"])
        .expect("told");
    let (h, body) = split(&notice.email);
    assert!(header(&h, "Subject").expect("a subject").contains("N5CALL"));
    let text = String::from_utf8(decoded_body(&h, &body)).expect("UTF-8");
    assert!(text.contains("jose@test.example"), "{text}");

    // approved from the operator's port: the mailbox is made, and José is welcomed
    let (_, decided) = http(
        &admin,
        "POST",
        "/decide",
        None,
        Some(&json!({"application": jose, "decision": "approve"})),
    );
    assert_eq!(decided["status"], "approved", "{decided}");
    until("José welcomed", || {
        smtp.took().iter().any(|t| t.to == ["jose@example.net"])
    });
    let made = stalwart_took.lock().expect("lock").clone();
    let create = made
        .iter()
        .find(|b| b["methodCalls"][0][0] == "x:Account/set")
        .expect("an account made")["methodCalls"][0][1]["create"]["new"]
        .clone();
    assert_eq!(
        (
            create["@type"].as_str(),
            create["name"].as_str(),
            create["domainId"].as_str()
        ),
        (Some("User"), Some("jose"), Some("d1"))
    );
    let password = create["credentials"][0]["secret"]
        .as_str()
        .expect("a password")
        .to_owned();
    let welcome = smtp
        .took()
        .into_iter()
        .find(|t| t.to == ["jose@example.net"])
        .expect("sent");
    assert_eq!(welcome.from, "postmaster@test.example");
    let (h, body) = split(&welcome.email);
    assert_eq!(
        header(&h, "Subject").as_deref(),
        Some("Welcome to RadioPost: jose@test.example")
    );
    let text = String::from_utf8(decoded_body(&h, &body)).expect("UTF-8");
    assert!(
        text.contains(&password) && text.contains("po.test.example:993"),
        "{text}"
    );
    let (_, list) = http(&admin, "GET", "/applications", None, None);
    let entry = &list["applications"][0];
    assert!(
        entry["mailbox"]["made"]
            .as_str()
            .is_some_and(|m| m.contains("acc7")),
        "{entry}"
    );
    assert_eq!(entry["told"]["to"], "jose@example.net");

    // a mailbox the mail server refuses is said on the entry; the approval stands
    let taken = apply(&api, "taken", "N6CALL", "Ta Ken", 6);
    http(
        &admin,
        "POST",
        "/decide",
        None,
        Some(&json!({"application": taken, "decision": "approve"})),
    );
    until("the refusal noted", || {
        let (_, list) = http(&admin, "GET", "/applications", None, None);
        list["applications"][1]["mailbox"]["error"].is_string()
    });
    let (_, list) = http(&admin, "GET", "/applications", None, None);
    assert_eq!(list["applications"][1]["status"], "approved");
    assert!(
        list["applications"][1]["mailbox"]["error"]
            .as_str()
            .is_some_and(|e| e.contains("alreadyExists"))
    );

    // declined: the reason goes to the applicant
    let lena = apply(&api, "lena", "N7CALL", "Lena Ortiz", 7);
    http(
        &admin,
        "POST",
        "/decide",
        None,
        Some(&json!({"application": lena, "decision": "decline",
                     "text": "No licence matches N7CALL."})),
    );
    until("Lena told", || {
        smtp.took().iter().any(|t| t.to == ["lena@example.net"])
    });
    let told = smtp
        .took()
        .into_iter()
        .find(|t| t.to == ["lena@example.net"])
        .expect("sent");
    let (h, body) = split(&told.email);
    let text = String::from_utf8(decoded_body(&h, &body)).expect("UTF-8");
    assert!(text.contains("No licence matches N7CALL."), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}
