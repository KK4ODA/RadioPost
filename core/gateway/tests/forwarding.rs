// SPDX-License-Identifier: MIT OR Apache-2.0
//! Forwarding by radio end to end (RP-7 §2, §3; wishlist W30), with the gateway program on both
//! sides and the post office over HTTP.
//!
//! 1. KK4ODA-1 and KK4ODA-2 are gateways of one post office, each naming the other a partner.
//! 2. KK4ODA-1 loses the Internet: its post office's address no longer answers.
//! 3. WD5EMA calls KK4ODA-1, which checks them with the keys it holds and queues their message
//!    in a session record (RP-7 §3.1).
//! 4. KK4ODA-1's operator forwards the queue to KK4ODA-2 by radio: one `PROOF`, one message.
//! 5. KK4ODA-2, online, checks the record, keeps it with its own callsign on the path, and
//!    reports it (RP-5 §5.6): the post office takes the message. KK4ODA-1 lets the record go and
//!    remembers the message as passed on (RP-7 §3.5).

mod common;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use radiopost_client::session::{self, Kind, Options};
use radiopost_gateway::monitor::Monitor;
use radiopost_gateway::store::Store as Held;
use radiopost_gateway::{operator, station};
use radiopost_postoffice::serve::Running;
use rpmi::channel::{Channel, StationConfig};
use rpmi::tcp::Server;
use rpmi::wire::Caps;
use serde_json::json;

use common::{client, draft, key, post_office, scratch};

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

/// A gateway registered by its operator, naming `partner`: its directory.
fn gateway(
    dir: &Path,
    callsign: &str,
    partner: &str,
    modem: SocketAddr,
    base: &str,
    running: &Running,
    po_key: &ed25519_dalek::SigningKey,
) -> PathBuf {
    gateway_holding(dir, callsign, partner, modem, base, running, po_key, None)
}

/// The same, holding copies placed ahead when `copies` says so (RP-5 §5.8).
#[allow(clippy::too_many_arguments, reason = "a gateway's settings")]
fn gateway_holding(
    dir: &Path,
    callsign: &str,
    partner: &str,
    modem: SocketAddr,
    base: &str,
    running: &Running,
    po_key: &ed25519_dalek::SigningKey,
    copies: Option<station::CopiesSettings>,
) -> PathBuf {
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
        copies,
        partners: vec![station::PartnerSettings {
            callsign: partner.into(),
        }],
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
    gw
}

/// A gateway answering on a thread of its own: its monitor, the thread, and what it says.
fn answer(gw: &Path) -> (Arc<Monitor>, JoinHandle<()>, Receiver<String>) {
    let monitor = Arc::new(Monitor::new());
    let (tx, rx) = std::sync::mpsc::channel();
    let (at, m) = (gw.to_owned(), monitor.clone());
    let thread = thread::spawn(move || {
        let _ = station::run(&at, &m, &mut |line| {
            let _ = tx.send(line.to_owned());
        });
    });
    (monitor, thread, rx)
}

/// Waits until the gateway answers on its modem: a call before that goes unanswered.
fn listening(monitor: &Monitor, says: &Receiver<String>) {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut said = Vec::new();
    while monitor.status().state != "listening" {
        said.extend(says.try_iter());
        assert!(Instant::now() < deadline, "not listening: {said:#?}");
        thread::sleep(Duration::from_millis(50));
    }
}

/// Waits until `ok` holds of the gateway's store, or says what it holds and what it said.
fn until(gw: &Path, says: &Receiver<String>, what: &str, ok: impl Fn(&Held) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut said = Vec::new();
    loop {
        said.extend(says.try_iter());
        let held = Held::load(gw);
        if ok(&held) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: {}\n{said:#?}",
            serde_json::to_string_pretty(&held).unwrap_or_default()
        );
        thread::sleep(Duration::from_millis(200));
    }
}

