// SPDX-License-Identifier: MIT OR Apache-2.0
//! A gateway's registration (RP-5 §6.4) from the account its operator chooses among their
//! client's (wishlist W46), and withdrawn again: by its operator from the gateway, and by the post
//! office's operators on its own port (§7.7). Two post offices, as for the tester who registered
//! with radiopost.org's account while trying his own post office (2026-11-18).

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use radiopost::credential::Credential;
use radiopost_gateway::store::{Copy, Record, Shared, Store};
use radiopost_gateway::{operator, station};
use radiopost_postoffice::serve::Running;
use serde_json::{Value, json};

use common::{client, key, post_office_for, scratch};

fn record() -> Value {
    json!({"channels": [{"dial_hz": 7_101_000, "mode": "USB", "modem": "aether",
                          "bandwidth_hz": 2300}],
           "grid": "EM73", "place": "Atlanta, GA", "hours": "always", "internet": true})
}

/// KK4ODA-1, not registered, knowing both post offices by their URLs.
fn gateway(dir: &Path, offices: &[(&str, &str)]) -> PathBuf {
    let gw = dir.join("gateway");
    std::fs::create_dir_all(&gw).expect("a directory");
    station::write(&gw, "key.hex", &radiopost::cbor::hex(&key().to_bytes())).expect("written");
    station::Settings {
        callsign: "KK4ODA-1".into(),
        modem: "127.0.0.1:9".into(),
        more_modems: Vec::new(),
        country: "US".into(),
        post_offices: offices
            .iter()
            .map(|(d, u)| ((*d).to_owned(), (*u).to_owned()))
            .collect::<BTreeMap<_, _>>(),
        resolver: String::new(),
        blocked: Vec::new(),
        only: None,
        operator_checks_rules: false,
        answer_at_start: true,
        report_status: false,
        copies: None,
        partners: Vec::new(),
        windows: Vec::new(),
        scan: Vec::new(),
    }
    .save(&gw)
    .expect("saved");
    gw
}

fn serial(gw: &Path) -> u64 {
    let hex = std::fs::read_to_string(gw.join("credential.hex")).expect("a credential");
    Credential::decode(&station::unhex(hex.trim()).expect("hex"))
        .expect("a credential")
        .body
        .serial
}

fn registered(running: &Running) -> Vec<String> {
    running.office().state.gateways.keys().cloned().collect()
}

