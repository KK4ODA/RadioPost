// SPDX-License-Identifier: MIT OR Apache-2.0
//! Listen stays on ([`session::keep_listening`]): on 2026-11-18 the house's client stopped
//! listening after one P2P session on 40 m, and the mobile's next calls went unanswered. A station
//! that keeps listening goes back to listening after every session, however it ends, and when its
//! modem's connection drops and comes back, until it is told to stop.

mod common;

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use radiopost_client::session::{self, Kind, Listening, Options};
use radiopost_client::store::Store;
use rpmi::host::{Event, Host};
use rpmi::tcp::Connection;

use common::{loopback, message, scratch, store_for};

/// What the listening station told, as plain words a test can wait for.
fn told(what: &Listening) -> String {
    match what {
        Listening::Waiting => "waiting".into(),
        Listening::Called(who) => format!("called by {who}"),
        Listening::Ended(Ok(r)) => match &r.failed {
            None => "ended".into(),
            Some(why) => format!("ended, failed: {why}"),
        },
        Listening::Ended(Err(why)) => format!("ended, broke: {why}"),
        Listening::Held(why) => format!("held: {why}"),
    }
}

/// Ana's station keeping on listening at `adapter`, on a thread of its own: what it tells, what it
/// says, and the switch that stops it.
struct Ana {
    home: PathBuf,
    told: Receiver<String>,
    said: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    runs: JoinHandle<()>,
}

impl Ana {
    fn listening(home: PathBuf, adapter: SocketAddr, stop_when_called: bool) -> Ana {
        let stop = Arc::new(AtomicBool::new(false));
        let mut opts = Options::new(&adapter.to_string());
        opts.stop = Some(Arc::clone(&stop));
        // a caller that says nothing is given up soon (RP-3 §8), so that a test sees it
        opts.quiet = Duration::from_secs(2);
        let (tx, told_rx) = channel();
        let said = Arc::new(Mutex::new(Vec::new()));
        let (lines, at, halt) = (Arc::clone(&said), home.clone(), Arc::clone(&stop));
        let runs = thread::spawn(move || {
            let mut say = |line: &str| lines.lock().expect("lines").push(line.to_owned());
            let mut tell = |what: Listening| {
                if stop_when_called && matches!(what, Listening::Called(_)) {
                    // Listen pressed again while a session runs
                    halt.store(true, Ordering::Relaxed);
                }
                let _ = tx.send(told(&what));
            };
            session::keep_listening(&at, &opts, &mut say, &mut tell);
            let _ = tx.send("returned".into());
        });
        Ana {
            home,
            told: told_rx,
            said,
            stop,
            runs,
        }
    }

    /// Waits for what the station tells to start with `what`, and gives it; what it told before
    /// is passed over.
    fn wait_for(&self, what: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let t = self
                .told
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("never told {what:?}: {:?}", self.said()));
            if t.starts_with(what) {
                return t;
            }
        }
    }

    fn said(&self) -> Vec<String> {
        self.said.lock().expect("lines").clone()
    }

    fn stop(self) -> Vec<String> {
        self.stop.store(true, Ordering::Relaxed);
        self.wait_for("returned");
        let said = self.said();
        self.runs.join().expect("Ana's station");
        said
    }
}

/// Facundo, at a home of his own in `dir`, calls N0CALL with `name` queued: the session completes.
fn facundo_calls(dir: &std::path::Path, at_f: SocketAddr, name: &str) {
    let mut facundo = store_for(
        &dir.join(format!("facundo-{name}")),
        "device:facundo",
        "facundo",
    );
    facundo.queue(&message(name)).expect("queued");
    let mut said = Vec::new();
    let called = session::call(
        &mut facundo,
        "N0CALL",
        Kind::Station,
        &Options::new(&at_f.to_string()),
        &mut |s| said.push(s.to_owned()),
    )
    .expect("called");
    assert_eq!(called.failed, None, "{said:?}");
}

/// A caller of its own on the adapter at `at_f`, speaking RP-4 by hand: its link to N0CALL up, and
/// the connection, to leave as it likes.
fn bare_call(at_f: SocketAddr) -> Connection {
    let mut conn = Connection::open(at_f).expect("the adapter");
    conn.set_timeout(Some(Duration::from_secs(10)))
        .expect("a wait");
    let mut host = Host::new();
    for request in [
        host.hello("a test"),
        host.callsigns(&["KK4ODA"]),
        host.connect("N0CALL", None, None),
    ] {
        conn.send(&request).expect("sent");
    }
    loop {
        let line = conn.read_message().expect("read").expect("a line");
        if let Ok(Event::Connected { .. }) = Event::read(&line) {
            return conn;
        }
    }
}

