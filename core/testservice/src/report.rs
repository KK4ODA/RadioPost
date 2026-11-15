// SPDX-License-Identifier: MIT OR Apache-2.0
//! A session's record: every transmission each way, decoded, what the service's engine made of
//! them, and the checks a caller's program is judged by. Kept as JSON in the reports' directory,
//! the newest [`KEEP`], and shown as a page.

use std::fmt::Write as _;
use std::path::Path;

use radiopost::ErrorCode;
use radiopost::cbor::{Value, hex};
use radiopost::pdu::{Deframer, kind};
use radiopost::session::Event as SessionEvent;
use serde_json::{Value as Json, json};

/// How many reports are kept.
pub const KEEP: usize = 500;

/// Who sent a transmission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum From {
    /// The program under test.
    Caller,
    /// The service's station.
    Service,
}

/// One session, as it went.
#[derive(Debug)]
pub struct Record {
    id: String,
    station: String,
    caller: String,
    started: u64,
    ended: u64,
    transmissions: Vec<(From, Vec<u8>)>,
    events: Vec<String>,
    notes: Vec<String>,
    ended_by: Option<String>,
}

impl Record {
    /// A session that came up at `started`, the service's station `station` called by `caller`.
    #[must_use]
    pub fn new(station: &str, caller: &str, started: u64) -> Record {
        let draw = crate::random::<4>();
        // ids sort as the sessions came: milliseconds, then a draw
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        Record {
            id: format!("{ms:013}-{}", hex(&draw)),
            station: station.to_uppercase(),
            caller: caller.to_uppercase(),
            started,
            ended: started,
            transmissions: Vec::new(),
            events: Vec::new(),
            notes: Vec::new(),
            ended_by: None,
        }
    }

    /// Bytes that crossed.
    pub fn transmission(&mut self, from: From, bytes: &[u8]) {
        self.transmissions.push((from, bytes.to_vec()));
    }

    /// What the engine reported.
    pub fn events(&mut self, events: &[SessionEvent]) {
        self.events.extend(events.iter().map(|e| format!("{e:?}")));
    }

    /// A line for the report.
    pub fn note(&mut self, line: &str) {
        self.notes.push(line.to_owned());
    }

    /// The session is over: `why`, when something other than a hang-up ended it.
    pub fn end(&mut self, at: u64, why: Option<String>) {
        self.ended = at;
        self.ended_by = why;
    }

    /// Its id, which names its report.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The report: the session, its transmissions decoded, and its checks.
    #[must_use]
    pub fn to_json(&self) -> Json {
        let transmissions: Vec<Json> = self
            .transmissions
            .iter()
            .enumerate()
            .map(|(i, (from, bytes))| {
                let (pdus, problem) = decode(bytes);
                json!({
                    "number": i + 1,
                    "from": if *from == From::Caller { "caller" } else { "service" },
                    "bytes": bytes.len(),
                    "hex": hex(bytes),
                    "pdus": pdus.iter().map(Value::diagnostic).collect::<Vec<_>>(),
                    "problem": problem,
                })
            })
            .collect();
        let checks = self.checks();
        json!({
            "id": self.id,
            "station": self.station,
            "caller": self.caller,
            "started": self.started,
            "ended": self.ended,
            "ended_by": self.ended_by,
            "passed": checks.iter().all(|c| c["passed"] == true),
            "checks": checks,
            "transmissions": transmissions,
            "events": self.events,
            "notes": self.notes,
        })
    }

    /// What the caller's program is judged by, each with the specification it rests on.
    fn checks(&self) -> Vec<Json> {
        let mut checks = Vec::new();
        let mut check = |name: &str, rp: &str, problem: Option<String>| {
            checks.push(json!({
                "check": name, "rp": rp, "passed": problem.is_none(), "detail": problem,
            }));
        };
        let caller: Vec<(usize, &Vec<u8>)> = self
            .transmissions
            .iter()
            .enumerate()
            .filter(|(_, (f, _))| *f == From::Caller)
            .map(|(i, (_, b))| (i + 1, b))
            .collect();
        let service: Vec<&Vec<u8>> = self
            .transmissions
            .iter()
            .filter(|(f, _)| *f == From::Service)
            .map(|(_, b)| b)
            .collect();
        // RP-3 §2: every PDU length-framed, deterministic CBOR, a known type
        let framing = caller
            .iter()
            .find_map(|(n, b)| decode(b).1.map(|p| format!("transmission {n}: {p}")));
        check(
            "Every PDU framed, in deterministic CBOR, of a known type",
            "RP-3 §2, RP-0 §4.1",
            framing,
        );
        // RP-3 §5.1: the caller speaks first, with HELLO
        let first = caller
            .first()
            .and_then(|(_, b)| decode(b).0.into_iter().next())
            .and_then(|p| p.get(0).and_then(Value::as_u64));
        check(
            "The caller's first PDU is HELLO",
            "RP-3 §5.1",
            match first {
                Some(kind::HELLO) => None,
                Some(k) => Some(format!("it is of type {k}")),
                None => Some("the caller sent nothing the service could read".into()),
            },
        );
        // RP-1 §5, §6.3, RP-3 §7: nothing refused
        let refused = service.iter().flat_map(|b| decode(b).0).find_map(|p| {
            (p.get(0).and_then(Value::as_u64) == Some(kind::ERROR)).then(|| {
                let code = p.get(1).and_then(Value::as_u64).unwrap_or(0);
                let name = ErrorCode::from_code(code).map_or("unknown", ErrorCode::name);
                let text = p.get(2).and_then(Value::as_text).unwrap_or_default();
                format!("the service answered ERROR {code} ({name}) {text}")
                    .trim()
                    .to_owned()
            })
        });
        check(
            "Credential, statement and every PDU accepted",
            "RP-1 §5, §6.3, RP-3 §5, §7",
            refused,
        );
        // RP-3 §8: both sent DONE, and the caller hung up
        let done = |list: &[&Vec<u8>]| {
            list.iter()
                .flat_map(|b| decode(b).0)
                .any(|p| p.get(0).and_then(Value::as_u64) == Some(kind::DONE))
        };
        let caller_bytes: Vec<&Vec<u8>> = caller.iter().map(|(_, b)| *b).collect();
        let ended = if done(&caller_bytes) {
            self.ended_by
                .as_ref()
                .map(|why| format!("the session did not end by the caller hanging up: {why}"))
        } else {
            Some("the caller never sent DONE".to_owned())
        };
        check(
            "The session ended with DONE, and the caller hung up",
            "RP-3 §5.7, §8",
            ended,
        );
        checks
    }
}

