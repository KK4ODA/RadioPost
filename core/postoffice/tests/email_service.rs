// SPDX-License-Identifier: MIT OR Apache-2.0
//! Email at a running post office (RP-6), over its real ports: the mail server's MTA hooks reach
//! the door in either shape, a message taken becomes a RadioPost message in the mailbox, mail a
//! user sent leaves through SMTP submission (here a stand-in mail server), a recipient refused
//! for good gets its sender a `Not delivered` message, and the bounce that comes back later is
//! taken at the door and becomes one too.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use radiopost::base64url;
use radiopost::cbor::Value;
use radiopost::credential::flags;
use radiopost::crypto::{self, context};
use radiopost::message::{Compression, Message, key};
use radiopost::statement::{Form, Statement};
use radiopost_postoffice::documents::Info;
use radiopost_postoffice::email as serve_email;
use radiopost_postoffice::office::{Outgoing, Route};
use radiopost_postoffice::serve::{self, Files, Running, Settings};
use serde_json::{Value as Json, json};

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("radiopost-email-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    dir
}

fn post_office(dir: &Path) -> Arc<Running> {
    let files = Files::new(dir);
    files.write("key.hex", &"07".repeat(32)).expect("written");
    let settings = Settings {
        domains: vec!["test.example".into()],
        base_url: "https://po.test.example/rp/v1".into(),
        listen: "127.0.0.1:0".into(),
        admin: "127.0.0.1:0".into(),
        info: Info {
            operator: "KK4ODA".into(),
            contact: "postmaster@test.example".into(),
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
        },
        known: Vec::new(),
        blocked: Vec::new(),
        resolver: None,
        hooks: None,
        hook_token: Some("door-secret".into()),
        smtp: None,
        operators: Vec::new(),
        lookups: radiopost_postoffice::lookups::LookupSettings::default(),
        mailboxes: radiopost_postoffice::mailboxes::MailboxSettings::default(),
        health: radiopost_postoffice::serve::HealthSettings::default(),
        compress: true,
    };
    files
        .write(
            "config.json",
            &serde_json::to_string(&settings).expect("JSON"),
        )
        .expect("written");
    Arc::new(Running::load(dir, serve::system_draw()).expect("loaded"))
}

/// One HTTP POST: the status and the body.
fn post(at: &str, token: &str, body: &Json) -> (u16, String) {
    let mut s = TcpStream::connect(at).expect("the door");
    let text = body.to_string();
    write!(
        s,
        "POST /hook HTTP/1.1\r\nHost: {at}\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
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
    (
        status,
        answer
            .split_once("\r\n\r\n")
            .map_or("", |(_, b)| b)
            .to_owned(),
    )
}

/// Stalwart 1.1's shape of a hook call: each header value keeps its leading space and line ending.
fn stalwart(stage: &str, from: &str, to: &str, email: &str) -> Json {
    let (head, contents) = email.split_once("\r\n\r\n").unwrap_or((email, ""));
    let headers: Vec<Json> = head
        .split("\r\n")
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| json!([n, format!("{v}\r\n")]))
        .collect();
    json!({
        "context": {"stage": stage, "protocol": {"version": 1},
                    "client": {"ip": "192.0.2.1", "helo": "mail.example.com"}},
        "envelope": {"from": {"address": from}, "to": [{"address": to}]},
        "message": {"headers": headers, "contents": contents, "size": email.len()}
    })
}

/// The MTA Hooks draft's shape: the raw message in standard base64.
fn draft(stage: &str, from: Option<&str>, to: &str, email: &str) -> Json {
    let b64 = base64url::encode(email.as_bytes())
        .replace('-', "+")
        .replace('_', "/");
    let pad = "=".repeat((4 - b64.len() % 4) % 4);
    json!({
        "stage": stage, "action": "accept",
        "envelope": {"from": {"address": from}, "to": [{"address": to}]},
        "rawMessage": format!("{b64}{pad}"),
        "senderAuth": {"dmarc": "pass"}
    })
}

const MARIA: &str = "From: Maria <maria@example.com>\r\nTo: Facundo <facundo@test.example>\r\n\
Subject: Are you safe?\r\nDate: Mon, 09 Nov 2026 08:00:00 -0600\r\n\
Message-ID: <m.1@mail.example.com>\r\nMIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\nWe heard about the storm. Are you safe?\r\n";

const ENCRYPTED: &str = "From: maria@example.com\r\nTo: facundo@test.example\r\n\
Subject: Secret\r\nMIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\n\
-----BEGIN PGP MESSAGE-----\r\nhQEMA\r\n-----END PGP MESSAGE-----\r\n";

