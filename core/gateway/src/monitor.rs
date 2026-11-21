// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the gateway is doing, for its operator: its state, the session under way, the sessions
//! it carried (kept in `sessions.json`), what it said, and how its post offices answered. The
//! answering loop writes here; the panel reads it from another thread.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use radiopost::ErrorCode;
use radiopost::session::Event;
use serde::{Deserialize, Serialize};

use crate::station::{now, write};

/// Lines of the log kept in memory.
pub const LOG_KEPT: usize = 2000;
/// Sessions kept in `sessions.json`.
pub const SESSIONS_KEPT: usize = 200;

/// One session the gateway carried.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionRecord {
    /// The caller's callsign.
    pub remote: String,
    /// The account its credential names, once read.
    pub account: Option<String>,
    /// When it called, Unix seconds.
    pub started: u64,
    /// When it ended; `None` while it is under way.
    pub ended: Option<u64>,
    /// Messages the caller sent that its post office took.
    pub handed_in: u32,
    /// Messages the caller picked up.
    pub picked_up: u32,
    /// Messages refused or damaged, either way.
    pub failed: u32,
    /// The post office could not be reached or refused the session: its error code's name.
    pub post_office: Option<String>,
    /// The operator refuses this caller (wishlist W24): the entry that refused it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    /// How it ended: `over` when the caller hung up, else why; `None` while under way.
    pub result: Option<String>,
    /// A call this gateway placed (RP-7 §2) whose link never came up: no session was carried, so
    /// it is not the last session its status reports (RP-5 §5.9, `carried`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unlinked: bool,
}

/// How a post office answered the gateway's login.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    /// Its domain.
    pub domain: String,
    /// Its API's base URL, when one was found.
    pub url: Option<String>,
    /// It let the gateway log in.
    pub ok: bool,
    /// What happened, in words.
    pub said: String,
    /// When, Unix seconds.
    pub at: u64,
}

/// The scan of its first modem (W50, RP-4 §4.2), as the panel shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Scanning {
    /// The dials scanned, the first the home; none once it has ended.
    pub dials: Vec<u64>,
    /// How long a call must keep trying to find the gateway, as the modem answered.
    pub scan_s: Option<u64>,
    /// The dial the radio is on, as the modem last said.
    pub at_hz: Option<u64>,
    /// Why it is not scanning though it is set to: the modem cannot, or ended the scan (the
    /// operator moved the radio), in words.
    pub not: Option<String>,
}

/// Where the first modem's radio is, and how the other modems are (W26), as the answering loop
/// last said: what the gateway's status report needs to say which channels it listens on (RP-5
/// §5.9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Radio {
    /// The first modem's radio's dial, as the modem last said (RP-4 §4.1).
    pub at_hz: Option<u64>,
    /// Whether the gateway has more modems than the first.
    pub several: bool,
    /// How many of the other modems do not answer.
    pub others_away: usize,
    /// How many of the other modems' radios are on a dial where it may not answer (RP-4 §8).
    pub others_outside: usize,
}

/// The gateway's last status report to its post office (RP-5 §5.9), as the panel shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Reported {
    /// The operator turned reporting off (Setup's *Report its status*).
    pub off: bool,
    /// The post office's domain, when known.
    pub to: Option<String>,
    /// The state last reported, and when, Unix seconds.
    pub state: Option<String>,
    /// When it last reached the post office, Unix seconds.
    pub at: Option<u64>,
    /// Why the last try failed, in words.
    pub error: Option<String>,
}

/// The gateway's state, as the panel shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Status {
    /// `stopped`, `opening` (the modem is being opened), `listening` or `session`.
    pub state: &'static str,
    /// Since when, Unix seconds.
    pub since: u64,
    /// What keeps it from answering, or why it stopped, when anything does.
    pub problem: Option<String>,
    /// The session under way.
    pub session: Option<SessionRecord>,
    /// Lines logged so far: the number to ask the log from next.
    pub logged: usize,
    /// How each post office answered the last check.
    pub checks: Vec<Check>,
    /// Its scan, when it is set to scan.
    pub scan: Option<Scanning>,
    /// Its last status report (RP-5 §5.9), when it has tried one.
    pub reported: Option<Reported>,
}

