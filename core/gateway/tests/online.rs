// SPDX-License-Identifier: MIT OR Apache-2.0
//! The gateway online, end to end: a user's client calls the gateway program through the loopback
//! adapter, and the gateway carries the session to the post office service over HTTP — the
//! client's message goes on, the mail waiting for it comes down, and the post office marks it
//! picked up. The gateway was registered by its operator's own client home (RP-5 §6.4).

mod common;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use radiopost::message::Compression;
use radiopost_client::session::{self, Kind, Options};
use radiopost_client::store::Store;
use radiopost_gateway::{operator, station};
use radiopost_postoffice::serve::{self, Running};
use serde_json::json;

use common::{client, draft, key, loopback, post_office, scratch};

/// A registered gateway, KK4ODA-1, answering calls, and its directory; it says the lines it
/// writes on `lines`.
fn gateway(
    dir: &Path,
    running: &Running,
    base: &str,
    po_key: &SigningKey,
    at_gateway: SocketAddr,
) -> (PathBuf, std::sync::mpsc::Receiver<String>) {
    // the gateway's operator, KK4ODA, registers it from their own client's home
    let _operator = client(
        &dir.join("kk4oda"),
        running,
        po_key,
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
        post_offices: BTreeMap::from([("test.example".into(), base.to_owned())]),
        resolver: String::new(),
        blocked: Vec::new(),
        only: None,
        operator_checks_rules: true,
        answer_at_start: true,
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
    (gw, rx)
}

/// Lines the gateway writes until a session ends.
fn session_lines(rx: &std::sync::mpsc::Receiver<String>) -> Vec<String> {
    let mut lines = Vec::new();
    while let Ok(line) = rx.recv_timeout(Duration::from_secs(10)) {
        let over = line.contains("is over") || line.contains("ended");
        lines.push(line);
        if over {
            break;
        }
    }
    lines
}

#[test]
fn a_user_calls_the_gateway_and_mail_moves_both_ways() {
    let dir = scratch("online");
    let (running, base, po_key) = post_office(&dir.join("po"));
    let (_server, at_user, at_gateway) = loopback();
    let (gw, rx) = gateway(&dir, &running, &base, &po_key, at_gateway);

    // the user, WD5EMA: one message out, and one waiting at the post office
    let mut user = client(
        &dir.join("wd5ema"),
        &running,
        &po_key,
        "club@test.example",
        "WD5EMA",
    );
    let out = draft(
        "club@test.example",
        "maria@example.com",
        "Net tonight",
        "Check-ins start at 8 pm on 3.590.\n",
    );
    let out_id = user.queue(&out).expect("queued");
    let waiting = draft(
        "maria@example.com",
        "club@test.example",
        "Re: Net tonight",
        "Count me in.\n",
    );
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
    let lines = session_lines(&rx);
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

/// RP-1 §6.3 step 5, RP-3 §5.2, RP-5 §5.2: a laptop whose clock is an hour off signs the
/// gateway's challenge instead, and the post office takes it.
#[test]
fn a_client_whose_clock_is_off_signs_the_challenge_and_gets_through() {
    let dir = scratch("skewed");
    let (running, base, po_key) = post_office(&dir.join("po"));
    let (_server, at_user, at_gateway) = loopback();
    let (_gw, rx) = gateway(&dir, &running, &base, &po_key, at_gateway);

    let mut user = client(
        &dir.join("wd5ema"),
        &running,
        &po_key,
        "club@test.example",
        "WD5EMA",
    );
    let out = draft(
        "club@test.example",
        "maria@example.com",
        "Clock",
        "My clock is wrong.\n",
    );
    let out_id = user.queue(&out).expect("queued");
    let mut opts = Options::new(&at_user.to_string());
    opts.clock = Some(serve::now() - 3600);
    let mut said = Vec::new();
    let report = session::call(&mut user, "KK4ODA-1", Kind::Gateway, &opts, &mut |s| {
        said.push(s.to_owned());
    })
    .expect("called");
    assert_eq!(report.failed, None, "{said:?}");
    assert_eq!(report.delivered, [out_id], "{said:?}");
    let lines = session_lines(&rx);
    assert!(
        lines.iter().any(|l| l == "the session with WD5EMA is over"),
        "{lines:?}"
    );
    let office = running.office();
    assert_eq!(office.state.outgoing.len(), 1);
    // the challenge is in the post office's replay cache: the same statement cannot open another
    assert!(
        office
            .state
            .replay
            .iter()
            .any(|(a, n, _)| a == "club@test.example" && n.len() == 32),
        "{:?}",
        office.state.replay
    );
}

/// An email whose two attachments together are over the post office's `max_air_bytes`: both are
/// held (RP-6 §2.4 step 5).
fn email_with_large_attachments(photo: &[u8], video: &[u8]) -> Vec<u8> {
    let b64 = |data: &[u8]| {
        let mut t = radiopost::base64url::encode(data)
            .replace('-', "+")
            .replace('_', "/");
        while t.len() % 4 != 0 {
            t.push('=');
        }
        t.as_bytes()
            .chunks(76)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect::<Vec<_>>()
            .join("\r\n")
    };
    format!(
        "From: Maria <maria@example.com>\r\n\
         To: club@test.example\r\n\
         Subject: The house\r\n\
         Message-ID: <house@example.com>\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"b\"\r\n\
         \r\n\
         --b\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         Here is the house, a photo and a walk round.\r\n\
         --b\r\n\
         Content-Type: image/jpeg\r\n\
         Content-Disposition: attachment; filename=\"house.jpg\"\r\n\
         Content-Transfer-Encoding: base64\r\n\
         \r\n\
         {}\r\n\
         --b\r\n\
         Content-Type: video/mp4\r\n\
         Content-Disposition: attachment; filename=\"house.mp4\"\r\n\
         Content-Transfer-Encoding: base64\r\n\
         \r\n\
         {}\r\n\
         --b--\r\n",
        b64(photo),
        b64(video)
    )
    .into_bytes()
}

/// RP-3 §5.6, RP-5 §5.3, RP-6 §2.5: an email's attachments too large for the radio are held; the
/// user picks the message up, asks for one attachment by name, and on the next call receives it,
/// in a new message from the post office.
#[test]
fn a_held_attachment_is_asked_for_by_radio_and_comes_on_the_next_call() {
    let dir = scratch("held");
    let (running, base, po_key) = post_office(&dir.join("po"));
    let (_server, at_user, at_gateway) = loopback();
    let (_gw, rx) = gateway(&dir, &running, &base, &po_key, at_gateway);
    let mut user = client(
        &dir.join("wd5ema"),
        &running,
        &po_key,
        "club@test.example",
        "WD5EMA",
    );
    let photo: Vec<u8> = (0..30_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let video: Vec<u8> = (0..50_000u32).map(|i| (i * 13 % 241) as u8).collect();
    running
        .email_in(
            "maria@example.com",
            &["club@test.example".into()],
            &email_with_large_attachments(&photo, &video),
        )
        .expect("taken");

    let call = |user: &mut Store| {
        let mut said = Vec::new();
        let report = session::call(
            user,
            "KK4ODA-1",
            Kind::Gateway,
            &Options::new(&at_user.to_string()),
            &mut |s| said.push(s.to_owned()),
        )
        .expect("called");
        assert_eq!(report.failed, None, "{said:?}");
        let lines = session_lines(&rx);
        assert!(
            lines.iter().any(|l| l == "the session with WD5EMA is over"),
            "{lines:?}"
        );
        (report, said)
    };

    // the first call: the message, its attachments held
    let (report, said) = call(&mut user);
    assert_eq!(report.received.len(), 1, "{said:?}");
    let original = user.message(&report.received[0]).expect("kept");
    let held = Store::held(&original);
    let names: Vec<&str> = held.iter().map(|(n, ..)| n.as_str()).collect();
    assert_eq!(names, ["house.jpg", "house.mp4"]);
    assert_eq!(held[0].2, 30_000);

    // the user asks for the photo
    let asked = user
        .ask_for_held(&report.received[0], &["house.jpg".into()])
        .expect("asked");
    assert_eq!(asked, ["house.jpg"]);
    assert_eq!(user.files.len(), 1);
    // kept in the home, with the serial it is asked by
    let reopened = Store::open(&dir.join("wd5ema")).expect("the home");
    assert_eq!(reopened.files, user.files);
    assert_eq!(reopened.inbox[0].serial, Some(user.files[0].0));
    assert_eq!(
        reopened.files_asked_for(),
        [format!(
            "house.jpg, 30000 bytes, of {}",
            &report.received[0].text()[..6]
        )]
    );
    let unknown = user.ask_for_held(&report.received[0], &["house.png".into()]);
    assert!(unknown.is_err(), "{unknown:?}");

    // the second call asks; the gateway says how the post office took it
    let (report, said) = call(&mut user);
    assert_eq!(report.received, [], "{said:?}");
    assert!(
        said.iter()
            .any(|s| s
                == "notice: Your attachments are in a new message, waiting for your next call."),
        "{said:?}"
    );
    assert_eq!(user.files, [], "{said:?}");

    // the third call brings the photo, in a message from the post office
    let (report, said) = call(&mut user);
    assert_eq!(report.received.len(), 1, "{said:?}");
    let came = user.message(&report.received[0]).expect("kept");
    assert_eq!(came.from(), Some("postmaster@test.example"));
    let map = came.map();
    assert_eq!(
        map.get(radiopost::message::key::SUBJECT)
            .and_then(radiopost::cbor::Value::as_text),
        Some("Attachments: The house")
    );
    let files = map
        .get(radiopost::message::key::FILES)
        .and_then(radiopost::cbor::Value::as_array)
        .expect("files");
    assert_eq!(files.len(), 1);
    assert_eq!(
        files[0].get(1).and_then(radiopost::cbor::Value::as_text),
        Some("house.jpg")
    );
    assert_eq!(
        files[0].get(3).and_then(radiopost::cbor::Value::as_bytes),
        Some(photo.as_slice())
    );
}

/// One HTTP exchange with a panel on loopback, as its page makes one: the status and the JSON.
fn panel_http(
    at: SocketAddr,
    method: &str,
    path: &str,
    body: &serde_json::Value,
) -> (u16, serde_json::Value) {
    use std::io::{Read as _, Write as _};
    let mut s = std::net::TcpStream::connect(at).expect("the panel");
    let body = if method == "POST" {
        body.to_string()
    } else {
        String::new()
    };
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-RadioPost: 1\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        at.port(),
        body.len()
    )
    .expect("sent");
    let mut text = String::new();
    s.read_to_string(&mut text).expect("an answer");
    let status = text[9..12].parse().expect("a status");
    let json = text
        .split_once("\r\n\r\n")
        .and_then(|(_, b)| serde_json::from_str(b).ok())
        .unwrap_or_default();
    (status, json)
}

/// Text that does not shrink much under DEFLATE: `n` hex digits of a fixed stream.
fn noise(n: usize) -> String {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            char::from_digit(u32::try_from(x % 16).expect("a digit"), 16).expect("hex")
        })
        .collect()
}