/// A stand-in mail server's submission port: it takes every message but those for
/// `refused@example.net`, and keeps what it took.
/// What a stand-in mail server took: envelope sender, recipients, the email.
type Took = Arc<Mutex<Vec<(String, Vec<String>, String)>>>;

fn smtp_server(took: Took) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let at = listener.local_addr().expect("an address").to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let took = took.clone();
            std::thread::spawn(move || {
                let mut out = stream.try_clone().expect("a stream");
                let mut lines = BufReader::new(stream);
                let mut say = |l: &str| {
                    let _ = out.write_all(format!("{l}\r\n").as_bytes());
                };
                say("220 stand-in ESMTP");
                let (mut from, mut to, mut data) = (String::new(), Vec::new(), String::new());
                let mut line = String::new();
                while lines.read_line(&mut line).is_ok_and(|n| n > 0) {
                    let l = line.trim_end().to_owned();
                    line.clear();
                    let upper = l.to_uppercase();
                    if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                        say("250-stand-in");
                        say("250 8BITMIME");
                    } else if upper.starts_with("MAIL FROM:") {
                        l[10..]
                            .trim()
                            .trim_matches(['<', '>'])
                            .clone_into(&mut from);
                        to.clear();
                        say("250 2.1.0 OK");
                    } else if upper.starts_with("RCPT TO:") {
                        let r = l[8..].trim().trim_matches(['<', '>']).to_owned();
                        if r == "refused@example.net" {
                            say("550 5.1.1 No such user");
                        } else {
                            to.push(r);
                            say("250 2.1.5 OK");
                        }
                    } else if upper == "DATA" {
                        say("354 go on");
                        data.clear();
                        while lines.read_line(&mut line).is_ok_and(|n| n > 0) {
                            if line == ".\r\n" {
                                line.clear();
                                break;
                            }
                            data.push_str(line.strip_prefix('.').unwrap_or(&line));
                            line.clear();
                        }
                        took.lock()
                            .expect("lock")
                            .push((from.clone(), to.clone(), data.clone()));
                        say("250 2.0.0 queued");
                    } else if upper == "QUIT" {
                        say("221 bye");
                        break;
                    } else if upper == "RSET" || upper == "NOOP" {
                        say("250 OK");
                    } else {
                        say("502 5.5.2 not here");
                    }
                }
            });
        }
    });
    at
}

fn subjects(running: &Running, address: &str) -> Vec<(String, bool)> {
    running.office().state.mailboxes[address]
        .iter()
        .map(|w| (w.subject.clone(), w.third_party))
        .collect()
}

