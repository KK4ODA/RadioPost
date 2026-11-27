// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the gateway is doing, for its operator: its state, the session under way, the sessions
//! it carried (kept in `sessions.json`), what it said, and how its post offices answered. The
//! answering loop writes here; the panel reads it from another thread.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
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
    /// How it ended: an [`End`]'s word, which the panel puts in plain words; `None` while under
    /// way.
    pub result: Option<String>,
    /// What the word leaves out: the modem's reason for a lost link, the code a refusal or an
    /// `ERROR` carried, the adapter's error, or what a forwarding call did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// This gateway placed the call, to a partner (RP-7 §2).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub placed: bool,
    /// A call this gateway placed (RP-7 §2) whose link never came up: no session was carried, so
    /// it is not the last session its status reports (RP-5 §5.9, `carried`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unlinked: bool,
}

/// How a session ended: its record's `result`. Each has a word of its own, which the panel shows as
/// a label (the manual's section 19). What the record's `detail` holds is said of each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// The session was done: the caller hung up (RP-3 §8) with no message left part sent, or, on a
    /// call this gateway placed, it hung up itself once its session was over. `detail`: what a
    /// placed call did.
    Completed,
    /// A station hung up before the session was done, or with a message part sent (the rest goes
    /// on the next call); or the modem ended the link unasked.
    CutShort,
    /// The modem lost the link (`timeout` or `failure`, RP-4 §5). `detail`: the modem's text, or
    /// its reason when it gave none.
    LinkLost,
    /// Nothing heard for 300 seconds: the gateway hung up (RP-3 §8).
    Silent,
    /// The gateway refused the caller: its operator refuses it (W24; the record's `refused` names
    /// the entry), or its `HELLO` or credential did not pass (`detail`: the code sent).
    Refused,
    /// The caller's post office could not be reached or refused the session: the record's
    /// `post_office` has the code.
    PostOffice,
    /// The other station sent `ERROR`. `detail`: its code.
    Error,
    /// The operator clicked **Disconnect**.
    Disconnected,
    /// The operator clicked **Abort**. `detail`, when the modem never said the link had ended.
    Aborted,
    /// The operator clicked **Stop calling**.
    CallStopped,
    /// The operator stopped the gateway.
    Stopped,
    /// The connection to the modem's adapter failed. `detail`: the error.
    Modem,
    /// A call this gateway placed: no link in the time allowed. `detail`: how long.
    NoAnswer,
    /// A call this gateway placed: the modem said it failed, or it could not start. `detail`: why.
    CallFailed,
    /// Kept by a version before 0.1.0-beta.41 as `over`: the link ended without an error, whether
    /// the session was done or not. `detail`: an older sentence this version does not read.
    Ended,
}

impl End {
    /// Every end.
    pub const ALL: [End; 15] = [
        End::Completed,
        End::CutShort,
        End::LinkLost,
        End::Silent,
        End::Refused,
        End::PostOffice,
        End::Error,
        End::Disconnected,
        End::Aborted,
        End::CallStopped,
        End::Stopped,
        End::Modem,
        End::NoAnswer,
        End::CallFailed,
        End::Ended,
    ];

    /// The word the record keeps.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            End::Completed => "completed",
            End::CutShort => "cut-short",
            End::LinkLost => "link-lost",
            End::Silent => "silent",
            End::Refused => "refused",
            End::PostOffice => "post-office",
            End::Error => "error",
            End::Disconnected => "disconnected",
            End::Aborted => "aborted",
            End::CallStopped => "call-stopped",
            End::Stopped => "stopped",
            End::Modem => "modem",
            End::NoAnswer => "no-answer",
            End::CallFailed => "call-failed",
            End::Ended => "ended",
        }
    }
}

/// How a session ended, with the words the log says of it: what a session's loop returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Over {
    /// How.
    pub end: End,
    /// What the word leaves out, for the record's `detail`.
    pub detail: Option<String>,
}

impl Over {
    /// An end with nothing more to say.
    #[must_use]
    pub fn just(end: End) -> Over {
        Over { end, detail: None }
    }

    /// An end and what it leaves out.
    #[must_use]
    pub fn with(end: End, detail: impl Into<String>) -> Over {
        Over {
            end,
            detail: Some(detail.into()),
        }
    }

