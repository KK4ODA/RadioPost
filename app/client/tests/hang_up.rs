// SPDX-License-Identifier: MIT OR Apache-2.0
//! The operator ends a call or a session before its time: the panel's **Stop calling**,
//! **Disconnect** and **Abort** ([`session::HangUp`]). Until 2026-11-23 the only way was closing
//! the program, which stopped the session's thread where it stood: a message received in part was
//! not kept to resume, and the call's record said nothing of whose end it was.
//!
//! Each end goes to the modem as RP-4 says (`abort` for a call, `disconnect` or `abort` for a
//! link), the call's record classes it as the operator's (choosing a gateway, §12.3, which learns
//! nothing from it), and what was received in part is kept (RP-3 §7).

mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use radiopost_client::compose::Draft;
use radiopost_client::selection::attempts;
use radiopost_client::session::{self, HangUp, Kind, Listening, Options, Packing, Report, Stop};
use radiopost_client::store::Store;
use rpmi::host::{Event, Host};
use rpmi::tcp::Connection;
use serde_json::Value;

use common::{loopback, loopback_with, message, scratch, store_for};

/// What a call said as it went.
type Said = Arc<Mutex<Vec<String>>>;

/// Facundo's call to `remote` on its own thread, his word in `opts`: what it came to, and what it
/// said as it went.
fn calling(home: &Path, remote: &str, opts: Options) -> (Receiver<Result<Report, String>>, Said) {
    let (tx, rx) = channel();
    let said = Arc::new(Mutex::new(Vec::new()));
    let (lines, home, remote) = (Arc::clone(&said), home.to_owned(), remote.to_owned());
    thread::spawn(move || {
        let mut say = |line: &str| lines.lock().expect("lines").push(line.to_owned());
        let result = Store::open(&home).and_then(|mut store| {
            session::call(&mut store, &remote, Kind::Station, &opts, &mut say)
        });
        let _ = tx.send(result);
    });
    (rx, said)
}

/// Waits until a line starting with `start` has been said.
fn wait_said(said: &Said, start: &str) {
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        if said
            .lock()
            .expect("lines")
            .iter()
            .any(|l| l.starts_with(start))
        {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("never said {start:?}: {:?}", said.lock().expect("lines"));
}

/// The class the last call's record was given, by the rules of today (choosing a gateway, §12.3).
fn last_class(home: &Path) -> &'static str {
    let records = attempts::read(home);
    let last = records.last().expect("a call recorded");
    radiopost_client::selection::classify::class_of(last).name()
}

/// Options for a call at `adapter`, the operator's word `word`.
fn options(adapter: SocketAddr, word: &HangUp) -> Options {
    let mut opts = Options::new(&adapter.to_string());
    opts.hang_up = Some(word.clone());
    opts
}

