// SPDX-License-Identifier: MIT OR Apache-2.0
//! Credentials the tool issues are what RP-1 §5 accepts, for the device they name, from a key
//! trusted as `trust_command` says.

use ed25519_dalek::SigningKey;
use radiopost::credential::{self, PostOfficeKey, Trust, flags};
use radiopost_issue::{PostOffice, flags_of};

const NOW: u64 = 1_794_249_600;

#[test]
fn a_credential_issued_is_one_rp1_accepts() {
    let mut po = PostOffice::new("RadioPost.org", NOW, 365).expect("a key");
    let device = SigningKey::from_bytes(&[9; 32]);
    let device_hex = radiopost::cbor::hex(&device.verifying_key().to_bytes());
    let bytes = po
        .issue(
            "facundo@radiopost.org",
            "kk4oda",
            &device_hex,
            NOW,
            90,
            flags_of("send,receive").expect("flags"),
        )
        .expect("issued");
    let mut trust = Trust::default();
    trust
        .keys
        .entry("radiopost.org".into())
        .or_default()
        .push(PostOfficeKey {
            public: po.public(),
            not_before: po.not_before,
            not_after: po.not_after,
        });
    let body = credential::verify(&bytes, &trust, NOW + 3600, flags::SEND | flags::RECEIVE)
        .expect("RP-1 §5 accepts it");
    assert_eq!(
        (
            body.serial,
            body.address.as_str(),
            body.operator.as_str(),
            body.device
        ),
        (
            1,
            "facundo@radiopost.org",
            "KK4ODA",
            device.verifying_key().to_bytes()
        )
    );
    assert_eq!(body.expires, NOW + 90 * 86_400);
    // each credential its own serial
    let second = po
        .issue("ana@radiopost.org", "N0CALL", &device_hex, NOW, 30, 1)
        .expect("issued");
    let body = credential::verify(&second, &trust, NOW, flags::SEND).expect("accepted");
    assert_eq!(body.serial, 2);
    // a key not trusted is refused
    assert!(credential::verify(&bytes, &Trust::default(), NOW, 0).is_err());
    assert!(
        po.trust_command()
            .starts_with("radiopost trust radiopost.org ")
    );
}

#[test]
fn what_the_tool_refuses() {
    let mut po = PostOffice::new("radiopost.org", NOW, 30).expect("a key");
    let device = radiopost::cbor::hex(&[1; 32]);
    for (address, days, device, why) in [
        ("ana@hampost.org", 30, device.as_str(), "another domain"),
        ("ana@radiopost.org", 91, device.as_str(), "over 90 days"),
        (
            "ana@radiopost.org",
            31,
            device.as_str(),
            "longer than the key",
        ),
        ("ana@radiopost.org", 30, "abcd", "a short device key"),
        ("radiopost.org", 30, device.as_str(), "no local part"),
    ] {
        assert!(
            po.issue(address, "N0CALL", device, NOW, days, 3).is_err(),
            "{why}"
        );
    }
    assert_eq!(po.next_serial, 1, "nothing refused took a serial");
    assert!(flags_of("send,admin").is_err());
}

#[test]
fn the_key_file_round_trips() {
    let dir = std::env::temp_dir().join(format!("radiopost-issue-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a directory");
    let path = dir.join("po.json");
    let mut po = PostOffice::new("radiopost.org", NOW, 365).expect("a key");
    po.issue(
        "a@radiopost.org",
        "N0CALL",
        &radiopost::cbor::hex(&[1; 32]),
        NOW,
        1,
        3,
    )
    .expect("issued");
    po.save(&path).expect("saved");
    let back = PostOffice::load(&path).expect("loaded");
    assert_eq!(
        (back.public(), back.next_serial, back.not_after),
        (po.public(), 2, po.not_after)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
