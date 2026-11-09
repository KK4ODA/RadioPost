// SPDX-License-Identifier: MIT OR Apache-2.0
//! Two clients, station to station (RP-3 §10), through the loopback adapter (`rpmi`) over TCP.
//!
//! The identities are the vectors' (`keys.json`, `credentials.json`): Facundo (KK4ODA, of
//! radiopost.org) and Ana (N0CALL, of hampost.org), whose post offices' test keys each trusts.

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use radiopost::message::Message;
use radiopost_client::session::{self, Fetch, Kind, Options, Packing};
use radiopost_client::store::{Store, unhex};
use rpmi::channel::{Channel, StationConfig};
use rpmi::tcp::Server;
use rpmi::wire::Caps;
use serde_json::Value;

fn vectors(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vectors")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).expect("a vector file")).expect("JSON")
}

fn key_entry(label: &str) -> Value {
    vectors("keys.json")["keys"]
        .as_array()
        .expect("keys")
        .iter()
        .find(|k| k["label"] == label)
        .unwrap_or_else(|| panic!("no key {label}"))
        .clone()
}

fn credential_hex(name: &str) -> String {
    vectors("credentials.json")["credentials"]
        .as_array()
        .expect("credentials")
        .iter()
        .find(|c| c["name"] == name)
        .and_then(|c| c["credential_hex"].as_str())
        .expect("a credential")
        .to_owned()
}