    /// What the log says of it, after "ended: ".
    #[must_use]
    pub fn said(&self) -> String {
        let d = self.detail.as_deref();
        let after = |text: &str, sep: &str| match d {
            Some(d) => format!("{text}{sep}{d}"),
            None => text.to_owned(),
        };
        match self.end {
            End::Completed => after("it was completed", ": "),
            End::CutShort => "it was cut short".to_owned(),
            End::LinkLost => match d {
                Some(d) => format!("the modem lost the link ({d})"),
                None => "the modem lost the link".to_owned(),
            },
            End::Silent => "nothing heard for 300 seconds".to_owned(),
            End::Refused => after("the caller was refused", ": "),
            End::PostOffice => "its post office failed the session".to_owned(),
            End::Error => after("the other station sent ERROR", " "),
            End::Disconnected => "the operator disconnected".to_owned(),
            End::Aborted => after("the operator aborted the session", "; "),
            End::CallStopped => "the operator stopped the call".to_owned(),
            End::Stopped => "the operator stopped the gateway".to_owned(),
            End::Modem => after("the modem's adapter", ": "),
            End::NoAnswer => d.unwrap_or("no link came up").to_owned(),
            End::CallFailed => after("the call failed", ": "),
            End::Ended => after("it ended", ": "),
        }
    }
}

impl SessionRecord {
    /// A record kept by a version before 0.1.0-beta.41, whose `result` was `over` or a sentence,
    /// read as the words kept now.
    fn read_old(&mut self) {
        const MODEM: &str = "the modem's adapter: ";
        const CALL_FAILED: &str = "the call failed: ";
        let Some(said) = self.result.clone() else {
            return;
        };
        if End::ALL.iter().any(|e| e.word() == said) {
            return;
        }
        let (end, detail) = if self.refused.is_some() {
            (End::Refused, None)
        } else if self.post_office.is_some() {
            (End::PostOffice, None)
        } else if said == "over" {
            (End::Ended, None)
        } else if said == "the operator disconnected" {
            (End::Disconnected, None)
        } else if said == "the operator aborted the session" {
            (End::Aborted, None)
        } else if said == "the operator stopped the call" {
            (End::CallStopped, None)
        } else if said == "the operator stopped the gateway" {
            (End::Stopped, None)
        } else if said.starts_with("nothing heard for ") {
            (End::Silent, None)
        } else if said.starts_with("the modem did not say the link had ended") {
            (End::Aborted, Some(said))
        } else if let Some(rest) = said.strip_prefix(MODEM) {
            (End::Modem, Some(rest.to_owned()))
        } else if said.starts_with("no link to ") {
            (End::NoAnswer, Some(said))
        } else if let Some(rest) = said.strip_prefix(CALL_FAILED) {
            (End::CallFailed, Some(rest.to_owned()))
        } else {
            (End::Ended, Some(said))
        };
        self.result = Some(end.word().to_owned());
        if self.detail.is_none() {
            self.detail = detail;
        }
        if matches!(end, End::NoAnswer | End::CallFailed | End::CallStopped) {
            self.placed = true;
        }
    }
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

/// How the operator ends the session under way before its time: the panel's **Stop calling**,
/// **Disconnect** and **Abort**.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stop {
    /// In order (RP-4 `disconnect`): what the modem holds goes first, then the link ends. A call
    /// the gateway placed and not yet answered is stopped as by `Abort`.
    Disconnect = 1,
    /// Now (RP-4 `abort`): the call, or the link, ends at once and what the modem holds is dropped.
    Abort = 2,
}

