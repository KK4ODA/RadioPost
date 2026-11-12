// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-1 §7.5: which post offices a post office knows. Two post offices in one process: a user of
//! `other.example` is unknown at `test.example` until its operators accept it; meeting its
//! credential puts it in line to be looked up, and what is found of it (its DNS records, info
//! document and revocation list, checked) is kept as seen; accepted, its credentials are taken,
//! its revocations honoured, and it is published in `/post-offices`; a refresh that fails keeps
//! what was held; blocked, it is unknown again and not met.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use ed25519_dalek::SigningKey;
use radiopost::base64url;
use radiopost::crypto::{self, context};
use radiopost_postoffice::documents::Info;
use radiopost_postoffice::known::{self, Found};
use radiopost_postoffice::serve::{self, Files, Running, Settings};
use serde_json::{Value as Json, json};

const HERE: &str = "https://po.test.example/rp/v1";
const THERE: &str = "https://po.other.example/rp/v1";

fn settings(domain: &str, base: &str, operator: &str) -> Settings {
    Settings {
        domains: vec![domain.into()],
        base_url: base.into(),
        listen: "127.0.0.1:0".into(),
        admin: "127.0.0.1:0".into(),
        info: Info {
            operator: operator.into(),
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
        known: Vec::new(),
        blocked: Vec::new(),
        // nothing listens here: a refresh fails, as when DNS does not answer
        resolver: Some("http://127.0.0.1:9/resolve".into()),
        hooks: None,
        hook_token: None,
        smtp: None,
        operators: Vec::new(),
        lookups: radiopost_postoffice::lookups::LookupSettings::default(),
        mailboxes: radiopost_postoffice::mailboxes::MailboxSettings::default(),
        health: radiopost_postoffice::serve::HealthSettings::default(),
    }
}

fn office(name: &str, s: &Settings, key: u8) -> (Running, PathBuf) {
    let dir = std::env::temp_dir().join(format!("radiopost-known-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    let files = Files::new(&dir);
    files
        .write("key.hex", &format!("{key:02x}").repeat(32))
        .expect("written");
    files
        .write("config.json", &serde_json::to_string(s).expect("JSON"))
        .expect("written");
    (load(&dir), dir)
}

fn load(dir: &Path) -> Running {
    Running::load(dir, serve::system_draw()).expect("loaded")
}

fn call(r: &Running, method: &str, path: &str, token: Option<&str>, body: &Json) -> (u16, Json) {
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        body.to_string().into_bytes()
    };
    let (status, _, text) = r.api(method, &format!("/rp/v1{path}"), token, &bytes);
    (
        status,
        serde_json::from_str(&text).unwrap_or(Json::String(text)),
    )
}

fn admin(r: &Running, method: &str, path: &str, body: &Json) -> (u16, Json) {
    let (status, text) = r.admin(method, path, body.to_string().as_bytes());
    (status, serde_json::from_str(&text).expect("JSON"))
}

/// A device of `other.example` enrolled with a code: its credential.
fn enroll(there: &Running, address: &str, device: &SigningKey) -> String {
    let (_, made) = admin(
        there,
        "POST",
        "/code",
        &json!({"address": address, "operator": "N0CALL"}),
    );
    let code = made["code"].as_str().expect("a code");
    let signed = format!("{code}{THERE}");
    let body = json!({"address": address, "operator": "N0CALL", "code": code,
        "device": base64url::encode(&device.verifying_key().to_bytes()),
        "signature": base64url::encode(&crypto::sign(context::API_ENROLL, signed.as_bytes(), device))});
    let (status, answer) = call(there, "POST", "/enroll", None, &body);
    assert_eq!(status, 200, "{answer}");
    answer["credential"]
        .as_str()
        .expect("a credential")
        .to_owned()
}

/// Logs in at `here` with a credential: the status and the error's name, or `ok`.
fn login(here: &Running, credential: &str, device: &SigningKey) -> String {
    let (_, c) = call(here, "POST", "/auth/challenge", None, &Json::Null);
    let nonce = c["nonce"].as_str().expect("a nonce").to_owned();
    let mut signed = base64url::decode(&nonce).expect("base64url");
    signed.extend_from_slice(HERE.as_bytes());
    let body = json!({"credential": credential, "nonce": nonce,
        "signature": base64url::encode(&crypto::sign(context::API_LOGIN, &signed, device))});
    let (status, answer) = call(here, "POST", "/auth/token", None, &body);
    if status == 200 {
        "ok".into()
    } else {
        format!("{status} {}", answer["name"].as_str().unwrap_or("?"))
    }
}

/// What a post office would find of `there` in DNS and at its API, checked.
fn found_of(
    there: &Running,
    domain: &str,
) -> (Vec<BTreeMap<String, String>>, Found, Option<String>) {
    let records: Vec<_> = there
        .office()
        .dns_records()
        .iter()
        .filter_map(|line| {
            line.split_once('"')
                .map(|(_, t)| t.trim_end_matches('"').to_owned())
        })
        .filter_map(|t| radiopost::dns::parse(&t))
        .collect();
    let (_, _, info) = there.api("GET", "/rp/v1/info", None, &[]);
    let (_, _, revocations) = there.api("GET", "/rp/v1/revocations", None, &[]);
    let (found, error) = known::assemble(
        domain,
        &records,
        true,
        Some(&info),
        Some(&revocations),
        serve::now(),
    )
    .expect("an API record");
    (records, found, error)
}

#[test]
#[allow(clippy::too_many_lines, reason = "one story, told in order")]
fn a_post_office_knows_another_only_once_its_operators_accept_it() {
    let (here, here_dir) = office("here", &settings("test.example", HERE, "KK4ODA"), 7);
    let (there, there_dir) = office("there", &settings("other.example", THERE, "N1CALL"), 9);
    let ana = SigningKey::from_bytes(&[21; 32]);
    let stolen = SigningKey::from_bytes(&[22; 32]);
    let ana_credential = enroll(&there, "ana@other.example", &ana);
    let stolen_credential = enroll(&there, "ana@other.example", &stolen);

    // unknown: refused, and its domain met, to be looked up
    assert_eq!(
        login(&here, &ana_credential, &ana),
        "422 unknown-post-office"
    );
    let view = here.post_offices_view();
    assert_eq!(view["looking_up"], json!(["other.example"]), "{view}");

    // what is found of it is checked: the documents must be signed by its keys in DNS, and
    // name the domain looked up
    let (records, found, error) = found_of(&there, "other.example");
    assert_eq!(error, None);
    assert_eq!(found.api, THERE);
    assert_eq!(found.info["operator"], "N1CALL");
    let (_, _, here_info) = here.api("GET", "/rp/v1/info", None, &[]);
    let (_, error) =
        known::assemble("other.example", &records, false, Some(&here_info), None, 1).expect("api");
    assert!(error.is_some_and(|e| e.contains("not signed by a key in DNS")));
    let (_, _, error) = found_of(&there, "elsewhere.example");
    assert!(error.is_some_and(|e| e.contains("does not name elsewhere.example")));

    // seen: kept for the operators, and still unknown
    let how: BTreeSet<String> = ["credential".to_owned()].into();
    here.saw("other.example", &records, &how, found.clone(), None);
    let view = here.post_offices_view();
    assert_eq!(view["seen"][0]["domain"], "other.example", "{view}");
    assert_eq!(view["seen"][0]["operator"], "N1CALL");
    assert_eq!(view["seen"][0]["how"], json!(["credential"]));
    assert_eq!(view["seen"][0]["dnssec"], true);
    assert_eq!(
        login(&here, &ana_credential, &ana),
        "422 unknown-post-office"
    );
    assert!(
        !here
            .office()
            .known_post_offices()
            .contains(&"other.example".to_owned())
    );

    // accepted: what was seen whole serves at once
    let (status, view) = admin(
        &here,
        "POST",
        "/post-offices",
        &json!({"accept": "other.example"}),
    );
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["known"][0]["domain"], "other.example");
    assert_eq!(view["known"][0]["api"], THERE);
    assert_eq!(view["seen"], json!([]));
    assert_eq!(login(&here, &ana_credential, &ana), "ok");
    assert_eq!(login(&here, &stolen_credential, &stolen), "ok");
    assert!(
        here.office()
            .known_post_offices()
            .contains(&"other.example".to_owned())
    );

    // published in /post-offices, with its API and keys, signed with this post office's key
    let (_, _, list) = here.api("GET", "/rp/v1/post-offices", None, &[]);
    let me = SigningKey::from_bytes(&[7; 32]).verifying_key().to_bytes();
    let keys: HashMap<String, [u8; 32]> = [(radiopost::cbor::hex(&crypto::kid(&me)), me)].into();
    let list = radiopost::jws::verify(&list, &keys).expect("signed by this post office");
    let entry = &list["post_offices"][0];
    assert_eq!(entry["domain"], "other.example", "{list}");
    assert_eq!(entry["api"], THERE);
    let theirs = SigningKey::from_bytes(&[9; 32]).verifying_key().to_bytes();
    assert_eq!(
        entry["keys"][0]["kid"],
        radiopost::cbor::hex(&crypto::kid(&theirs))
    );

    // a device stolen there is revoked there; once fetched again, it is refused here too
    admin(
        &there,
        "POST",
        "/revoke",
        &json!({"device": base64url::encode(&stolen.verifying_key().to_bytes())}),
    );
    let (_, fresh, error) = found_of(&there, "other.example");
    assert_eq!(error, None);
    assert_eq!(fresh.serials.len(), 1, "the stolen device's credential");
    here.learned("other.example", fresh);
    assert_eq!(
        login(&here, &stolen_credential, &stolen),
        "422 credential-revoked"
    );
    assert_eq!(login(&here, &ana_credential, &ana), "ok");

    // a refresh that fails keeps what it held, and says why
    here.refresh("other.example");
    let view = here.post_offices_view();
    assert!(
        view["known"][0]["error"]
            .as_str()
            .is_some_and(|e| e.contains("DNS did not answer")),
        "{view}"
    );
    assert_eq!(
        login(&here, &stolen_credential, &stolen),
        "422 credential-revoked"
    );
    assert_eq!(login(&here, &ana_credential, &ana), "ok");

    // blocked: unknown, unpublished, and not met again; it stays so after a restart
    let (status, view) = admin(
        &here,
        "POST",
        "/post-offices",
        &json!({"block": "other.example"}),
    );
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["known"], json!([]));
    assert_eq!(view["blocked"], json!(["other.example"]));
    assert_eq!(
        login(&here, &ana_credential, &ana),
        "422 unknown-post-office"
    );
    assert!(
        !here
            .office()
            .known_post_offices()
            .contains(&"other.example".to_owned())
    );
    let (_, _, list) = here.api("GET", "/rp/v1/post-offices", None, &[]);
    let list = radiopost::jws::verify(&list, &keys).expect("signed");
    assert_eq!(list["post_offices"], json!([]));
    let again = load(&here_dir);
    assert_eq!(
        login(&again, &ana_credential, &ana),
        "422 unknown-post-office"
    );
    assert_eq!(again.post_offices_view()["looking_up"], json!([]));
    assert_eq!(
        again.post_offices_view()["blocked"],
        json!(["other.example"])
    );

    // its own domain, and what is not a domain, cannot be accepted
    let (status, _) = admin(
        &here,
        "POST",
        "/post-offices",
        &json!({"accept": "test.example"}),
    );
    assert_eq!(status, 400);
    let (status, _) = admin(
        &here,
        "POST",
        "/post-offices",
        &json!({"accept": "not a domain"}),
    );
    assert_eq!(status, 400);

    let _ = std::fs::remove_dir_all(here_dir);
    let _ = std::fs::remove_dir_all(there_dir);
}

#[test]
fn a_key_typed_by_the_operator_serves_until_one_is_fetched() {
    let theirs = SigningKey::from_bytes(&[9; 32]).verifying_key().to_bytes();
    let mut s = settings("test.example", HERE, "KK4ODA");
    s.known.push(json!({"domain": "other.example", "api": THERE,
        "keys": [{"public": base64url::encode(&theirs), "not_before": 0, "not_after": u64::MAX}]}));
    let (here, here_dir) = office("typed", &s, 7);
    let (there, there_dir) = office(
        "typed-there",
        &settings("other.example", THERE, "N1CALL"),
        9,
    );
    let ana = SigningKey::from_bytes(&[21; 32]);
    let credential = enroll(&there, "ana@other.example", &ana);
    assert_eq!(login(&here, &credential, &ana), "ok");
    let view = here.post_offices_view();
    assert_eq!(view["known"][0]["typed"], true, "{view}");
    let (_, _, list) = here.api("GET", "/rp/v1/post-offices", None, &[]);
    assert!(list.contains('.'), "a JWS");
    let _ = std::fs::remove_dir_all(here_dir);
    let _ = std::fs::remove_dir_all(there_dir);
}
