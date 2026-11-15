// SPDX-License-Identifier: MIT OR Apache-2.0
//! The backup mail server (wishlist W38) with the service: its token made on the operator's port,
//! the door's snapshot served only to it, its own door refusing as the post office's does, and
//! the verdicts it seals taken by the post office's door when the message is handed on.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::Arc;

use radiopost::base64url;
use radiopost_postoffice::backup_mx::{self, HEADER, Snapshot};
use radiopost_postoffice::documents::Info;
use radiopost_postoffice::serve::{self, Files, Running, Settings};
use serde_json::{Value as Json, json};

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("radiopost-backup-{name}-{}", std::process::id()));
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

/// One call of the post office's door over HTTP: the JSON answer.
fn door(at: &str, body: &Json) -> Json {
    let mut s = TcpStream::connect(at).expect("the door");
    let text = body.to_string();
    write!(
        s,
        "POST /hook HTTP/1.1\r\nHost: {at}\r\nAuthorization: Bearer door-secret\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    )
    .expect("sent");
    let mut answer = String::new();
    s.read_to_string(&mut answer).expect("an answer");
    serde_json::from_str(answer.split_once("\r\n\r\n").map_or("", |(_, b)| b)).expect("JSON")
}

/// Stalwart 1.1's shape: the client given, no verdicts, so the door checks the sender itself.
fn stalwart(stage: &str, from: &str, to: &str, email: &str) -> Json {
    let (head, contents) = email.split_once("\r\n\r\n").unwrap_or((email, ""));
    let headers: Vec<Json> = head
        .split("\r\n")
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| json!([n, format!("{v}\r\n")]))
        .collect();
    json!({
        "context": {"stage": stage, "protocol": {"version": 1},
                    "client": {"ip": "192.0.2.25", "helo": "mx2.test.example"}},
        "envelope": {"from": {"address": from}, "to": [{"address": to}]},
        "message": {"headers": headers, "contents": contents, "size": email.len()}
    })
}

