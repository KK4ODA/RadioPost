// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-5 §4.4, D36: a client's gateway list read from its own post office and every post office
//! that one knows. Five post offices in one process: the home lists two gateways and knows four
//! others; one of them lists a callsign the home lists, two list the same callsign, and one signs
//! its list with a key its entry does not give. Then one stops answering, and its last list is
//! kept; then the home lets in only its own gateways.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use radiopost::base64url;
use radiopost::credential::{PostOfficeKey, flags};
use radiopost_client::online;
use radiopost_client::store::Store;
use radiopost_postoffice::documents::Info;
use radiopost_postoffice::serve::{self, Files, Running, Settings};
use serde_json::{Value, json};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("radiopost-lists-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    dir
}

/// A post office for `domain`, serving at a base URL that is the port it listens on, knowing
/// `known` (entries as `know --public` writes them), with `gateways` registered.
fn post_office(
    dir: &Path,
    domain: &str,
    seed: u8,
    known: Vec<Value>,
    gateways: &[(&str, &str)],
) -> (Arc<Running>, String) {
    let port = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("a free port")
        .port();
    let base = format!("http://127.0.0.1:{port}");
    std::fs::create_dir_all(dir).expect("a directory");
    let files = Files::new(dir);
    files
        .write("key.hex", &format!("{seed:02x}").repeat(32))
        .expect("written");
    let settings = Settings {
        domains: vec![domain.into()],
        base_url: base.clone(),
        listen: format!("127.0.0.1:{port}"),
        admin: "127.0.0.1:0".into(),
        info: Info {
            operator: "KK4ODA".into(),
            contact: format!("postmaster@{domain}"),
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
        known,
        blocked: Vec::new(),
        // nothing listens here: the typed keys serve, and no test asks the Internet
        resolver: Some("http://127.0.0.1:9/resolve".into()),
        hooks: None,
        hook_token: None,
        smtp: None,
        operators: Vec::new(),
        lookups: radiopost_postoffice::lookups::LookupSettings::default(),
        mailboxes: radiopost_postoffice::mailboxes::MailboxSettings::default(),
        health: radiopost_postoffice::serve::HealthSettings::default(),
    };
    files
        .write(
            "config.json",
            &serde_json::to_string(&settings).expect("JSON"),
        )
        .expect("written");
    let running = Arc::new(Running::load(dir, serve::system_draw()).expect("loaded"));
    {
        let mut office = running.office();
        for (callsign, place) in gateways {
            office.state.gateways.insert(
                (*callsign).to_owned(),
                json!({"callsign": callsign, "operator": callsign.split('-').next(),
                       "grid": "EM73", "place": place, "hours": "always", "internet": true,
                       "channels": [{"dial_hz": 7_101_000, "mode": "USB", "modem": "aether",
                                     "bandwidth_hz": 2300}], "updated": 1}),
            );
        }
    }
    serve::serve(running.clone(), &settings.listen, "127.0.0.1:0").expect("serving");
    (running, base)
}

/// `domain`'s entry for the home's `known`: its API and the key of `seed`.
fn entry(domain: &str, api: &str, seed: u8) -> Value {
    let public = SigningKey::from_bytes(&[seed; 32])
        .verifying_key()
        .to_bytes();
    json!({"domain": domain, "api": api,
           "keys": [{"public": base64url::encode(&public), "not_before": 0, "not_after": u64::MAX}]})
}

/// What the client keeps: each gateway as `CALLSIGN@post office`, sorted.
fn listed(store: &Store) -> Vec<String> {
    let mut out: Vec<String> = store.gateway_list.as_ref().expect("a list")["gateways"]
        .as_array()
        .expect("gateways")
        .iter()
        .map(|g| {
            format!(
                "{}@{}",
                g["callsign"].as_str().unwrap_or(""),
                g["post_office"].as_str().unwrap_or("")
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
#[allow(clippy::too_many_lines, reason = "one story, told in order")]
fn a_client_lists_the_gateways_of_every_post_office_its_own_knows() {
    let dir = scratch("five");
    let (_other, other) = post_office(
        &dir.join("other"),
        "other.example",
        0x31,
        Vec::new(),
        &[
            ("N1CALL-10", "Austin, TX"),
            ("N2CALL-5", "Atlanta, as other.example lists it"),
        ],
    );
    let (_third, third) = post_office(
        &dir.join("third"),
        "third.example",
        0x32,
        Vec::new(),
        &[("N8CALL-7", "Detroit")],
    );
    let (_fourth, fourth) = post_office(
        &dir.join("fourth"),
        "fourth.example",
        0x33,
        Vec::new(),
        &[("N8CALL-7", "Detroit EOC"), ("W4EOC-2", "Macon")],
    );
    let (_forged, forged) = post_office(
        &dir.join("forged"),
        "forged.example",
        0x34,
        Vec::new(),
        &[("W0BAD-1", "Somewhere")],
    );
    let known = vec![
        entry("other.example", &other, 0x31),
        entry("third.example", &third, 0x32),
        entry("fourth.example", &fourth, 0x33),
        // its list is signed by its own key, which this entry does not give
        entry("forged.example", &forged, 0x35),
    ];
    let (home, base) = post_office(
        &dir.join("home"),
        "test.example",
        0x2a,
        known,
        &[
            ("KK4ODA-1", "Atlanta, GA"),
            ("N2CALL-5", "Atlanta, GA: the county EOC"),
        ],
    );

    let mut store = Store::open(&dir.join("kk4oda")).expect("a home");
    let device = SigningKey::from_bytes(&[0x41; 32]);
    store.set_device(&device).expect("kept");
    let credential = {
        let mut office = home.office();
        office.at(serve::now());
        office.issue(
            "kk4oda@test.example",
            "KK4ODA",
            device.verifying_key().to_bytes(),
            flags::SEND | flags::RECEIVE,
        )
    };
    store.set_credential(&credential).expect("a credential");
    store
        .add_trust(
            "test.example",
            &PostOfficeKey {
                public: SigningKey::from_bytes(&[0x2a; 32])
                    .verifying_key()
                    .to_bytes(),
                not_before: 0,
                not_after: u64::MAX,
            },
        )
        .expect("trusted");
    store.post_office = Some(base);

    // every list taken: the home's record wins N2CALL-5; N8CALL-7 comes from each of the two
    // that list it, and is named ambiguous; the forged list is left out, and says why
    online::gateways(&mut store).expect("the lists");
    assert_eq!(
        listed(&store),
        [
            "KK4ODA-1@test.example",
            "N1CALL-10@other.example",
            "N2CALL-5@test.example",
            "N8CALL-7@fourth.example",
            "N8CALL-7@third.example",
            "W4EOC-2@fourth.example",
        ]
    );
    let list = store.gateway_list.clone().expect("a list");
    assert_eq!(list["ambiguous"], json!(["N8CALL-7"]));
    assert!(
        list["errors"]["forged.example"]
            .as_str()
            .is_some_and(|e| e.contains("not signed by its keys")),
        "{list}"
    );
    let n2 = list["gateways"]
        .as_array()
        .expect("gateways")
        .iter()
        .find(|g| g["callsign"] == "N2CALL-5")
        .expect("N2CALL-5");
    assert_eq!(n2["place"], "Atlanta, GA: the county EOC");

    // one stops answering: its last list is kept, and the client says it was not reached
    home.change_settings(|s| {
        for k in &mut s.known {
            if k["domain"] == "fourth.example" {
                k["api"] = json!("http://127.0.0.1:9");
            }
        }
    })
    .expect("changed");
    online::gateways(&mut store).expect("the lists");
    assert!(listed(&store).contains(&"W4EOC-2@fourth.example".to_owned()));
    let list = store.gateway_list.clone().expect("a list");
    assert!(
        list["errors"]["fourth.example"]
            .as_str()
            .is_some_and(|e| e.contains("cannot reach")),
        "{list}"
    );

    // the home lets in only its own gateways: only its list is taken
    home.change_settings(|s| s.info.gateways = "listed".into())
        .expect("changed");
    online::gateways(&mut store).expect("the lists");
    assert_eq!(
        listed(&store),
        ["KK4ODA-1@test.example", "N2CALL-5@test.example"]
    );
    assert_eq!(
        store.gateway_list.as_ref().expect("a list")["lists"],
        json!({})
    );

    let _ = std::fs::remove_dir_all(dir);
}
