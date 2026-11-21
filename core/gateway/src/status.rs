// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whether the gateway is on the air, reported to its own post office (RP-5 §5.9, design D45),
//! which publishes it for anyone to read (RP-5 §4.6): radiopost.org's gateway directory shows it.
//!
//! The report says the gateway's state (`listening`, `session`, `opening`, `stopped`), whether it
//! listens on each channel of its record, the problems it knows of by RP-0 §9.7's names, and when
//! its last session ended, to the hour. Nothing about who called or what was carried. It goes when
//! it changes and at least every five minutes, never twice within ten seconds, and only while the
//! operator leaves *Report its status* on (`Settings::report_status`, on by default). Turned off,
//! the report is withdrawn (`DELETE /status`).
//!
//! [`report`] makes the report from what the monitor shows, with no I/O; [`Reporter`] decides
//! when to send, with no clock of its own; [`start`] runs both on a thread beside the gateway.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use radiopost::ErrorCode;
use radiopost_api::{Account, Http};
use serde_json::{Value as Json, json};

use crate::monitor::{Monitor, Radio, Reported, Scanning, Status};
use crate::station::{Settings, now};

/// A report is sent at least this often while nothing changes (RP-5 §5.9: 5 minutes).
pub const EVERY: Duration = Duration::from_secs(300);
/// Never two reports within this (RP-5 §5.9: 10 seconds).
pub const SOONEST: Duration = Duration::from_secs(10);
/// After a report that failed, the next try.
pub const RETRY: Duration = Duration::from_secs(60);
/// After a `404`: the post office does not take status reports (one before RP-5 1.6.0), or does
/// not list this gateway. It is asked again an hour later.
pub const SILENT: Duration = Duration::from_secs(3600);

/// What the gateway sends its post office.
#[derive(Clone, Debug, PartialEq)]
pub enum Send {
    /// `PUT /status` with this report.
    Report(Json),
    /// `DELETE /status`: reporting was turned off.
    Withdraw,
}

/// How a try went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The post office took it.
    Taken,
    /// It did not; try again after [`RETRY`].
    Failed,
    /// It answered `404`: try again after [`SILENT`].
    NotTaken,
}

/// When to send, from what was last sent and how it went: no clock of its own.
#[derive(Debug)]
pub struct Reporter {
    /// The last report the post office took, and when.
    sent: Option<(Json, Instant)>,
    /// The last try, and how long to wait after it.
    tried: Option<(Instant, Duration)>,
    /// A withdrawal was taken since reporting was turned off.
    withdrawn: bool,
}

impl Default for Reporter {
    fn default() -> Self {
        Reporter::new()
    }
}

impl Reporter {
    /// Nothing sent yet: a report goes at once, or, with reporting off, a withdrawal, since an
    /// earlier run may have reported.
    #[must_use]
    pub fn new() -> Reporter {
        Reporter {
            sent: None,
            tried: None,
            withdrawn: false,
        }
    }

    /// What to send at `at`, with reporting `on` and `report` what the gateway would say now.
    #[must_use]
    pub fn next(&self, on: bool, report: &Json, at: Instant) -> Option<Send> {
        if let Some((t, wait)) = self.tried
            && at.saturating_duration_since(t) < wait
        {
            return None;
        }
        if !on {
            return (!self.withdrawn).then_some(Send::Withdraw);
        }
        match &self.sent {
            Some((last, t)) if last == report && at.saturating_duration_since(*t) < EVERY => None,
            _ => Some(Send::Report(report.clone())),
        }
    }

    /// `sent` was tried at `at`, and went as `outcome` says.
    pub fn done(&mut self, sent: &Send, outcome: &Outcome, at: Instant) {
        let wait = match outcome {
            Outcome::Taken => SOONEST,
            Outcome::Failed => RETRY,
            Outcome::NotTaken => SILENT,
        };
        self.tried = Some((at, wait));
        if *outcome != Outcome::Taken {
            return;
        }
        match sent {
            Send::Report(r) => {
                self.sent = Some((r.clone(), at));
                self.withdrawn = false;
            }
            Send::Withdraw => {
                self.sent = None;
                self.withdrawn = true;
            }
        }
    }