struct Inner {
    state: &'static str,
    since: u64,
    problem: Option<String>,
    session: Option<SessionRecord>,
    sessions: Vec<SessionRecord>,
    log: VecDeque<(u64, String)>,
    logged: usize,
    checks: Vec<Check>,
    scan: Option<Scanning>,
    /// A partner the operator asked to forward to (RP-7 §2), for the answering loop.
    forward: Option<String>,
    radio: Radio,
    reported: Option<Reported>,
}

/// What the gateway is doing, shared between its answering loop and its panel.
pub struct Monitor {
    inner: Mutex<Inner>,
    stop: AtomicBool,
    keep: Option<PathBuf>,
}

impl Default for Monitor {
    fn default() -> Self {
        Monitor::new()
    }
}

impl Monitor {
    /// A monitor that keeps nothing on disk.
    #[must_use]
    pub fn new() -> Monitor {
        Monitor {
            inner: Mutex::new(Inner {
                state: "stopped",
                since: now(),
                problem: None,
                session: None,
                sessions: Vec::new(),
                log: VecDeque::new(),
                logged: 0,
                checks: Vec::new(),
                scan: None,
                forward: None,
                radio: Radio::default(),
                reported: None,
            }),
            stop: AtomicBool::new(false),
            keep: None,
        }
    }

