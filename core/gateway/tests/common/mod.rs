// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the gateway's tests share: a post office on a port of its own, clients' homes it issued
//! credentials for, the loopback adapter with a user's station and the gateway's, and messages.

#![allow(dead_code, reason = "each test file uses some of these")]

use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use radiopost::credential::{PostOfficeKey, flags};
use radiopost_client::compose::Draft;
use radiopost_client::store::Store;
use radiopost_postoffice::documents::Info;
use radiopost_postoffice::serve::{self, Files, Running, Settings};
use rpmi::channel::{Channel, StationConfig};
use rpmi::tcp::Server;
use rpmi::wire::Caps;
use serde_json::json;

pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("radiopost-gw-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    dir
}

pub fn key() -> SigningKey {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).expect("random numbers");
    SigningKey::from_bytes(&seed)
}

/// The post office for `test.example`, serving on a port of its own, at a base URL that is that
/// port (what the gateway signs and the post office checks).
pub fn post_office(dir: &Path) -> (Arc<Running>, String, SigningKey) {
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
            applications: None,
        },
        known: Vec::new(),
        hooks: None,
        hook_token: None,
        smtp: None,
        operators: Vec::new(),
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
pub fn client(
    home: &Path,
    running: &Running,
    po_key: &SigningKey,
    address: &str,
    call: &str,
) -> Store {
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
pub fn loopback() -> (Server, SocketAddr, SocketAddr) {
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

pub fn draft(from: &str, to: &str, subject: &str, body: &str) -> radiopost::message::Message {
    Draft {
        from: from.into(),
        to: vec![to.into()],
        cc: Vec::new(),
        date: serve::now(),
        subject: subject.into(),
        body: body.into(),
        files: Vec::new(),
        priority: 0,
        reply_to: None,
    }
    .message()
    .expect("a message")
}