/// A call to KK4ODA-1 made from the client's panel, fetching `fetch`: the panel's state once it
/// is over, the call having succeeded.
fn call_from_panel(
    panel: SocketAddr,
    at_user: SocketAddr,
    fetch: &serde_json::Value,
) -> serde_json::Value {
    let (status, said) = panel_http(
        panel,
        "POST",
        "/api/call",
        &json!({"callsign": "KK4ODA-1", "station": false, "adapter": at_user.to_string(),
                "fetch": fetch}),
    );
    assert_eq!(status, 200, "{said}");
    let until = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let (_, state) = panel_http(panel, "GET", "/api/state", &json!(null));
        if state["activity"]["state"] == "idle" {
            assert_eq!(
                state["activity"]["failed"],
                serde_json::Value::Null,
                "{state}"
            );
            return state;
        }
        assert!(
            std::time::Instant::now() < until,
            "the call never ends: {state}"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

/// The client's panel, as a user finds a gateway and chooses what to fetch: the post office's
/// gateway list fetched and checked against its key (RP-5 §4.4); a call that fetches only the
/// list of what waits (RP-3 §6.2); one message marked; the next call fetches it alone.
#[test]
fn a_user_finds_the_gateway_and_fetches_what_they_chose() {
    use radiopost_client::panel::Panel;

    let dir = scratch("choose");
    let (running, base, po_key) = post_office(&dir.join("po"));
    let (_server, at_user, at_gateway) = loopback();
    let (_gw, rx) = gateway(&dir, &running, &base, &po_key, at_gateway);
    let mut user = client(
        &dir.join("wd5ema"),
        &running,
        &po_key,
        "club@test.example",
        "WD5EMA",
    );
    user.post_office = Some(base.clone());
    user.save().expect("saved");
    let mut serials = Vec::new();
    for subject in ["Antenna plans", "Net minutes", "Field day"] {
        let m = draft(
            "maria@example.com",
            "club@test.example",
            subject,
            &noise(3000),
        );
        assert!(
            m.air_form(Compression::Deflate).len() > 1000,
            "more than comes unasked"
        );
        let mut office = running.office();
        let serial = office.next_mailbox_serial("club@test.example");
        office.deliver(
            "club@test.example",
            serial,
            &m.air_form(Compression::Deflate),
            "maria@example.com",
            subject,
            0,
            false,
        );
        serials.push(serial);
    }
    let panel = Panel::new(&dir.join("wd5ema"), None)
        .start("127.0.0.1:0")
        .expect("a port");
    let call = |fetch: serde_json::Value| call_from_panel(panel, at_user, &fetch);

    // the gateway list, from the post office over the Internet, kept in the home
    let (status, list) = panel_http(panel, "POST", "/api/gateways", &json!({}));
    assert_eq!(status, 200, "{list}");
    assert_eq!(list["domain"], "test.example");
    assert_eq!(list["gateways"][0]["callsign"], "KK4ODA-1");
    assert_eq!(list["gateways"][0]["channels"][0]["dial_hz"], 7_101_000);
    let (status, ranked) = panel_http(panel, "POST", "/api/grid", &json!({"grid": "em73"}));
    assert_eq!(status, 200, "{ranked}");
    assert_eq!(ranked["grid"], "EM73");
    let (status, _) = panel_http(panel, "POST", "/api/grid", &json!({"grid": "nowhere"}));
    assert_eq!(status, 400, "not a grid square");

    // the first call fetches only the list of what waits
    let state = call(json!("none"));
    assert_eq!(state["counts"]["inbox"], 0, "{state}");
    assert_eq!(state["counts"]["waiting"], 3, "{state}");
    session_lines(&rx);
    let (_, waiting) = panel_http(panel, "GET", "/api/waiting", &json!(null));
    assert_eq!(waiting["via"], "KK4ODA-1");
    let listed: Vec<u64> = waiting["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|e| e["serial"].as_u64().expect("a serial"))
        .collect();
    assert_eq!(listed, serials);

    // the user marks the second; a serial that did not wait is refused
    let (status, marked) = panel_http(panel, "POST", "/api/fetch", &json!({"serial": serials[1]}));
    assert_eq!(status, 200, "{marked}");
    assert_eq!(marked["entries"][1]["fetch"], true);
    assert_eq!(marked["entries"][0]["fetch"], false);
    let (status, _) = panel_http(panel, "POST", "/api/fetch", &json!({"serial": 9999}));
    assert_eq!(status, 400);

    // the next call, still fetching only the list, brings the one marked
    let state = call(json!("none"));
    assert_eq!(state["counts"]["inbox"], 1, "{state}");
    session_lines(&rx);
    let (_, inbox) = panel_http(panel, "GET", "/api/folder?name=inbox", &json!(null));
    assert_eq!(inbox[0]["subject"], "Net minutes");
    let (_, waiting) = panel_http(panel, "GET", "/api/waiting", &json!(null));
    let left: Vec<(u64, bool)> = waiting["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|e| (e["serial"].as_u64().expect("a serial"), e["fetch"] == true))
        .collect();
    assert_eq!(left, [(serials[0], false), (serials[2], false)]);
    let home = Store::open(&dir.join("wd5ema")).expect("the home");
    assert!(
        home.fetch_next.is_empty(),
        "a mark goes once its message came: {:?}",
        home.fetch_next
    );
}

/// RP-5 §4: a home that trusts another key for its post office refuses the post office's gateway
/// list, and keeps none.
#[test]
fn a_gateway_list_signed_by_another_key_is_refused() {
    let dir = scratch("stranger");
    let (running, base, po_key) = post_office(&dir.join("po"));
    let (_server, _at_user, at_gateway) = loopback();
    let (_gw, _rx) = gateway(&dir, &running, &base, &po_key, at_gateway);
    let mut stranger = client(
        &dir.join("n0call"),
        &running,
        &po_key,
        "n0call@test.example",
        "N0CALL",
    );
    stranger.post_office = Some(base);
    std::fs::write(
        dir.join("n0call/trust.json"),
        json!({"test.example": [{"public": radiopost::cbor::hex(&key().verifying_key().to_bytes()),
                                 "not_before": 0, "not_after": u64::MAX}]})
        .to_string(),
    )
    .expect("another key trusted");
    let refused = radiopost_client::online::gateways(&mut stranger).expect_err("refused");
    assert!(refused.contains("not signed"), "{refused}");
    assert!(stranger.gateway_list.is_none());
}

/// RP-1 §4.2, RP-3 §5.8: a client asks for a renewed credential in `HELLO` only in the
/// credential's last month, and keeps what the post office issues through the gateway.
#[test]
fn a_client_renews_its_credential_on_a_gateway_call_when_it_is_due() {
    let dir = scratch("renew-by-radio");
    let (running, base, po_key) = post_office(&dir.join("po"));
    let (_server, at_user, at_gateway) = loopback();
    let (_gw, _rx) = gateway(&dir, &running, &base, &po_key, at_gateway);
    let mut user = client(
        &dir.join("wd5ema"),
        &running,
        &po_key,
        "club@test.example",
        "WD5EMA",
    );
    let before = user.me().expect("a credential");
    let mut opts = Options::new(&at_user.to_string());
    let call = |user: &mut radiopost_client::store::Store, opts: &Options| {
        let mut said = Vec::new();
        let report = session::call(user, "KK4ODA-1", Kind::Gateway, opts, &mut |s| {
            said.push(s.to_owned());
        })
        .expect("called");
        assert_eq!(report.failed, None, "{said:?}");
        said
    };

    // ninety days to run: nothing asked, nothing changed
    let said = call(&mut user, &opts);
    assert!(
        !said.iter().any(|l| l.starts_with("asking for a renewed")),
        "{said:?}"
    );
    assert_eq!(user.me().expect("a credential").serial, before.serial);

    // renewing in the last hundred days: asked, and the new one kept
    opts.renew_within = 100 * 86_400;
    let said = call(&mut user, &opts);
    assert!(
        said.iter().any(|l| l
            .starts_with("asking for a renewed credential: this one runs out in ")
            && l.ends_with(" days")),
        "{said:?}"
    );
    assert!(
        said.iter()
            .any(|l| l == "a renewed credential, kept for the next session"),
        "{said:?}"
    );
    let after = user.me().expect("a credential");
    assert_ne!(after.serial, before.serial);
    assert_eq!(
        (
            after.address.as_str(),
            after.operator.as_str(),
            after.device
        ),
        (
            before.address.as_str(),
            before.operator.as_str(),
            before.device
        )
    );
    assert!(after.expires >= before.expires);
}

/// RP-5 §6.2: in its credential's last month the gateway renews its own credential with its
/// account's post office, and the gateway already answering takes it up at its next session; with
/// the post office out of reach it says why once, not at every try.
#[test]
fn the_gateway_renews_its_own_credential_and_answers_with_it() {
    let dir = scratch("gateway-renewal");
    let (running, base, po_key) = post_office(&dir.join("po"));
    let (_server, at_user, at_gateway) = loopback();
    let (gw, rx) = gateway(&dir, &running, &base, &po_key, at_gateway);
    let credential = || {
        station::Station::load(&gw)
            .expect("the gateway")
            .credential()
            .expect("a credential")
    };
    let before = credential();

    // ninety days to run: not due
    assert_eq!(
        station::renew_if_due(&gw, station::now(), radiopost::credential::RENEW_WITHIN),
        Ok(None)
    );

    // the answering loop's renewal, with a hundred days' notice: its first try is at once, and
    // the next an hour on (with every try at once, a credential just renewed would be due again)
    let mut renewal = station::Renewal::new(&gw, Duration::from_secs(3600), 100 * 86_400);
    let mut said = Vec::new();
    let until = std::time::Instant::now() + Duration::from_secs(20);
    while said.is_empty() {
        assert!(std::time::Instant::now() < until, "no renewal");
        renewal.tick(&mut |l: &str| said.push(l.to_owned()));
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        said[0].starts_with("renewed the gateway's credential: serial "),
        "{said:?}"
    );
    let after = credential();
    assert_ne!(after.serial, before.serial);
    assert_eq!(
        (
            after.address.as_str(),
            after.operator.as_str(),
            after.device,
            after.flags
        ),
        (
            before.address.as_str(),
            before.operator.as_str(),
            before.device,
            before.flags
        )
    );

    // the gateway already answering uses it from its next session
    let mut user = client(
        &dir.join("wd5ema"),
        &running,
        &po_key,
        "club@test.example",
        "WD5EMA",
    );
    let opts = Options::new(&at_user.to_string());
    let report =
        session::call(&mut user, "KK4ODA-1", Kind::Gateway, &opts, &mut |_| {}).expect("called");
    assert_eq!(report.failed, None);
    let lines = session_lines(&rx);
    assert!(
        lines
            .iter()
            .any(|l| l == "this session uses the gateway's renewed credential"),
        "{lines:?}"
    );

    // its account's post office out of reach: said once, however many tries
    let mut settings = station::Settings::load(&gw).expect("settings");
    settings
        .post_offices
        .insert("test.example".into(), "http://127.0.0.1:9".into());
    settings.save(&gw).expect("saved");
    let mut renewal = station::Renewal::new(&gw, Duration::ZERO, 100 * 86_400);
    let mut said = Vec::new();
    let until = std::time::Instant::now() + Duration::from_millis(1500);
    while std::time::Instant::now() < until {
        renewal.tick(&mut |l: &str| said.push(l.to_owned()));
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].starts_with("could not renew the gateway's credential: test.example's post office"),
        "{said:?}"
    );
    assert_eq!(credential().serial, after.serial, "nothing kept");
}
