// SPDX-License-Identifier: MIT OR Apache-2.0
//! The record of every call this client places (choosing a gateway, §12.1, §15.2): one line of
//! JSON for each, appended to `selection/attempts/<YYYY-MM>.jsonl` in the first home, the
//! computer's, since a call is the station's whatever callsign placed it.
//!
//! A line is written when the call starts (`"stub": true`) and again, whole, when it ends. A reader
//! takes the last line of each `id`, so a call the program never finished (it was closed, it
//! crashed) is still there, as `unknown`. Lines are never rewritten: what is learned from them is
//! computed again from them, so a better method never loses the evidence.
//!
//! A record says what happened, never what was said: callsigns, the channel, times, counts and
//! sizes, the modem's readings and codes; no correspondent's address and no subject, as the log
//! keeps none. The position is where the station was, a GPS's fix or the place in use, to about a
//! metre: the records stay on this computer (§15.1).
//!
//! Why the records start before anything reads them: every later part of choosing a gateway
//! (learning, the ranking, its validation) rests on them, and a call not recorded is lost.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use rpmi::host::Event;
use serde_json::{Value, json};

use super::classify::{self, Class, Evidence};

/// Where the records are, under the first home.
pub const DIR: &str = "selection/attempts";

/// The records' format version.
pub const VERSION: u64 = 1;

/// A reading of the link's quality is kept no oftener than this, in seconds; the SNR's spread is
/// kept over every reading.
const QUALITY_EVERY_S: f64 = 5.0;

/// The most readings kept for one link: an hour at the pace above.
const QUALITY_MAX: usize = 720;

/// What a call sets out to carry, as known before it is placed (choosing a gateway, §13.1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Messages offered.
    pub messages: usize,
    /// Their air forms' bytes.
    pub out_bytes: u64,
    /// The bytes of the messages that waited at the last call and that this call will ask for.
    pub in_listed_bytes: u64,
    /// The highest priority offered (RP-2: 0 routine, 1 priority, 2 emergency).
    pub priority: u64,
}

/// A call about to be placed.
#[derive(Clone, Debug, Default)]
pub struct Call {
    /// The station called.
    pub remote: String,
    /// A gateway (RP-3 §5), not a station (§10).
    pub gateway: bool,
    /// The dial the call is placed on, when the operator gave one.
    pub dial_hz: Option<u64>,
    /// The bandwidth asked of the modem.
    pub bandwidth_hz: Option<i64>,
    /// How long the call's tries keep starting, for a gateway that scans (RP-5 §4.4).
    pub call_s: Option<u64>,
    /// Where the station is (§15.2): latitude, longitude and grid square, where they came from
    /// and, for a GPS's fix, its age; the grid square alone when that is all there is.
    pub place: Option<Value>,
    /// What it sets out to carry.
    pub plan: Plan,
}

/// What the session said, for the record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionNotes {
    /// What `WELCOME` said of the post office (RP-3 §5.2).
    pub online: Option<bool>,
    /// The codes of the other station's `ERROR`s.
    pub errors: Vec<String>,
    /// `NOTICE`s heard.
    pub notices: u32,
    /// Both stations said `DONE`.
    pub over: bool,
    /// This station refused the other.
    pub refused_other: bool,
    /// Messages the other station took, those queued at a cut-off gateway among them.
    pub taken: usize,
    /// Of those, queued (RP-7 §3.6).
    pub queued: usize,
    /// Messages received.
    pub received: usize,
}

/// One reading of the link's quality (RP-4 §5 `quality`), seconds after the link came up.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Reading {
    at_s: f64,
    snr_db: Option<f64>,
    rate_bps: Option<f64>,
    receiving: Option<bool>,
}

/// What the modem said during one call, as the connection to its adapter saw it.
#[derive(Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each a separate fact the modem reported"
)]
pub struct Trace {
    remote: String,
    begun: Instant,
    opened: bool,
    modem: Option<String>,
    as_call: Option<String>,
    busy: Option<bool>,
    busy_at_call: Option<bool>,
    call_started: Option<Instant>,
    call_ended: Option<Instant>,
    dial_hz: Option<u64>,
    heard_remote: bool,
    heard_snr_db: Option<f64>,
    connected: Option<Instant>,
    bandwidth_hz: Option<i64>,
    failed: Option<(String, Option<String>)>,
    disconnected: Option<(Instant, String)>,
    quality: Vec<Reading>,
    snrs: Vec<f64>,
    handed: u64,
    queued: Option<u64>,
    delivered: u64,
    received: u64,
    refused: Option<(String, String)>,
    adapter_fault: bool,
    silent: bool,
    session: Option<SessionNotes>,
}

