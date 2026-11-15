// SPDX-License-Identifier: MIT OR Apache-2.0
//! The public test service end to end, on one computer: a RadioPost client gets an enrollment
//! code from the page, enrolls at the test post office, and calls RPTEST-1 and RPTEST-2 through a
//! TCP adapter of its own (RP-4 §9.5); each session's report says it passed. A program that sends
//! what RP-3 does not is told so in its report.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use radiopost::credential::PostOfficeKey;
use radiopost_client::compose::Draft;
use radiopost_client::online;
use radiopost_client::session::{self, Kind, Options};
use radiopost_client::store::Store;
use radiopost_tcp::{Adapter, Config, Security};
use radiopost_test_service::{GATEWAY, STATION, Service, Settings, start};
use rpmi::host::{Event, Host};
use rpmi::tcp::Connection;
use serde_json::{Value as Json, json};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("radiopost-service-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    dir
}

fn service(dir: &std::path::Path) -> Service {
    start(&Settings {
        dir: dir.to_owned(),
        domain: "test.example".into(),
        url: String::new(),
        web: "127.0.0.1:0".into(),
        air: "127.0.0.1:0".into(),
        plain: true,
    })
    .expect("the service starts")
}

/// One HTTP request to the page: its status and body.
fn http(at: SocketAddr, method: &str, path: &str, body: Option<&Json>) -> (u16, String) {
    let mut s = TcpStream::connect(at).expect("the page");
    let text = body.map(Json::to_string).unwrap_or_default();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: {at}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    )
    .expect("sent");
    let mut answer = String::new();
    s.read_to_string(&mut answer).expect("an answer");
    let status = answer
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = answer
        .split_once("\r\n\r\n")
        .map_or("", |(_, b)| b)
        .to_owned();
    (status, body)
}

/// The caller's own TCP adapter, which names the service's stations as peers: its RPMI address.
fn adapter(air: SocketAddr) -> SocketAddr {
    let a = Adapter::new(Config {
        security: Security::Plain,
        peers: HashMap::from([
            (GATEWAY.to_owned(), air.to_string()),
            (STATION.to_owned(), air.to_string()),
        ]),
        name: "the caller's TCP adapter".into(),
    });
    let hosts = TcpListener::bind("127.0.0.1:0").expect("a port");
    let at = hosts.local_addr().expect("an address");
    a.serve_hosts(hosts);
    at
}

