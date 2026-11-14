// SPDX-License-Identifier: MIT OR Apache-2.0
//! The app on its modem between calls: while the panel runs and its modem is Aether, one
//! connection to the Aether adapter that says `hello` and nothing more (RP-4 §3, §9.1). Aether then
//! names the app among the programs using the station (its ADR-0114) whether it is idle, calling
//! or listening. The name is the one its calls claim under ([`crate::session::app_name`]), so
//! Aether shows the two connections as one program.
//!
//! * **No callsign.** This connection claims none. Under Aether's ADR-0108 any claim makes a
//!   program speak for the station, and the station's own callsigns would go unanswered while the
//!   app is merely open.
//! * **Beside the calls, never in their way.** A call and listening keep connections of their own
//!   ([`crate::session`]), as before. This one runs on a thread of its own and holds nothing they
//!   wait for.
//! * **Only Aether.** An adapter that is not shared would turn the app's own call away while this
//!   connection stays open (RP-4 §3), and only Aether names the programs it serves. So another
//!   modem's address holds no connection, nor does an Aether without claims (`caps.shared` false),
//!   nor a home in conformance mode, which reaches only the test adapter.
//! * **Quiet.** Aether not running, restarting, or closing the connection: it is tried again every
//!   few seconds. The log file says once when Aether names the app and once when that stops, with
//!   why; the panel says nothing.
//! * **Gone with the app.** [`Presence::stop`] closes it, and the process's end does too: Aether
//!   then says the program has gone.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use rpmi::host::{Event, Host};
use rpmi::tcp::Connection;

use crate::{callsigns, session, store};

/// How long after a try that failed, or a connection lost, the modem is tried again.
pub const RETRY: Duration = Duration::from_secs(5);
/// How long an Aether without claims is left before it is asked again: it names no programs
/// until it is updated, which restarts it.
const NOT_SHARED: Duration = Duration::from_secs(60);
/// How long a read of the adapter waits before the settings are looked at again.
const TICK: Duration = Duration::from_millis(250);
/// How long the adapter has to answer `hello`, which it answers once Aether has answered it.
const ANSWER: Duration = Duration::from_secs(10);
/// How often, while connected, the home is checked for conformance mode.
const RECHECK: Duration = Duration::from_secs(5);
/// How long [`Presence::stop`] waits for the connection to close.
const CLOSING: Duration = Duration::from_secs(2);
/// How long [`Presence::stop`] then gives the adapter, in this process, to close Aether's WebSocket
/// with a closing handshake: it sees the connection end within its 10 ms poll. Without it the
/// program exits first, and Aether reads the end as a connection reset.
const SETTLE: Duration = Duration::from_millis(300);

/// The app's connection to its modem between calls. Clones share it.
#[derive(Clone)]
pub struct Presence {
    shared: Arc<Shared>,
    retry: Duration,
}

struct Shared {
    /// The name the `hello` gives.
    app: String,
    /// The first callsign's home, which says which callsign is chosen.
    base: PathBuf,
    /// The modem as the page names it (its Modem box), once it has.
    modem: Mutex<Option<String>>,
    started: AtomicBool,
    stop: AtomicBool,
    /// A connection is open now.
    holding: AtomicBool,
    /// The thread has ended, holding no connection.
    ended: AtomicBool,
}

/// Why a connection ended.
enum Ended {
    /// [`Presence::stop`].
    Stopped,
    /// The modem setting changed, or conformance mode began: look again at once.
    Changed,
    /// The modem could not be opened or went away: try again after `wait`.
    Lost { why: String, wait: Duration },
}

/// What the log was last told of a modem: once named, and once not, with the first reason. While
/// the app stays unnamed, later reasons (Aether still away, say) are the adapter's to note.
#[derive(PartialEq)]
enum Said {
    Nothing,
    Named(String),
    Away(String),
}

impl Presence {
    /// The app's presence for the callsigns whose first home is `base`, not started yet and with
    /// no modem known.
    #[must_use]
    pub fn new(base: &Path) -> Presence {
        Presence {
            shared: Arc::new(Shared {
                app: session::app_name(),
                base: base.to_owned(),
                modem: Mutex::new(None),
                started: AtomicBool::new(false),
                stop: AtomicBool::new(false),
                holding: AtomicBool::new(false),
                ended: AtomicBool::new(false),
            }),
            retry: RETRY,
        }
    }

    /// The same, trying again after `retry` rather than [`RETRY`]: for tests, which cannot wait.
    #[must_use]
    pub fn retrying_every(mut self, retry: Duration) -> Presence {
        self.retry = retry;
        self
    }

    /// Starts its thread, once: it holds the connection whenever the modem is Aether.
    pub fn start(&self) {
        if self.shared.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let shared = Arc::clone(&self.shared);
        let retry = self.retry;
        thread::spawn(move || {
            run(&shared, retry);
            shared.ended.store(true, Ordering::SeqCst);
        });
    }

    /// The modem as the page names it: Aether's control API (`ws://…`), or another adapter's
    /// address, which holds no connection. An empty one is none.
    pub fn set_modem(&self, modem: &str) {
        let modem = modem.trim();
        *self
            .shared
            .modem
            .lock()
            .unwrap_or_else(PoisonError::into_inner) =
            (!modem.is_empty()).then(|| modem.to_owned());
    }