/// The MTA Hooks draft's shape, with the mail server's own DMARC verdict.
fn draft(stage: &str, from: &str, to: &str, email: &str) -> Json {
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

/// A sender whose domain's DNS fails for now: the post office's own checks hold its mail back.
const FLAKY: &str = "From: Maria <maria@_dns_error.example.com>\r\n\
To: Facundo <facundo@test.example>\r\nSubject: Are you safe?\r\n\
Date: Mon, 09 Nov 2026 08:00:00 -0600\r\nMessage-ID: <m.2@mail.example.com>\r\n\
MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n\
We heard about the storm. Are you safe?\r\n";

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one backup, made, fed, judging, and taken away"
)]
fn a_backup_refuses_as_the_door_does_and_its_sealed_verdicts_are_taken() {
    let dir = scratch("door");
    let running = post_office(&dir);
    {
        let mut office = running.office();
        office.at(serve::now());
        office.add_code("7Q4F-AZ9K", "facundo@test.example", "KK4ODA", 3);
        office.state.accept.insert(
            "facundo@test.example".into(),
            vec!["@_dns_error.example.com".into()],
        );
    }
    // the operator makes the backup's token
    let (status, text) = running.admin("POST", "/backup-mx", br#"{"add": "mx2"}"#);
    assert_eq!(status, 200, "{text}");
    let answer: Json = serde_json::from_str(&text).expect("JSON");
    let token = answer["token"].as_str().expect("a token").to_owned();
    assert_eq!(answer["backup_mx"][0]["name"], "mx2");
    let (_, listed) = running.admin("GET", "/backup-mx", b"");
    assert!(
        !listed.contains(&token),
        "the token is shown once, never listed"
    );

    // the snapshot: refused without the token or with another, given with it
    let path = format!("/rp/v1{}", backup_mx::SNAPSHOT_PATH);
    assert_eq!(running.api("GET", &path, None, b"").0, 401);
    assert_eq!(running.api("GET", &path, Some("not-it"), b"").0, 401);
    let (status, _, text) = running.api("GET", &path, Some(&token), b"");
    assert_eq!(status, 200, "{text}");
    let snapshot: Snapshot = serde_json::from_str(&text).expect("a snapshot");
    assert_eq!(snapshot.domains, ["test.example"]);
    assert_eq!(snapshot.max_email_bytes, 100_000);
    assert_eq!(
        snapshot.accounts["facundo@test.example"],
        ["@_dns_error.example.com"]
    );

    // the backup fetches it as it runs: over HTTP, its token in the Authorization header
    let (api, _) = serve::serve(running.clone(), "127.0.0.1:0", "127.0.0.1:0").expect("serving");
    let fetched =
        backup_mx::fetch(&format!("http://{api}/rp/v1"), &token).expect("fetched over HTTP");
    assert_eq!(fetched.accounts, snapshot.accounts);
    assert!(backup_mx::fetch(&format!("http://{api}/rp/v1"), "not-it").is_err());

    // the backup's own door, with no DNS: refuses whom the door refuses, leaves postmaster@ to
    // its mail server, holds back what it cannot check, and seals what its mail server checked
    let key = backup_mx::key_of(&token);
    let now = serve::now();
    let decide = |body: &Json| -> Json {
        let (_, text) = backup_mx::decide(
            body.to_string().as_bytes(),
            &snapshot,
            Err("no DNS in this test".into()),
            &key,
            now,
        );
        serde_json::from_str(&text).unwrap_or(Json::Null)
    };
    let a = decide(&stalwart(
        "rcpt",
        "stranger@example.net",
        "facundo@test.example",
        FLAKY,
    ));
    assert_eq!(a["response"]["status"], 550, "not on the accept list: {a}");
    let a = decide(&stalwart(
        "rcpt",
        "stranger@example.net",
        "postmaster@test.example",
        FLAKY,
    ));
    assert_eq!(a["action"], "accept", "{a}");
    let a = decide(&stalwart(
        "data",
        "maria@_dns_error.example.com",
        "facundo@test.example",
        FLAKY,
    ));
    assert_eq!(a["response"]["status"], 451, "not checked, held back: {a}");
    let a = decide(&draft(
        "data",
        "maria@_dns_error.example.com",
        "facundo@test.example",
        FLAKY,
    ));
    let added = &a["add"][0];
    assert_eq!(added["path"], "/message/headers", "{a}");
    assert_eq!(added["value"]["name"], HEADER);
    let sealed = added["value"]["value"].as_str().expect("a seal").to_owned();

    // the post office's door, with no DNS either: the mail as the backup hands it on is taken on
    // the backup's verdicts; the same mail with a forged seal, or none, is held back
    let at = serve::serve_hooks(running.clone(), "127.0.0.1:0").expect("the door");
    let relayed = format!("{HEADER}: {sealed}\r\n{FLAKY}");
    let forged = format!("{HEADER}: v=1; t={now}; dmarc=pass; dkim=; mac=00ff\r\n{FLAKY}");
    for (email, taken) in [
        (&forged, false),
        (&FLAKY.to_owned(), false),
        (&relayed, true),
    ] {
        let a = door(
            &at,
            &stalwart(
                "data",
                "maria@_dns_error.example.com",
                "facundo@test.example",
                email,
            ),
        );
        if taken {
            assert_eq!(a["action"], "accept", "{a}");
        } else {
            assert_eq!(a["response"]["status"], 451, "{a}");
        }
    }
    assert_eq!(
        running.office().state.mailboxes["facundo@test.example"].len(),
        1,
        "the sealed one, once"
    );

    // a backup taken away seals nothing the door takes
    let _ = running.admin("POST", "/backup-mx", br#"{"remove": "mx2"}"#);
    let a = door(
        &at,
        &stalwart(
            "data",
            "maria@_dns_error.example.com",
            "facundo@test.example",
            &relayed,
        ),
    );
    assert_eq!(a["response"]["status"], 451, "{a}");
    assert_eq!(running.api("GET", &path, Some(&token), b"").0, 401);
    let _ = std::fs::remove_dir_all(&dir);
}
