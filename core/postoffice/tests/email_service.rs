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

use radiopost::base64url;
use radiopost::cbor::Value;
use radiopost::message::{Compression, Message, key};
use radiopost_postoffice::documents::Info;
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
        },
        known: Vec::new(),
        hooks: None,
        hook_token: Some("door-secret".into()),
        smtp: None,
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