fn message(name: &str) -> Message {
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
fn post_office_keys() -> Vec<(String, String, u64, u64)> {
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

fn loopback() -> (Server, SocketAddr, SocketAddr) {
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
    };
    let station = |name: &str, prefix: &str| StationConfig {
        name: name.into(),
        modem: "the loopback adapter".into(),
        caps: caps.clone(),
        prefix: prefix.into(),
        frame_bytes: 128,
    };
    let mut ch = Channel::new();
    let f = ch.add_station(station("KK4ODA", "c"));
    let a = ch.add_station(station("N0CALL", "a"));
    let server = Server::new(ch, Duration::from_secs(10));
    let at_f = server.listen(f, "127.0.0.1:0").expect("a port");
    let at_a = server.listen(a, "127.0.0.1:0").expect("a port");
    (server, at_f, at_a)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("radiopost-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    dir
}

/// A store set up as `radiopost init` and `trust` would.
fn store_for(home: &Path, device: &str, credential: &str) -> Store {
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

/// Session-03's clocks and draws, as the program's replay options (the tester's §2.4).
fn replay_options(file: &Value, at_f: SocketAddr, at_a: SocketAddr) -> (Options, Options) {
    let context = &file["context"];
    let first: Vec<u8> =
        unhex(file["transmissions"][0]["bytes_hex"].as_str().expect("hex")).expect("bytes");
    let hello = radiopost::pdu::unframe(&first).expect("HELLO");
    let nonce: [u8; 8] = hello
        .get(7)
        .and_then(radiopost::cbor::Value::as_bytes)
        .and_then(|n| n.try_into().ok())
        .expect("a nonce");
    let mut ana_opts = Options::new(&at_a.to_string());
    ana_opts.clock = context["called_clock"].as_u64();
    ana_opts.session_id =
        unhex(context["session_id_hex"].as_str().expect("hex")).and_then(|b| b.try_into().ok());
    ana_opts.challenge = Some([7; 16]);
    ana_opts.packing = Packing::None;
    let mut facundo_opts = Options::new(&at_f.to_string());
    facundo_opts.clock = context["caller_clock"].as_u64();
    facundo_opts.nonce = Some(nonce);
    facundo_opts.packing = Packing::None;
    facundo_opts.fetch = Fetch::Nothing;
    (facundo_opts, ana_opts)
}

/// The conformance tester's replay (its §2.4): with session-03's clocks and draws fixed, the two
/// clients write session-03 byte for byte, and their folders end as the session says.
#[test]
fn two_clients_replay_session_03_exactly() {
    let file = vectors("session-03.json");
    let context = &file["context"];
    let dir = scratch("replay");
    let mut facundo = store_for(&dir.join("facundo"), "device:facundo", "facundo");
    let mut ana = store_for(&dir.join("ana"), "device:ana", "ana");
    for name in ["facundo-direct", "facundo-to-eoc"] {
        facundo.queue(&message(name)).expect("queued");
    }
    ana.queue(&message("ana-to-facundo")).expect("queued");

    let (_server, at_f, at_a) = loopback();
    let (facundo_opts, ana_opts) = replay_options(&file, at_f, at_a);

    let (tx, rx) = std::sync::mpsc::channel();
    let ana_runs = thread::spawn(move || {
        let mut say = |s: &str| {
            if s.starts_with("listening") {
                let _ = tx.send(());
            }
        };
        let report = session::answer(&mut ana, &ana_opts, &mut say).expect("answered");
        (ana, report)
    });
    // the loopback adapter answers only a host already listening
    rx.recv_timeout(Duration::from_secs(10))
        .expect("Ana's station listens");
    let mut said = Vec::new();
    let called = session::call(
        &mut facundo,
        "N0CALL",
        Kind::Station,
        &facundo_opts,
        &mut |s| {
            said.push(s.to_owned());
        },
    )
    .expect("called");
    let (ana, answered) = ana_runs.join().expect("Ana's side");
    assert_eq!(called.failed, None, "{said:?}");

    let mut c = called.transmissions.iter();
    let mut a = answered.transmissions.iter();
    for (i, t) in file["transmissions"]
        .as_array()
        .expect("t")
        .iter()
        .enumerate()
    {
        let ours = if t["from"] == "caller" {
            c.next()
        } else {
            a.next()
        };
        let want = unhex(t["bytes_hex"].as_str().expect("hex")).expect("bytes");
        assert_eq!(ours, Some(&want), "transmission {}", i + 1);
    }
    assert_eq!((c.next(), a.next()), (None, None), "nothing more");

    let id = |name: &str| message(name).content_id();
    assert_eq!(
        facundo.inbox.iter().map(|a| a.id).collect::<Vec<_>>(),
        [id("ana-to-facundo")]
    );
    // taken by Ana's station, the message reaches Maria, copied, when the session is reported
    // (D30), not over the air again; the one for the EOC was not Ana's to take
    assert_eq!(facundo.sent, [id("facundo-direct")]);
    assert_eq!(
        facundo.outbox.iter().map(|w| w.id).collect::<Vec<_>>(),
        [id("facundo-to-eoc")],
        "mail for others stays"
    );
    // each keeps the session's report, as direct.json has it (RP-5 §6.6)
    let direct = vectors("direct.json");
    let report = |what: &str| {
        direct["cases"]
            .as_array()
            .expect("cases")
            .iter()
            .find(|c| c["what"] == what)
            .map(|c| c["session"].clone())
            .expect("a case")
    };
    assert_eq!(facundo.reports, [report("as Facundo reports it")]);
    assert_eq!(
        ana.reports,
        [report("Ana's own report, sent to radiopost.org")]
    );
    assert_eq!(
        ana.inbox.iter().map(|a| a.id).collect::<Vec<_>>(),
        [id("facundo-direct")]
    );
    assert_eq!(ana.sent, [id("ana-to-facundo")]);
    assert_eq!(
        ana.replay_cache(context["called_clock"].as_u64().expect("clock"))
            .into_iter()
            .map(|(address, _)| address)
            .collect::<Vec<_>>(),
        ["facundo@radiopost.org"],
        "the statement in the replay cache"
    );
    // what was kept is what a new run reads back
    let again = Store::open(&dir.join("facundo")).expect("reopened");
    assert_eq!(again.inbox, facundo.inbox);
    assert_eq!(again.outbox, facundo.outbox);
    let _ = std::fs::remove_dir_all(&dir);
}

fn radiopost(home: &Path, args: &[&str], stdin: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_radiopost"))
        .arg("--home")
        .arg(home)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("radiopost runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("written");
    let out = child.wait_with_output().expect("it ends");
    assert!(
        out.status.success(),
        "radiopost {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("UTF-8")
}

fn setup(home: &Path, device: &str, credential: &str) {
    let dir = home.parent().expect("a parent");
    let key = dir.join(format!("{device}.key").replace(':', "-"));
    std::fs::write(&key, key_entry(device)["seed_hex"].as_str().expect("seed")).expect("written");
    let cred = dir.join(format!("{credential}.cred"));
    std::fs::write(&cred, credential_hex(credential)).expect("written");
    radiopost(
        home,
        &[
            "init",
            "--device-key",
            key.to_str().expect("a path"),
            "--credential",
            cred.to_str().expect("a path"),
        ],
        "",
    );
    for (domain, public, nb, na) in post_office_keys() {
        radiopost(
            home,
            &["trust", &domain, &public, &nb.to_string(), &na.to_string()],
            "",
        );
    }
}

/// The program as a user runs it: each writes to the other, Ana listens, Facundo calls her
/// station, and each finds the other's message in the inbox and its own among the sent.
#[test]
fn two_clients_exchange_mail_with_the_program() {
    let dir = scratch("cli");
    let (facundo, ana) = (dir.join("facundo"), dir.join("ana"));
    setup(&facundo, "device:facundo", "facundo");
    setup(&ana, "device:ana", "ana");
    assert_eq!(
        radiopost(&facundo, &["whoami"], "")
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>(),
        ["facundo@radiopost.org", "(KK4ODA),"]
    );
    let to_ana = radiopost(
        &facundo,
        &[
            "compose",
            "--to",
            "ana@hampost.org",
            "--subject",
            "Hola Ana",
        ],
        "We are fine here.\r\nFacundo\r\n",
    );
    let to_facundo = radiopost(
        &ana,
        &[
            "compose",
            "--to",
            "facundo@radiopost.org",
            "--subject",
            "Re: hola",
        ],
        "Glad to hear it.\n",
    );
    let (_server, at_f, at_a) = loopback();
    let mut listener = Command::new(env!("CARGO_BIN_EXE_radiopost"))
        .arg("--home")
        .arg(&ana)
        .args(["listen", "--adapter", &at_a.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("radiopost listens");
    let mut lines = BufReader::new(listener.stdout.take().expect("stdout")).lines();
    let first = lines.next().expect("a line").expect("text");
    assert!(first.starts_with("listening as N0CALL"), "{first}");
    let call = radiopost(
        &facundo,
        &[
            "call",
            "n0call",
            "--station",
            "--adapter",
            &at_f.to_string(),
        ],
        "",
    );
    assert!(
        call.contains("the other station is N0CALL (ana@hampost.org)"),
        "{call}"
    );
    assert!(listener.wait().expect("it ends").success());
    let heard: Vec<String> = lines.map_while(Result::ok).collect();
    assert!(
        heard
            .iter()
            .any(|l| l.contains("is KK4ODA (facundo@radiopost.org)")),
        "{heard:?}"
    );

    let short = |id: &str| id.trim()[..6].to_owned();
    let inbox = radiopost(&ana, &["inbox"], "");
    assert!(
        inbox.starts_with(&short(&to_ana)) && inbox.contains("via KK4ODA"),
        "{inbox}"
    );
    let inbox = radiopost(&facundo, &["inbox"], "");
    assert!(inbox.starts_with(&short(&to_facundo)), "{inbox}");
    assert!(radiopost(&facundo, &["sent"], "").starts_with(&short(&to_ana)));
    assert!(radiopost(&ana, &["sent"], "").starts_with(&short(&to_facundo)));
    assert_eq!(radiopost(&facundo, &["outbox"], ""), "");
    let shown = radiopost(&ana, &["show", &short(&to_ana)], "");
    assert!(
        shown.contains("Subject: Hola Ana") && shown.ends_with("We are fine here.\nFacundo\n\n"),
        "{shown:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_program_refuses_what_it_should() {
    let dir = scratch("refuses");
    let home = dir.join("someone");
    let key = radiopost(&home, &["key"], "");
    assert_eq!(key.trim().len(), 64, "a public key in hex");
    assert_eq!(radiopost(&home, &["key"], ""), key, "made once");
    // Facundo's credential is not for this device's key
    let cred = dir.join("facundo.cred");
    std::fs::write(&cred, credential_hex("facundo")).expect("written");
    let out = Command::new(env!("CARGO_BIN_EXE_radiopost"))
        .arg("--home")
        .arg(&home)
        .args(["init", "--credential", cred.to_str().expect("a path")])
        .output()
        .expect("it runs");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("another device's key"));
    let _ = std::fs::remove_dir_all(&dir);
}