#[test]
#[allow(clippy::too_many_lines, reason = "one story, told in order")]
fn email_comes_in_at_the_door_and_goes_out_by_smtp() {
    let dir = scratch("story");
    let running = post_office(&dir);
    {
        let mut office = running.office();
        office.at(serve::now());
        office.add_code("7Q4F-AZ9K", "facundo@test.example", "KK4ODA", 3);
        office.state.accept.insert(
            "facundo@test.example".into(),
            vec!["maria@example.com".into()],
        );
        office
            .state
            .names
            .insert("facundo@test.example".into(), "Facundo".into());
    }
    let door = serve::serve_hooks(running.clone(), "127.0.0.1:0").expect("the door");

    // the door, as Stalwart calls it: a stranger refused at RCPT TO, Maria taken
    let (status, _) = post(
        &door,
        "wrong",
        &stalwart("rcpt", "maria@example.com", "facundo@test.example", MARIA),
    );
    assert_eq!(status, 401, "a hook without the token");
    let (_, body) = post(
        &door,
        "door-secret",
        &stalwart(
            "rcpt",
            "stranger@example.net",
            "facundo@test.example",
            MARIA,
        ),
    );
    let answer: Json = serde_json::from_str(&body).expect("JSON");
    assert_eq!(answer["action"], "reject");
    assert_eq!(answer["response"]["status"], 550);
    assert!(
        answer["response"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("Ask them to add you"))
    );
    let (_, body) = post(
        &door,
        "door-secret",
        &stalwart("rcpt", "maria@example.com", "facundo@test.example", MARIA),
    );
    assert_eq!(body, r#"{"action":"accept"}"#);
    let (_, body) = post(
        &door,
        "door-secret",
        &stalwart("data", "maria@example.com", "facundo@test.example", MARIA),
    );
    assert_eq!(body, r#"{"action":"accept"}"#);
    assert_eq!(
        subjects(&running, "facundo@test.example"),
        [("Are you safe?".to_owned(), true)]
    );

    // the same door, as the MTA Hooks draft calls it: an encrypted message refused at DATA
    let (status, body) = post(
        &door,
        "door-secret",
        &draft(
            "data",
            Some("maria@example.com"),
            "facundo@test.example",
            ENCRYPTED,
        ),
    );
    assert_eq!(status, 200);
    let answer: Json = serde_json::from_str(&body).expect("JSON");
    assert_eq!(answer["set"][1]["value"]["code"], 554);
    assert_eq!(
        subjects(&running, "facundo@test.example").len(),
        1,
        "nothing more in the mailbox"
    );

    // Facundo's answer, handed in by radio, waits to go on to Maria and to an address that
    // does not exist
    let reply_to = running.office().state.mailboxes["facundo@test.example"][0]
        .id
        .clone();
    let reply = Message::from_map(&Value::int_map(vec![
        (key::V, Value::Uint(1)),
        (key::FROM, Value::text("facundo@test.example")),
        (
            key::TO,
            Value::Array(vec![
                Value::text("maria@example.com"),
                Value::text("refused@example.net"),
            ]),
        ),
        (key::DATE, Value::Uint(1_794_235_000)),
        (key::SUBJECT, Value::text("Re: Are you safe?")),
        (key::BODY, Value::text("We are fine.\n")),
        (
            key::REPLY_TO,
            Value::bytes(
                &radiopost::crypto::ContentId::from_text(&reply_to)
                    .expect("an id")
                    .0,
            ),
        ),
    ]))
    .expect("a message");
    running.office().state.outgoing.push(Outgoing {
        id: reply.content_id().text(),
        air: base64url::encode(&reply.air_form(Compression::None)),
        to: vec!["maria@example.com".into(), "refused@example.net".into()],
        route: Some(Route {
            operator: "KK4ODA".into(),
            gateways: vec!["KK4ODA-1".into()],
            started: 1_794_235_240,
            session: "ec7b31841ec47c47".into(),
            ..Route::default()
        }),
        queued: serve::now(),
    });
    let took = Arc::new(Mutex::new(Vec::new()));
    let smtp = smtp_server(took.clone());
    let said = Arc::new(Mutex::new(Vec::new()));
    let s = said.clone();
    serve::send_mail(running.clone(), &format!("smtp://{smtp}"), move |l| {
        s.lock().expect("lock").push(l.to_owned());
    })
    .expect("sending");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while running.office().state.outgoing.len() == 1 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        running.office().state.outgoing.len(),
        0,
        "{:?}",
        said.lock().expect("lock")
    );
    let took = took.lock().expect("lock").clone();
    assert_eq!(took.len(), 1, "one transaction, Maria taken: {took:?}");
    let (from, to, email) = &took[0];
    assert_eq!(
        (from.as_str(), to.as_slice()),
        (
            "facundo@test.example",
            &["maria@example.com".to_owned()][..]
        )
    );
    let message_id = format!("<{}@test.example>", reply.content_id().text());
    for header in [
        "From: Facundo <facundo@test.example>".to_owned(),
        format!("Message-ID: {message_id}"),
        "In-Reply-To: <m.1@mail.example.com>".to_owned(),
        "Radio-Path: KK4ODA via KK4ODA-1; at=2026-11-09T14:40:40Z; session=ec7b31841ec47c47"
            .to_owned(),
    ] {
        assert!(email.contains(&header), "{header} in\n{email}");
    }
    // the refused recipient: a `Not delivered` message in Facundo's mailbox
    assert_eq!(
        subjects(&running, "facundo@test.example")[1],
        ("Not delivered: Re: Are you safe?".to_owned(), true)
    );

    // later Maria's server bounces it after all: a delivery report, null sender, taken
    let report = format!(
        "From: Mail Delivery System <MAILER-DAEMON@mx.example.com>\r\nTo: facundo@test.example\r\n\
         Subject: Undelivered\r\nMIME-Version: 1.0\r\n\
         Content-Type: multipart/report; report-type=delivery-status; boundary=\"r\"\r\n\r\n\
         --r\r\nContent-Type: text/plain\r\n\r\nNot delivered.\r\n\
         --r\r\nContent-Type: message/delivery-status\r\n\r\nReporting-MTA: dns; mx.example.com\r\n\r\n\
         Final-Recipient: rfc822; maria@example.com\r\nAction: failed\r\nStatus: 5.2.2\r\n\
         Diagnostic-Code: smtp; 552 5.2.2 Mailbox full\r\n\
         --r\r\nContent-Type: text/rfc822-headers\r\n\r\nFrom: Facundo <facundo@test.example>\r\n\
         Subject: Re: Are you safe?\r\nMessage-ID: {message_id}\r\n--r--\r\n"
    );
    let (status, _) = post(
        &door,
        "door-secret",
        &draft("data", None, "facundo@test.example", &report),
    );
    assert_eq!(status, 204, "taken");
    let office = running.office();
    let bounced = office.state.mailboxes["facundo@test.example"]
        .last()
        .expect("the bounce");
    let m = Message::from_air(&base64url::decode(&bounced.air).expect("air")).expect("a message");
    assert_eq!(m.from(), Some("postmaster@test.example"));
    assert_eq!(
        m.map().get(key::BODY).and_then(Value::as_text),
        Some("maria@example.com: 552 5.2.2 Mailbox full\n")
    );
    assert_eq!(
        m.map().get(key::REPLY_TO).and_then(Value::as_bytes),
        Some(&reply.content_id().0[..])
    );
    // and a null sender with no report about a message sent from here is refused
    drop(office);
    let (_, body) = post(
        &door,
        "door-secret",
        &draft("data", None, "facundo@test.example", MARIA),
    );
    let answer: Json = serde_json::from_str(&body).expect("JSON");
    assert_eq!(answer["set"][1]["value"]["code"], 550);
}

/// Stalwart 1.1 gives the door no verdicts, so the door checks the sender itself (RP-6 §2.1).
/// Here DNS is `mail-auth`'s test resolver: every name is missing, and a name with
/// `_dns_error.` in it fails for now. And an address that is no account is the mail server's.
#[test]
fn the_door_checks_senders_itself_and_leaves_other_addresses_to_the_mail_server() {
    let dir = scratch("checks");
    let running = post_office(&dir);
    {
        let mut office = running.office();
        office.at(serve::now());
        office.add_code("7Q4F-AZ9K", "facundo@test.example", "KK4ODA", 3);
        office.state.accept.insert(
            "facundo@test.example".into(),
            vec!["maria@example.com".into(), "@_dns_error.example.com".into()],
        );
    }
    let door = serve::serve_hooks(running.clone(), "127.0.0.1:0").expect("the door");
    let answer = |request: &Json| -> Json {
        let (_, body) = post(&door, "door-secret", request);
        serde_json::from_str(&body).expect("JSON")
    };

    // no DMARC record for example.com: taken
    let a = answer(&stalwart(
        "data",
        "maria@example.com",
        "facundo@test.example",
        MARIA,
    ));
    assert_eq!(a["action"], "accept", "{a}");

    // a lookup that fails for now: the sending server is asked to try again
    let flaky = MARIA.replace("maria@example.com", "maria@_dns_error.example.com");
    let a = answer(&stalwart(
        "data",
        "maria@_dns_error.example.com",
        "facundo@test.example",
        &flaky,
    ));
    assert_eq!(a["action"], "reject", "{a}");
    assert_eq!(a["response"]["status"], 451);
    assert_eq!(a["response"]["enhanced_status"], "4.7.1");

    // no verdicts and no client to check: the same
    let mut blind = stalwart("data", "maria@example.com", "facundo@test.example", MARIA);
    blind["context"]["client"] = Json::Null;
    let a = answer(&blind);
    assert_eq!(a["response"]["status"], 451, "{a}");
    assert_eq!(
        running.office().state.mailboxes["facundo@test.example"].len(),
        1,
        "only the first is in the mailbox"
    );

    // postmaster@ is no account: the door lets the mail server have it, and files nothing
    for stage in ["rcpt", "data"] {
        let a = answer(&stalwart(
            stage,
            "stranger@example.net",
            "postmaster@test.example",
            MARIA,
        ));
        assert_eq!(a["action"], "accept", "{stage}: {a}");
    }
    assert!(
        !running
            .office()
            .state
            .mailboxes
            .contains_key("postmaster@test.example")
    );
}

// -- one mailbox, by radio and by mail program (wishlist W36) -------------------------------

const BASE: &str = "https://po.test.example/rp/v1";

/// An API call through the service: the status and the JSON answer.
fn call(r: &Running, method: &str, path: &str, token: Option<&str>, body: &Json) -> (u16, Json) {
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        body.to_string().into_bytes()
    };
    let (status, _, text) = r.api(method, &format!("/rp/v1{path}"), token, &bytes);
    (status, serde_json::from_str(&text).unwrap_or(Json::Null))
}

