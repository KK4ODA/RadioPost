// SPDX-License-Identifier: MIT OR Apache-2.0
//! The client and its post office over the Internet (RP-5 §6): two users enroll with the codes
//! their operator gave, using the `radiopost` program; one changes a setting and the accept list
//! and renews; they carry a station-to-station session through the loopback adapter; and each
//! reports it, so the post office files what crossed the air and sends the copy on (RP-5 §6.6).

mod common;

use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use radiopost_client::compose::Draft;
use radiopost_client::session::{self, Fetch, Kind, Options, Packing};
use radiopost_client::store::Store;
use radiopost_postoffice::documents::Info;
use radiopost_postoffice::serve::{self, Files, Running, Settings};
use serde_json::json;

use common::{loopback, scratch};

/// The post office for `test.example`, at a base URL that is the port it listens on.
fn post_office(dir: &Path) -> (Arc<Running>, String, String) {
    let port = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("a free port")
        .port();
    let base = format!("http://127.0.0.1:{port}");
    std::fs::create_dir_all(dir).expect("a directory");
    let files = Files::new(dir);
    files.write("key.hex", &"2a".repeat(32)).expect("written");
    let key = ed25519_dalek::SigningKey::from_bytes(&[0x2a; 32]);
    let settings = Settings {
        domains: vec!["test.example".into()],
        base_url: base.clone(),
        listen: format!("127.0.0.1:{port}"),
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
        },
        known: Vec::new(),
        hooks: None,
        hook_token: None,
        smtp: None,
    };
    files
        .write(
            "config.json",
            &serde_json::to_string(&settings).expect("JSON"),
        )
        .expect("written");
    let running = Arc::new(Running::load(dir, serve::system_draw()).expect("loaded"));
    serve::serve(running.clone(), &settings.listen, "127.0.0.1:0").expect("serving");
    let public = radiopost::cbor::hex(&key.verifying_key().to_bytes());
    (running, base, public)
}

/// Runs the `radiopost` program on a home; its output, or why it failed.
fn radiopost(home: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new(env!("CARGO_BIN_EXE_radiopost"))
        .arg("--home")
        .arg(home)
        .args(args)
        .output()
        .expect("the program runs");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(text)
    } else {
        Err(format!("{text}{}", String::from_utf8_lossy(&out.stderr)))
    }
}

