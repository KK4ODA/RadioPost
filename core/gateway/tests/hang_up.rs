// SPDX-License-Identifier: MIT OR Apache-2.0
//! The gateway's operator ends the session under way and the gateway goes on answering: the
//! session card's **Disconnect**, **Abort** and, for a call the gateway places to a partner,
//! **Stop calling** ([`Monitor::end_session`]). Before 2026-11-23 the only way was stopping the
//! whole gateway.

mod common;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use radiopost_gateway::monitor::{Monitor, Stop};
use radiopost_gateway::{operator, station};
use radiopost_postoffice::serve::Running;
use rpmi::host::{Event, Host};
use rpmi::tcp::Connection;
use serde_json::json;

use common::{client, key, loopback, post_office, scratch};

/// KK4ODA-1, registered by its operator, on the modem at `modem`, naming KK4ODA-2 its partner:
/// its directory.
fn gateway(
    dir: &Path,
    modem: SocketAddr,
    base: &str,
    running: &Running,
    po_key: &ed25519_dalek::SigningKey,
) -> PathBuf {
    let home = dir.join("operator");
    let _operator = client(&home, running, po_key, "facundo@test.example", "KK4ODA");
    let gw = dir.join("KK4ODA-1");
    std::fs::create_dir_all(&gw).expect("a directory");
    station::write(&gw, "key.hex", &radiopost::cbor::hex(&key().to_bytes())).expect("written");
    station::Settings {
        callsign: "KK4ODA-1".into(),
        modem: modem.to_string(),
        more_modems: Vec::new(),
        country: "US".into(),
        post_offices: BTreeMap::from([("test.example".into(), base.to_owned())]),
        resolver: String::new(),
        blocked: Vec::new(),
        only: None,
        operator_checks_rules: true,
        answer_at_start: true,
        report_status: false,
        copies: None,
        partners: vec![station::PartnerSettings {
            callsign: "KK4ODA-2".into(),
        }],
        windows: Vec::new(),
        scan: Vec::new(),
    }
    .save(&gw)
    .expect("saved");
    operator::register(
        &gw,
        &home,
        json!({"channels": [{"dial_hz": 7_101_000, "mode": "USB", "modem": "tcp",
                             "bandwidth_hz": 2300}],
               "hours": "always", "internet": true}),
    )
    .expect("registered");
    gw
}

/// The gateway answering on a thread of its own: its monitor, the thread, and what it says.
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

/// Waits until the gateway's state is `state`, or says what it said.
fn wait_for(monitor: &Monitor, says: &Receiver<String>, state: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut said = Vec::new();
    while monitor.status().state != state {
        said.extend(says.try_iter());
        assert!(Instant::now() < deadline, "never {state}: {said:#?}");
        thread::sleep(Duration::from_millis(20));
    }
}

/// WD5EMA's station by hand, speaking RP-4: it calls KK4ODA-1 and then says nothing, so the
/// session stands still until someone ends it.
fn silent_caller(at_user: SocketAddr) -> (Connection, Host) {
    let mut conn = Connection::open(at_user).expect("the adapter");
    conn.set_timeout(Some(Duration::from_secs(10)))
        .expect("a wait");
    let mut host = Host::new();
    for request in [host.hello("a test"), host.callsigns(&["WD5EMA"])] {
        conn.send(&request).expect("sent");
    }
    (conn, host)
}

/// Calls KK4ODA-1 on `conn`.
fn call(conn: &mut Connection, host: &mut Host) {
    conn.send(&host.connect("KK4ODA-1", None, None))
        .expect("sent");
}

/// Reads `conn` until a `disconnected`, and gives its reason.
fn disconnected(conn: &mut Connection) -> String {
    loop {
        let line = conn.read_message().expect("read").expect("a line");
        if let Ok(Event::Disconnected { reason, .. }) = Event::read(&line) {
            return reason;
        }
    }
}

/// Disconnect, then Abort on the next call: each ends its session only, the caller hears the link
/// end, the record says it was the operator, and the gateway answers the next call; the word for
/// one session is not heard by the next.
#[test]
fn the_operator_ends_a_session_and_the_gateway_answers_on() {
    let dir = scratch("gateway-hang-up");
    let (running, base, po_key) = post_office(&dir);
    let (_server, at_user, at_gateway) = loopback();
    let gw = gateway(&dir, at_gateway, &base, &running, &po_key);
    let (monitor, thread, says) = answer(&gw);
    wait_for(&monitor, &says, "listening");
    assert_eq!(
        monitor.end_session(Stop::Disconnect),
        Err("no session is under way".into())
    );

    let (mut caller, mut host) = silent_caller(at_user);
    call(&mut caller, &mut host);
    wait_for(&monitor, &says, "session");
    monitor
        .end_session(Stop::Disconnect)
        .expect("a session to end");
    assert_eq!(monitor.status().stopping, Some("disconnect"));
    assert_eq!(
        disconnected(&mut caller),
        "remote",
        "the caller hears it end"
    );
    wait_for(&monitor, &says, "listening");
    let last = &monitor.sessions()[0];
    assert_eq!(last.remote, "WD5EMA");
    assert_eq!(last.result.as_deref(), Some("disconnected"), "{last:?}");
    assert_eq!(monitor.status().stopping, None);

    // the next call: the word was the last session's, so this one stands until it is said
    call(&mut caller, &mut host);
    wait_for(&monitor, &says, "session");
    thread::sleep(Duration::from_millis(600));
    assert_eq!(
        monitor.status().state,
        "session",
        "not ended by the last word"
    );
    monitor.end_session(Stop::Abort).expect("a session to end");
    assert_eq!(disconnected(&mut caller), "remote");
    wait_for(&monitor, &says, "listening");
    assert_eq!(monitor.sessions()[0].result.as_deref(), Some("aborted"));
    let (logged, _) = monitor.log_since(0);
    let said: Vec<&str> = logged.iter().map(|(_, l)| l.as_str()).collect();
    assert!(
        said.iter()
            .any(|l| l.starts_with("disconnecting from WD5EMA, as the operator asked")),
        "{said:#?}"
    );
    assert!(
        said.contains(&"aborting the session with WD5EMA, as the operator asked"),
        "{said:#?}"
    );
    caller.close();
    monitor.stop();
    thread.join().expect("the answering loop");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Stop calling: a call the gateway places to its partner, nobody answering, ends at once, and the
/// gateway goes back to answering.
#[test]
fn a_call_to_a_partner_is_stopped() {
    let dir = scratch("gateway-hang-up-call");
    let (running, base, po_key) = post_office(&dir);
    // nobody is KK4ODA-2: the modem would give up after ten seconds
    let (_server, _, at_gateway) = loopback();
    let gw = gateway(&dir, at_gateway, &base, &running, &po_key);
    let (monitor, thread, says) = answer(&gw);
    wait_for(&monitor, &says, "listening");
    monitor.request_forward("KK4ODA-2");
    wait_for(&monitor, &says, "session");
    assert!(
        monitor.status().session.is_some_and(|s| s.unlinked),
        "calling, no link yet"
    );
    let asked = Instant::now();
    monitor.end_session(Stop::Abort).expect("a call to stop");
    wait_for(&monitor, &says, "listening");
    assert!(asked.elapsed() < Duration::from_secs(3), "{asked:?}");
    assert_eq!(
        monitor.sessions()[0].result.as_deref(),
        Some("call-stopped")
    );
    monitor.stop();
    thread.join().expect("the answering loop");
    let _ = std::fs::remove_dir_all(&dir);
}