fn seconds(since: Instant, at: Instant) -> f64 {
    (at.duration_since(since).as_secs_f64() * 10.0).round() / 10.0
}

impl Trace {
    /// A trace for a call to `remote`, begun now.
    #[must_use]
    pub fn new(remote: &str) -> Trace {
        Trace {
            remote: remote.to_owned(),
            begun: Instant::now(),
            opened: false,
            modem: None,
            as_call: None,
            busy: None,
            busy_at_call: None,
            call_started: None,
            call_ended: None,
            dial_hz: None,
            heard_remote: false,
            heard_snr_db: None,
            connected: None,
            bandwidth_hz: None,
            failed: None,
            disconnected: None,
            quality: Vec::new(),
            snrs: Vec::new(),
            handed: 0,
            queued: None,
            delivered: 0,
            received: 0,
            refused: None,
            adapter_fault: false,
            silent: false,
            session: None,
        }
    }

    /// The adapter answered `hello`: the modem's name, and the callsign the call is placed as.
    pub fn opened(&mut self, modem: &str, as_call: &str) {
        self.opened = true;
        self.modem = Some(modem.to_owned());
        self.as_call = Some(as_call.to_owned());
    }

    /// An event from the adapter (RP-4 §5).
    pub fn saw(&mut self, e: &Event) {
        let now = Instant::now();
        match e {
            Event::Busy(b) => self.busy = Some(*b),
            Event::Dial { dial_hz, .. } => self.dial_hz = Some(*dial_hz),
            Event::Heard {
                callsign, snr_db, ..
            } if self.call_started.is_some()
                && self.connected.is_none()
                && callsign.eq_ignore_ascii_case(&self.remote) =>
            {
                self.heard_remote = true;
                if let Some(s) = snr_db {
                    self.heard_snr_db = Some(self.heard_snr_db.map_or(*s, |b| b.max(*s)));
                }
            }
            Event::Connected { bandwidth, .. } if self.connected.is_none() => {
                self.connected = Some(now);
                self.call_ended = Some(now);
                self.bandwidth_hz = *bandwidth;
            }
            Event::Failed { code, text, .. } if self.connected.is_none() => {
                self.failed = Some((code.clone(), text.clone()));
                self.call_ended = Some(now);
            }
            Event::Disconnected { reason, .. }
                if self.connected.is_some() && self.disconnected.is_none() =>
            {
                self.disconnected = Some((now, reason.clone()));
            }
            Event::Quality {
                snr_db,
                rate_bps,
                receiving,
                ..
            } => {
                let Some(up) = self.connected else { return };
                let t = seconds(up, now);
                if let Some(s) = snr_db {
                    self.snrs.push(*s);
                }
                let due = self
                    .quality
                    .last()
                    .is_none_or(|last| t - last.at_s >= QUALITY_EVERY_S);
                if due && self.quality.len() < QUALITY_MAX {
                    self.quality.push(Reading {
                        at_s: t,
                        snr_db: *snr_db,
                        rate_bps: *rate_bps,
                        receiving: *receiving,
                    });
                }
            }
            Event::Received { data, .. } => self.received += data.len() as u64,
            Event::Queued { bytes, .. } => self.queued = Some(*bytes),
            Event::Delivered { .. } => self.delivered += 1,
            _ => {}
        }
    }

    /// A request to the adapter, as it is written: the call's start, and the bytes handed over.
    pub fn sending(&mut self, line: &rpmi::wire::Message) {
        match line.get("op").and_then(Value::as_str) {
            Some("connect") if self.call_started.is_none() => {
                self.call_started = Some(Instant::now());
                self.busy_at_call = self.busy;
            }
            Some("send") => {
                if let Some(bytes) = line
                    .get("data")
                    .and_then(Value::as_str)
                    .and_then(rpmi::base64::decode)
                {
                    self.handed += bytes.len() as u64;
                }
            }
            _ => {}
        }
    }

    /// The adapter refused a request of the call's own with an `error` (RP-4 §7).
    pub fn refused(&mut self, op: &str, code: &str) {
        if self.refused.is_none() && matches!(op, "connect" | "tune" | "dial") {
            self.refused = Some((op.to_owned(), code.to_owned()));
        }
    }

