// SPDX-License-Identifier: MIT OR Apache-2.0
//! Updates against a stand-in update channel on loopback: a manifest, the tools archive and its
//! signature, made with a throwaway test key whose private half was discarded (`tests/data`).
//! The signature is checked as Tauri's updater checks it; a tampered archive, another key and a
//! build with no key are refused, and nothing is replaced.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use radiopost_update::{Product, Watch, check, install_tools, platform, replace_programs, verify};
use serde_json::json;

fn data(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name)
}

fn read(name: &str) -> Vec<u8> {
    std::fs::read(data(name)).expect("a fixture")
}

fn text(name: &str) -> String {
    String::from_utf8(read(name)).expect("text")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("radiopost-update-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    dir
}

/// Serves `files` by name on `server`, on a thread of its own.
fn serve(server: tiny_http::Server, files: Vec<(&'static str, Vec<u8>)>) {
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let name = request.url().trim_start_matches('/').to_owned();
            let response = match files.iter().find(|(n, _)| *n == name) {
                Some((_, body)) => tiny_http::Response::from_data(body.clone()),
                None => tiny_http::Response::from_data(b"not here".to_vec()).with_status_code(404),
            };
            let _ = request.respond(response);
        }
    });
}

/// A channel on loopback serving `files` by name; returns its base URL.
fn channel(files: Vec<(&'static str, Vec<u8>)>) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("a port");
    let at = server.server_addr().to_ip().expect("an address");
    serve(server, files);
    format!("http://{at}")
}

/// A channel offering `version` of the tools, this platform's file being `zip` signed `sig`.
fn tools_channel(version: &str, zip: Vec<u8>, sig: &str) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("a port");
    let base = format!(
        "http://{}",
        server.server_addr().to_ip().expect("an address")
    );
    let manifest = json!({
        "version": version, "notes": "Better.", "pub_date": "2026-11-11T00:00:00Z",
        "platforms": {platform(): {"url": format!("{base}/tools.zip"), "signature": sig}},
    });
    serve(
        server,
        vec![
            ("tools.json", manifest.to_string().into_bytes()),
            ("tools.zip", zip),
        ],
    );
    base
}

fn programs(dir: &Path) {
    std::fs::write(dir.join("radiopost-po"), "the old one").expect("written");
    std::fs::write(dir.join("unrelated"), "stays").expect("written");
}

#[test]
fn a_signature_is_checked_as_tauri_checks_it() {
    let zip = read("tools.zip");
    let sig = text("tools.zip.sig");
    let key = text("test.pub");
    assert_eq!(verify(&zip, &sig, &key), Ok(()));
    let mut tampered = zip.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    assert!(verify(&tampered, &sig, &key).is_err(), "tampered");
    // another key: the project's real one is not the test key
    let other = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IENEMDU2MDNEMUZDRDI4MzAKUldRd0tNMGZQV0FGemJ1cnhLNjJXRDVuZXJhTW1RTGE5RC9FdnN4eWVxLzFuOE9JSXlXSXJnQjYK";
    assert!(verify(&zip, &sig, other).is_err(), "another key");
    let none = verify(&zip, &sig, "").expect_err("no key");
    assert!(none.contains("no update key"), "{none}");
}

#[test]
fn the_channel_says_what_is_newer() {
    let base = tools_channel("0.1.0-beta.5", read("tools.zip"), &text("tools.zip.sig"));
    let offer = check(&base, Product::Tools, "0.1.0-beta.4")
        .expect("checked")
        .expect("newer");
    assert_eq!(offer.version, "0.1.0-beta.5");
    assert_eq!(offer.notes, "Better.");
    assert_eq!(
        offer.page,
        "https://github.com/KK4ODA/RadioPost/releases/tag/v0.1.0-beta.5"
    );
    assert!(offer.asset.is_some(), "this platform's file");
    assert_eq!(
        check(&base, Product::Tools, "0.1.0-beta.5"),
        Ok(None),
        "the same"
    );
    assert_eq!(check(&base, Product::Tools, "0.1.0"), Ok(None), "older");
    assert!(
        check(&base, Product::Client, "0.1.0-beta.4").is_err(),
        "no client.json here"
    );
    assert!(
        check("http://127.0.0.1:9", Product::Tools, "0.1.0").is_err(),
        "no channel"
    );
}