    /// Whether the last report taken says the same as `report`.
    #[must_use]
    pub fn said(&self, report: &Json) -> bool {
        self.sent.as_ref().is_some_and(|(r, _)| r == report)
    }
}

/// The channels of the gateway's record, as its post office has it: (`dial_hz`, `modem`).
#[must_use]
pub fn record_channels(record: &Json) -> Vec<(u64, String)> {
    record["channels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| Some((c["dial_hz"].as_u64()?, c["modem"].as_str()?.to_owned())))
        .collect()
}

/// RP-5 §5.9: the report, from the gateway's state (`status`), its radio (`radio`), its record's
/// `channels` and when its last session ended (`carried`, Unix seconds).
///
/// A channel is up when the gateway listens there between sessions: never while it is opening
/// or stopped. With one modem, a channel is up when the radio is on its dial or scanning it; with
/// no dial known, the record's only channel is up, and several are left out. With several
/// modems (W26) the gateway cannot tell which channel is whose: each channel is up while every
/// modem answers where it may, and all are left out while one does not.
#[must_use]
pub fn report(
    status: &Status,
    radio: &Radio,
    channels: &[(u64, String)],
    carried: Option<u64>,
) -> Json {
    let mut problems: Vec<&str> = Vec::new();
    let (state, listens) = match status.state {
        "stopped" => {
            if status.problem.is_some() {
                problems.push("setup-error");
            }
            ("stopped", false)
        }
        "opening" => {
            if status.problem.is_some() {
                problems.push("modem-away");
            }
            ("opening", false)
        }
        // running, but not answering where the first modem's radio is (RP-4 §8)
        "outside" => {
            problems.push("outside-segment");
            ("listening", false)
        }
        "session" => ("session", true),
        _ => ("listening", true),
    };
    let running = matches!(state, "listening" | "session");
    if running && radio.others_away > 0 && !problems.contains(&"modem-away") {
        problems.push("modem-away");
    }
    if running && radio.others_outside > 0 && !problems.contains(&"outside-segment") {
        problems.push("outside-segment");
    }
    let up = |dial: u64| -> Option<bool> {
        if !running {
            return Some(false);
        }
        if radio.several {
            let all = listens && radio.others_away == 0 && radio.others_outside == 0;
            return all.then_some(true);
        }
        if !listens {
            return Some(false);
        }
        if let Some(Scanning { dials, .. }) = &status.scan
            && !dials.is_empty()
        {
            return Some(dials.contains(&dial));
        }
        match radio.at_hz {
            Some(at) => Some(at == dial),
            None => (channels.len() == 1).then_some(true),
        }
    };
    let said: Vec<Json> = channels
        .iter()
        .filter_map(|(dial, modem)| {
            up(*dial).map(|up| json!({"dial_hz": dial, "modem": modem, "up": up}))
        })
        .collect();
    let mut out = json!({"state": state});
    if !said.is_empty() {
        out["channels"] = Json::Array(said);
    }
    if !problems.is_empty() {
        out["problems"] = json!(problems);
    }
    if let Some(c) = carried {
        out["carried"] = json!(c / 3600 * 3600);
    }
    out
}

/// The reporter's life: it ends when this is dropped.
pub struct Guard {
    alive: Arc<AtomicBool>,
    finishing: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Guard {
    /// Says what the gateway does now, once more, before it goes: waits up to `within` for a
    /// report that differs from the last one taken (`stopped`, as the gateway closes), then ends
    /// the reporter.
    pub fn finish(&mut self, within: Duration) {
        self.finishing.store(true, Ordering::Relaxed);
        let until = Instant::now() + within;
        while self.thread.as_ref().is_some_and(|t| !t.is_finished()) && Instant::now() < until {
            thread::sleep(Duration::from_millis(50));
        }
        self.alive.store(false, Ordering::Relaxed);
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}

/// Starts reporting the status of the gateway in `dir`, as `monitor` shows it, until the guard
/// returned is dropped. It reports only once the gateway is registered.
#[must_use]
pub fn start(dir: &Path, monitor: Arc<Monitor>) -> Guard {
    let alive = Arc::new(AtomicBool::new(true));
    let finishing = Arc::new(AtomicBool::new(false));
    let (a, f, dir) = (alive.clone(), finishing.clone(), dir.to_owned());
    let thread = thread::spawn(move || run(&dir, &monitor, &a, &f));
    Guard {
        alive,
        finishing,
        thread: Some(thread),
    }
}

/// Where the gateway's own post office is reached, and its domain.
struct Target {
    domain: String,
    http: Http,
}

fn run(dir: &Path, monitor: &Monitor, alive: &AtomicBool, finishing: &AtomicBool) {
    let mut reporter = Reporter::new();
    let mut target: Option<Target> = None;
    let mut said_failure: Option<String> = None;
    let mut said_first = false;
    while alive.load(Ordering::Relaxed) {
        if let Ok((settings, account)) = crate::station::identity(dir) {
            let record = sent_record(dir);
            let status = monitor.status();
            let now_report = report(
                &status,
                &monitor.radio(),
                &record_channels(&record),
                monitor.last_carried(),
            );
            let finishing_now = finishing.load(Ordering::Relaxed);
            if finishing_now && (reporter.said(&now_report) || !settings.report_status) {
                return;
            }
            if let Some(send) = reporter.next(settings.report_status, &now_report, Instant::now()) {
                if target.is_none() {
                    target = find(dir, &settings, account.clone());
                }
                let (outcome, error) = match &mut target {
                    Some(t) => tell(t, &send),
                    None => (
                        Outcome::Failed,
                        Some("its post office cannot be found".to_owned()),
                    ),
                };
                if outcome == Outcome::Failed {
                    target = None; // found again: DNS may have moved
                }
                reporter.done(&send, &outcome, Instant::now());
                let to = target.as_ref().map(|t| t.domain.clone());
                let place = to.clone().unwrap_or_else(|| "its post office".to_owned());
                match (&send, &error) {
                    (_, Some(e)) => {
                        if said_failure.as_ref() != Some(e) {
                            monitor.log(&format!("status: not reported to {place}: {e}"));
                            said_failure = Some(e.clone());
                        }
                    }
                    (Send::Withdraw, None) => {
                        monitor.log(&format!(
                            "status: no longer reported to {place}, as Setup says"
                        ));
                        said_failure = None;
                    }
                    (Send::Report(r), None) => {
                        if said_failure.take().is_some() {
                            monitor.log(&format!("status: reported to {place} again"));
                        } else if !said_first {
                            said_first = true;
                            monitor.log(&format!(
                                "status: reported to {place}: {}",
                                r["state"].as_str().unwrap_or("?")
                            ));
                        }
                    }
                }
                monitor.set_reported(Reported {
                    off: !settings.report_status,
                    to,
                    state: match (&send, &outcome) {
                        (Send::Report(r), Outcome::Taken) => r["state"].as_str().map(str::to_owned),
                        _ => None,
                    },
                    at: (outcome == Outcome::Taken).then(now),
                    error,
                });
                if finishing_now && outcome == Outcome::Taken {
                    return;
                }
            }
        }
        let tick = if finishing.load(Ordering::Relaxed) {
            Duration::from_millis(200)
        } else {
            Duration::from_secs(1)
        };
        let until = Instant::now() + tick;
        while Instant::now() < until && alive.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(50));
        }
    }
}

/// The record as the post office has it (`record.json`'s `sent`, or `record` in a file written
/// before `sent` was kept); `null` when the gateway was registered from no computer it knows.
fn sent_record(dir: &Path) -> Json {
    let kept: Json = std::fs::read_to_string(dir.join(crate::operator::RECORD))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Json::Null);
    match kept.get("sent") {
        Some(s) => s.clone(),
        None => kept.get("record").cloned().unwrap_or(Json::Null),
    }
}