    /// The connection to the adapter failed or closed.
    pub fn fault(&mut self) {
        self.adapter_fault = true;
    }

    /// Nothing arrived for the silence of RP-3 §8.
    pub fn silence(&mut self) {
        self.silent = true;
    }

    /// What the session said, once it is over.
    pub fn session(&mut self, notes: SessionNotes) {
        self.session = Some(notes);
    }

    fn evidence(&self, left_out: usize) -> Evidence {
        let s = self.session.clone().unwrap_or_default();
        Evidence {
            opened: self.opened,
            refused_request: self.refused.as_ref().map(|(_, code)| code.clone()),
            adapter_fault: self.adapter_fault,
            failed: self.failed.as_ref().map(|(code, _)| code.clone()),
            heard_remote: self.heard_remote,
            connected: self.connected.is_some(),
            disconnected: self.disconnected.as_ref().map(|(_, r)| r.clone()),
            silent: self.silent,
            over: s.over,
            errors: s.errors,
            refused_other: s.refused_other,
            left_out,
        }
    }

    fn call_json(&self) -> Value {
        let result = if self.connected.is_some() {
            Value::from("connected")
        } else if let Some((code, _)) = &self.failed {
            Value::from(code.as_str())
        } else {
            Value::Null
        };
        json!({
            "busy_before": self.busy_at_call,
            "duration_s": match (self.call_started, self.call_ended) {
                (Some(a), Some(b)) => json!(seconds(a, b)),
                _ => Value::Null,
            },
            "result": result,
            "text": self.failed.as_ref().and_then(|(_, t)| t.clone()),
            "heard_remote": self.heard_remote,
            "heard_snr_db": self.heard_snr_db,
            "refused": self.refused.as_ref().map(|(op, code)| json!({"op": op, "code": code})),
        })
    }

    fn link_json(&self, ended: Instant) -> Value {
        let Some(up) = self.connected else {
            return Value::Null;
        };
        let end = self.disconnected.as_ref().map_or(ended, |(at, _)| *at);
        let mut snrs = self.snrs.clone();
        snrs.sort_by(f64::total_cmp);
        let snr = (!snrs.is_empty()).then(|| {
            json!({"min": snrs[0], "median": snrs[snrs.len() / 2],
                   "max": snrs[snrs.len() - 1], "readings": snrs.len()})
        });
        json!({
            "bandwidth_hz": self.bandwidth_hz,
            "duration_s": seconds(up, end),
            "end": self.disconnected.as_ref().map(|(_, r)| r.clone()),
            "quality": self
                .quality
                .iter()
                .map(|r| json!([r.at_s, r.snr_db, r.rate_bps, r.receiving]))
                .collect::<Vec<_>>(),
            "snr_db": snr,
            "bytes_handed": self.handed,
            "bytes_unacknowledged": self.queued,
            "deliveries": self.delivered,
            "bytes_in": self.received,
        })
    }
}

/// One call's record, from its start to its end.
pub struct Recorder {
    base: PathBuf,
    id: String,
    start: u64,
    call: Call,
    trace: Arc<Mutex<Trace>>,
}

impl Recorder {
    /// Starts the record of a call about to be placed at `start` (Unix seconds), and writes its
    /// stub. `base` is the first home.
    #[must_use]
    pub fn begin(base: &Path, start: u64, call: Call) -> Recorder {
        let trace = Arc::new(Mutex::new(Trace::new(&call.remote)));
        let record = Recorder {
            base: base.to_owned(),
            id: new_id(start),
            start,
            call,
            trace,
        };
        let stub = json!({
            "v": VERSION, "id": record.id, "start": start, "stub": true,
            "kind": record.kind(), "remote": record.call.remote,
            "channel": record.channel_json(None),
        });
        record.append(&stub);
        record
    }

    /// The trace the connection to the adapter feeds.
    #[must_use]
    pub fn trace(&self) -> Arc<Mutex<Trace>> {
        Arc::clone(&self.trace)
    }