/// Stop calling: the modem's `abort` ends the call nobody answers, at once, and it is the
/// operator's end, not a call that found nobody.
#[test]
fn stopping_a_call_ends_it_at_once_as_the_operators() {
    let dir = scratch("hang-up-call");
    let home = dir.join("facundo");
    store_for(&home, "device:facundo", "facundo");
    // nobody listens as N0CALL, and the modems would give up only after a minute
    let (_server, at_f, _) = loopback_with(None, Duration::from_secs(60));
    let word = HangUp::new();
    let (ends, said) = calling(&home, "N0CALL", options(at_f, &word));
    wait_said(&said, "calling N0CALL");
    let asked = Instant::now();
    word.ask(Stop::Abort);
    let report = ends
        .recv_timeout(Duration::from_secs(5))
        .expect("the call ends at once")
        .expect("a call stopped is no error");
    assert!(asked.elapsed() < Duration::from_secs(2), "{asked:?}");
    assert!(report.stopped);
    assert_eq!(report.failed.as_deref(), Some("you stopped the call"));
    let said = said.lock().expect("lines").clone();
    assert!(
        said.iter().any(|l| l == "stopping the call to N0CALL"),
        "{said:?}"
    );
    assert_eq!(last_class(&home), "cancelled", "{said:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A station of its own as N0CALL, speaking RP-4 by hand, listening: it answers the call and then
/// says nothing, so the session stands still until someone ends it.
fn silent_station(at_a: SocketAddr) -> Connection {
    let mut conn = Connection::open(at_a).expect("the adapter");
    conn.set_timeout(Some(Duration::from_secs(10)))
        .expect("a wait");
    let mut host = Host::new();
    for request in [
        host.hello("a test"),
        host.callsigns(&["N0CALL"]),
        host.listen(true),
    ] {
        conn.send(&request).expect("sent");
    }
    conn
}

/// Reads `conn` until a `disconnected`, and gives its reason.
fn disconnected(conn: &mut Connection) -> String {
    loop {
        let line = conn.read_message().expect("read").expect("a line");
        if let Ok(Event::Disconnected { reason, .. }) = Event::read(&line) {
            return reason;
        }
    }
}

/// Disconnect: the modem is asked to `disconnect` the link, the other station hears it end, what
/// waited to go stays in the Outbox, and the end is the operator's.
#[test]
fn disconnect_ends_a_session_in_order() {
    let dir = scratch("hang-up-disconnect");
    let home = dir.join("facundo");
    let mut facundo = store_for(&home, "device:facundo", "facundo");
    facundo.queue(&message("facundo-direct")).expect("queued");
    drop(facundo);
    let (_server, at_f, at_a) = loopback();
    let mut other = silent_station(at_a);
    let word = HangUp::new();
    let (ends, said) = calling(&home, "N0CALL", options(at_f, &word));
    wait_said(&said, "connected to N0CALL");
    word.ask(Stop::Disconnect);
    let report = ends
        .recv_timeout(Duration::from_secs(5))
        .expect("the session ends")
        .expect("a session the operator ended is no error");
    assert!(report.stopped);
    assert_eq!(report.failed.as_deref(), Some("you disconnected"));
    assert_eq!(
        disconnected(&mut other),
        "remote",
        "the other station hears it end"
    );
    let said = said.lock().expect("lines").clone();
    assert!(
        said.iter()
            .any(|l| l.starts_with("disconnecting from N0CALL")),
        "{said:?}"
    );
    let kept = Store::open(&home).expect("Facundo's home");
    assert_eq!(kept.outbox.len(), 1, "nothing crossed: {said:?}");
    assert_eq!(last_class(&home), "cancelled", "{said:?}");
    other.close();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A stand-in for the line from the adapter to a client that passes the data received only up to
/// `bytes`, as a link that fades in the middle of a message, and everything else as it is. It says
/// on the channel it gives when it first holds data back.
fn fading_line(adapter: SocketAddr, bytes: usize) -> (SocketAddr, Receiver<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let at = listener.local_addr().expect("its address");
    let (tx, faded) = channel();
    thread::spawn(move || {
        for client in listener.incoming() {
            let Ok(client) = client else { return };
            let Ok(upstream) = TcpStream::connect(adapter) else {
                return;
            };
            let (mut from_client, mut to_adapter) = (
                client.try_clone().expect("a clone"),
                upstream.try_clone().expect("a clone"),
            );
            thread::spawn(move || {
                let _ = std::io::copy(&mut from_client, &mut to_adapter);
                let _ = to_adapter.shutdown(Shutdown::Write);
            });
            let tx = tx.clone();
            thread::spawn(move || {
                let (mut to_client, mut passed, mut told) = (client, 0, false);
                for line in BufReader::new(upstream).lines() {
                    let Ok(line) = line else { break };
                    let v: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                    if v["op"] == "received" {
                        let n = v["data"]
                            .as_str()
                            .and_then(rpmi::base64::decode)
                            .map_or(0, |d| d.len());
                        if passed + n > bytes {
                            if !told {
                                let _ = tx.send(());
                                told = true;
                            }
                            continue;
                        }
                        passed += n;
                    }
                    if writeln!(to_client, "{line}").is_err() {
                        break;
                    }
                }
                let _ = to_client.shutdown(Shutdown::Both);
            });
        }
    });
    (at, faded)
}

/// Text that does not compress, so a message of it goes in as many pieces as its length says.
fn noise(len: usize) -> String {
    let mut x: u64 = 0x2545_f491_4f6c_dd1d;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            char::from(b'a' + u8::try_from(x % 26).expect("a letter"))
        })
        .collect()
}

/// Ana's station listening at `at_a` for her home `home`, with `stop` to end it: what it tells.
fn ana_listening(
    home: PathBuf,
    at_a: SocketAddr,
    stop: &Arc<AtomicBool>,
    word: Option<&HangUp>,
) -> Receiver<String> {
    let mut opts = Options::new(&at_a.to_string());
    opts.stop = Some(Arc::clone(stop));
    opts.hang_up = word.cloned();
    // its message goes as it is, so its pieces are what the test counts on
    opts.packing = Packing::None;
    opts.quiet = Duration::from_secs(30);
    let (tx, told) = channel();
    thread::spawn(move || {
        let mut tell = |what: Listening| {
            let _ = tx.send(match what {
                Listening::Waiting => "waiting".to_owned(),
                Listening::Called(who) => format!("called by {who}"),
                Listening::Ended(Ok(r)) => match r.failed {
                    None => "ended".into(),
                    Some(why) => format!("ended: {why}"),
                },
                Listening::Ended(Err(why)) => format!("broke: {why}"),
                Listening::Held(why) => format!("held: {why}"),
            });
        };
        session::keep_listening(&home, &opts, &mut |_| {}, &mut tell);
        let _ = tx.send("returned".into());
    });
    told
}

/// Waits for what a listening station tells to start with `start`, and gives it.
fn wait_told(told: &Receiver<String>, start: &str) -> String {
    loop {
        let line = told
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("never told {start:?}"));
        if line.starts_with(start) {
            return line;
        }
    }
}

