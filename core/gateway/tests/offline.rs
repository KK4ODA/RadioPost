// SPDX-License-Identifier: MIT OR Apache-2.0
//! Phase 3 end to end (wishlist W27, W28, W29): copies placed ahead, a gateway cut off from the
//! Internet, and the sender's client following its queued mail.
//!
//! 1. Mail for WD5EMA is copied ahead to both of their usual gateways, KK4ODA-1 and KK4ODA-2,
//!    which poll the post office (RP-5 §5.8).
//! 2. The Internet is cut.
//! 3. WD5EMA calls KK4ODA-1: it checks them with the keys it holds (RP-1 §5, §6.3), serves the
//!    copy, and takes their message into its queue (RP-7 §3.1). The client shows it queued there
//!    (RP-7 §3.6).
//! 4. The Internet comes back, and KK4ODA-1 reports the session (RP-5 §5.6).
//! 5. The post office drops the copy at KK4ODA-2 too, and when WD5EMA calls KK4ODA-2, online, it
//!    offers the message again, the post office already has it, and the client shows it sent.

mod common;

use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::{Duration, Instant};

use radiopost_client::session::{self, Kind, Options};
use radiopost_client::store::Store;
use radiopost_gateway::store::Store as Held;
use radiopost_gateway::{operator, station};
use radiopost_postoffice::serve::Running;
use rpmi::channel::{Channel, StationConfig};
use rpmi::tcp::Server;
use rpmi::wire::Caps;
use serde_json::json;

use common::{Internet, client, draft, key, post_office_at, scratch};

/// The loopback adapter with the user's station and two gateways'.
fn air() -> (Server, SocketAddr, SocketAddr, SocketAddr) {
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
    let one = ch.add_station(station("KK4ODA-1", "g"));
    let two = ch.add_station(station("KK4ODA-2", "h"));
    let server = Server::new(ch, Duration::from_secs(10));
    let at_user = server.listen(user, "127.0.0.1:0").expect("a port");
    let at_one = server.listen(one, "127.0.0.1:0").expect("a port");
    let at_two = server.listen(two, "127.0.0.1:0").expect("a port");
    (server, at_user, at_one, at_two)
}

/// A gateway holding copies, registered by its operator and answering: its directory and what it
/// says.
fn gateway(
    dir: &Path,
    callsign: &str,
    modem: SocketAddr,
    base: &str,
    running: &Running,
    po_key: &ed25519_dalek::SigningKey,
) -> (PathBuf, Receiver<String>) {
    let home = dir.join(format!("{callsign}-operator"));
    let _operator = client(&home, running, po_key, "facundo@test.example", "KK4ODA");
    let gw = dir.join(callsign);
    std::fs::create_dir_all(&gw).expect("a directory");
    station::write(&gw, "key.hex", &radiopost::cbor::hex(&key().to_bytes())).expect("written");
    station::Settings {
        callsign: callsign.into(),
        modem: modem.to_string(),
        country: "US".into(),
        post_offices: BTreeMap::from([("test.example".into(), base.to_owned())]),
        resolver: String::new(),
        blocked: Vec::new(),
        only: None,
        operator_checks_rules: true,
        answer_at_start: true,
        copies: Some(station::CopiesSettings {
            users: 10,
            days: 7,
            max_air_bytes: 16_384,
        }),
    }
    .save(&gw)
    .expect("saved");
    operator::register(
        &gw,
        &home,
        json!({"channels": [{"dial_hz": 7_101_000, "mode": "USB", "modem": "tcp", "bandwidth_hz": 2300}],
               "hours": "always", "internet": true}),
    )
    .expect("registered");
    let (tx, rx) = std::sync::mpsc::channel();
    let at = gw.clone();
    thread::spawn(move || {
        let _ = station::serve(&at, &mut |line| {
            let _ = tx.send(line.to_owned());
        });
    });
    (gw, rx)
}

/// Waits until `ok` holds of the gateway's store, or says what it holds.
fn until(gw: &Path, what: &str, ok: impl Fn(&Held) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let held = Held::load(gw);
        if ok(&held) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: {}",
            serde_json::to_string_pretty(&held).unwrap_or_default()
        );
        thread::sleep(Duration::from_millis(200));
    }
}

fn lines(rx: &Receiver<String>) -> Vec<String> {
    rx.try_iter().collect()
}