    fn kind(&self) -> &'static str {
        if self.call.gateway {
            "gateway"
        } else {
            "station"
        }
    }

    fn channel_json(&self, dial_read: Option<u64>) -> Value {
        json!({
            "dial_hz": self.call.dial_hz.or(dial_read),
            "bandwidth_hz": self.call.bandwidth_hz,
            "call_s": self.call.call_s,
        })
    }

    /// Ends the record: classes the call from what was traced and writes it whole. `why` is the
    /// error the call ended in, when it ended in one. Returns the record as written.
    pub fn finish(self, why: Option<&str>) -> Value {
        let ended = Instant::now();
        let t = self.trace.lock().unwrap_or_else(PoisonError::into_inner);
        let plan = &self.call.plan;
        let taken = t.session.as_ref().map_or(0, |s| s.taken);
        let left_out = plan.messages.saturating_sub(taken);
        let class = classify::classify(&t.evidence(left_out));
        let elapsed = ended.duration_since(t.begun).as_secs();
        let session = t.session.as_ref().map(|s| {
            json!({
                "online": s.online, "errors": s.errors, "notices": s.notices, "over": s.over,
                "refused_other": s.refused_other, "taken": s.taken, "queued": s.queued,
                "received": s.received,
            })
        });
        let record = json!({
            "v": VERSION,
            "id": self.id,
            "start": self.start,
            "end": self.start + elapsed,
            "kind": self.kind(),
            "remote": self.call.remote,
            "as": t.as_call,
            "channel": self.channel_json(t.dial_hz),
            "modem": t.modem,
            "place": self.call.place,
            "station": Value::Null,
            "predicted": Value::Null,
            "transaction": {
                "messages": plan.messages, "out_bytes": plan.out_bytes,
                "in_listed_bytes": plan.in_listed_bytes, "priority": plan.priority,
            },
            "call": t.call_json(),
            "link": t.link_json(ended),
            "session": session,
            "outcome": {
                "class": class.name(),
                "plan_done": class.completed() && left_out == 0,
                "left_out": left_out,
                "why": why,
            },
            "classifier": classify::RULES,
        });
        drop(t);
        self.append(&record);
        record
    }

    /// Appends a line to this month's file. A record that cannot be written is said in the log
    /// and dropped: it must never stop a call.
    fn append(&self, line: &Value) {
        let dir = self.base.join(DIR);
        let path = dir.join(format!("{}.jsonl", month_of(self.start)));
        let written = fs::create_dir_all(&dir).and_then(|()| {
            let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
            writeln!(f, "{line}")
        });
        if let Err(e) = written {
            radiopost::logfile::line(&format!(
                "could not keep the record of a call in {}: {e}",
                path.display()
            ));
        }
    }
}

/// Every call recorded under the first home `base`, oldest first: the last line of each `id`, and
/// a call that has only its stub marked `unknown` (the program ended before the call did).
#[must_use]
pub fn read(base: &Path) -> Vec<Value> {
    let mut files: Vec<PathBuf> = fs::read_dir(base.join(DIR))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    files.sort();
    let mut by_id: BTreeMap<String, Value> = BTreeMap::new();
    for file in files {
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if let Some(id) = v["id"].as_str() {
                by_id.insert(id.to_owned(), v);
            }
        }
    }
    let mut all: Vec<Value> = by_id
        .into_values()
        .map(|mut v| {
            if v["stub"] == true {
                v["outcome"] = json!({"class": Class::Unknown.name(), "plan_done": false,
                    "why": "the program ended before the call did"});
            }
            v
        })
        .collect();
    all.sort_by_key(|v| (v["start"].as_u64(), v["id"].as_str().map(str::to_owned)));
    all
}

/// A record's ID: `a-YYYYMMDD-HHMMSS-xxxx`, its start in UTC and four hex digits drawn at random.
fn new_id(start: u64) -> String {
    let mut r = [0u8; 2];
    let _ = getrandom::fill(&mut r);
    let (y, m, d, hh, mm, ss) = civil(start);
    format!(
        "a-{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}-{:02x}{:02x}",
        r[0], r[1]
    )
}

/// The month a time falls in, UTC: `YYYY-MM`, the file its record goes in.
fn month_of(at: u64) -> String {
    let (y, m, ..) = civil(at);
    format!("{y:04}-{m:02}")
}