/// A transmission's PDUs, and what is wrong with it, if anything: a PDU that is not RP-3 §2's,
/// or bytes left over that are no whole PDU.
fn decode(bytes: &[u8]) -> (Vec<Value>, Option<String>) {
    let mut d = Deframer::new();
    d.push(bytes);
    let mut pdus = Vec::new();
    loop {
        match d.next(usize::from(u16::MAX)) {
            Some(Ok(p)) => pdus.push(p),
            Some(Err(code)) => return (pdus, Some(format!("a PDU refused as {}", code.name()))),
            None => break,
        }
    }
    let problem = (d.held() > 0).then(|| format!("{} bytes that are no whole PDU", d.held()));
    let unknown = pdus.iter().find_map(|p| {
        let k = p.get(0).and_then(Value::as_u64);
        (!matches!(k, Some(1..=11))).then(|| format!("a PDU of unknown type {k:?}"))
    });
    (pdus, problem.or(unknown))
}

/// Keeps the report, and lets the oldest go past [`KEEP`].
///
/// # Errors
///
/// The directory cannot be written.
pub fn save(dir: &Path, record: &Record) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{}.json", record.id()));
    let text = serde_json::to_string_pretty(&record.to_json()).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut all = list_files(dir);
    while all.len() > KEEP {
        let oldest = all.remove(0);
        let _ = std::fs::remove_file(dir.join(oldest));
    }
    Ok(())
}

/// The reports' file names, oldest first (their ids begin with the time).
fn list_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| {
            std::path::Path::new(n)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        })
        .collect();
    names.sort();
    names
}

/// The reports, newest first, each without its transmissions.
#[must_use]
pub fn list(dir: &Path) -> Vec<Json> {
    list_files(dir)
        .iter()
        .rev()
        .filter_map(|n| read(dir, n.trim_end_matches(".json")))
        .map(|mut r| {
            if let Some(o) = r.as_object_mut() {
                o.remove("transmissions");
                o.remove("events");
            }
            r
        })
        .collect()
}

/// One report, by its id.
#[must_use]
pub fn read(dir: &Path, id: &str) -> Option<Json> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return None;
    }
    let text = std::fs::read_to_string(dir.join(format!("{id}.json"))).ok()?;
    serde_json::from_str(&text).ok()
}

/// Text made safe for HTML.
#[must_use]
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A report as a page's body.
#[must_use]
pub fn html(report: &Json) -> String {
    let t = |k: &str| escape(report[k].as_str().unwrap_or(""));
    let mut out = format!(
        "<h1>{} called {}</h1><p>Session {}, {} to {} (Unix seconds). <a href=\"/runs/{}.json\">JSON</a></p>",
        t("caller"),
        t("station"),
        t("id"),
        report["started"],
        report["ended"],
        t("id"),
    );
    out.push_str(
        "<h2>Checks</h2><table><tr><th>Check</th><th>Specification</th><th>Result</th></tr>",
    );
    for c in report["checks"].as_array().into_iter().flatten() {
        let passed = c["passed"] == true;
        let _ = write!(
            out,
            "<tr><td>{}</td><td>{}</td><td class=\"{}\">{}</td></tr>",
            escape(c["check"].as_str().unwrap_or("")),
            escape(c["rp"].as_str().unwrap_or("")),
            if passed { "ok" } else { "bad" },
            if passed {
                "passed".to_owned()
            } else {
                format!("failed: {}", escape(c["detail"].as_str().unwrap_or("")))
            }
        );
    }
    out.push_str("</table><h2>Transmissions</h2>");
    for tr in report["transmissions"].as_array().into_iter().flatten() {
        let _ = write!(
            out,
            "<h3>{} from the {}, {} bytes</h3>",
            tr["number"],
            escape(tr["from"].as_str().unwrap_or("")),
            tr["bytes"]
        );
        if let Some(p) = tr["problem"].as_str() {
            let _ = write!(out, "<p class=\"bad\">{}</p>", escape(p));
        }
        for p in tr["pdus"].as_array().into_iter().flatten() {
            let _ = write!(out, "<pre>{}</pre>", escape(p.as_str().unwrap_or("")));
        }
    }
    out.push_str("<h2>What the service's station made of it</h2><ul>");
    for e in report["events"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(report["notes"].as_array().into_iter().flatten())
    {
        let _ = write!(
            out,
            "<li><code>{}</code></li>",
            escape(e.as_str().unwrap_or(""))
        );
    }
    out.push_str("</ul>");
    out
}
