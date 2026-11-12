// SPDX-License-Identifier: MIT OR Apache-2.0
//! `radiopost-po check`'s connections: the door called with its token (and without it), and the
//! submission port signed in to and asked for `MAIL FROM`, here a stand-in that speaks AUTH
//! PLAIN. DNS and HTTPS are checked against the real domain by hand (the operator's guide).

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;

use radiopost_postoffice::check::{self, Verdict};
use radiopost_postoffice::documents::Info;
use radiopost_postoffice::serve::{self, Files, Running, Settings};
use serde_json::json;

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("radiopost-check-{name}-{}", std::process::id()));
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
    };
    files
        .write(
            "config.json",
            &serde_json::to_string(&settings).expect("JSON"),
        )
        .expect("written");
    Arc::new(Running::load(dir, serve::system_draw()).expect("loaded"))
}

fn unused_port() -> String {
    let l = TcpListener::bind("127.0.0.1:0").expect("a port");
    l.local_addr().expect("an address").to_string()
}

#[test]
fn the_door_is_checked_with_its_token() {
    let running = post_office(&scratch("door"));
    let at = serve::serve_hooks(running, "127.0.0.1:0").expect("the door");
    let domains = vec!["test.example".to_owned()];
    let line = check::door(&at, Some("door-secret"), &domains);
    assert_eq!(line.verdict, Verdict::Ok, "{}", line.text);
    let line = check::door(&at, Some("another"), &domains);
    assert_eq!(line.verdict, Verdict::Fail);
    assert!(line.text.contains("refuses the token"), "{}", line.text);
    assert_eq!(check::door(&at, None, &domains).verdict, Verdict::Fail);
    assert_eq!(
        check::door(&unused_port(), Some("door-secret"), &domains).verdict,
        Verdict::Fail
    );
}

/// A submission port that takes `radiopost` with `secret` (AUTH PLAIN) and answers `MAIL FROM`
/// with `mail_reply`; it never takes a message.
fn submission_port(mail_reply: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let at = listener.local_addr().expect("an address").to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let Ok(mut out) = stream.try_clone() else {
                continue;
            };
            let mut lines = BufReader::new(stream);
            let mut say = |l: &str| {
                let _ = out.write_all(format!("{l}\r\n").as_bytes());
            };
            say("220 stand-in");
            let mut line = String::new();
            while lines.read_line(&mut line).is_ok_and(|n| n > 0) {
                let l = line.trim_end().to_owned();
                line.clear();
                let upper = l.to_uppercase();
                if upper.starts_with("EHLO") {
                    say("250-stand-in");
                    say("250 AUTH PLAIN LOGIN");
                } else if let Some(b64) = l.strip_prefix("AUTH PLAIN ") {
                    let plain = radiopost::base64url::decode(
                        b64.trim_end_matches('=')
                            .replace('+', "-")
                            .replace('/', "_")
                            .as_str(),
                    )
                    .unwrap_or_default();
                    if plain == b"\0radiopost\0secret" {
                        say("235 2.7.0 Authentication succeeded");
                    } else {
                        say("535 5.7.8 Authentication credentials invalid.");
                    }
                } else if upper.starts_with("MAIL FROM:") {
                    say(mail_reply);
                } else if upper == "RSET" {
                    say("250 2.0.0 OK");
                } else if upper == "QUIT" {
                    say("221 2.0.0 Bye");
                    break;
                } else {
                    say("502 5.5.2 not here");
                }
            }
        }
    });
    at
}

#[test]
fn the_submission_port_is_signed_in_to_and_asked_for_mail_from() {
    let domains = vec!["test.example".to_owned()];
    let at = submission_port("250 2.1.0 OK");
    let line = check::smtp(
        &format!("smtp://radiopost:secret@{at}"),
        "localhost",
        &domains,
    );
    assert_eq!(line.verdict, Verdict::Ok, "{}", line.text);

    let line = check::smtp(
        &format!("smtp://radiopost:wrong@{at}"),
        "localhost",
        &domains,
    );
    assert_eq!(line.verdict, Verdict::Fail);
    assert!(
        line.text.contains("refuses radiopost's password"),
        "{}",
        line.text
    );

    let refusing = submission_port("501 5.5.4 You are not allowed to send from this address.");
    let line = check::smtp(
        &format!("smtp://radiopost:secret@{refusing}"),
        "localhost",
        &domains,
    );
    assert_eq!(line.verdict, Verdict::Fail);
    assert!(line.text.contains("Must Match Sender"), "{}", line.text);

    let line = check::smtp(
        &format!("smtp://radiopost:secret@{}", unused_port()),
        "localhost",
        &domains,
    );
    assert_eq!(line.verdict, Verdict::Fail);
    assert!(line.text.contains("cannot connect"), "{}", line.text);
}

/// D34: the operator's `status` says the program's version, and what the update channel offers
/// once the post office has asked it (here a stand-in channel offering a newer release).
#[test]
fn status_says_the_version_and_a_newer_release() {
    let running = post_office(&scratch("status"));
    let (code, body) = running.admin("GET", "/status", b"");
    assert_eq!(code, 200, "{body}");
    let status: serde_json::Value = serde_json::from_str(&body).expect("JSON");
    assert_eq!(status["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(status["update"], serde_json::Value::Null, "not asked yet");

    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let base = format!("http://{}", listener.local_addr().expect("an address"));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut out = stream.try_clone().expect("a stream");
            let mut request = String::new();
            let _ = BufReader::new(stream).read_line(&mut request);
            let manifest = json!({"version": "99.0.0", "platforms": {}}).to_string();
            let _ = write!(
                out,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{manifest}",
                manifest.len()
            );
        }
    });
    let watch = radiopost_update::Watch::new(
        radiopost_update::Product::Tools,
        env!("CARGO_PKG_VERSION"),
        &base,
    );
    watch.check();
    running.watch_updates(watch);
    let (_, body) = running.admin("GET", "/status", b"");
    let status: serde_json::Value = serde_json::from_str(&body).expect("JSON");
    assert_eq!(status["update"]["offer"]["version"], "99.0.0", "{status}");
}
