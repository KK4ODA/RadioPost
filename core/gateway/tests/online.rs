// SPDX-License-Identifier: MIT OR Apache-2.0
//! The gateway online, end to end: a user's client calls the gateway program through the loopback
//! adapter, and the gateway carries the session to the post office service over HTTP — the
//! client's message goes on, the mail waiting for it comes down, and the post office marks it
//! picked up. The gateway was registered by its operator's own client home (RP-5 §6.4).

use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use radiopost::credential::{PostOfficeKey, flags};
use radiopost::message::Compression;
use radiopost_client::compose::Draft;
use radiopost_client::session::{self, Kind, Options};
use radiopost_client::store::Store;
use radiopost_gateway::{operator, station};
use radiopost_postoffice::documents::Info;
use radiopost_postoffice::serve::{self, Files, Running, Settings};
use rpmi::channel::{Channel, StationConfig};
use rpmi::tcp::Server;
use rpmi::wire::Caps;
use serde_json::json;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("radiopost-gw-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    dir
}

fn key() -> SigningKey {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).expect("random numbers");
    SigningKey::from_bytes(&seed)
}

/// The post office for `test.example`, serving on a port of its own, at a base URL that is that
/// port (what the gateway signs and the post office checks).
fn post_office(dir: &Path) -> (Arc<Running>, String, SigningKey) {
    let port = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("a free port")
        .port();
    let base = format!("http://127.0.0.1:{port}");
    std::fs::create_dir_all(dir).expect("a directory");
    let files = Files::new(dir);
    let key = key();
    files
        .write("key.hex", &radiopost::cbor::hex(&key.to_bytes()))
        .expect("written");
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
    };
    files
        .write(
            "config.json",
            &serde_json::to_string(&settings).expect("JSON"),
        )
        .expect("written");
    let running = Arc::new(Running::load(dir, serve::system_draw()).expect("loaded"));
    serve::serve(running.clone(), &settings.listen, "127.0.0.1:0").expect("serving");
    (running, base, key)
}

/// A client's home with a fresh device key, a credential the post office issued for it, and
/// that post office's key trusted.
fn client(home: &Path, running: &Running, po_key: &SigningKey, address: &str, call: &str) -> Store {
    let store = Store::open(home).expect("a home");
    let device = key();
    store.set_device(&device).expect("kept");
    let credential = {
        let mut office = running.office();
        office.at(serve::now());
        office.issue(
            address,
            call,
            device.verifying_key().to_bytes(),
            flags::SEND | flags::RECEIVE,
        )
    };
    store
        .set_credential(&credential)
        .expect("a credential for this key");
    store
        .add_trust(
            "test.example",
            &PostOfficeKey {
                public: po_key.verifying_key().to_bytes(),
                not_before: 0,
                not_after: u64::MAX,
            },
        )
        .expect("trusted");
    store
}

/// The loopback adapter (RP-4 §9.4) with the user's station and the gateway's.
fn loopback() -> (Server, SocketAddr, SocketAddr) {
    let caps = Caps {
        bandwidths: vec![2300],
        max_links: 1,
        delivery: true,
        broadcast: true,
        quality: false,
        busy: true,
        regulatory: false,
        max_queue: 65_536,
        shared: false,
    };
    let station = |name: &str, prefix: &str| StationConfig {
        name: name.into(),
        modem: "the loopback adapter".into(),
        caps: caps.clone(),
        prefix: prefix.into(),
        frame_bytes: 128,
    };
    let mut ch = Channel::new();
    let user = ch.add_station(station("WD5EMA", "u"));
    let gateway = ch.add_station(station("KK4ODA-1", "g"));
    let server = Server::new(ch, Duration::from_secs(10));
    let at_user = server.listen(user, "127.0.0.1:0").expect("a port");
    let at_gateway = server.listen(gateway, "127.0.0.1:0").expect("a port");
    (server, at_user, at_gateway)
}