#[test]
#[allow(clippy::too_many_lines, reason = "one story, told in order")]
fn copies_offline_sessions_reports_and_queued_mail() {
    let dir = scratch("offline");
    // the post office listens on one port and is reached through a link that can be cut
    let port = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("a free port")
        .port();
    let internet = Internet::new(SocketAddr::from(([127, 0, 0, 1], port)));
    let base = format!("http://{}", internet.at);
    let (running, _, po_key) = post_office_at(&dir.join("po"), port, &base);
    let (_air, at_user, at_one, at_two) = air();
    let (one, one_says) = gateway(&dir, "KK4ODA-1", at_one, &base, &running, &po_key);
    let (two, _two_says) = gateway(&dir, "KK4ODA-2", at_two, &base, &running, &po_key);
    for gw in [&one, &two] {
        until(gw, "the post office's keys kept", |h| {
            h.keys.contains_key("test.example")
        });
    }
    assert_eq!(
        running.office().state.gateways["KK4ODA-1"]["copies"],
        json!({"users": 10, "days": 7, "max_air_bytes": 16384}),
        "the record says it holds copies (RP-5 §4.4)"
    );

    // 1. WD5EMA names both as usual, and mail arrives for them: copied ahead to both
    let mut user = client(
        &dir.join("wd5ema"),
        &running,
        &po_key,
        "club@test.example",
        "WD5EMA",
    );
    user.post_office = Some(base.clone());
    user.save().expect("saved");
    radiopost_client::online::settings(
        &user,
        Some(&json!({"usual_gateways": ["KK4ODA-1", "KK4ODA-2"]})),
    )
    .expect("settings saved at the post office");
    running
        .email_in(
            "maria@example.com",
            &["club@test.example".to_owned()],
            b"From: maria@example.com\r\nTo: club@test.example\r\nSubject: Net tonight?\r\n\
              Date: Fri, 13 Nov 2026 10:00:00 +0000\r\nMessage-ID: <net.1@example.com>\r\n\r\n\
              Is the net on tonight?\r\n",
        )
        .expect("taken");
    for gw in [&one, &two] {
        until(gw, "the copy held", |h| {
            h.copies
                .iter()
                .any(|c| c.subject == "Net tonight?" && c.air.is_some())
        });
    }

    // 2. the Internet goes down
    internet.cut();

    // 3. WD5EMA calls KK4ODA-1: checked offline, the copy comes down, the reply is queued
    let reply = draft(
        "club@test.example",
        "maria@example.com",
        "Re: Net tonight?",
        "Yes, at 8 pm on 3.590.\n",
    );
    let reply_id = user.queue(&reply).expect("queued");
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
    assert_eq!(report.received.len(), 1, "the copy came down: {said:?}");
    assert_eq!(report.queued, [reply_id], "queued, not sent: {said:?}");
    assert!(
        said.iter()
            .any(|l| l.starts_with("notice: This gateway cannot reach your post office")),
        "{said:?}"
    );
    assert_eq!(user.state_of(&reply_id, station::now()), "queued");
    assert_eq!(
        user.queued_for(&reply_id).map(|q| q.gateway.as_str()),
        Some("KK4ODA-1")
    );
    until(&one, "the session record kept", |h| {
        h.records.len() == 1 && h.records[0].done && h.records[0].pickups.len() == 1
    });
    assert!(
        !running.office().state.had.contains(&reply_id.text()),
        "the post office has not seen it"
    );
    // offered again only to a gateway that was not cut off when last called
    assert_eq!(user.outbox_for("KK4ODA-1", station::now()).len(), 0);
    assert_eq!(user.outbox_for("KK4ODA-2", station::now()).len(), 1);

    // 4. the Internet comes back: KK4ODA-1 reports the session, oldest first
    internet.restore();
    until(&one, "the record reported", |h| {
        h.records.is_empty() && h.passed_on.contains_key(&reply_id.text())
    });
    assert!(running.office().state.had.contains(&reply_id.text()));
    assert!(
        running.office().state.mailboxes["club@test.example"]
            .iter()
            .all(|w| w.picked),
        "the pickup reported"
    );
    let outgoing: Vec<String> = running
        .office()
        .state
        .outgoing
        .iter()
        .map(|o| o.id.clone())
        .collect();
    assert_eq!(outgoing, [reply_id.text()], "the reply goes on to Maria");

    // 5. the copy is dropped at both gateways
    for gw in [&one, &two] {
        until(gw, "the copy dropped", |h| h.copies.is_empty());
    }
    let said_one = lines(&one_says);
    assert!(
        said_one.iter().any(|l| l.starts_with("reported 1 session")),
        "{said_one:?}"
    );

    // WD5EMA calls KK4ODA-2, online: offered again, the post office has it, so it is sent
    let mut said = Vec::new();
    let report = session::call(
        &mut user,
        "KK4ODA-2",
        Kind::Gateway,
        &Options::new(&at_user.to_string()),
        &mut |s| said.push(s.to_owned()),
    )
    .expect("called");
    assert_eq!(report.failed, None, "{said:?}");
    assert_eq!(report.delivered, [reply_id], "{said:?}");
    let user = Store::open(&dir.join("wd5ema")).expect("the home");
    assert_eq!(user.state_of(&reply_id, station::now()), "sent");
    assert!(user.outbox.is_empty() && user.queued.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}