    /// A monitor that keeps the sessions it records in `sessions.json` in `dir`, starting from
    /// what is there.
    #[must_use]
    pub fn keeping(dir: &Path) -> Monitor {
        let mut m = Monitor::new();
        m.lock().sessions = std::fs::read_to_string(dir.join("sessions.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        m.keep = Some(dir.to_owned());
        m
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A line for the operator, in the panel and in the gateway's log file when the program
    /// named one (`radiopost::logfile`).
    pub fn log(&self, line: &str) {
        radiopost::logfile::line(line);
        let mut i = self.lock();
        i.log.push_back((now(), line.to_owned()));
        i.logged += 1;
        while i.log.len() > LOG_KEPT {
            i.log.pop_front();
        }
    }

    /// The lines from number `since` on (as many as are still kept), and the number after them.
    #[must_use]
    pub fn log_since(&self, since: usize) -> (Vec<(u64, String)>, usize) {
        let i = self.lock();
        let first = i.logged - i.log.len();
        let skip = since.saturating_sub(first);
        (i.log.iter().skip(skip).cloned().collect(), i.logged)
    }

    /// The gateway is now in `state`, held back by `problem` when anything holds it.
    pub fn set_state(&self, state: &'static str, problem: Option<String>) {
        let mut i = self.lock();
        if i.state != state {
            i.since = now();
        }
        i.state = state;
        i.problem = problem;
    }

    /// The problem now, if any.
    #[must_use]
    pub fn problem(&self) -> Option<String> {
        self.lock().problem.clone()
    }

    /// A station called.
    pub fn begin(&self, remote: &str) {
        let mut i = self.lock();
        i.session = Some(SessionRecord {
            remote: remote.to_owned(),
            started: now(),
            ..SessionRecord::default()
        });
        i.state = "session";
        i.since = now();
    }

    /// The gateway calls a partner (RP-7 §2): a session under way, though its link is not up yet.
    pub fn calling(&self, partner: &str) {
        self.begin(partner);
        if let Some(s) = self.lock().session.as_mut() {
            s.unlinked = true;
        }
    }

    /// The link of the call it placed came up.
    pub fn linked(&self) {
        if let Some(s) = self.lock().session.as_mut() {
            s.unlinked = false;
        }
    }

    /// When the last session it carried ended: the newest whose link came up (RP-5 §5.9,
    /// `carried`).
    #[must_use]
    pub fn last_carried(&self) -> Option<u64> {
        self.lock()
            .sessions
            .iter()
            .rev()
            .filter(|s| !s.unlinked)
            .find_map(|s| s.ended)
    }

    /// Where the first modem's radio is, and how the other modems are, as the answering loop sees
    /// them now.
    pub fn set_radio(&self, radio: Radio) {
        self.lock().radio = radio;
    }

    /// Where the radio is and how the other modems are, as last said.
    #[must_use]
    pub fn radio(&self) -> Radio {
        self.lock().radio
    }

    /// How the last status report went.
    pub fn set_reported(&self, reported: Reported) {
        self.lock().reported = Some(reported);
    }

    /// What the session's engine said. An `ERROR` from the caller is logged with its text (RP-3
    /// §5.9), so the operator reads why a caller left: a client that called for station-to-station
    /// mail, say, which the gateway answered.
    pub fn event(&self, event: &Event) {
        if let Event::Error { code, text } = event {
            let who = self.lock().session.as_ref().map(|s| s.remote.clone());
            let text: String = text
                .as_deref()
                .map(|t| format!(": {}", t.chars().take(200).collect::<String>()))
                .unwrap_or_default();
            self.log(&format!(
                "{} sent ERROR {}{text}",
                who.as_deref().unwrap_or("the caller"),
                code.name()
            ));
        }
        let mut i = self.lock();
        let Some(s) = i.session.as_mut() else {
            return;
        };
        match event {
            Event::Received { .. } => s.handed_in += 1,
            Event::Delivered(_) => s.picked_up += 1,
            Event::Failed { .. } | Event::NotTaken { .. } => s.failed += 1,
            _ => {}
        }
    }

    /// The account the caller's credential names.
    pub fn account(&self, account: Option<&str>) {
        if let (Some(a), Some(s)) = (account, self.lock().session.as_mut()) {
            s.account = Some(a.to_owned());
        }
    }

    /// The caller was refused by the operator's choice of `entry` (wishlist W24).
    pub fn refused(&self, entry: &str) {
        if let Some(s) = self.lock().session.as_mut() {
            s.refused = Some(entry.to_owned());
        }
        self.log(&format!("refused the caller: the operator refuses {entry}"));
    }

    /// The post office failed the session with `code`.
    pub fn post_office_failed(&self, code: ErrorCode) {
        if let Some(s) = self.lock().session.as_mut() {
            s.post_office = Some(code.name().to_owned());
        }
    }

    /// The session ended: `over`, or why it ended otherwise. It is kept.
    pub fn end(&self, result: &Result<(), String>) {
        let mut i = self.lock();
        let Some(mut s) = i.session.take() else {
            return;
        };
        s.ended = Some(now());
        s.result = Some(match result {
            Ok(()) => "over".to_owned(),
            Err(e) => e.clone(),
        });
        i.sessions.push(s);
        let excess = i.sessions.len().saturating_sub(SESSIONS_KEPT);
        i.sessions.drain(..excess);
        i.state = "listening";
        i.since = now();
        let text = serde_json::to_string_pretty(&i.sessions).unwrap_or_default();
        drop(i);
        if let Some(dir) = &self.keep
            && let Err(e) = write(dir, "sessions.json", &text)
        {
            self.log(&format!("cannot keep the sessions: {e}"));
        }
    }

    /// The sessions carried, newest first.
    #[must_use]
    pub fn sessions(&self) -> Vec<SessionRecord> {
        self.lock().sessions.iter().rev().cloned().collect()
    }

    /// How the post offices answered the last check.
    pub fn set_checks(&self, checks: Vec<Check>) {
        self.lock().checks = checks;
    }

    /// The state, the session under way and the checks.
    #[must_use]
    pub fn status(&self) -> Status {
        let i = self.lock();
        Status {
            state: i.state,
            since: i.since,
            problem: i.problem.clone(),
            session: i.session.clone(),
            logged: i.logged,
            checks: i.checks.clone(),
            scan: i.scan.clone(),
            reported: i.reported.clone(),
        }
    }

    /// The scan as it now stands; `None` when the gateway is not set to scan.
    pub fn set_scan(&self, scan: Option<Scanning>) {
        self.lock().scan = scan;
    }

    /// The radio is on `hz` (RP-4 §4.1), for a scan's line.
    pub fn scan_at(&self, hz: u64) {
        if let Some(s) = self.lock().scan.as_mut() {
            s.at_hz = Some(hz);
        }
    }

    /// Asks the answering loop to stop: it ends a session under way and lets the modem go.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Whether the answering loop was asked to stop.
    #[must_use]
    pub fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Asks the answering loop to call `partner` between sessions and forward the queue to it
    /// (RP-7 §2); a second request before then replaces the first.
    pub fn request_forward(&self, partner: &str) {
        self.lock().forward = Some(partner.to_owned());
    }

    /// The partner the operator asked to forward to, taken once.
    pub fn take_forward(&self) -> Option<String> {
        self.lock().forward.take()
    }

    /// Clears a stop asked for, before the loop starts again.
    pub fn clear_stop(&self) {
        self.stop.store(false, Ordering::Relaxed);
    }
}