impl Stop {
    /// Its name, as the panel's API and status say it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Stop::Disconnect => "disconnect",
            Stop::Abort => "abort",
        }
    }
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
    /// The operator asked the session under way to end: `disconnect` or `abort`, until it has.
    pub stopping: Option<&'static str>,
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
    /// The operator's word to end the session under way ([`Stop`] as a number, 0 for none):
    /// forgotten when the next session begins.
    end: AtomicU8,
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
            end: AtomicU8::new(0),
            keep: None,
        }
    }

    /// A monitor that keeps the sessions it records in `sessions.json` in `dir`, starting from
    /// what is there.
    #[must_use]
    pub fn keeping(dir: &Path) -> Monitor {
        let mut m = Monitor::new();
        let mut sessions: Vec<SessionRecord> = std::fs::read_to_string(dir.join("sessions.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        for s in &mut sessions {
            s.read_old();
        }
        m.lock().sessions = sessions;
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

    /// A station called. The operator's word to end the last session is forgotten here, under
    /// the lock [`Monitor::end_session`] takes, so a word for this one cannot be lost.
    pub fn begin(&self, remote: &str) {
        let mut i = self.lock();
        self.end.store(0, Ordering::SeqCst);
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
            s.placed = true;
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

    /// The session ended as `over` says. It is kept.
    pub fn end(&self, over: &Over) {
        let mut i = self.lock();
        let Some(mut s) = i.session.take() else {
            return;
        };
        s.ended = Some(now());
        s.result = Some(over.end.word().to_owned());
        s.detail.clone_from(&over.detail);
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

    /// Asks the answering loop to end the session under way (**Stop calling**, **Disconnect**,
    /// **Abort**), without stopping the gateway: it hears the word within a quarter of a second,
    /// and goes on answering after. An abort after a disconnect still under way ends the link at
    /// once.
    ///
    /// # Errors
    ///
    /// No session is under way.
    pub fn end_session(&self, how: Stop) -> Result<(), String> {
        let i = self.lock();
        if i.session.is_none() {
            return Err("no session is under way".into());
        }
        self.end.fetch_max(how as u8, Ordering::SeqCst);
        drop(i);
        Ok(())
    }

    /// What the operator asked of the session under way, if anything.
    #[must_use]
    pub fn end_asked(&self) -> Option<Stop> {
        match self.end.load(Ordering::SeqCst) {
            0 => None,
            1 => Some(Stop::Disconnect),
            _ => Some(Stop::Abort),
        }
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
            stopping: i.session.as_ref().and(self.end_asked()).map(Stop::name),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(result: &str) -> SessionRecord {
        SessionRecord {
            remote: "WD5EMA".into(),
            result: Some(result.into()),
            ..SessionRecord::default()
        }
    }

    /// A `sessions.json` written before 0.1.0-beta.41 reads as the words kept now, and is written
    /// back in them.
    #[test]
    fn old_records_read_as_words() {
        let dir = std::env::temp_dir().join(format!("gateway-old-sessions-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch folder");
        let mut refused = kept("over");
        refused.refused = Some("N3CALL".into());
        let mut post_office = kept("over");
        post_office.post_office = Some("post-office-unreachable".into());
        let old = vec![
            kept("over"),
            refused,
            post_office,
            kept("the operator disconnected"),
            kept("the operator aborted the session"),
            kept("the operator stopped the call"),
            kept("the operator stopped the gateway"),
            kept("nothing heard for 300 seconds"),
            kept("the modem's adapter: connection reset"),
            kept("no link to KK4ODA-2 in 180 seconds"),
            kept("the call failed: busy (the channel is busy)"),
            kept(&format!(
                "{}: the session is ended here",
                "the modem did not say the link had ended"
            )),
            kept("could not scan: no modem"),
            kept("link-lost"),
        ];
        let text = serde_json::to_string(&old).expect("JSON");
        std::fs::write(dir.join("sessions.json"), text).expect("written");
        let m = Monitor::keeping(&dir);
        let read: Vec<(Option<String>, Option<String>, bool)> = m
            .lock()
            .sessions
            .iter()
            .map(|s| (s.result.clone(), s.detail.clone(), s.placed))
            .collect();
        let w = |e: End| Some(e.word().to_owned());
        let d = |t: &str| Some(t.to_owned());
        assert_eq!(
            read,
            vec![
                (w(End::Ended), None, false),
                (w(End::Refused), None, false),
                (w(End::PostOffice), None, false),
                (w(End::Disconnected), None, false),
                (w(End::Aborted), None, false),
                (w(End::CallStopped), None, true),
                (w(End::Stopped), None, false),
                (w(End::Silent), None, false),
                (w(End::Modem), d("connection reset"), false),
                (
                    w(End::NoAnswer),
                    d("no link to KK4ODA-2 in 180 seconds"),
                    true
                ),
                (w(End::CallFailed), d("busy (the channel is busy)"), true),
                (
                    w(End::Aborted),
                    d("the modem did not say the link had ended: the session is ended here"),
                    false
                ),
                (w(End::Ended), d("could not scan: no modem"), false),
                (w(End::LinkLost), None, false),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every end has a word of its own, and the record keeps it with its detail.
    #[test]
    fn each_end_is_kept_as_its_word() {
        let mut words: Vec<&str> = End::ALL.iter().map(|e| e.word()).collect();
        words.sort_unstable();
        words.dedup();
        assert_eq!(words.len(), End::ALL.len());
        let m = Monitor::new();
        m.begin("WD5EMA");
        m.end(&Over::with(End::LinkLost, "link timeout"));
        let s = &m.sessions()[0];
        assert_eq!(s.result.as_deref(), Some("link-lost"));
        assert_eq!(s.detail.as_deref(), Some("link timeout"));
        assert!(!s.placed);
        assert_eq!(
            Over::with(End::LinkLost, "link timeout").said(),
            "the modem lost the link (link timeout)"
        );
    }
}