    /// Closes the connection and ends the thread, waiting a moment for the close so that Aether
    /// hears it, in order, before the program exits.
    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if !self.shared.started.load(Ordering::SeqCst) {
            return;
        }
        let held = self.shared.holding.load(Ordering::SeqCst);
        let until = Instant::now() + CLOSING;
        while !self.shared.ended.load(Ordering::SeqCst) && Instant::now() < until {
            thread::sleep(Duration::from_millis(10));
        }
        if held {
            thread::sleep(SETTLE);
        }
    }
}

impl Shared {
    fn stopping(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn modem(&self) -> Option<String> {
        self.modem
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The home chosen is in conformance mode: it reaches only the test adapter.
    fn testing(&self) -> bool {
        store::conformance_at(&callsigns::chosen(&self.base)).is_some()
    }

    /// The Aether to hold a connection to, if any.
    fn wanted(&self) -> Option<String> {
        self.modem()
            .filter(|m| m.starts_with("ws://"))
            .filter(|_| !self.testing())
    }

    /// Waits up to `d`, less when stopping or when the modem is no longer `modem`.
    fn pause(&self, d: Duration, modem: Option<&str>) {
        let until = Instant::now() + d;
        while Instant::now() < until && !self.stopping() && self.modem().as_deref() == modem {
            thread::sleep(TICK.min(until.saturating_duration_since(Instant::now())));
        }
    }
}

fn run(shared: &Shared, retry: Duration) {
    let mut said = Said::Nothing;
    while !shared.stopping() {
        let Some(modem) = shared.wanted() else {
            let now = shared.modem();
            shared.pause(TICK, now.as_deref());
            continue;
        };
        shared.holding.store(true, Ordering::SeqCst);
        let ended = hold(shared, &modem, retry, &mut said);
        shared.holding.store(false, Ordering::SeqCst);
        match ended {
            Ended::Stopped => return,
            Ended::Changed => {}
            Ended::Lost { why, wait } => {
                let secs = wait.as_secs().max(1);
                say(
                    &mut said,
                    Said::Away(modem.clone()),
                    &format!(
                        "Aether at {modem} does not name this app between calls: {why}; \
                         trying again every {secs} s"
                    ),
                );
                shared.pause(wait, Some(&modem));
            }
        }
    }
}

/// Puts `line` in the log file when the state it tells of is a change.
fn say(said: &mut Said, now: Said, line: &str) {
    if *said != now {
        radiopost::logfile::line(line);
        *said = now;
    }
}

/// The adapter's next event; `None` when a tick passes without one, or for a line this program
/// cannot read (RP-4 §2: a host ignores what it does not know).
fn next(conn: &mut Connection) -> Result<Option<Event>, String> {
    match conn.read_message() {
        Ok(Some(m)) => Ok(Event::read(&m).ok()),
        Ok(None) => Err("the adapter closed the connection".into()),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(format!("the adapter's connection failed: {e}")),
    }
}

/// Opens the connection, says `hello`, and holds it until it ends.
fn hold(shared: &Shared, modem: &str, retry: Duration, said: &mut Said) -> Ended {
    let lost = |why: String| Ended::Lost { why, wait: retry };
    let adapter = match session::adapter_for(modem) {
        Ok(a) => a,
        Err(why) => return lost(why),
    };
    let mut conn = match Connection::open(adapter.as_str()) {
        Ok(c) => c,
        Err(e) => return lost(format!("cannot reach the modem's adapter: {e}")),
    };
    if let Err(e) = conn.set_timeout(Some(TICK)) {
        return lost(e.to_string());
    }
    let hello = Host::new().hello(&shared.app);
    if let Err(e) = conn.send(&hello) {
        return lost(format!("the adapter's connection failed: {e}"));
    }
    let asked = Instant::now();
    loop {
        if shared.stopping() {
            conn.close();
            return Ended::Stopped;
        }
        match next(&mut conn) {
            Ok(Some(Event::Hello { caps, .. })) if caps.shared => break,
            Ok(Some(Event::Hello { .. })) => {
                conn.close();
                return Ended::Lost {
                    why: "this Aether is older than 0.2.0-beta.146 and names no programs".into(),
                    wait: NOT_SHARED,
                };
            }
            Ok(Some(Event::Error {
                id: Some(_),
                code,
                text,
            })) => {
                conn.close();
                let text = text.map(|t| format!(" ({t})")).unwrap_or_default();
                return lost(format!("the adapter answered hello {code}{text}"));
            }
            Ok(_) if asked.elapsed() < ANSWER => {}
            Ok(_) => {
                conn.close();
                return lost("the adapter did not answer hello".into());
            }
            Err(why) => return lost(why),
        }
    }
    say(
        said,
        Said::Named(modem.to_owned()),
        &format!("Aether at {modem} names this app among the programs using the station"),
    );
    let mut checked = Instant::now();
    loop {
        if shared.stopping() {
            conn.close();
            return Ended::Stopped;
        }
        if shared.modem().as_deref() != Some(modem) {
            conn.close();
            return Ended::Changed;
        }
        if checked.elapsed() >= RECHECK {
            checked = Instant::now();
            if shared.testing() {
                conn.close();
                return Ended::Changed;
            }
        }
        // the adapter's events (`busy`, mostly) are read and let go: nothing here acts on them,
        // and a connection that stopped reading them would fill and stall the adapter
        if let Err(why) = next(&mut conn) {
            return lost(why);
        }
    }
}