#[test]
fn the_tools_are_replaced_from_a_signed_archive() {
    let base = tools_channel("0.1.0-beta.5", read("tools.zip"), &text("tools.zip.sig"));
    let dir = scratch("install");
    programs(&dir);
    let said = install_tools(&base, "0.1.0-beta.4", &dir, &text("test.pub")).expect("installed");
    assert_eq!(
        said,
        "updated radiopost-po from 0.1.0-beta.4 to 0.1.0-beta.5"
    );
    let po = std::fs::read_to_string(dir.join("radiopost-po")).expect("read");
    assert!(po.contains("the new one"), "{po}");
    assert_eq!(
        std::fs::read_to_string(dir.join("unrelated")).expect("read"),
        "stays"
    );
    assert!(
        !dir.join("radiopost-gateway").exists(),
        "only what was there"
    );
    assert!(!dir.join("README.md").exists(), "only programs");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(dir.join("radiopost-po"))
            .expect("there")
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "executable");
    }
    // the newest already: nothing to do
    let said = install_tools(&base, "0.1.0-beta.5", &dir, &text("test.pub")).expect("checked");
    assert_eq!(said, "0.1.0-beta.5 is the newest");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_tampered_archive_or_another_key_replaces_nothing() {
    let mut tampered = read("tools.zip");
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    for (zip, key) in [
        (tampered, text("test.pub")),
        (read("tools.zip"), String::new()),
    ] {
        let base = tools_channel("0.1.0-beta.5", zip, &text("tools.zip.sig"));
        let dir = scratch("refused");
        programs(&dir);
        let refused = install_tools(&base, "0.1.0-beta.4", &dir, &key).expect_err("refused");
        assert!(
            refused.contains("signature") || refused.contains("no update key"),
            "{refused}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("radiopost-po")).expect("read"),
            "the old one"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// An older archive, signed by the same key, offered as a newer version: refused by the version
/// it says, so no one can turn a station back to a release with known faults.
#[test]
fn an_older_signed_archive_is_not_taken_for_a_newer() {
    let base = tools_channel(
        "0.1.0-beta.5",
        read("tools-old.zip"),
        &text("tools-old.zip.sig"),
    );
    let dir = scratch("older");
    programs(&dir);
    let refused =
        install_tools(&base, "0.1.0-beta.4", &dir, &text("test.pub")).expect_err("refused");
    assert!(
        refused.contains("is 0.1.0-beta.3, not the 0.1.0-beta.5"),
        "{refused}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("radiopost-po")).expect("read"),
        "the old one"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn replacing_leaves_alone_what_the_archive_does_not_hold() {
    let dir = scratch("replace");
    std::fs::write(dir.join("radiopost-tcp"), "not in the archive").expect("written");
    assert_eq!(replace_programs(&read("tools.zip"), &dir), Ok(Vec::new()));
    assert_eq!(
        std::fs::read_to_string(dir.join("radiopost-tcp")).expect("read"),
        "not in the archive"
    );
    assert!(replace_programs(b"not a zip", &dir).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// What a panel shows: the offer once checked, and an install by the application's own updater,
/// or by hand when the program has none.
#[test]
fn a_watch_shows_the_offer_and_installs_through_the_application() {
    let base = channel(vec![(
        "gateway.json",
        json!({"version": "0.1.0-beta.5", "notes": "Better.",
               "platforms": {platform(): {"url": "http://x/y", "signature": "z"}}})
        .to_string()
        .into_bytes(),
    )]);
    let watch = Watch::new(Product::Gateway, "0.1.0-beta.4", &base);
    assert_eq!(
        watch.status()["offer"],
        serde_json::Value::Null,
        "not checked yet"
    );
    watch.check();
    let status = watch.status();
    assert_eq!(status["offer"]["version"], "0.1.0-beta.5");
    assert_eq!(
        status["can_install"], false,
        "no installer: a download by hand"
    );
    let by_hand = watch.install().expect_err("by hand");
    assert!(by_hand.contains("/releases/tag/v0.1.0-beta.5"), "{by_hand}");
    let (tx, rx) = std::sync::mpsc::channel();
    watch.set_install(Arc::new(move || {
        let _ = tx.send(());
        Err("the installer stopped".into())
    }));
    assert_eq!(watch.status()["can_install"], true);
    watch.install().expect("started");
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .expect("the application's installer ran");
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while watch.status()["installing"] == true {
        assert!(std::time::Instant::now() < until, "still installing");
        thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(watch.status()["failed"], "the installer stopped");
    let up_to_date = Watch::new(Product::Gateway, "0.1.0-beta.5", &base);
    up_to_date.check();
    assert_eq!(up_to_date.status()["offer"], serde_json::Value::Null);
    assert!(up_to_date.install().is_err(), "nothing on offer");
}