/// A token for a credential and its device key (RP-5 §3).
fn login(r: &Running, credential: &[u8], device: &SigningKey) -> String {
    let (_, c) = call(r, "POST", "/auth/challenge", None, &Json::Null);
    let nonce = c["nonce"].as_str().expect("a nonce").to_owned();
    let mut signed = base64url::decode(&nonce).expect("base64url");
    signed.extend_from_slice(BASE.as_bytes());
    let body = json!({"credential": base64url::encode(credential), "nonce": nonce,
        "signature": base64url::encode(&crypto::sign(context::API_LOGIN, &signed, device))});
    let (status, answer) = call(r, "POST", "/auth/token", None, &body);
    assert_eq!(status, 200, "{answer}");
    answer["token"].as_str().expect("a token").to_owned()
}

/// A gateway here, KK4ODA-1, that holds copies: its token.
fn gateway_here(r: &Running) -> String {
    let key = SigningKey::from_bytes(&[0x61; 32]);
    let credential = {
        let mut office = r.office();
        office.at(serve::now());
        office.state.gateways.insert(
            "KK4ODA-1".into(),
            json!({"callsign": "KK4ODA-1", "operator": "KK4ODA", "grid": "EM73",
                   "place": "Atlanta, GA", "hours": "always", "internet": true,
                   "copies": {"users": 20, "days": 7, "max_air_bytes": 16384}, "updated": 1}),
        );
        office.issue(
            "kk4oda@test.example",
            "KK4ODA-1",
            key.verifying_key().to_bytes(),
            flags::SEND | flags::GATEWAY,
        )
    };
    login(r, &credential, &key)
}