/// The reports, once there are `n`.
fn reports(at: SocketAddr, n: usize) -> Vec<Json> {
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        let (_, body) = http(at, "GET", "/runs.json", None);
        let runs: Vec<Json> = serde_json::from_str(&body).unwrap_or_default();
        if runs.len() >= n {
            return runs;
        }
        assert!(Instant::now() < until, "{n} reports: {runs:#?}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one visit to the service, told in order"
)]
fn a_client_enrolls_calls_both_stations_and_passes() {
    let dir = scratch("pass");
    let svc = service(&dir.join("service"));

    // the page names a callsign's code, and refuses what is no callsign
    let (status, body) = http(
        svc.web,
        "POST",
        "/codes",
        Some(&json!({"callsign": "nocall"})),
    );
    assert_eq!(status, 400, "{body}");
    let (status, body) = http(
        svc.web,
        "POST",
        "/codes",
        Some(&json!({"callsign": "KK4ODA"})),
    );
    assert_eq!(status, 200, "{body}");
    let code: Json = serde_json::from_str(&body).expect("JSON");
    assert_eq!(code["address"], "kk4oda@test.example");
    let public: [u8; 32] = (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&code["public_hex"].as_str().expect("a key")[i..i + 2], 16))
        .collect::<Result<Vec<u8>, _>>()
        .expect("hex")
        .try_into()
        .expect("32 bytes");

    // the client trusts the test post office's key by hand, and enrolls
    let mut store = Store::open(&dir.join("client")).expect("a home");
    store
        .add_trust(
            "test.example",
            &PostOfficeKey {
                public,
                not_before: 0,
                not_after: u64::MAX,
            },
        )
        .expect("trusted");
    let said = online::enroll(
        &mut store,
        code["api"].as_str(),
        "kk4oda@test.example",
        "KK4ODA",
        code["code"].as_str().expect("a code"),
    )
    .expect("enrolled");
    assert!(said.contains("kk4oda@test.example"), "{said}");
    let note = Draft {
        from: "kk4oda@test.example".into(),
        to: vec!["rptest-1@test.example".into()],
        cc: Vec::new(),
        date: radiopost_test_service::now(),
        subject: "Testing".into(),
        body: "A message to the test gateway.\n".into(),
        files: Vec::new(),
        priority: 0,
        reply_to: None,
    }
    .message()
    .expect("a message");
    let sent = store.queue(&note).expect("queued");

    // a call to the test gateway: the message goes, the three waiting come
    let rpmi = adapter(svc.air);
    let mut lines = Vec::new();
    let report = session::call(
        &mut store,
        GATEWAY,
        Kind::Gateway,
        &Options::new(&rpmi.to_string()),
        &mut |s| lines.push(s.to_owned()),
    )
    .expect("called");
    assert_eq!(report.failed, None, "{lines:#?}");
    assert_eq!(report.delivered, [sent], "{lines:#?}");
    assert_eq!(report.received.len(), 3, "{lines:#?}");
    let runs = reports(svc.web, 1);
    assert_eq!(
        (runs[0]["station"].as_str(), runs[0]["caller"].as_str()),
        (Some(GATEWAY), Some("KK4ODA"))
    );
    assert_eq!(runs[0]["passed"], true, "{:#?}", runs[0]);

    // a call to the test station: it proves itself, and hands over its note
    let mut lines = Vec::new();
    let report = session::call(
        &mut store,
        STATION,
        Kind::Station,
        &Options::new(&rpmi.to_string()),
        &mut |s| lines.push(s.to_owned()),
    )
    .expect("called");
    assert_eq!(report.failed, None, "{lines:#?}");
    assert_eq!(report.received.len(), 1, "{lines:#?}");
    let runs = reports(svc.web, 2);
    assert_eq!(runs[0]["station"], STATION, "the newest first");
    assert_eq!(runs[0]["passed"], true, "{:#?}", runs[0]);

    // the report as a page, and the page itself
    let id = runs[0]["id"].as_str().expect("an id");
    let (status, page) = http(svc.web, "GET", &format!("/runs/{id}"), None);
    assert_eq!(status, 200);
    assert!(
        page.contains("passed") && page.contains("Transmissions") && !page.contains("failed"),
        "{page}"
    );
    let (_, home) = http(svc.web, "GET", "/", None);
    assert!(
        home.contains(GATEWAY) && home.contains("test.example"),
        "{home}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A program that sends bytes that are no PDU: the service answers ERROR, and the report says
/// which check failed and why.
#[test]
fn a_program_that_breaks_rp3_is_told_in_its_report() {
    let dir = scratch("fail");
    let svc = service(&dir.join("service"));
    let rpmi = adapter(svc.air);
    let mut conn = Connection::open(rpmi).expect("the adapter");
    conn.set_timeout(Some(Duration::from_secs(10)))
        .expect("a timeout");
    let mut host = Host::new();
    for request in [
        host.hello("a broken program 0.0"),
        host.callsigns(&["W1AW"]),
        host.connect(GATEWAY, None, None),
    ] {
        conn.send(&request).expect("sent");
    }
    let link = loop {
        let m = conn.read_message().expect("read").expect("a line");
        if let Ok(Event::Connected { link, .. }) = Event::read(&m) {
            break link;
        }
    };
    // two bytes of length, then what is not CBOR
    conn.send(&host.send(&link, &[0, 3, 0xff, 0xff, 0xff], None))
        .expect("sent");
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        match conn.read_message() {
            Ok(Some(m)) => {
                if let Ok(Event::Received { .. }) = Event::read(&m) {
                    break;
                }
            }
            _ => break,
        }
    }
    conn.send(&host.disconnect(&link)).expect("sent");
    let runs = reports(svc.web, 1);
    assert_eq!(runs[0]["passed"], false, "{:#?}", runs[0]);
    let failed: Vec<&str> = runs[0]["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .filter(|c| c["passed"] == false)
        .filter_map(|c| c["check"].as_str())
        .collect();
    assert!(
        failed.contains(&"Every PDU framed, in deterministic CBOR, of a known type"),
        "{failed:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