#[test]
#[allow(clippy::too_many_lines, reason = "one story, told in order")]
fn a_user_calls_the_gateway_and_mail_moves_both_ways() {
    let dir = scratch("online");
    let (running, base, po_key) = post_office(&dir.join("po"));
    let (_server, at_user, at_gateway) = loopback();

    // the gateway's operator, KK4ODA, registers it from their own client's home
    let _operator = client(
        &dir.join("kk4oda"),
        &running,
        &po_key,
        "facundo@test.example",
        "KK4ODA",
    );
    let gw = dir.join("gateway");
    std::fs::create_dir_all(&gw).expect("a directory");
    station::write(&gw, "key.hex", &radiopost::cbor::hex(&key().to_bytes())).expect("written");
    station::Settings {
        callsign: "KK4ODA-1".into(),
        modem: at_gateway.to_string(),
        country: "US".into(),
        post_offices: BTreeMap::from([("test.example".into(), base.clone())]),
        resolver: String::new(),
        blocked: Vec::new(),
        only: None,
        operator_checks_rules: true,
    }
    .save(&gw)
    .expect("saved");
    let said = operator::register(
        &gw,
        &dir.join("kk4oda"),
        json!({"channels": [{"dial_hz": 7_101_000, "mode": "USB", "modem": "tcp", "bandwidth_hz": 2300}],
               "hours": "always", "internet": true}),
    )
    .expect("registered");
    assert!(said.starts_with("KK4ODA-1 registered"), "{said}");
    assert!(running.office().state.gateways.contains_key("KK4ODA-1"));

    // the user, WD5EMA: one message out, and one waiting at the post office
    let mut user = client(
        &dir.join("wd5ema"),
        &running,
        &po_key,
        "club@test.example",
        "WD5EMA",
    );
    let out = Draft {
        from: "club@test.example".into(),
        to: vec!["maria@example.com".into()],
        cc: Vec::new(),
        date: serve::now(),
        subject: "Net tonight".into(),
        body: "Check-ins start at 8 pm on 3.590.\n".into(),
        files: Vec::new(),
        priority: 0,
        reply_to: None,
    }
    .message()
    .expect("a message");
    let out_id = user.queue(&out).expect("queued");
    let waiting = Draft {
        from: "maria@example.com".into(),
        to: vec!["club@test.example".into()],
        subject: "Re: Net tonight".into(),
        body: "Count me in.\n".into(),
        ..Draft {
            from: String::new(),
            to: Vec::new(),
            cc: Vec::new(),
            date: serve::now(),
            subject: String::new(),
            body: String::new(),
            files: Vec::new(),
            priority: 0,
            reply_to: None,
        }
    }
    .message()
    .expect("a message");
    {
        let mut office = running.office();
        let serial = office.next_mailbox_serial("club@test.example");
        office.deliver(
            "club@test.example",
            serial,
            &waiting.air_form(Compression::Deflate),
            "maria@example.com",
            "Re: Net tonight",
            0,
            false,
        );
    }

    // the gateway answers calls
    let (tx, rx) = std::sync::mpsc::channel();
    let gw_dir = gw.clone();
    thread::spawn(move || {
        let _ = station::serve(&gw_dir, &mut |line| {
            let _ = tx.send(line.to_owned());
        });
    });
    let first = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the gateway says something");
    assert!(first.starts_with("listening as KK4ODA-1"), "{first}");

    let mut said = Vec::new();
    let report = session::call(
        &mut user,
        "KK4ODA-1",
        Kind::Gateway,
        &Options::new(&at_user.to_string()),
        &mut |s| said.push(s.to_owned()),
    )
    .expect("called");
    assert_eq!(report.failed, None, "{said:?}");
    assert_eq!(report.delivered, [out_id], "{said:?}");
    assert_eq!(report.received, [waiting.content_id()], "{said:?}");
    assert_eq!(user.sent, [out_id]);
    assert_eq!(user.outbox.iter().map(|w| w.id).collect::<Vec<_>>(), []);

    // the gateway's own account of it
    let mut lines = Vec::new();
    while let Ok(line) = rx.recv_timeout(Duration::from_secs(10)) {
        let over = line.contains("is over") || line.contains("ended");
        lines.push(line);
        if over {
            break;
        }
    }
    assert!(
        lines.iter().any(|l| l == "the session with WD5EMA is over"),
        "{lines:?}"
    );

    // the post office: the message waits to go on by email, and the one picked up is marked
    let office = running.office();
    let outgoing: Vec<&str> = office
        .state
        .outgoing
        .iter()
        .map(|o| o.id.as_str())
        .collect();
    assert_eq!(outgoing, [out_id.text().as_str()]);
    let mailbox = &office.state.mailboxes["club@test.example"];
    assert!(mailbox.iter().all(|w| w.picked), "{mailbox:?}");
    // and the gateway kept the caller's credential, to be named by hash next time (RP-3 §5.1)
    let held = std::fs::read_to_string(gw.join("held.json")).expect("held");
    assert!(held.contains("credentials"), "{held}");
}