/// A session a user opens through KK4ODA-1 (RP-5 §5.2), offering `offers`: its ID and answer.
fn open_session(
    r: &Running,
    gateway: &str,
    address: &str,
    seed: u8,
    offers: &[&Message],
) -> (String, Json) {
    let device = SigningKey::from_bytes(&[seed; 32]);
    let credential = {
        let mut office = r.office();
        office.at(serve::now());
        office.issue(
            address,
            "KK4ODA",
            device.verifying_key().to_bytes(),
            flags::SEND | flags::RECEIVE,
        )
    };
    let at = serve::now();
    let stmt = Statement {
        address: address.into(),
        operator: "KK4ODA".into(),
        gateway: "KK4ODA-1".into(),
        messages: offers.iter().map(|m| m.content_id()).collect(),
        form: Form::Time {
            time: at,
            nonce: [seed; 8],
        },
    };
    let (status, answer) = call(
        r,
        "POST",
        "/sessions",
        Some(gateway),
        &json!({"statement": base64url::encode(&stmt.encode()),
                "signature": base64url::encode(&stmt.sign(&device)),
                "credential": base64url::encode(&credential),
                "started": at, "gateway_session": "ec7b31841ec47c47"}),
    );
    assert_eq!(status, 201, "{answer}");
    (
        answer["session"].as_str().expect("a session").to_owned(),
        answer,
    )
}

fn message(from: &str, to: &[&str], subject: &str, body: &str) -> Message {
    Message::from_map(&Value::int_map(vec![
        (key::V, Value::Uint(1)),
        (key::FROM, Value::text(from)),
        (
            key::TO,
            Value::Array(to.iter().map(|t| Value::text(t)).collect()),
        ),
        (key::DATE, Value::Uint(1_794_235_000)),
        (key::SUBJECT, Value::text(subject)),
        (key::BODY, Value::text(body)),
    ]))
    .expect("a message")
}

/// Stalwart's hook call for mail a signed-in user submits on port 465.
fn submitted(stage: &str, login: &str, from: &str, to: &[&str], email: &str) -> Json {
    let mut call = stalwart(stage, from, to[0], email);
    call["context"]["sasl"] = json!({"login": login});
    call["context"]["server"] = json!({"name": "Stalwart v1.1.0", "port": 465, "ip": "0.0.0.0"});
    call["envelope"]["to"] = to.iter().map(|t| json!({"address": t})).collect();
    call
}

fn with_users(running: &Running) {
    let mut office = running.office();
    office.at(serve::now());
    office.add_code("FACU-0001", "facundo@test.example", "KK4ODA", 3);
    office.add_code("JOSE-0001", "jose@test.example", "KK4ODB", 3);
}

const FROM_THUNDERBIRD: &str = "From: Facundo <facundo@test.example>\r\n\
To: Jose <jose@test.example>, Maria <maria@example.com>\r\nSubject: From my laptop\r\n\
Date: Fri, 13 Nov 2026 20:00:00 +0000\r\nMessage-ID: <tb.1@laptop.example>\r\n\
MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nWritten in Thunderbird.\r\n";