/// Abort in the middle of a message coming in: the link ends at once, and what came of the message
/// is kept to offer under `partials` on the next call (RP-3 §7), as when a link breaks.
#[test]
fn an_abort_keeps_a_message_received_in_part() {
    let dir = scratch("hang-up-abort");
    let (ana_home, facundo_home) = (dir.join("ana"), dir.join("facundo"));
    let mut ana = store_for(&ana_home, "device:ana", "ana");
    let facundo = store_for(&facundo_home, "device:facundo", "facundo");
    let big = Draft {
        from: ana.me().expect("Ana").address,
        to: vec![facundo.me().expect("Facundo").address],
        cc: Vec::new(),
        date: 1_790_000_000,
        subject: "The repeater survey".into(),
        body: noise(20_000),
        files: Vec::new(),
        priority: 0,
        reply_to: None,
        form: None,
    }
    .message()
    .expect("a message");
    let total = big.air_form(radiopost::message::Compression::None).len();
    ana.queue(&big).expect("queued");
    drop((ana, facundo));
    let (_server, at_f, at_a) = loopback();
    let stop = Arc::new(AtomicBool::new(false));
    let told = ana_listening(ana_home, at_a, &stop, None);
    wait_told(&told, "waiting");
    // past Ana's WELCOME and the message's first piece, the link fades
    let (line, faded) = fading_line(at_f, 9000);
    let word = HangUp::new();
    let (ends, said) = calling(&facundo_home, "N0CALL", options(line, &word));
    faded
        .recv_timeout(Duration::from_secs(10))
        .expect("the message comes in");
    // what came is taken in before the word is said
    thread::sleep(Duration::from_millis(300));
    word.ask(Stop::Abort);
    let report = ends
        .recv_timeout(Duration::from_secs(5))
        .expect("the session ends at once")
        .expect("a session the operator ended is no error");
    assert!(report.stopped);
    assert_eq!(report.failed.as_deref(), Some("you aborted the session"));
    let said = said.lock().expect("lines").clone();
    let kept = Store::open(&facundo_home).expect("Facundo's home");
    let partials = kept.partials();
    assert_eq!(partials.len(), 1, "{said:?}");
    let held = partials[0].held.len();
    assert!(held > 0 && held < total, "{held} of {total}: {said:?}");
    assert!(kept.inbox.is_empty(), "{said:?}");
    assert_eq!(last_class(&facundo_home), "cancelled", "{said:?}");
    // Ana's side sees the link end, and listens on
    wait_told(&told, "ended");
    wait_told(&told, "waiting");
    stop.store(true, Ordering::Relaxed);
    wait_told(&told, "returned");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A listening station's operator ends one session: the station listens on, and the word belongs
/// to that session alone, so the next call is carried to its end.
#[test]
fn a_listening_station_ends_one_session_and_listens_on() {
    let dir = scratch("hang-up-listening");
    let ana_home = dir.join("ana");
    store_for(&ana_home, "device:ana", "ana");
    let (_server, at_f, at_a) = loopback();
    let stop = Arc::new(AtomicBool::new(false));
    let word = HangUp::new();
    let told = ana_listening(ana_home.clone(), at_a, &stop, Some(&word));
    wait_told(&told, "waiting");

    // a caller that says nothing: Ana's operator disconnects
    let mut silent = Connection::open(at_f).expect("the adapter");
    silent
        .set_timeout(Some(Duration::from_secs(10)))
        .expect("a wait");
    let mut host = Host::new();
    for request in [
        host.hello("a test"),
        host.callsigns(&["KK4ODA"]),
        host.connect("N0CALL", None, None),
    ] {
        silent.send(&request).expect("sent");
    }
    wait_told(&told, "called by KK4ODA");
    word.ask(Stop::Disconnect);
    assert_eq!(wait_told(&told, "ended"), "ended: you disconnected");
    assert_eq!(disconnected(&mut silent), "remote");
    silent.close();
    wait_told(&told, "waiting");

    // the next call: the word was for the last session, and this one is carried to its end
    let facundo_home = dir.join("facundo");
    let mut facundo = store_for(&facundo_home, "device:facundo", "facundo");
    facundo.queue(&message("facundo-to-ana")).expect("queued");
    let report = session::call(
        &mut facundo,
        "N0CALL",
        Kind::Station,
        &Options::new(&at_f.to_string()),
        &mut |_| {},
    )
    .expect("called");
    assert_eq!(report.failed, None);
    assert_eq!(wait_told(&told, "ended"), "ended");
    stop.store(true, Ordering::Relaxed);
    wait_told(&told, "returned");
    let kept = Store::open(&ana_home).expect("Ana's home");
    assert_eq!(kept.inbox.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}