#[test]
#[allow(clippy::too_many_lines, reason = "one move, told in order")]
fn the_operator_chooses_the_account_and_moves_the_gateway_to_another_post_office() {
    let dir = scratch("registration");
    let (hampost, hampost_url, hampost_key) = post_office_for(&dir.join("po-a"), "hampost.test");
    let (own, own_url, own_key) = post_office_for(&dir.join("po-b"), "own.test");
    // the operator's client: its first account at hampost.test, a second at their own post
    // office, chosen in the app (W46)
    let base = dir.join("client");
    let _first = client(
        &base,
        &hampost,
        &hampost_key,
        "kk4oda@hampost.test",
        "KK4ODA",
    );
    let second_home = base.join("callsigns").join("2");
    let _second = client(&second_home, &own, &own_key, "kk4oda@own.test", "KK4ODA");
    std::fs::write(base.join("chosen"), "2").expect("written");
    let gw = gateway(
        &dir,
        &[("hampost.test", &hampost_url), ("own.test", &own_url)],
    );

    let accounts = operator::accounts(&base);
    assert_eq!(
        accounts
            .iter()
            .map(|a| (a.id, a.address.as_str(), a.domain(), a.chosen))
            .collect::<Vec<_>>(),
        [
            (1, "kk4oda@hampost.test", "hampost.test", false),
            (2, "kk4oda@own.test", "own.test", true),
        ]
    );
    // two accounts: the gateway is told which, never guesses
    let none = operator::account(&base, None).expect_err("two accounts");
    assert!(none.contains("--account"), "{none}");
    assert!(operator::account(&base, Some("N0CALL")).is_err());
    assert_eq!(
        operator::account(&base, Some("2")).expect("by number").home,
        second_home
    );
    let mine = operator::account(&base, Some("KK4ODA@OWN.TEST")).expect("by address");
    assert_eq!(mine.home, second_home);

    // registered with the account chosen: its own post office, and only there
    let said = operator::register(&gw, &mine.home, record()).expect("registered");
    assert!(said.contains("own.test"), "{said}");
    assert_eq!(registered(&own), ["KK4ODA-1"]);
    assert_eq!(registered(&hampost), Vec::<String>::new());
    let now = operator::registration(&gw).expect("registered");
    assert_eq!(
        (
            now["domain"].as_str(),
            now["address"].as_str(),
            now["post_office"].as_str()
        ),
        (
            Some("own.test"),
            Some("kk4oda@own.test"),
            Some(own_url.as_str())
        )
    );
    // RP-5 §6.4: one post office at a time
    let first = operator::account(&base, Some("kk4oda@hampost.test")).expect("the first");
    let twice = operator::register(&gw, &first.home, record()).expect_err("registered already");
    assert!(twice.contains("unregister it there first"), "{twice}");
    assert_eq!(registered(&hampost), Vec::<String>::new());

    // a session carried while cut off waits to be reported: users' mail, kept until it is
    let store = Shared::open(&gw);
    store
        .change(|s| {
            s.records.push(Record::default());
            s.copies.push(Copy {
                address: "ana@own.test".into(),
                serial: 7,
                id: "AAAA".into(),
                from: "maria@example.com".into(),
                subject: "Copy".into(),
                size: 10,
                priority: 0,
                third_party: true,
                air: None,
                expires: u64::MAX,
                by: "own.test".into(),
                picked: false,
            });
        })
        .expect("kept");
    let waiting = operator::unregister(&gw).expect_err("a session waits");
    assert!(waiting.contains("wait"), "{waiting}");
    assert_eq!(registered(&own), ["KK4ODA-1"]);

    // reported: withdrawn at its post office, its credential revoked, the copies dropped
    store.change(|s| s.records.clear()).expect("kept");
    let revoked = serial(&gw);
    let said = operator::unregister(&gw).expect("withdrawn");
    assert!(
        said.contains("no longer registered with own.test"),
        "{said}"
    );
    assert_eq!(registered(&own), Vec::<String>::new());
    assert!(own.office().state.revoked.contains(&revoked));
    assert!(!gw.join("credential.hex").exists());
    assert!(operator::registration(&gw).is_none());
    assert_eq!(Store::load(&gw).copies, Vec::new());
    // record.json keeps the record to register again from, and no registration
    assert_eq!(
        operator::last_record(&gw).expect("kept")["place"],
        "Atlanta, GA"
    );
    assert!(operator::register_again(&gw).is_none());

    // and registered with the other account's post office
    let said = operator::register(&gw, &first.home, record()).expect("registered there");
    assert!(said.contains("hampost.test"), "{said}");
    assert_eq!(registered(&hampost), ["KK4ODA-1"]);

    // its post office's operators withdraw it on the operator's port (RP-5 §7.7) ...
    let (status, answer) = hampost.admin("DELETE", "/gateways/kk4oda-1", b"");
    assert_eq!(status, 200, "{answer}");
    let answer: Value = serde_json::from_str(&answer).expect("JSON");
    assert_eq!(answer["callsign"], "KK4ODA-1");
    assert_eq!(answer["revoked"], json!([serial(&gw)]));
    let (status, _) = hampost.admin("DELETE", "/gateways/KK4ODA-1", b"");
    assert_eq!(status, 400);
    // ... and the gateway, told 404 and finding itself on no list, forgets it too
    let said = operator::unregister(&gw).expect("forgotten");
    assert!(said.contains("no longer registered"), "{said}");
    assert!(operator::registration(&gw).is_none());
}

#[test]
fn unregister_here_only_forgets_without_asking() {
    let dir = scratch("registration-here");
    let (own, own_url, own_key) = post_office_for(&dir.join("po"), "own.test");
    let base = dir.join("client");
    let _me = client(&base, &own, &own_key, "kk4oda@own.test", "KK4ODA");
    let gw = gateway(&dir, &[("own.test", &own_url)]);
    let one = operator::account(&base, None).expect("the only account");
    operator::register(&gw, &one.home, record()).expect("registered");
    operator::forget(&gw).expect("forgotten");
    assert!(operator::registration(&gw).is_none());
    // the post office was not asked: its operators remove the record
    assert_eq!(registered(&own), ["KK4ODA-1"]);
}