/// The gateway's own post office (RP-5 §4.4): the domain of its credential's address, at the
/// base URL it was registered with when `record.json` says so, else as the settings or DNS give
/// it (RP-1 §7.1).
fn find(dir: &Path, settings: &Settings, account: Arc<Account>) -> Option<Target> {
    let body = radiopost::credential::Credential::decode(&account.credential)?.body;
    let domain = body.address.rsplit_once('@')?.1.to_lowercase();
    let kept: Json = std::fs::read_to_string(dir.join(crate::operator::RECORD))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Json::Null);
    let registered = (kept["address"].as_str() == Some(body.address.as_str()))
        .then(|| kept["post_office"].as_str().map(str::to_owned))
        .flatten();
    let base = registered
        .or_else(|| settings.post_offices.get(&domain).cloned())
        .or_else(|| {
            (!settings.resolver.is_empty())
                .then(|| crate::station::lookup(&settings.resolver, &domain))
                .flatten()
        })?;
    Some(Target {
        domain,
        http: Http::new(&base, account, Arc::new(Mutex::new(None))),
    })
}

/// Sends `send` to the post office: how it went, and why not in words.
fn tell(target: &mut Target, send: &Send) -> (Outcome, Option<String>) {
    let answer = match send {
        Send::Report(r) => target.http.call("PUT", "/status", Some(r)),
        Send::Withdraw => target.http.call("DELETE", "/status", None),
    };
    match answer {
        Ok((204 | 200, _)) => (Outcome::Taken, None),
        Ok((404, _)) => (
            Outcome::NotTaken,
            Some(
                "it does not take status reports yet (RP-5 1.6.0), or does not list this \
                 gateway: asked again in an hour"
                    .to_owned(),
            ),
        ),
        Ok((status, body)) => (
            Outcome::Failed,
            Some(format!(
                "{status} {}",
                radiopost_api::code_of(body.as_ref()).name()
            )),
        ),
        Err(ErrorCode::PostOfficeUnreachable) => {
            (Outcome::Failed, Some("it cannot be reached".to_owned()))
        }
        Err(code) => (Outcome::Failed, Some(code.name().to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: &'static str, problem: Option<&str>) -> Status {
        Status {
            state,
            since: 0,
            problem: problem.map(str::to_owned),
            session: None,
            logged: 0,
            checks: Vec::new(),
            scan: None,
            reported: None,
        }
    }

    fn two() -> Vec<(u64, String)> {
        vec![(3_590_000, "aether".into()), (7_101_000, "aether".into())]
    }

    #[test]
    fn a_report_says_what_the_gateway_does() {
        let radio = Radio {
            at_hz: Some(7_101_000),
            ..Radio::default()
        };
        // one modem, the radio on 40 m: that channel up, the other not
        assert_eq!(
            report(&status("listening", None), &radio, &two(), Some(7300)),
            json!({"state": "listening", "channels": [
                {"dial_hz": 3_590_000, "modem": "aether", "up": false},
                {"dial_hz": 7_101_000, "modem": "aether", "up": true}], "carried": 7200})
        );
        // scanning both dials: both up, wherever the radio is now
        let mut scanning = status("session", None);
        scanning.scan = Some(Scanning {
            dials: vec![3_590_000, 7_101_000],
            ..Scanning::default()
        });
        assert_eq!(
            report(&scanning, &radio, &two(), None),
            json!({"state": "session", "channels": [
                {"dial_hz": 3_590_000, "modem": "aether", "up": true},
                {"dial_hz": 7_101_000, "modem": "aether", "up": true}]})
        );
        // waiting for the modem, and stopped on an error: every channel down, the problem named
        assert_eq!(
            report(&status("opening", Some("refused")), &radio, &two(), None),
            json!({"state": "opening", "problems": ["modem-away"], "channels": [
                {"dial_hz": 3_590_000, "modem": "aether", "up": false},
                {"dial_hz": 7_101_000, "modem": "aether", "up": false}]})
        );
        assert_eq!(
            report(&status("stopped", Some("no modem")), &radio, &[], None),
            json!({"state": "stopped", "problems": ["setup-error"]})
        );
        assert_eq!(
            report(&status("stopped", None), &radio, &[], None),
            json!({"state": "stopped"})
        );
        // the radio outside the segments it may answer in
        assert_eq!(
            report(&status("outside", Some("…")), &radio, &two()[..1], None),
            json!({"state": "listening", "problems": ["outside-segment"], "channels": [
                {"dial_hz": 3_590_000, "modem": "aether", "up": false}]})
        );
    }

    #[test]
    fn what_it_cannot_tell_it_leaves_out() {
        let unknown = Radio::default();
        // no dial known: the only channel is up, two are left out
        assert_eq!(
            report(&status("listening", None), &unknown, &two()[1..], None),
            json!({"state": "listening", "channels": [
                {"dial_hz": 7_101_000, "modem": "aether", "up": true}]})
        );
        assert_eq!(
            report(&status("listening", None), &unknown, &two(), None),
            json!({"state": "listening"})
        );
        // several modems: all up while every one answers, left out while one does not
        let several = Radio {
            several: true,
            ..Radio::default()
        };
        assert_eq!(
            report(&status("listening", None), &several, &two(), None)["channels"],
            json!([{"dial_hz": 3_590_000, "modem": "aether", "up": true},
                   {"dial_hz": 7_101_000, "modem": "aether", "up": true}])
        );
        let one_away = Radio {
            several: true,
            others_away: 1,
            ..Radio::default()
        };
        assert_eq!(
            report(&status("listening", None), &one_away, &two(), None),
            json!({"state": "listening", "problems": ["modem-away"]})
        );
    }

    #[test]
    fn it_reports_on_change_every_five_minutes_and_never_twice_in_ten_seconds() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let listening = json!({"state": "listening"});
        let session = json!({"state": "session"});
        let mut r = Reporter::new();
        let first = r.next(true, &listening, at(0)).expect("at once");
        r.done(&first, &Outcome::Taken, at(0));
        assert_eq!(r.next(true, &listening, at(60)), None, "nothing new");
        assert_eq!(r.next(true, &session, at(5)), None, "within ten seconds");
        assert_eq!(
            r.next(true, &session, at(10)),
            Some(Send::Report(session.clone()))
        );
        assert_eq!(
            r.next(true, &listening, at(300)),
            Some(Send::Report(listening.clone())),
            "five minutes on, the same again"
        );
        // a failure waits a minute; a 404 an hour
        r.done(&Send::Report(session.clone()), &Outcome::Failed, at(400));
        assert_eq!(r.next(true, &session, at(459)), None);
        assert!(r.next(true, &session, at(460)).is_some());
        r.done(&Send::Report(session.clone()), &Outcome::NotTaken, at(460));
        assert_eq!(r.next(true, &session, at(460 + 3599)), None);
        assert!(r.next(true, &session, at(460 + 3600)).is_some());
    }

    #[test]
    fn turned_off_it_withdraws_once_and_turned_on_reports_again() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let listening = json!({"state": "listening"});
        let mut r = Reporter::new();
        // an earlier run may have reported: off at start withdraws
        assert_eq!(r.next(false, &listening, at(0)), Some(Send::Withdraw));
        r.done(&Send::Withdraw, &Outcome::Failed, at(0));
        assert_eq!(r.next(false, &listening, at(30)), None);
        assert_eq!(r.next(false, &listening, at(60)), Some(Send::Withdraw));
        r.done(&Send::Withdraw, &Outcome::Taken, at(60));
        assert_eq!(r.next(false, &listening, at(4000)), None, "once");
        assert_eq!(
            r.next(true, &listening, at(4000)),
            Some(Send::Report(listening.clone()))
        );
        r.done(&Send::Report(listening.clone()), &Outcome::Taken, at(4000));
        assert_eq!(r.next(false, &listening, at(4010)), Some(Send::Withdraw));
    }
}
