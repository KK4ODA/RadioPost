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
    /// How it ended: `over` when the caller hung up, else why; `None` while under way.
    pub result: Option<String>,
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

    /// What the session's engine said.
    pub fn event(&self, event: &Event) {
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

    /// Clears a stop asked for, before the loop starts again.
    pub fn clear_stop(&self) {
        self.stop.store(false, Ordering::Relaxed);
    }
}