/// Unix seconds as a UTC date and time: (year, month, day, hour, minute, second). Days to the
/// civil date by Howard Hinnant's method, which needs no table.
fn civil(at: u64) -> (i64, u64, u64, u64, u64, u64) {
    let days = i64::try_from(at / 86_400).unwrap_or(i64::MAX);
    let secs = at % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (
        y,
        u64::try_from(m).unwrap_or(1),
        u64::try_from(d).unwrap_or(1),
        secs / 3_600,
        secs % 3_600 / 60,
        secs % 60,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("radiopost-attempts-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a directory");
        dir
    }

    #[test]
    fn dates_are_utc() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0, 0));
        // 2026-11-20 03:15:22 UTC
        assert_eq!(civil(1_795_144_522), (2026, 11, 20, 3, 15, 22));
        // the last second of a leap year's February
        assert_eq!(civil(951_868_799), (2000, 2, 29, 23, 59, 59));
        assert_eq!(month_of(1_795_144_522), "2026-11");
        let id = new_id(1_795_144_522);
        assert!(id.starts_with("a-20261120-031522-"), "{id}");
        assert_eq!(id.len(), "a-20261120-031522-7f3a".len());
    }

    #[test]
    fn a_stub_alone_reads_as_unknown_and_a_finished_call_as_its_class() {
        let base = scratch("read");
        let call = Call {
            remote: "KK4ODA-1".into(),
            gateway: true,
            dial_hz: Some(7_101_000),
            plan: Plan {
                messages: 2,
                out_bytes: 900,
                ..Plan::default()
            },
            ..Call::default()
        };
        let crashed = Recorder::begin(&base, 1_795_230_900, call.clone());
        drop(crashed);
        let done = Recorder::begin(&base, 1_795_230_922, call);
        {
            let mut t = done.trace.lock().expect("the trace");
            t.opened("Aether HF 0.2.0", "KK4ODA");
            t.sending(&serde_json::from_value(json!({"op": "connect", "id": 3})).expect("a line"));
            t.saw(&Event::Failed {
                id: None,
                code: "timeout".into(),
                text: Some("no answer".into()),
            });
        }
        let written = done.finish(None);
        assert_eq!(written["outcome"]["class"], "no-answer");
        assert_eq!(written["outcome"]["left_out"], 2);
        assert_eq!(written["call"]["result"], "timeout");
        assert_eq!(written["channel"]["dial_hz"], 7_101_000);
        let all = read(&base);
        assert_eq!(all.len(), 2, "{all:?}");
        assert_eq!(all[0]["outcome"]["class"], "unknown");
        assert_eq!(all[1]["outcome"]["class"], "no-answer");
        assert_eq!(all[1]["modem"], "Aether HF 0.2.0");
        assert!(
            all[1].get("stub").is_none(),
            "the whole line replaces the stub"
        );
        let files = fs::read_dir(base.join(DIR)).expect("the folder").count();
        assert_eq!(files, 1, "one month, one file");
    }

    #[test]
    fn a_link_keeps_its_readings_spaced_and_its_snr_spread_whole() {
        let mut t = Trace::new("N0CALL");
        t.opened("the loopback adapter", "KK4ODA");
        t.saw(&Event::Connected {
            id: None,
            link: "c1".into(),
            remote: "N0CALL".into(),
            local: "KK4ODA".into(),
            direction: rpmi::host::Direction::Out,
            bandwidth: Some(2300),
        });
        for snr in [3.0, 9.0, 6.0] {
            t.saw(&Event::Quality {
                link: None,
                snr_db: Some(snr),
                rate_bps: Some(900.0),
                receiving: Some(true),
            });
        }
        let link = t.link_json(Instant::now());
        assert_eq!(link["quality"].as_array().map(Vec::len), Some(1), "{link}");
        assert_eq!(link["snr_db"]["readings"], 3);
        assert_eq!(link["snr_db"]["min"], 3.0);
        assert_eq!(link["snr_db"]["median"], 6.0);
        assert_eq!(link["snr_db"]["max"], 9.0);
        assert_eq!(link["bandwidth_hz"], 2300);
    }

    #[test]
    fn the_station_called_counts_as_heard_only_during_the_call() {
        let heard = |callsign: &str| Event::Heard {
            callsign: callsign.into(),
            snr_db: Some(-12.0),
            bandwidth: None,
        };
        let mut t = Trace::new("KK4ODA-1");
        t.saw(&heard("KK4ODA-1"));
        assert!(!t.heard_remote, "before the call is placed");
        t.sending(&serde_json::from_value(json!({"op": "connect", "id": 1})).expect("a line"));
        t.saw(&heard("N0CALL"));
        assert!(!t.heard_remote, "another station");
        t.saw(&heard("kk4oda-1"));
        assert!(t.heard_remote);
        assert_eq!(t.heard_snr_db, Some(-12.0));
    }
}
