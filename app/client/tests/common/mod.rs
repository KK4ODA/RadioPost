// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the client's tests share: the vectors' identities, a loopback adapter, homes set up.

#![allow(dead_code, reason = "each test file uses part of it")]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use radiopost::message::Message;
use radiopost_client::store::{Store, unhex};
use rpmi::channel::{Channel, StationConfig};
use rpmi::tcp::Server;
use rpmi::wire::Caps;
use serde_json::Value;

pub fn vectors(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vectors")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).expect("a vector file")).expect("JSON")
}

pub fn key_entry(label: &str) -> Value {
    vectors("keys.json")["keys"]
        .as_array()
        .expect("keys")
        .iter()
        .find(|k| k["label"] == label)
        .unwrap_or_else(|| panic!("no key {label}"))
        .clone()
}

pub fn credential_hex(name: &str) -> String {
    vectors("credentials.json")["credentials"]
        .as_array()
        .expect("credentials")
        .iter()
        .find(|c| c["name"] == name)
        .and_then(|c| c["credential_hex"].as_str())
        .expect("a credential")
        .to_owned()
}

pub fn message(name: &str) -> Message {
    let m = vectors("messages.json")["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|m| m["name"] == name)
        .and_then(|m| m["canonical_hex"].as_str())
        .and_then(unhex)
        .expect("a message");
    Message::from_canonical(&m).expect("canonical")
}

/// The test post offices' keys, as `trust` takes them: domain, public key, validity.
pub fn post_office_keys() -> Vec<(String, String, u64, u64)> {
    vectors("keys.json")["keys"]
        .as_array()
        .expect("keys")
        .iter()
        .filter_map(|k| {
            let label = k["label"].as_str()?;
            let domain = label.strip_prefix("po:")?.split(':').next()?.to_owned();
            let seed: [u8; 32] = unhex(k["seed_hex"].as_str()?)?.try_into().ok()?;
            let public = ed25519_dalek::SigningKey::from_bytes(&seed)
                .verifying_key()
                .to_bytes();
            Some((
                domain,
                radiopost::cbor::hex(&public),
                k["not_before"].as_u64()?,
                k["not_after"].as_u64()?,
            ))
        })
        .collect()
}

pub fn loopback() -> (Server, SocketAddr, SocketAddr) {
    loopback_dialled(None)
}

/// The loopback adapter, its radios on `dial_hz` and able to tune (RP-4 §4.1) when it is given.
pub fn loopback_dialled(dial_hz: Option<u64>) -> (Server, SocketAddr, SocketAddr) {
    loopback_with(dial_hz, Duration::from_secs(10))
}

/// The loopback adapter as [`loopback_dialled`], its modems giving up on a call nobody answers
/// after `call_timeout`.
pub fn loopback_with(
    dial_hz: Option<u64>,
    call_timeout: Duration,
) -> (Server, SocketAddr, SocketAddr) {
    loopback_full(dial_hz, call_timeout, "the loopback adapter")
}

/// The loopback adapter, its modems naming themselves `modem` in `hello` (RP-4 §3), as Mercury's
/// adapter says "Mercury".
pub fn loopback_named(modem: &str) -> (Server, SocketAddr, SocketAddr) {
    loopback_full(None, Duration::from_secs(10), modem)
}

fn loopback_full(
    dial_hz: Option<u64>,
    call_timeout: Duration,
    modem: &str,
) -> (Server, SocketAddr, SocketAddr) {
    let caps = Caps {
        bandwidths: vec![500, 2300],
        max_links: 1,
        delivery: true,
        broadcast: true,
        quality: false,
        busy: true,
        regulatory: false,
        max_queue: 65536,
        shared: false,
        dial: dial_hz.is_some(),
        scan: false,
        call_s: false,
        tone: dial_hz.is_some(),
    };
    let station = |name: &str, prefix: &str| StationConfig {
        name: name.into(),
        modem: modem.into(),
        caps: caps.clone(),
        prefix: prefix.into(),
        frame_bytes: 128,
        dial_hz,
    };
    let mut ch = Channel::new();
    let f = ch.add_station(station("KK4ODA", "c"));
    let a = ch.add_station(station("N0CALL", "a"));
    let server = Server::new(ch, call_timeout);
    let at_f = server.listen(f, "127.0.0.1:0").expect("a port");
    let at_a = server.listen(a, "127.0.0.1:0").expect("a port");
    (server, at_f, at_a)
}

pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("radiopost-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    dir
}

/// A store set up as `radiopost init` and `trust` would.
pub fn store_for(home: &Path, device: &str, credential: &str) -> Store {
    let store = Store::open(home).expect("a home");
    let seed: [u8; 32] = unhex(key_entry(device)["seed_hex"].as_str().expect("a seed"))
        .and_then(|b| b.try_into().ok())
        .expect("32 bytes");
    store
        .set_device(&ed25519_dalek::SigningKey::from_bytes(&seed))
        .expect("kept");
    store
        .set_credential(&unhex(&credential_hex(credential)).expect("hex"))
        .expect("a credential for this key");
    for (domain, public, nb, na) in post_office_keys() {
        let public: [u8; 32] = unhex(&public).and_then(|b| b.try_into().ok()).expect("32");
        store
            .add_trust(
                &domain,
                &radiopost::credential::PostOfficeKey {
                    public,
                    not_before: nb,
                    not_after: na,
                },
            )
            .expect("trusted");
    }
    store
}

/// The message a home keeps under the content ID `id`.
pub fn kept(home: &Path, id: &str) -> Message {
    let store = Store::open(home).expect("a home");
    let id = store.find(id).expect("kept");
    store.message(&id).expect("a message")
}