#[test]
#[allow(clippy::too_many_lines, reason = "one story, told in order")]
fn users_enroll_change_settings_and_report_their_sessions() {
    let dir = scratch("online");
    let (running, base, public) = post_office(&dir.join("po"));
    let code = |address: &str, operator: &str| {
        let (_, answer) = running.admin(
            "POST",
            "/code",
            json!({"address": address, "operator": operator, "flags": ["send", "receive"]})
                .to_string()
                .as_bytes(),
        );
        serde_json::from_str::<serde_json::Value>(&answer).expect("JSON")["code"]
            .as_str()
            .expect("a code")
            .to_owned()
    };
    let facundo = dir.join("facundo");
    let ana = dir.join("ana");
    for (home, address, call) in [
        (&facundo, "facundo@test.example", "KK4ODA"),
        (&ana, "ana@test.example", "n0call"),
    ] {
        radiopost(home, &["trust", "test.example", &public]).expect("trusted");
        // a wrong code first: refused, and nothing kept
        let refused = radiopost(
            home,
            &[
                "enroll",
                "--address",
                address,
                "--operator",
                call,
                "--code",
                "AAAA-BBBB",
                "--post-office",
                &base,
            ],
        )
        .expect_err("a wrong code");
        assert!(refused.contains("not-authorized"), "{refused}");
        let c = code(address, &call.to_uppercase());
        let said = radiopost(
            home,
            &[
                "enroll",
                "--address",
                address,
                "--operator",
                call,
                "--code",
                &c,
                "--post-office",
                &base,
            ],
        )
        .expect("enrolled");
        assert!(
            said.contains(&format!("{address} ({})", call.to_uppercase())),
            "{said}"
        );
        // the code is used once
        radiopost(
            home,
            &[
                "enroll",
                "--address",
                address,
                "--operator",
                call,
                "--code",
                &c,
            ],
        )
        .expect_err("a code used");
    }

    // Facundo's settings and accept list, and a renewed credential
    let s = radiopost(
        &facundo,
        &["settings", "--set", r#"{"attach_bytes": 8000}"#],
    )
    .expect("set");
    assert!(s.contains("\"attach_bytes\": 8000"), "{s}");
    let list = radiopost(
        &facundo,
        &[
            "accept",
            "--add",
            "Maria@Example.com",
            "--add",
            "@redcross.example",
        ],
    )
    .expect("added");
    assert_eq!(list, "maria@example.com\n@redcross.example\n");
    let list = radiopost(&facundo, &["accept", "--remove", "@redcross.example"]).expect("removed");
    assert_eq!(list, "maria@example.com\n");
    let before = Store::open(&facundo)
        .expect("home")
        .me()
        .expect("me")
        .serial;
    radiopost(&facundo, &["renew"]).expect("renewed");
    assert!(
        Store::open(&facundo)
            .expect("home")
            .me()
            .expect("me")
            .serial
            > before
    );

    // a station-to-station session: Facundo writes to Ana, with a copy to Maria by email
    let mut f = Store::open(&facundo).expect("home");
    let mut a = Store::open(&ana).expect("home");
    let message = Draft {
        from: "facundo@test.example".into(),
        to: vec!["ana@test.example".into()],
        cc: vec!["maria@example.com".into()],
        date: serve::now(),
        subject: "Net at 8".into(),
        body: "See you on 3.590.\n".into(),
        ..Draft::default()
    }
    .message()
    .expect("a message");
    let id = f.queue(&message).expect("queued");
    let (_server, at_f, at_a) = loopback();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut a_opts = Options::new(&at_a.to_string());
    a_opts.packing = Packing::None;
    let answering = thread::spawn(move || {
        let mut say = |s: &str| {
            if s.starts_with("listening") {
                let _ = tx.send(());
            }
        };
        session::answer(&mut a, &a_opts, &mut say).expect("answered");
        a
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("Ana listens");
    let mut f_opts = Options::new(&at_f.to_string());
    f_opts.fetch = Fetch::Nothing;
    let mut said = Vec::new();
    let report = session::call(&mut f, "N0CALL", Kind::Station, &f_opts, &mut |s| {
        said.push(s.to_owned());
    })
    .expect("called");
    let a = answering.join().expect("Ana's side");
    assert_eq!(report.failed, None, "{said:?}");
    assert_eq!((f.reports.len(), a.reports.len()), (1, 1));
    drop((f, a));

    // each reports it: Ana's copy filed as picked up, Maria's copy waiting to go by email
    let out = radiopost(&facundo, &["report"]).expect("reported");
    assert!(out.contains(&format!("{} accepted", id.text())), "{out}");
    let out = radiopost(&ana, &["report"]).expect("reported");
    assert!(out.contains(&format!("{} accepted", id.text())), "{out}");
    assert_eq!(
        radiopost(&ana, &["report"]).expect("again"),
        "nothing to report\n"
    );
    let office = running.office();
    let filed = &office.state.mailboxes["ana@test.example"];
    assert_eq!(filed.len(), 1, "Ana's mailbox holds it once");
    assert!(
        filed[0].picked,
        "already picked up: no gateway offers it again"
    );
    let outgoing: Vec<(&str, &[String])> = office
        .state
        .outgoing
        .iter()
        .map(|o| (o.id.as_str(), o.to.as_slice()))
        .collect();
    assert_eq!(
        outgoing,
        [(id.text().as_str(), &["maria@example.com".to_owned()][..])]
    );
    // and the recipients joined Facundo's accept list (RP-5 §6.5): Maria was there, Ana is new
    assert_eq!(
        office.state.accept["facundo@test.example"],
        ["maria@example.com", "ana@test.example"]
    );
}

/// One request to the panel's API, marked as the panel marks its own: the status and the JSON.
fn panel(
    at: std::net::SocketAddr,
    path: &str,
    body: &serde_json::Value,
) -> (u16, serde_json::Value) {
    use std::io::{Read as _, Write as _};
    let mut s = std::net::TcpStream::connect(at).expect("the panel");
    let text = body.to_string();
    write!(
        s,
        "POST /api/{path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-RadioPost: 1\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        at.port(),
        text.len()
    )
    .expect("sent");
    let mut answer = String::new();
    s.read_to_string(&mut answer).expect("an answer");
    let status = answer[9..12].parse().expect("a status");
    let json = answer
        .split_once("\r\n\r\n")
        .and_then(|(_, b)| serde_json::from_str(b).ok())
        .unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// The panel's Setup: a device enrolls with its code, and the accept list is read and changed.
#[test]
fn a_device_enrolls_from_the_panel() {
    let dir = scratch("panel-enroll");
    let (running, base, public) = post_office(&dir.join("po"));
    let (_, answer) = running.admin(
        "POST",
        "/code",
        json!({"address": "shelter3@test.example", "operator": "N3CALL",
               "flags": ["send", "receive"]})
        .to_string()
        .as_bytes(),
    );
    let code = serde_json::from_str::<serde_json::Value>(&answer).expect("JSON")["code"]
        .as_str()
        .expect("a code")
        .to_owned();
    let at = radiopost_client::panel::Panel::new(&dir.join("home"), None)
        .start("127.0.0.1:0")
        .expect("the panel");
    let (status, _) = panel(
        at,
        "trust",
        &json!({"domain": "test.example", "public": public}),
    );
    assert_eq!(status, 200);
    let (status, said) = panel(
        at,
        "enroll",
        &json!({"address": "shelter3@test.example", "operator": "n3call", "code": code,
                "url": base}),
    );
    assert_eq!(status, 200, "{said}");
    assert!(
        said["said"]
            .as_str()
            .is_some_and(|s| s.starts_with("shelter3@test.example (N3CALL)")),
        "{said}"
    );
    let (_, state) = panel(at, "post-office", &json!({"url": base}));
    assert_eq!(state["me"]["operator"], "N3CALL");
    assert_eq!(state["post_office"], base.as_str());
    let (_, list) = panel(at, "accept", &json!({"add": ["@RedCross.example"]}));
    assert_eq!(list["entries"], json!(["@redcross.example"]));
    let (status, refused) = panel(
        at,
        "enroll",
        &json!({"address": "x@test.example",
        "operator": "N0CALL", "code": "AAAA-BBBB", "url": base}),
    );
    assert_eq!(status, 400, "{refused}");
}