/// RP-6 §2.6, RP-5 §6.5: mail a signed-in user sends from a mail program is not judged; each
/// recipient who is an account here gets its radio copy (third-party until G16 is settled), and
/// the recipients join the sender's accept list. As the draft calls the hook, the same.
#[test]
fn mail_a_user_submits_reaches_the_radio_and_fills_the_accept_list() {
    let dir = scratch("submitted");
    let running = post_office(&dir);
    with_users(&running);
    let door = serve::serve_hooks(running.clone(), "127.0.0.1:0").expect("the door");
    let to = ["jose@test.example", "maria@example.com", "bcc@example.org"];
    for stage in ["rcpt", "data"] {
        let (_, body) = post(
            &door,
            "door-secret",
            &submitted(
                stage,
                "facundo@test.example",
                "facundo@test.example",
                &to,
                FROM_THUNDERBIRD,
            ),
        );
        assert_eq!(body, r#"{"action":"accept"}"#, "{stage}: not judged");
    }
    assert_eq!(
        subjects(&running, "jose@test.example"),
        [("From my laptop".to_owned(), true)],
        "Jose's radio copy, flagged third-party"
    );
    {
        let office = running.office();
        assert!(
            !office.state.mailboxes.contains_key("facundo@test.example"),
            "nothing filed for the sender"
        );
        assert!(!office.state.mailboxes.contains_key("maria@example.com"));
        assert_eq!(
            office.state.accept["facundo@test.example"],
            to.map(str::to_owned),
            "every recipient of the submission, Bcc too"
        );
        // its Message-ID is kept, for `radio_skips_read` and for replies
        let id = &office.state.mailboxes["jose@test.example"][0].id;
        let cid = radiopost::crypto::ContentId::from_text(id).expect("an id");
        assert_eq!(
            office.email_message_id(&cid.0).as_deref(),
            Some("<tb.1@laptop.example>")
        );
    }

    // the draft's shape, `auth.login`; with `accept_on_send` false the list stays as it is
    running.office().state.settings.insert(
        "facundo@test.example".into(),
        json!({"accept_on_send": false}),
    );
    let mut call = draft(
        "data",
        Some("facundo@test.example"),
        "jose@test.example",
        &FROM_THUNDERBIRD
            .replace("tb.1@", "tb.2@")
            .replace("maria@example.com", "ana@example.net"),
    );
    call["auth"] = json!({"login": "facundo@test.example", "method": "plain"});
    call["envelope"]["to"] = json!([{"address": "jose@test.example"},
                                    {"address": "ana@example.net"}]);
    let (status, _) = post(&door, "door-secret", &call);
    assert_eq!(status, 204, "taken");
    assert_eq!(subjects(&running, "jose@test.example").len(), 2);
    assert!(
        !running.office().state.accept["facundo@test.example"]
            .contains(&"ana@example.net".to_owned())
    );

    // mail from the Internet is judged as before: a stranger is refused
    let stranger = FROM_THUNDERBIRD.replace("facundo@test.example", "stranger@example.net");
    let (_, body) = post(
        &door,
        "door-secret",
        &stalwart(
            "data",
            "stranger@example.net",
            "jose@test.example",
            &stranger,
        ),
    );
    assert!(body.contains("reject"), "{body}");
    assert_eq!(subjects(&running, "jose@test.example").len(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

/// What the service itself submits, signed in as its own account, is radio mail already filed:
/// the door lets it pass and files nothing, in either form of the account's name.
#[test]
fn the_services_own_submissions_file_nothing() {
    let dir = scratch("own");
    let running = post_office(&dir);
    with_users(&running);
    {
        let files = Files::new(&dir);
        let mut settings = files.settings().expect("settings");
        settings.smtp = Some("smtps://radiopost:secret@127.0.0.1:465".into());
        files
            .write(
                "config.json",
                &serde_json::to_string(&settings).expect("JSON"),
            )
            .expect("written");
    }
    let running = Arc::new(Running::load(&dir, serve::system_draw()).expect("loaded"));
    let door = serve::serve_hooks(running.clone(), "127.0.0.1:0").expect("the door");
    for login in [
        "radiopost",
        "radiopost@test.example",
        "RadioPost@Test.Example",
    ] {
        let (_, body) = post(
            &door,
            "door-secret",
            &submitted(
                "data",
                login,
                "facundo@test.example",
                &["jose@test.example", "maria@example.com"],
                FROM_THUNDERBIRD,
            ),
        );
        assert_eq!(body, r#"{"action":"accept"}"#, "{login}");
    }
    let office = running.office();
    assert!(!office.state.mailboxes.contains_key("jose@test.example"));
    assert!(!office.state.accept.contains_key("facundo@test.example"));
    drop(office);
    assert!(serve_email::is_service_login(
        "radiopost@test.example",
        Some("smtps://radiopost%40test.example:x@h:465"),
        &["test.example".to_owned()]
    ));
    assert!(!serve_email::is_service_login(
        "facundo@test.example",
        Some("smtps://radiopost:x@h:465"),
        &["test.example".to_owned()]
    ));
    assert!(!serve_email::is_service_login("radiopost", None, &[]));
    let _ = std::fs::remove_dir_all(dir);
}

/// RP-6 §2.6: mail sent by radio to users here is filed in their radio mailboxes and handed to
/// the mail server too, as RP-6 §3.1 makes its email, so a user who reads by mail program sees
/// it; an address here with no account (an email-only member, concept §3.1) gets only the
/// email. The mail server hands it back to the door signed in as the service: nothing more is
/// filed.
#[test]
fn radio_mail_between_users_here_reaches_the_mail_server() {
    let dir = scratch("between");
    let running = post_office(&dir);
    with_users(&running);
    let gateway = gateway_here(&running);
    let m = message(
        "facundo@test.example",
        &["jose@test.example", "ana@test.example", "maria@example.com"],
        "Net tonight",
        "The net is at 8.\n",
    );
    let (session, _) = open_session(&running, &gateway, "facundo@test.example", 0x41, &[&m]);
    let (status, answer) = call(
        &running,
        "POST",
        &format!("/sessions/{session}/messages"),
        Some(&gateway),
        &json!({"messages": [{"id": m.content_id().text(),
                              "air": base64url::encode(&m.air_form(Compression::None))}]}),
    );
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["results"][0]["status"], "accepted");
    assert_eq!(
        subjects(&running, "jose@test.example"),
        [("Net tonight".to_owned(), false)],
        "a user's own mail: not third-party"
    );
    assert!(
        !running
            .office()
            .state
            .mailboxes
            .contains_key("ana@test.example"),
        "Ana has no account: no radio mailbox"
    );

    let took = Arc::new(Mutex::new(Vec::new()));
    let smtp = smtp_server(took.clone());
    serve::send_mail(running.clone(), &format!("smtp://{smtp}"), |_| {}).expect("sending");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !running.office().state.outgoing.is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let took = took.lock().expect("lock").clone();
    assert_eq!(took.len(), 1, "one transaction: {took:?}");
    let (from, to, email) = &took[0];
    assert_eq!(from, "facundo@test.example");
    assert_eq!(
        to,
        &["jose@test.example", "ana@test.example", "maria@example.com"].map(str::to_owned)
    );
    let message_id = format!("<{}@test.example>", m.content_id().text());
    assert!(
        email.contains(&format!("Message-ID: {message_id}")),
        "{email}"
    );
    assert!(email.contains("Radio-Path: KK4ODA via KK4ODA-1"), "{email}");
    assert!(
        !email.contains("application/radiopost-message"),
        "no RadioPost part for the users' own mail programs: {email}"
    );

    // Stalwart delivers it, calling the door as the service signed in: nothing more is filed
    {
        let files = Files::new(&dir);
        let mut settings = files.settings().expect("settings");
        settings.smtp = Some(format!("smtp://radiopost:secret@{smtp}"));
        running.set_settings(settings);
    }
    let door = serve::serve_hooks(running.clone(), "127.0.0.1:0").expect("the door");
    let (_, body) = post(
        &door,
        "door-secret",
        &submitted(
            "data",
            "radiopost@test.example",
            "facundo@test.example",
            &["jose@test.example", "ana@test.example"],
            email,
        ),
    );
    assert_eq!(body, r#"{"action":"accept"}"#);
    assert_eq!(subjects(&running, "jose@test.example").len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

/// A stand-in for Stalwart's JMAP: its accounts, and the `Message-ID`s each has read.
fn jmap_server(read: Json) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("a port");
    let at = format!("http://{}", server.server_addr());
    std::thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            let asked: Json = serde_json::from_str(&body).unwrap_or(Json::Null);
            let authorised = request
                .headers()
                .iter()
                .any(|h| h.field.equiv("Authorization") && h.value.as_str() == "Bearer API_reader");
            if !authorised {
                let _ = request.respond(tiny_http::Response::from_string("").with_status_code(401));
                continue;
            }
            let mut answers = Vec::new();
            for c in asked["methodCalls"].as_array().cloned().unwrap_or_default() {
                let account = c[1]["accountId"].as_str().unwrap_or("").to_owned();
                match c[0].as_str() {
                    Some("x:Account/get") => answers.push(json!(["x:Account/get",
                        {"list": [{"id": "b", "emailAddress": "facundo@test.example"},
                                  {"id": "c", "emailAddress": "jose@test.example"}]}, c[2]])),
                    Some("Email/query") => {
                        assert_eq!(c[1]["filter"], json!({"hasKeyword": "$seen"}));
                        let n = read[&account].as_array().map_or(0, Vec::len);
                        let ids: Vec<String> = (0..n).map(|i| format!("e{i}")).collect();
                        answers.push(json!(["Email/query", {"ids": ids}, c[2]]));
                    }
                    Some("Email/get") => {
                        let list: Vec<Json> = read[&account]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .enumerate()
                            .map(|(i, m)| json!({"id": format!("e{i}"), "messageId": [m]}))
                            .collect();
                        answers.push(json!(["Email/get", {"list": list}, c[2]]));
                    }
                    _ => answers.push(json!(["error", {"type": "unknownMethod"}, c[2]])),
                }
            }
            let text = json!({"methodResponses": answers}).to_string();
            let _ = request.respond(tiny_http::Response::from_string(text));
        }
    });
    at
}

/// RP-5 §6.3: with `radio_skips_read` on, a message the user read in a mail program is taken as
/// picked up: the next waiting list leaves it out and its copies placed ahead are dropped. One
/// sent by radio from another user here is matched by the `Message-ID` RP-6 §3.1 gave its email.
#[test]
#[allow(clippy::too_many_lines, reason = "one story, told in order")]
fn radio_skips_read_takes_what_was_read_as_picked_up() {
    let dir = scratch("seen");
    let running = post_office(&dir);
    with_users(&running);
    let gateway = gateway_here(&running);
    running.office().state.settings.insert(
        "jose@test.example".into(),
        json!({"usual_gateways": ["KK4ODA-1"], "radio_skips_read": true}),
    );
    let maria = |n: u8| {
        MARIA
            .replace("facundo@test.example", "jose@test.example")
            .replace("m.1@", &format!("m.{n}@"))
            .replace("Are you safe?\r\nDate", &format!("Note {n}\r\nDate"))
    };
    for n in 1..=2 {
        running
            .email_in(
                "maria@example.com",
                &["jose@test.example".to_owned()],
                maria(n).as_bytes(),
            )
            .expect("taken");
    }
    let by_radio = message(
        "facundo@test.example",
        &["jose@test.example"],
        "By radio",
        "Read it on the laptop.\n",
    );
    let (session, _) = open_session(
        &running,
        &gateway,
        "facundo@test.example",
        0x41,
        &[&by_radio],
    );
    call(
        &running,
        "POST",
        &format!("/sessions/{session}/messages"),
        Some(&gateway),
        &json!({"messages": [{"id": by_radio.content_id().text(),
                              "air": base64url::encode(&by_radio.air_form(Compression::None))}]}),
    );
    assert_eq!(subjects(&running, "jose@test.example").len(), 3);

    // no way to read the mail server yet: said, nothing changed
    assert!(
        running
            .learn_read()
            .expect_err("no mailboxes")
            .contains("radio_skips_read")
    );

    let jmap = jmap_server(json!({"c": [
        "m.1@mail.example.com",
        format!("{}@test.example", by_radio.content_id().text()),
        "unrelated@elsewhere.example"
    ]}));
    {
        let files = Files::new(&dir);
        let mut settings = files.settings().expect("settings");
        settings.mailboxes.kind = "stalwart".into();
        settings.mailboxes.url = jmap;
        settings.mailboxes.api_key = Some("API_reader".into());
        running.set_settings(settings);
    }
    assert_eq!(running.learn_read().expect("read"), 2);
    assert_eq!(running.learn_read().expect("read"), 0, "nothing new");

    let (_, answer) = open_session(&running, &gateway, "jose@test.example", 0x42, &[]);
    let waiting: Vec<&str> = answer["waiting"]
        .as_array()
        .expect("waiting")
        .iter()
        .filter_map(|w| w["subject"].as_str())
        .collect();
    assert_eq!(
        waiting,
        ["Note 2"],
        "only what was not read in the mail program"
    );
    let (_, polled) = call(
        &running,
        "GET",
        "/copies?after=c_0&wait=0",
        Some(&gateway),
        &Json::Null,
    );
    let drops: Vec<u64> = polled["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter(|i| i["kind"] == "drop")
        .filter_map(|i| i["serial"].as_u64())
        .collect();
    assert_eq!(
        drops,
        [1, 3],
        "the copies of the two read are dropped: {polled}"
    );

    // with the setting off, nothing is taken
    running.office().state.settings.insert(
        "jose@test.example".into(),
        json!({"usual_gateways": ["KK4ODA-1"]}),
    );
    running
        .email_in(
            "maria@example.com",
            &["jose@test.example".to_owned()],
            maria(1).replace("m.1@", "m.9@").as_bytes(),
        )
        .expect("taken");
    assert_eq!(running.learn_read().expect("read"), 0);
    let _ = std::fs::remove_dir_all(dir);
}