/// The house's station of 2026-11-18, kept listening: a session that completes, a caller who goes
/// away in the middle of one, and a caller who says nothing, so that the session breaks on the
/// listening side. After each the station listens again, and the next call is answered; pressed
/// again during a session, it stops once the session is over.
#[test]
fn listening_goes_on_after_every_session_however_it_ends() {
    let dir = scratch("keep-listening");
    store_for(&dir.join("ana"), "device:ana", "ana");
    let (_server, at_f, at_a) = loopback();
    let ana = Ana::listening(dir.join("ana"), at_a, false);
    ana.wait_for("waiting");

    // done
    facundo_calls(&dir, at_f, "facundo-direct");
    assert_eq!(ana.wait_for("ended"), "ended", "{:?}", ana.said());
    ana.wait_for("waiting");

    // the other station gone: its connection to its modem closes with the link up
    let gone = bare_call(at_f);
    ana.wait_for("called by KK4ODA");
    gone.close();
    ana.wait_for("ended");
    ana.wait_for("waiting");

    // broken on this side: the caller says nothing, and the session is given up (RP-3 §8)
    let silent = bare_call(at_f);
    let ended = ana.wait_for("ended");
    assert!(ended.starts_with("ended, broke"), "{ended}");
    ana.wait_for("waiting");
    silent.close();

    // still listening: the next call is answered, and its mail kept
    facundo_calls(&dir, at_f, "facundo-to-ana");
    assert_eq!(ana.wait_for("ended"), "ended", "{:?}", ana.said());
    ana.wait_for("waiting");
    let home = ana.home.clone();
    let said = ana.stop();
    let kept = Store::open(&home).expect("Ana's home");
    assert_eq!(
        kept.inbox.iter().map(|a| a.id).collect::<Vec<_>>(),
        [
            message("facundo-direct").content_id(),
            message("facundo-to-ana").content_id()
        ],
        "{said:?}"
    );
    assert_eq!(
        said.iter().filter(|l| *l == "listening again").count(),
        3,
        "after each session that left the connection up: {said:?}"
    );
    assert_eq!(said.last().map(String::as_str), Some("stopped listening"));

    // pressed again while a session runs: the session goes on to its end, then listening stops
    let again = dir.join("again");
    store_for(&again.join("ana"), "device:ana", "ana");
    let (_server, at_f, at_a) = loopback();
    let ana = Ana::listening(again.join("ana"), at_a, true);
    ana.wait_for("waiting");
    facundo_calls(&again, at_f, "facundo-direct");
    assert_eq!(ana.wait_for("ended"), "ended", "{:?}", ana.said());
    ana.wait_for("returned");
    let said = ana.said();
    assert!(!said.iter().any(|l| l == "listening again"), "{said:?}");
    assert_eq!(said.last().map(String::as_str), Some("stopped listening"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A stand-in for the line between the client and its modem's adapter, which can be cut and
/// mended: cut, it closes the connections it carries and every new one at once, as Aether's
/// adapter does when Aether restarts.
struct Line {
    at: SocketAddr,
    up: Arc<AtomicBool>,
    carried: Arc<Mutex<Vec<TcpStream>>>,
}

impl Line {
    fn to(adapter: SocketAddr) -> Line {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let at = listener.local_addr().expect("an address");
        let up = Arc::new(AtomicBool::new(true));
        let carried: Arc<Mutex<Vec<TcpStream>>> = Arc::default();
        let (is_up, all) = (Arc::clone(&up), Arc::clone(&carried));
        thread::spawn(move || {
            for client in listener.incoming() {
                let Ok(client) = client else { continue };
                if !is_up.load(Ordering::Relaxed) {
                    let _ = client.shutdown(Shutdown::Both);
                    continue;
                }
                let Ok(modem) = TcpStream::connect(adapter) else {
                    continue;
                };
                let mut kept = all.lock().expect("kept");
                for (from, to) in [(&client, &modem), (&modem, &client)] {
                    let (Ok(mut from), Ok(mut to)) = (from.try_clone(), to.try_clone()) else {
                        continue;
                    };
                    thread::spawn(move || {
                        let mut buf = [0u8; 4096];
                        while let Ok(n) = from.read(&mut buf) {
                            if n == 0 || to.write_all(&buf[..n]).is_err() {
                                break;
                            }
                        }
                        let _ = to.shutdown(Shutdown::Both);
                    });
                }
                kept.extend([client, modem]);
            }
        });
        Line { at, up, carried }
    }

    fn cut(&self) {
        self.up.store(false, Ordering::Relaxed);
        for s in self.carried.lock().expect("kept").drain(..) {
            let _ = s.shutdown(Shutdown::Both);
        }
    }

    fn mend(&self) {
        self.up.store(true, Ordering::Relaxed);
    }
}

/// The modem's connection drops while the station listens (Aether restarting, as at the house at
/// 18:10Z that day, when listening ended for good): it is tried again until it comes back, and the
/// next call is answered.
#[test]
fn listening_comes_back_with_the_modem() {
    let dir = scratch("listening-modem-back");
    store_for(&dir.join("ana"), "device:ana", "ana");
    let (_server, at_f, at_a) = loopback();
    let line = Line::to(at_a);
    let ana = Ana::listening(dir.join("ana"), line.at, false);
    ana.wait_for("waiting");
    line.cut();
    ana.wait_for("held");
    // tried again while the line is cut, and held again
    ana.wait_for("held");
    line.mend();
    ana.wait_for("waiting");
    facundo_calls(&dir, at_f, "facundo-direct");
    assert_eq!(ana.wait_for("ended"), "ended", "{:?}", ana.said());
    let said = ana.stop();
    assert!(
        said.iter()
            .any(|l| l.starts_with("the modem's connection ended")),
        "{said:?}"
    );
    let cannot: Vec<&String> = said
        .iter()
        .filter(|l| l.starts_with("cannot listen"))
        .collect();
    assert_eq!(cannot.len(), 1, "said once, however often tried: {said:?}");
    assert_eq!(
        said.iter()
            .filter(|l| l.starts_with("listening as"))
            .count(),
        2,
        "claimed again when the modem came back: {said:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
