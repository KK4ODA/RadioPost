// SPDX-License-Identifier: MIT OR Apache-2.0
//! Two clients, station to station (RP-3 §10), through the loopback adapter (`rpmi`) over TCP.
//!
//! The identities are the vectors' (`keys.json`, `credentials.json`): Facundo (KK4ODA, of
//! radiopost.org) and Ana (N0CALL, of hampost.org), whose post offices' test keys each trusts.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use radiopost_client::session::{self, Fetch, Kind, Options, Packing};
use radiopost_client::store::{Store, unhex};
use serde_json::Value;

use common::{
    credential_hex, key_entry, loopback, loopback_dialled, message, post_office_keys, scratch,
    store_for, vectors,
};

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

/// W46: a client with two callsigns listens as both. Its first is KK4ODA (Facundo's identity),
/// its second N0CALL (Ana's); a call to N0CALL is answered by the second's home, which takes the
/// mail for Ana, while the first's folders stay as they were.
#[test]
fn a_client_answers_a_call_to_any_of_its_callsigns() {
    let dir = scratch("any-callsign");
    let base = dir.join("listener");
    let mut first = store_for(&base, "device:facundo", "facundo");
    let second = radiopost_client::callsigns::home_of(&base, 2);
    store_for(&second, "device:ana", "ana");
    let mut facundo = store_for(&dir.join("caller"), "device:facundo", "facundo");
    facundo.queue(&message("facundo-direct")).expect("queued");

    let (_server, at_f, at_a) = loopback();
    let mut listen = Options::new(&at_a.to_string());
    listen.also = radiopost_client::callsigns::list(&base)
        .into_iter()
        .map(|c| c.home)
        .collect();
    let (tx, rx) = std::sync::mpsc::channel();
    let listener = thread::spawn(move || {
        let mut said = Vec::new();
        let report = session::answer(&mut first, &listen, &mut |s| {
            if s.starts_with("listening") {
                let _ = tx.send(s.to_owned());
            }
            said.push(s.to_owned());
        })
        .expect("answered");
        (first, report, said)
    });
    let listening = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the station listens");
    assert_eq!(
        listening, "listening as KK4ODA, N0CALL",
        "both callsigns claimed"
    );
    let mut said = Vec::new();
    let called = session::call(
        &mut facundo,
        "N0CALL",
        Kind::Station,
        &Options::new(&at_f.to_string()),
        &mut |s| said.push(s.to_owned()),
    )
    .expect("called");
    let (first, answered, heard) = listener.join().expect("the listener");
    assert_eq!(
        (called.failed, answered.failed),
        (None, None),
        "{said:?} {heard:?}"
    );
    assert!(
        heard.iter().any(|l| l.starts_with("answering as N0CALL")),
        "says which callsign answered: {heard:?}"
    );
    let id = message("facundo-direct").content_id();
    let ana = Store::open(&second).expect("the second home");
    assert_eq!(ana.inbox.iter().map(|a| a.id).collect::<Vec<_>>(), [id]);
    assert_eq!(first.inbox, [], "the first callsign's folders untouched");
    assert_eq!(facundo.sent, [id]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A call on a chosen channel (wishlist W14): with a modem that has a dial (RP-4 §4.1) the radio
/// goes there before the call and back to where it was after; with one that has none, the user is
/// told where to tune.
#[test]
fn a_call_on_a_channel_tunes_the_radio_there_and_back() {
    let dir = scratch("dial");
    let mut facundo = store_for(&dir.join("facundo"), "device:facundo", "facundo");
    let mut ana = store_for(&dir.join("ana"), "device:ana", "ana");
    facundo.queue(&message("facundo-direct")).expect("queued");
    for dial in [Some(7_101_000), None] {
        let (server, at_f, at_a) = loopback_dialled(dial);
        let (tx, rx) = std::sync::mpsc::channel();
        let ana_opts = Options::new(&at_a.to_string());
        let listener = thread::spawn(move || {
            let report = session::answer(&mut ana, &ana_opts, &mut |s: &str| {
                if s.starts_with("listening") {
                    let _ = tx.send(());
                }
            })
            .expect("answered");
            (ana, report)
        });
        rx.recv_timeout(Duration::from_secs(10))
            .expect("Ana's station listens");
        let mut opts = Options::new(&at_f.to_string());
        opts.dial_hz = Some(3_591_500);
        let mut said = Vec::new();
        let called = session::call(&mut facundo, "N0CALL", Kind::Station, &opts, &mut |s| {
            said.push(s.to_owned());
        })
        .expect("called");
        let answered;
        (ana, answered) = listener.join().expect("Ana's side");
        assert_eq!((called.failed, answered.failed), (None, None), "{said:?}");
        let at = |line: &str| said.iter().position(|s| s == line);
        if dial.is_some() {
            let tuned = at("tuned the radio to 3.5915 MHz").expect("tuned");
            let calling = at("calling N0CALL").expect("called");
            let back = at("tuned the radio back to 7.101 MHz").expect("back");
            assert!(tuned < calling && calling < back, "{said:?}");
            assert_eq!(server.with_channel(|ch| ch.dial(0)), Some(7_101_000));
        } else {
            assert_eq!(
                said.first().map(String::as_str),
                Some("set the radio's dial to 3.5915 MHz (USB) yourself: the modem cannot set it"),
                "{said:?}"
            );
            assert!(!said.iter().any(|s| s.starts_with("tuned")), "{said:?}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
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

    answers_with_the_program(&ana, &to_ana, &to_facundo);
    let _ = std::fs::remove_dir_all(&dir);
}

/// W1 with the program: Ana answers the message Facundo sent her (`to_ana`), passes it on, and
/// cannot reply to her own (`to_facundo`).
fn answers_with_the_program(ana: &Path, to_ana: &str, to_facundo: &str) {
    let short = |id: &str| id.trim()[..6].to_owned();
    // W1: Ana answers, and passes Facundo's message on
    let reply = radiopost(ana, &["compose", "--reply", &short(to_ana)], "Good.\n");
    let shown = radiopost(ana, &["show", &short(&reply)], "");
    assert!(
        shown.contains("To: facundo@radiopost.org\n")
            && shown.contains("Subject: Re: Hola Ana\n")
            && shown.ends_with("\n\nGood.\n\n"),
        "{shown:?}"
    );
    let store = Store::open(ana).expect("the home");
    let id = store.find(reply.trim()).expect("queued");
    let original = radiopost::ContentId::from_text(to_ana.trim()).expect("an ID");
    assert_eq!(
        store
            .message(&id)
            .expect("kept")
            .map()
            .get(radiopost::message::key::REPLY_TO)
            .and_then(radiopost::cbor::Value::as_bytes),
        Some(original.0.as_slice()),
        "in the thread"
    );
    let forward = radiopost(
        ana,
        &[
            "compose",
            "--forward",
            &short(to_ana),
            "--to",
            "maria@example.com",
        ],
        "From Facundo.\n",
    );
    let shown = radiopost(ana, &["show", &short(&forward)], "");
    assert!(
        shown.contains("To: maria@example.com\n")
            && shown.contains("Subject: Fwd: Hola Ana\n")
            && shown
                .contains("\n\nFrom Facundo.\n\nForwarded message\nFrom: facundo@radiopost.org\n")
            && shown.ends_with("Subject: Hola Ana\n\nWe are fine here.\nFacundo\n\n"),
        "{shown:?}"
    );
    // a message sent is forwarded, never replied to
    let out = Command::new(env!("CARGO_BIN_EXE_radiopost"))
        .arg("--home")
        .arg(ana)
        .args(["compose", "--reply", &short(to_facundo)])
        .stdin(Stdio::null())
        .output()
        .expect("it runs");
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("only a message received is replied to"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Aether HF named as the modem, and not running: said plainly.
#[test]
fn a_modem_not_running_is_said_plainly() {
    let dir = scratch("no-modem");
    let mut facundo = store_for(&dir.join("facundo"), "device:facundo", "facundo");
    // nothing listens on port 9 here: the adapter starts, and Aether cannot be reached
    let opts = Options::new("ws://127.0.0.1:9/v1");
    let e = session::call(&mut facundo, "N0CALL", Kind::Station, &opts, &mut |_| {})
        .expect_err("no modem");
    assert_eq!(
        e,
        "the modem at ws://127.0.0.1:9/v1 cannot be reached: is it running?"
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