#[test]
#[allow(clippy::too_many_lines, reason = "one story, told in order")]
fn a_cut_off_gateway_forwards_its_queue_to_a_partner() {
    let dir = scratch("forwarding");
    let (running, base, po_key) = post_office(&dir.join("po"));
    let (_air, at_user, at_one, at_two) = air();

    // 1. two gateways, each the other's partner
    let one = gateway(
        &dir, "KK4ODA-1", "KK4ODA-2", at_one, &base, &running, &po_key,
    );
    let two = gateway(
        &dir, "KK4ODA-2", "KK4ODA-1", at_two, &base, &running, &po_key,
    );
    let (m_one, t_one, says_one) = answer(&one);
    let (m_two, _t_two, says_two) = answer(&two);
    listening(&m_two, &says_two);
    until(&one, &says_one, "the post office's keys kept", |h| {
        h.keys.contains_key("test.example")
    });
    until(&two, &says_two, "the post office's keys kept", |h| {
        h.keys.contains_key("test.example")
    });

    // 2. KK4ODA-1 loses the Internet: answering again with an address that does not answer
    m_one.stop();
    t_one.join().expect("stopped");
    let mut settings = station::Settings::load(&one).expect("settings");
    settings
        .post_offices
        .insert("test.example".into(), "http://127.0.0.1:1".into());
    settings.save(&one).expect("saved");
    let (m_one, _t_one, says_one) = answer(&one);
    listening(&m_one, &says_one);

    // 3. WD5EMA calls it: checked offline, their message queued
    let mut user = client(
        &dir.join("wd5ema"),
        &running,
        &po_key,
        "club@test.example",
        "WD5EMA",
    );
    user.post_office = Some(base.clone());
    user.save().expect("saved");
    let note = draft(
        "club@test.example",
        "maria@example.com",
        "Shelter supplies",
        "We need water and blankets for 40 people.\n",
    );
    let note_id = user.queue(&note).expect("queued");
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
    assert_eq!(
        report.queued,
        [note_id],
        "queued at the cut-off gateway: {said:?}"
    );
    until(&one, &says_one, "the session record kept", |h| {
        h.records.len() == 1 && h.records[0].done
    });
    assert!(!running.office().state.had.contains(&note_id.text()));

    // 4. its operator forwards the queue to KK4ODA-2
    m_one.request_forward("KK4ODA-2");
    until(&one, &says_one, "the record let go", |h| {
        h.records.is_empty() && h.passed_on.contains_key(&note_id.text())
    });
    let held = Held::load(&one);
    let partner = &held.partners["KK4ODA-2"];
    assert_eq!(partner.online, Some(true), "KK4ODA-2 said it is online");
    assert!(
        partner
            .said
            .starts_with("forwarded to KK4ODA-2: 1 record, 1 message passed on"),
        "{}",
        partner.said
    );
    let wd5ema = radiopost::cbor::hex(&radiopost::credential::hash(
        &user.credential().expect("a credential"),
    ));
    assert!(
        partner.holds.contains(&wd5ema),
        "WD5EMA's credential is held there now, so the next record goes by its hash"
    );

    // 5. KK4ODA-2 reports it with the path, and the post office takes the message
    let deadline = Instant::now() + Duration::from_secs(60);
    while !running.office().state.had.contains(&note_id.text()) {
        assert!(
            Instant::now() < deadline,
            "not reported: {:#?}",
            says_two.try_iter().collect::<Vec<_>>()
        );
        thread::sleep(Duration::from_millis(200));
    }
    until(&two, &says_two, "the record reported", |h| {
        h.records.is_empty() && h.passed_on.contains_key(&note_id.text())
    });
    let outgoing: Vec<String> = running
        .office()
        .state
        .outgoing
        .iter()
        .map(|o| o.id.clone())
        .collect();
    assert_eq!(outgoing, [note_id.text()], "it goes on to Maria");
    let reported = Held::load(&two).reported;
    assert_eq!(
        (reported.len(), reported[0].taken),
        (1, 1),
        "one session reported, its message taken"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Answers a gateway again with its post office's address as `url`: one that does not answer cuts
/// it off from the Internet, and its own puts it back.
fn reconnect(
    gw: &Path,
    url: &str,
    monitor: &Monitor,
    thread: JoinHandle<()>,
) -> (Arc<Monitor>, JoinHandle<()>, Receiver<String>) {
    monitor.stop();
    thread.join().expect("stopped");
    let mut settings = station::Settings::load(gw).expect("settings");
    settings
        .post_offices
        .insert("test.example".into(), url.into());
    settings.save(gw).expect("saved");
    let (m, t, says) = answer(gw);
    listening(&m, &says);
    (m, t, says)
}

/// RP-7 §4 end to end (W30 stage 4): mail for a user of a cut-off gateway reaches them by radio.
///
/// 1. The post office's route says KK4ODA-2 holds KK4ODA-1's users' mail while it is cut off.
/// 2. KK4ODA-1 polls, then loses the Internet; after 15 minutes the post office counts it cut off.
/// 3. Mail arrives for WD5EMA, who names KK4ODA-1 as usual: a forward item, with the post office's
///    signed placement, goes to KK4ODA-2, which polls and holds it for KK4ODA-1.
/// 4. KK4ODA-1 calls its partner: the item comes over with its placement, checked, and is held as
///    a copy placed ahead. WD5EMA calls KK4ODA-1 and takes the mail, the Internet still down.
/// 5. KK4ODA-1's Internet comes back: its poll ends the forwarding, and the post office drops the
///    item at KK4ODA-2.
#[test]
#[allow(clippy::too_many_lines, reason = "one story, told in order")]
fn mail_for_a_cut_off_area_reaches_it_by_radio() {
    let dir = scratch("inward");
    let (running, base, po_key) = post_office(&dir.join("po"));
    let (_air, at_user, at_one, at_two) = air();
    let copies = Some(station::CopiesSettings {
        users: 10,
        days: 7,
        max_air_bytes: 16_384,
    });
    let one = gateway_holding(
        &dir, "KK4ODA-1", "KK4ODA-2", at_one, &base, &running, &po_key, copies,
    );
    let two = gateway_holding(
        &dir, "KK4ODA-2", "KK4ODA-1", at_two, &base, &running, &po_key, copies,
    );

    // 1. the route, set on the post office's operator port
    let (status, routes) = running.admin(
        "POST",
        "/routes",
        json!({"via": "kk4oda-2", "users_at": ["KK4ODA-1"]})
            .to_string()
            .as_bytes(),
    );
    assert_eq!(status, 200, "{routes}");
    assert!(routes.contains(r#""via":"KK4ODA-2""#), "{routes}");
    let mut user = client(
        &dir.join("wd5ema"),
        &running,
        &po_key,
        "club@test.example",
        "WD5EMA",
    );
    user.post_office = Some(base.clone());
    user.save().expect("saved");
    radiopost_client::online::settings(&user, Some(&json!({"usual_gateways": ["KK4ODA-1"]})))
        .expect("settings saved at the post office");

    // 2. KK4ODA-2 answers and polls. KK4ODA-1 has lost the Internet: it answers with its post
    //    office's address unreachable and the keys it kept from before (as KK4ODA-2 has them),
    //    and the post office last heard its poll 16 minutes ago
    let (m_two, _t_two, says_two) = answer(&two);
    listening(&m_two, &says_two);
    until(&two, &says_two, "the post office's keys kept", |h| {
        h.keys.contains_key("test.example")
    });
    let keys = Held::load(&two).keys;
    radiopost_gateway::store::Shared::open(&one)
        .change(|s| s.keys = keys)
        .expect("written");
    let mut settings = station::Settings::load(&one).expect("settings");
    settings
        .post_offices
        .insert("test.example".into(), "http://127.0.0.1:1".into());
    settings.save(&one).expect("saved");
    let (m_one, t_one, says_one) = answer(&one);
    listening(&m_one, &says_one);
    let deadline = Instant::now() + Duration::from_secs(60);
    while !running.office().state.last_poll.contains_key("KK4ODA-2") {
        assert!(Instant::now() < deadline, "KK4ODA-2 polls");
        thread::sleep(Duration::from_millis(200));
    }
    running
        .office()
        .state
        .last_poll
        .insert("KK4ODA-1".into(), station::now() - 16 * 60);

    // 3. mail for WD5EMA: a forward item at KK4ODA-2, held for KK4ODA-1
    running
        .email_in(
            "maria@example.com",
            &["club@test.example".to_owned()],
            b"From: maria@example.com\r\nTo: club@test.example\r\nSubject: Water\r\n\
              Date: Sun, 15 Nov 2026 10:00:00 +0000\r\nMessage-ID: <water.1@example.com>\r\n\r\n\
              The truck comes at noon.\r\n",
        )
        .expect("taken");
    assert!(
        running.office().state.cut.contains("KK4ODA-1"),
        "counted cut off"
    );
    until(&two, &says_two, "the forward item held for KK4ODA-1", |h| {
        h.forwards_for("KK4ODA-1", 0).len() == 1
    });

    // 4. KK4ODA-1 calls its partner and holds the mail; WD5EMA takes it there
    m_one.request_forward("KK4ODA-2");
    until(&one, &says_one, "the mail held for WD5EMA", |h| {
        h.copies
            .iter()
            .any(|c| c.address == "club@test.example" && c.subject == "Water" && c.air.is_some())
    });
    until(&two, &says_two, "the forward item gone once taken", |h| {
        h.forwards_for("KK4ODA-1", 0).is_empty()
    });
    assert_eq!(Held::load(&one).partners["KK4ODA-2"].cursor, 1);
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
    assert_eq!(report.received.len(), 1, "the mail came by radio: {said:?}");

    // 5. KK4ODA-1 is back: its poll ends the forwarding, the item dropped at KK4ODA-2
    let _back = reconnect(&one, &base, &m_one, t_one);
    let deadline = Instant::now() + Duration::from_secs(60);
    while running.office().state.cut.contains("KK4ODA-1") {
        assert!(Instant::now() < deadline, "KK4ODA-1 polls again");
        thread::sleep(Duration::from_millis(200));
    }
    let office = running.office();
    assert!(office.state.forwarded.is_empty());
    let last = office.state.copies["KK4ODA-2"]
        .last()
        .cloned()
        .expect("an item");
    assert_eq!(last["kind"], "drop", "{last}");
    drop(office);
    let _ = std::fs::remove_dir_all(dir);
}
