// SPDX-License-Identifier: MIT OR Apache-2.0
//! Alerts between stations (RP-8, wishlist W52), and the settings the panel sounds by (W51).
//!
//! * **Circles.** Each is a keyword the operator typed, kept as its key and its normalised
//!   keyword in `circles.json` in the first home, with how loud its alerts are and whether the
//!   station says `heard` by itself. The page shows the keyword only when asked
//!   ([`Alerts::keyword`]), so the operator can tell it in person to someone joining (RP-8 §2).
//!   The computer's, not a callsign's: one alarm for every callsign it works.
//! * **The log.** Alerts sent and heard, with their answers, in `alerts.json` beside it: the last
//!   [`KEPT`].
//! * **Carried by whoever holds the modem.** A broadcast heard comes in through [`Alerts::heard`]
//!   from any connection: the panel's connection between calls ([`crate::presence`]), or a station
//!   listening ([`crate::session`]); both may hand over the same one, and the second is let go.
//!   Frames to send wait in a queue that the connection holding the station's callsigns takes
//!   ([`Alerts::due`]) and reports on ([`Alerts::went`]).
//! * **Sans clock.** Every call takes `now`, so the tests replay a morning in a moment.
//! * **Sounds and levels** (`notify.json`): the page's own settings, kept here so they hold
//!   whatever port the panel is on. The page decides what to sound; [`Alerts::alarming`] tells the
//!   window to come forward.
//! * **Reports** (RP-8 0.3.0, the forms design's step 5): a form sent to a circle by broadcast,
//!   once or again every few minutes ([`Alerts::report`], [`Alerts::repeat_due`]), and the reports
//!   sent and heard, the last [`KEPT`], in [`REPORTS`] beside the log, for the Reports view. Nobody
//!   answers a report.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use radiopost::alert::{self, Circle, Draft, Frame, Heard, Kind};
use radiopost::cbor::hex;
use serde_json::{Value, json};

use crate::callsigns;
use crate::store::{Error, Store, unhex, write};

/// The reports sent and heard (RP-8 §7), in the first home.
pub const REPORTS: &str = "circle-reports.json";
/// How long a report goes on being sent again, at most, unless the operator starts it again
/// (RP-8 §7, §8).
pub const REPEAT_FOR_S: u64 = 4 * 3600;
/// How often a report may be sent again: the choices offered (RP-8 §7).
pub const REPEAT_EVERY_S: [u64; 4] = [600, 900, 1800, 3600];

/// The circles' file, in the first home.
pub const CIRCLES: &str = "circles.json";
/// The alerts' log, in the first home.
pub const LOG: &str = "alerts.json";
/// The page's sound and notification settings, in the first home.
pub const NOTIFY: &str = "notify.json";
/// How many alerts the log keeps.
pub const KEPT: usize = 200;
/// How long a sender waits for `heard` before it sends the alert once more (RP-8 §5).
pub const RETRY_AFTER_S: u64 = 60;
/// How many times an alert goes out at most (RP-8 §5: once more).
const TRIES: u32 = 2;
/// The spread of the stations' `heard` to an alert for the whole circle (RP-8 §5).
const CIRCLE_SPREAD_S: u64 = 10;
/// Alerts from one callsign that sound within [`LOUD_WINDOW_S`]; the rest are shown quietly.
const LOUD_AT_MOST: usize = 5;
const LOUD_WINDOW_S: u64 = 600;
/// One broadcast handed over by two connections arrives twice within this; a sender's second try
/// comes [`RETRY_AFTER_S`] later.
const SAME_TRANSMISSION_S: u64 = 5;
/// How long a frame handed to the modem may go without `broadcast-sent` before it is given up.
const FLYING_AT_MOST_S: u64 = 180;
/// How long an alert may wait for the modem before it is given up: the station busy with a
/// session, or a modem that carries no broadcasts.
const WAITING_AT_MOST_S: u64 = 300;
/// The most the page's settings may hold.
const NOTIFY_MAX: usize = 64 << 10;

/// How loudly something arrives: the same four levels for a circle's alerts and for mail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Counted, no sound.
    Silent,
    /// A soft sound.
    Normal,
    /// Its own sound, and a banner until it is opened.
    Important,
    /// An alarm until acknowledged, through quiet hours.
    TimeSensitive,
}

impl Level {
    /// Its name, as the files and the page write it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Level::Silent => "silent",
            Level::Normal => "normal",
            Level::Important => "important",
            Level::TimeSensitive => "time-sensitive",
        }
    }

    /// A level by its name.
    #[must_use]
    pub fn named(name: &str) -> Option<Level> {
        [
            Level::Silent,
            Level::Normal,
            Level::Important,
            Level::TimeSensitive,
        ]
        .into_iter()
        .find(|l| l.name() == name)
    }
}

/// A circle as this computer keeps it.
#[derive(Clone, Debug)]
pub struct Kept {
    /// Its name here; the other members may call it something else.
    pub name: String,
    /// Its key and ID.
    pub circle: Circle,
    /// Its keyword, normalised (RP-8 §2), to tell someone joining. `None` for a circle kept
    /// before keywords were kept (0.1.0-beta.41 and earlier) until it is typed again.
    pub keyword: Option<String>,
    /// How its alerts arrive.
    pub level: Level,
    /// The station says `heard` by itself (RP-8 §5, §8).
    pub answer: bool,
    /// The circle's watch dial, agreed with the others like the keyword: where an alert to the
    /// whole circle, or to a station with no listed gateway, goes (RP-8 §7, D46).
    pub watch_hz: Option<u64>,
}

impl Kept {
    fn json(&self) -> Value {
        json!({"name": self.name, "key": hex(&self.circle.key), "keyword": self.keyword,
               "level": self.level.name(), "answer": self.answer, "watch_hz": self.watch_hz})
    }

    fn from_json(v: &Value) -> Option<Kept> {
        let key: [u8; 32] = unhex(v["key"].as_str()?)?.try_into().ok()?;
        Some(Kept {
            name: v["name"].as_str()?.to_owned(),
            circle: Circle::from_key(key),
            keyword: v["keyword"].as_str().map(str::to_owned),
            level: v["level"]
                .as_str()
                .and_then(Level::named)
                .unwrap_or(Level::TimeSensitive),
            answer: v["answer"].as_bool().unwrap_or(true),
            watch_hz: v["watch_hz"].as_u64().filter(|&hz| hz > 0),
        })
    }

    /// What the page shows of it: never the key, and whether its keyword is kept but not the
    /// keyword itself, which [`Alerts::keyword`] gives when the operator asks.
    fn shown(&self) -> Value {
        json!({"name": self.name, "check": self.circle.check(), "level": self.level.name(),
               "answer": self.answer, "watch_hz": self.watch_hz,
               "keyword_kept": self.keyword.is_some()})
    }
}

/// An answer to an alert this station sent.
#[derive(Clone, Debug)]
struct Answer {
    kind: Kind,
    from: String,
    time: u64,
    text: Option<String>,
}

/// An alert sent or heard.
#[derive(Clone, Debug)]
struct Entry {
    /// Its tag, which names it (RP-8 §3).
    tag: [u8; 8],
    /// This station sent it.
    out: bool,
    /// Its circle's name here, when it was sent or heard.
    circle: String,
    from: String,
    to: Option<String>,
    text: String,
    /// The alert's own time.
    time: u64,
    /// Heard: when this station heard it. Sent: when it was written.
    at: u64,
    /// Sent: `waiting`, `sent`, `heard`, `seen`, `replied`, `not heard` or `failed`. Heard:
    /// `new`, `seen` or `replied`.
    state: &'static str,
    /// How it arrived (heard alerts), from its circle.
    level: Level,
    /// Heard, but shown without a sound: the sender's sixth in ten minutes.
    quiet: bool,
    /// Sent: the frame, to send again.
    frame: Vec<u8>,
    /// Sent: when it went out last, and how many times.
    sent_at: Option<u64>,
    tries: u32,
    /// Sent: the answers heard, in order.
    answers: Vec<Answer>,
    /// Heard: the line this station replied with.
    reply: Option<String>,
    /// Heard: the station's callsign it answers as, the one it was sent to.
    me: String,
    /// Sent and failed: why.
    why: Option<String>,
    /// Sent: the dial it goes on. Heard: the dial the modem heard it on, where its answers go
    /// (RP-8 §5). `None`: where the radio is, or a modem that says no dial.
    dial_hz: Option<u64>,
    /// Sent: how long its copies keep going, the `scan_s` of the station alerted (RP-8 §5).
    outlast_s: Option<u64>,
    /// Sent: until when this sending's copies go on, set as its first copy goes to the modem.
    copies_until: Option<u64>,
}

impl Entry {
    fn json(&self) -> Value {
        json!({
            "tag": hex(&self.tag), "out": self.out, "circle": self.circle, "from": self.from,
            "to": self.to, "text": self.text, "time": self.time, "at": self.at,
            "state": self.state, "level": self.level.name(), "quiet": self.quiet,
            "frame": hex(&self.frame), "sent_at": self.sent_at, "tries": self.tries,
            "answers": self.answers.iter().map(|a| json!({"kind": a.kind.name(), "from": a.from,
                "time": a.time, "text": a.text})).collect::<Vec<_>>(),
            "reply": self.reply, "why": self.why, "me": self.me,
            "dial_hz": self.dial_hz, "outlast_s": self.outlast_s,
        })
    }

    fn from_json(v: &Value) -> Option<Entry> {
        let tag: [u8; 8] = unhex(v["tag"].as_str()?)?.try_into().ok()?;
        let state = [
            "waiting",
            "sent",
            "heard",
            "seen",
            "replied",
            "not heard",
            "failed",
            "new",
        ]
        .into_iter()
        .find(|s| v["state"].as_str() == Some(*s))?;
        let answers = v["answers"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| {
                        Some(Answer {
                            kind: Kind::named(x["kind"].as_str()?)?,
                            from: x["from"].as_str()?.to_owned(),
                            time: x["time"].as_u64()?,
                            text: x["text"].as_str().map(str::to_owned),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let text = |k: &str| v[k].as_str().map(str::to_owned);
        Some(Entry {
            tag,
            out: v["out"].as_bool()?,
            circle: text("circle").unwrap_or_default(),
            from: text("from")?,
            to: text("to"),
            text: text("text").unwrap_or_default(),
            time: v["time"].as_u64()?,
            at: v["at"].as_u64().unwrap_or(0),
            state,
            level: v["level"]
                .as_str()
                .and_then(Level::named)
                .unwrap_or(Level::TimeSensitive),
            quiet: v["quiet"].as_bool().unwrap_or(false),
            frame: v["frame"].as_str().and_then(unhex).unwrap_or_default(),
            sent_at: v["sent_at"].as_u64(),
            tries: v["tries"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
            answers,
            reply: text("reply"),
            why: text("why"),
            me: text("me").unwrap_or_default(),
            dial_hz: v["dial_hz"].as_u64().filter(|&hz| hz > 0),
            outlast_s: v["outlast_s"].as_u64(),
            copies_until: None,
        })
    }

    /// What the page shows: the frame stays here.
    fn shown(&self) -> Value {
        let mut v = self.json();
        if let Some(o) = v.as_object_mut() {
            o.remove("frame");
            o.insert("alarm".into(), self.alarms().into());
        }
        v
    }

    /// A heard alert that sounds until it is acknowledged.
    fn alarms(&self) -> bool {
        !self.out && self.state == "new" && !self.quiet && self.level == Level::TimeSensitive
    }
}

/// A report sent or heard (RP-8 §3, §7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CircleReport {
    /// Its tag, which names it.
    pub tag: [u8; 8],
    /// This station sent it.
    pub out: bool,
    /// Its circle's name here.
    pub circle: String,
    /// The sender's callsign.
    pub from: String,
    /// The station it was for; `None`, the whole circle.
    pub to: Option<String>,
    /// Its own time.
    pub time: u64,
    /// When it was heard, or written.
    pub at: u64,
    /// The form.
    pub form: radiopost::forms::Form,
    /// Sent: `waiting`, `sent` or `failed`. Heard: `heard`.
    pub state: &'static str,
    /// The dial it was sent or heard on.
    pub dial_hz: Option<u64>,
}

impl CircleReport {
    fn json(&self) -> Value {
        json!({"tag": hex(&self.tag), "out": self.out, "circle": self.circle, "from": self.from,
               "to": self.to, "time": self.time, "at": self.at, "state": self.state,
               "form": hex(&self.form.to_cbor().encode().unwrap_or_default()),
               "template": hex(&self.form.template), "dial_hz": self.dial_hz})
    }

    fn from_json(v: &Value) -> Option<CircleReport> {
        let form = radiopost::cbor::decode(&unhex(v["form"].as_str()?)?).ok()?;
        let state = ["waiting", "sent", "failed", "heard"]
            .into_iter()
            .find(|s| v["state"].as_str() == Some(*s))?;
        Some(CircleReport {
            tag: unhex(v["tag"].as_str()?)?.try_into().ok()?,
            out: v["out"].as_bool()?,
            circle: v["circle"].as_str()?.to_owned(),
            from: v["from"].as_str()?.to_owned(),
            to: v["to"].as_str().map(str::to_owned),
            time: v["time"].as_u64()?,
            at: v["at"].as_u64()?,
            form: radiopost::forms::Form::from_value(&form)?,
            state,
            dial_hz: v["dial_hz"].as_u64().filter(|&hz| hz > 0),
        })
    }
}

/// A report sent again every few minutes while the window is open (RP-8 §7): the fields in `here`
/// get the station's position as it is then.
#[derive(Clone, Debug)]
struct Repeat {
    circle: String,
    to: Option<String>,
    form: radiopost::forms::Form,
    from: String,
    how: Sending,
    every_s: u64,
    next_at: u64,
    until: u64,
    here: Vec<u64>,
}

/// A frame waiting for the modem.
#[derive(Clone, Debug)]
struct Outgoing {
    frame: Vec<u8>,
    /// Not before this time: the spread of an answer to the whole circle.
    not_before: u64,
    /// The alert it is, when it is one this station sent (answers have none).
    alert: Option<[u8; 8]>,
    /// The alert it answers, when it is an answer.
    answers: Option<[u8; 8]>,
    /// The report it is, when it is one this station sent.
    report: Option<[u8; 8]>,
    /// The dial it belongs on (RP-4 §6's `dial_hz`).
    dial_hz: Option<u64>,
}

/// How an alert goes (RP-8 §5, §7): on a dial, the radio tuned there to send it; and, when the
/// station alerted scans that dial, its copies going on for `outlast_s`, its gateway's `scan_s`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sending {
    /// The dial; `None` is wherever the radio is.
    pub dial_hz: Option<u64>,
    /// How long its copies go on, from the first (only with a dial).
    pub outlast_s: Option<u64>,
}

/// A frame due for the modem: its reference for `broadcast`, its bytes, and its dial. An alert
/// of this station's has the radio tuned there first (`tune`); an answer names the dial it was
/// heard on, which a scanning modem goes back to (RP-8 §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Due {
    /// The reference to report `broadcast-sent` with ([`Alerts::went`]).
    pub reference: String,
    /// The frame.
    pub frame: Vec<u8>,
    /// The dial it belongs on.
    pub dial_hz: Option<u64>,
    /// Tune the radio to `dial_hz` before it goes: an alert this station sends.
    pub tune: bool,
}

/// A frame the modem has, until `broadcast-sent`.
#[derive(Clone, Debug)]
struct Flying {
    reference: String,
    outgoing: Outgoing,
    at: u64,
}

#[derive(Debug)]
struct State {
    base: PathBuf,
    circles: Vec<Kept>,
    entries: Vec<Entry>,
    /// Tags heard, and when last.
    heard: HashMap<[u8; 8], u64>,
    queue: VecDeque<Outgoing>,
    flying: Vec<Flying>,
    references: u64,
    /// Moves on every change, so the page knows to look.
    seq: u64,
    /// Broadcasts dropped without a word (RP-8 §4), told to the log once a kind.
    dropped: HashSet<&'static str>,
    /// The reports sent and heard, oldest first.
    reports: Vec<CircleReport>,
    /// The report sent again every few minutes, if one is.
    repeat: Option<Repeat>,
}

/// The client's alerts. Clones share them.
#[derive(Clone, Debug)]
pub struct Alerts(Arc<Mutex<State>>);

/// The callsigns this computer works: the chosen one first, then the others (W46).
#[must_use]
pub fn callsigns_of(base: &Path) -> Vec<String> {
    let chosen = callsigns::chosen(base);
    let mut out: Vec<String> = Store::open(&chosen)
        .ok()
        .and_then(|s| s.me().ok())
        .map(|m| vec![m.operator])
        .unwrap_or_default();
    for entry in callsigns::list(base) {
        if let Some(me) = Store::open(&entry.home).ok().and_then(|s| s.me().ok())
            && !out.iter().any(|c| c.eq_ignore_ascii_case(&me.operator))
        {
            out.push(me.operator);
        }
    }
    out
}

fn random_below(n: u64) -> u64 {
    let mut b = [0u8; 8];
    if getrandom::fill(&mut b).is_err() || n == 0 {
        return 0;
    }
    u64::from_le_bytes(b) % n
}

/// The page's settings (`notify.json`), or an empty object.
#[must_use]
pub fn notify_settings(base: &Path) -> Value {
    std::fs::read_to_string(base.join(NOTIFY))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

/// Keeps the page's settings.
///
/// # Errors
/// Not an object, too large, or a file that cannot be written.
pub fn set_notify_settings(base: &Path, settings: &Value) -> Result<(), Error> {
    if !settings.is_object() {
        return Err("the settings are an object".into());
    }
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    if text.len() > NOTIFY_MAX {
        return Err("the settings are too large".into());
    }
    write(&base.join(NOTIFY), text.as_bytes())
}

impl Alerts {
    /// The alerts of the computer whose first home is `base`, as its files keep them. An alert
    /// that was still waiting for the modem when the program closed is not sent now.
    #[must_use]
    pub fn open(base: &Path) -> Alerts {
        let read = |name: &str| {
            std::fs::read_to_string(base.join(name))
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        };
        let circles = read(CIRCLES)
            .and_then(|v| {
                v.as_array()
                    .map(|a| a.iter().filter_map(Kept::from_json).collect())
            })
            .unwrap_or_default();
        let mut entries: Vec<Entry> = read(LOG)
            .and_then(|v| {
                v.as_array()
                    .map(|a| a.iter().filter_map(Entry::from_json).collect())
            })
            .unwrap_or_default();
        for e in &mut entries {
            if e.out && e.state == "waiting" {
                e.state = "failed";
                e.why = Some("the program closed before it went out".into());
            }
        }
        let mut reports: Vec<CircleReport> = read(REPORTS)
            .and_then(|v| {
                v.as_array()
                    .map(|a| a.iter().filter_map(CircleReport::from_json).collect())
            })
            .unwrap_or_default();
        for r in &mut reports {
            if r.out && r.state == "waiting" {
                r.state = "failed";
            }
        }
        Alerts(Arc::new(Mutex::new(State {
            base: base.to_owned(),
            circles,
            entries,
            heard: HashMap::new(),
            queue: VecDeque::new(),
            flying: Vec::new(),
            references: 0,
            seq: 0,
            dropped: HashSet::new(),
            reports,
            repeat: None,
        })))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether any circle is kept: without one nothing is sent or heard.
    #[must_use]
    pub fn any_circle(&self) -> bool {
        !self.lock().circles.is_empty()
    }

    /// The circles, as the page shows them.
    #[must_use]
    pub fn circles(&self) -> Value {
        Value::Array(self.lock().circles.iter().map(Kept::shown).collect())
    }

    /// Keeps a circle from its keyword (RP-8 §2), with the keyword. The keyword of a circle kept
    /// before keywords were kept fills it in: the circle keeps its name and settings, and what is
    /// returned says `"filled": true`.
    ///
    /// # Errors
    /// No name, a name or a keyword another circle has, a keyword too short, or a file that
    /// cannot be written.
    pub fn add_circle(&self, name: &str, keyword: &str, level: Level) -> Result<Value, Error> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a name for the circle".into());
        }
        // the slow part, outside the lock
        let circle = Circle::from_keyword(keyword)?;
        let keyword = alert::normalise_keyword(keyword);
        let mut s = self.lock();
        if let Some(same) = s.circles.iter_mut().find(|c| c.circle.id == circle.id) {
            if same.keyword.is_some() {
                return Err(format!("that keyword is already the circle {}", same.name));
            }
            same.keyword = Some(keyword);
            let mut shown = same.shown();
            shown["filled"] = json!(true);
            s.save_circles()?;
            s.seq += 1;
            return Ok(shown);
        }
        if s.circles.iter().any(|c| c.name.eq_ignore_ascii_case(name)) {
            return Err(format!("a circle is already called {name}"));
        }
        let kept = Kept {
            name: name.to_owned(),
            circle,
            keyword: Some(keyword),
            level,
            answer: true,
            watch_hz: None,
        };
        let shown = kept.shown();
        s.circles.push(kept);
        s.save_circles()?;
        s.seq += 1;
        Ok(shown)
    }

    /// Changes how a circle's alerts arrive, whether the station says `heard` by itself, and its
    /// watch dial (`Some(None)` clears it).
    ///
    /// # Errors
    /// No such circle, or a file that cannot be written.
    pub fn set_circle(
        &self,
        name: &str,
        level: Option<Level>,
        answer: Option<bool>,
        watch_hz: Option<Option<u64>>,
    ) -> Result<(), Error> {
        let mut s = self.lock();
        let c = s
            .circles
            .iter_mut()
            .find(|c| c.name == name)
            .ok_or_else(|| format!("no circle called {name}"))?;
        if let Some(l) = level {
            c.level = l;
        }
        if let Some(a) = answer {
            c.answer = a;
        }
        if let Some(w) = watch_hz {
            c.watch_hz = w.filter(|&hz| hz > 0);
        }
        s.save_circles()?;
        s.seq += 1;
        Ok(())
    }

    /// A circle's keyword, normalised, for its operator to tell someone joining (RP-8 §2).
    ///
    /// # Errors
    /// No such circle, or one kept before keywords were kept.
    pub fn keyword(&self, name: &str) -> Result<String, Error> {
        let s = self.lock();
        let kept = s
            .circles
            .iter()
            .find(|c| c.name == name)
            .ok_or_else(|| format!("no circle called {name}"))?;
        kept.keyword.clone().ok_or_else(|| {
            format!(
                "{name} was added before RadioPost kept keywords: add it again with its keyword \
                 to keep it"
            )
        })
    }

    /// Deletes a circle, its key and its keyword: its alerts are no longer heard nor sent. What
    /// was still to go for it does not go: its alerts and reports waiting or to be tried again
    /// are marked failed, answers to its alerts are dropped, and a report sent again stops. A
    /// frame the modem already has goes. The log keeps what it holds.
    ///
    /// # Errors
    /// No such circle, or a file that cannot be written.
    pub fn remove_circle(&self, name: &str) -> Result<(), Error> {
        let mut s = self.lock();
        let before = s.circles.len();
        s.circles.retain(|c| c.name != name);
        if s.circles.len() == before {
            return Err(format!("no circle called {name}"));
        }
        s.save_circles()?;
        let why = "its circle was deleted";
        let mut dropped: HashSet<[u8; 8]> = HashSet::new();
        for e in s.entries.iter_mut().filter(|e| e.circle == name) {
            dropped.insert(e.tag);
            if e.out && (e.state == "waiting" || e.state == "sent") {
                e.state = "failed";
                e.why = Some(why.into());
            }
        }
        for r in s.reports.iter_mut().filter(|r| r.circle == name) {
            dropped.insert(r.tag);
            if r.out && r.state == "waiting" {
                r.state = "failed";
            }
        }
        s.queue.retain(|o| {
            ![o.alert, o.answers, o.report]
                .into_iter()
                .flatten()
                .any(|t| dropped.contains(&t))
        });
        if s.repeat.as_ref().is_some_and(|r| r.circle == name) {
            s.repeat = None;
        }
        s.changed();
        Ok(())
    }

    /// Writes an alert from `from` to `to` (or the whole circle), queued for the modem, on
    /// whatever dial the radio is on.
    ///
    /// # Errors
    /// No such circle, or a line or callsign RP-8 §3 refuses.
    pub fn send(
        &self,
        circle: &str,
        to: Option<&str>,
        text: &str,
        from: &str,
        now: u64,
    ) -> Result<String, Error> {
        self.send_on(circle, to, text, from, now, Sending::default())
    }

    /// Writes an alert as [`Alerts::send`] does, on a dial, its copies going on for as long as
    /// the station alerted takes to scan round to it (RP-8 §5).
    ///
    /// # Errors
    /// No such circle, or a line or callsign RP-8 §3 refuses.
    pub fn send_on(
        &self,
        circle: &str,
        to: Option<&str>,
        text: &str,
        from: &str,
        now: u64,
        how: Sending,
    ) -> Result<String, Error> {
        let text = text.trim();
        let to = to
            .map(|t| t.trim().to_ascii_uppercase())
            .filter(|t| !t.is_empty());
        let mut s = self.lock();
        let kept = s
            .circles
            .iter()
            .find(|c| c.name == circle)
            .ok_or_else(|| format!("no circle called {circle}"))?
            .clone();
        let frame = alert::make(
            &kept.circle,
            &Draft {
                kind: Kind::Alert,
                from,
                to: to.as_deref(),
                time: now,
                text: Some(text),
                re: None,
                form: None,
            },
        )
        .map_err(|why| match why.as_str() {
            "text: 1 to 100 bytes" => {
                format!("a line of 1 to {} bytes", alert::TEXT_MAX)
            }
            "text: no control characters" => "one line, with no line breaks".into(),
            "to: a callsign" => "to: a callsign, such as N0CALL".into(),
            "from: a callsign" => format!("{from} is not a callsign this station can send as"),
            _ => why,
        })?;
        let tag = tag_of_frame(&frame);
        s.entries.push(Entry {
            tag,
            out: true,
            circle: kept.name,
            from: from.to_owned(),
            to,
            text: radiopost::alert::normalise_text(text),
            time: now,
            at: now,
            state: "waiting",
            level: kept.level,
            quiet: false,
            frame: frame.clone(),
            sent_at: None,
            tries: 1,
            answers: Vec::new(),
            reply: None,
            why: None,
            me: from.to_owned(),
            dial_hz: how.dial_hz,
            outlast_s: how.outlast_s.filter(|_| how.dial_hz.is_some()),
            copies_until: None,
        });
        s.queue.push_back(Outgoing {
            frame,
            not_before: now,
            alert: Some(tag),
            answers: None,
            report: None,
            dial_hz: how.dial_hz,
        });
        s.changed();
        Ok(hex(&tag))
    }

    /// The operator has seen a heard alert: it stops sounding, and the station says `seen`.
    ///
    /// # Errors
    /// No such alert heard.
    pub fn acknowledge(&self, tag: &str, now: u64) -> Result<(), Error> {
        self.answer_heard(tag, Kind::Seen, None, now)
    }

    /// The operator answers a heard alert with a line; it counts as seen too.
    ///
    /// # Errors
    /// No such alert heard, or a line RP-8 §3 refuses.
    pub fn reply(&self, tag: &str, text: &str, now: u64) -> Result<(), Error> {
        let text = text.trim();
        if text.is_empty() {
            return Err("a line to reply with".into());
        }
        self.answer_heard(tag, Kind::Reply, Some(text), now)
    }

    fn answer_heard(
        &self,
        tag: &str,
        kind: Kind,
        text: Option<&str>,
        now: u64,
    ) -> Result<(), Error> {
        let tag: [u8; 8] = unhex(tag)
            .and_then(|b| b.try_into().ok())
            .ok_or("which alert, by its tag")?;
        let mut s = self.lock();
        let i = s
            .entries
            .iter()
            .position(|e| !e.out && e.tag == tag)
            .ok_or("no such alert heard")?;
        let (circle, frame) = s.frame_of(i)?;
        let me = s.entries[i].me.clone();
        let dial_hz = s.entries[i].dial_hz;
        // a seen already sent is not sent again; a reply is sent each time it is written; both
        // go on the dial the alert was heard on, wherever a scan has taken the radio since
        if kind == Kind::Reply || s.entries[i].state == "new" {
            let raw = alert::answer(&circle, &frame, kind, &me, now, text)?;
            s.queue.push_back(Outgoing {
                frame: raw,
                not_before: now,
                alert: None,
                answers: Some(tag),
                report: None,
                dial_hz,
            });
        }
        let e = &mut s.entries[i];
        if kind == Kind::Reply {
            e.state = "replied";
            e.reply = text.map(radiopost::alert::normalise_text);
        } else if e.state == "new" {
            e.state = "seen";
        }
        s.changed();
        Ok(())
    }

    /// Every heard alert still sounding is acknowledged: the operator's *Quiet* for them all.
    pub fn acknowledge_all(&self, now: u64) {
        let tags: Vec<String> = self
            .lock()
            .entries
            .iter()
            .filter(|e| !e.out && e.state == "new")
            .map(|e| hex(&e.tag))
            .collect();
        for tag in tags {
            let _ = self.acknowledge(&tag, now);
        }
    }

    /// The log for the page, newest first, with the circles and whether the modem carries alerts.
    #[must_use]
    pub fn json(&self) -> Value {
        let s = self.lock();
        json!({
            "seq": s.seq,
            "circles": s.circles.iter().map(Kept::shown).collect::<Vec<_>>(),
            "log": s.entries.iter().rev().map(Entry::shown).collect::<Vec<_>>(),
            "alarming": s.entries.iter().any(Entry::alarms),
            "reports": s.reports.iter().rev().map(CircleReport::json).collect::<Vec<_>>(),
            "repeat": s.repeat.as_ref().map(|r| json!({
                "circle": r.circle, "to": r.to, "template": hex(&r.form.template),
                "every_s": r.every_s, "next_at": r.next_at, "until": r.until})),
        })
    }

    /// Sends a form as a report (RP-8 §3) from `from` to the circle, or to one station, on the
    /// dial `how` names; with `every_s`, again that often while the window is open, for
    /// [`REPEAT_FOR_S`] at most, the fields `here` taking the station's position each time (§7).
    /// A new report replaces the one being repeated.
    ///
    /// # Errors
    /// No such circle, a callsign RP-8 §3 refuses, a frame too long, or a repeat not offered.
    #[allow(
        clippy::too_many_arguments,
        reason = "what a report is, one argument a part"
    )]
    pub fn report(
        &self,
        circle: &str,
        to: Option<&str>,
        form: radiopost::forms::Form,
        from: &str,
        now: u64,
        how: Sending,
        every_s: Option<u64>,
        here: Vec<u64>,
    ) -> Result<String, Error> {
        if every_s.is_some_and(|e| !REPEAT_EVERY_S.contains(&e)) {
            return Err("every: 10, 15, 30 or 60 minutes".into());
        }
        let to = to
            .map(|t| t.trim().to_ascii_uppercase())
            .filter(|t| !t.is_empty());
        let mut s = self.lock();
        let tag = s.queue_report(circle, to.as_deref(), &form, from, now, how)?;
        s.repeat = every_s.map(|every_s| Repeat {
            circle: circle.to_owned(),
            to,
            form,
            from: from.to_owned(),
            how,
            every_s,
            next_at: now + every_s,
            until: now + REPEAT_FOR_S,
            here,
        });
        s.changed();
        Ok(hex(&tag))
    }

    /// The fields to bring up to date with the station's position, when the report being
    /// repeated is due again; a repeat past [`REPEAT_FOR_S`] stops here.
    #[must_use]
    pub fn repeat_due(&self, now: u64) -> Option<Vec<u64>> {
        let mut s = self.lock();
        let r = s.repeat.as_ref()?;
        if now >= r.until {
            s.repeat = None;
            s.changed();
            return None;
        }
        (now >= r.next_at).then(|| r.here.clone())
    }

    /// Sends the report being repeated again, now, with `here` (the station's position, RP-2
    /// §8.1's value) in the fields that held the station's own.
    ///
    /// # Errors
    /// None is being repeated, or the circle has been forgotten.
    pub fn repeat_now(&self, now: u64, here: Option<&radiopost::cbor::Value>) -> Result<(), Error> {
        let mut s = self.lock();
        let mut r = s.repeat.clone().ok_or("no report is being sent again")?;
        if let Some(here) = here {
            for (n, v) in &mut r.form.values {
                if r.here.contains(n) {
                    *v = here.clone();
                }
            }
        }
        r.next_at = now + r.every_s;
        let sent = s.queue_report(&r.circle, r.to.as_deref(), &r.form, &r.from, now, r.how);
        s.repeat = if sent.is_ok() { Some(r) } else { None };
        s.changed();
        sent.map(|_| ())
    }

    /// Stops sending the report again.
    pub fn stop_repeat(&self) {
        let mut s = self.lock();
        if s.repeat.take().is_some() {
            s.changed();
        }
    }

    /// The reports sent and heard, newest first, for the Reports view.
    #[must_use]
    pub fn reports(&self) -> Vec<CircleReport> {
        self.lock().reports.iter().rev().cloned().collect()
    }

    /// What the state shows: whether to look again, and whether an alarm sounds.
    #[must_use]
    pub fn summary(&self) -> Value {
        let s = self.lock();
        json!({"seq": s.seq, "alarming": s.entries.iter().any(Entry::alarms),
               "circles": s.circles.len()})
    }

    /// A heard alert sounds and waits to be acknowledged: the window comes forward.
    #[must_use]
    pub fn alarming(&self) -> bool {
        self.lock().entries.iter().any(Entry::alarms)
    }

    /// A broadcast heard on any connection (RP-4 §6), checked as RP-8 §4 says. `mine` are the
    /// station's callsigns ([`callsigns_of`]). Returns a line for the log, when there is one.
    #[must_use = "a line for the log"]
    pub fn heard(&self, data: &[u8], mine: &[String], now: u64) -> Option<String> {
        self.heard_on(data, mine, now, None)
    }

    /// A broadcast heard as [`Alerts::heard`] takes it, with the dial the modem heard it on
    /// (RP-4 §6's `dial_hz`): where its answers go.
    #[must_use = "a line for the log"]
    pub fn heard_on(
        &self,
        data: &[u8],
        mine: &[String],
        now: u64,
        dial_hz: Option<u64>,
    ) -> Option<String> {
        let mut s = self.lock();
        if s.circles.is_empty() {
            return None;
        }
        let window: HashSet<[u8; 8]> = s
            .heard
            .iter()
            .filter(|(_, t)| now.saturating_sub(**t) <= alert::REMEMBER_S)
            .map(|(k, _)| *k)
            .chain(
                s.entries
                    .iter()
                    .filter(|e| now.saturating_sub(e.at) <= alert::REMEMBER_S)
                    .map(|e| e.tag),
            )
            .collect();
        let circles: Vec<Circle> = s.circles.iter().map(|c| c.circle.clone()).collect();
        let verdict = alert::hear(data, mine, &circles, now, &window);
        match verdict {
            Heard::New(frame) => {
                s.heard.insert(frame.tag, now);
                s.take(frame, mine, now, dial_hz)
            }
            Heard::HeardAgain(frame) => {
                let last = s.heard.insert(frame.tag, now);
                if last.is_some_and(|t| now.saturating_sub(t) < SAME_TRANSMISSION_S) {
                    return None;
                }
                s.again(&frame, mine, now, dial_hz)
            }
            Heard::ForAnotherStation(_) | Heard::NotAnAlert => None,
            other => {
                let why = other.verdict();
                s.dropped.insert(why).then(|| {
                    format!("alerts: a broadcast dropped ({why}); others like it are not logged")
                })
            }
        }
    }

    /// A frame is due for the modem now: its reference for `broadcast` and its bytes. The caller
    /// sends it and reports `broadcast-sent` with [`Alerts::went`].
    #[must_use]
    pub fn due(&self, now: u64) -> Option<(String, Vec<u8>)> {
        self.due_with(now).map(|d| (d.reference, d.frame))
    }

    /// A frame due for the modem now, with its dial ([`Due`]). The first copy of a sending
    /// starts the time its copies go on for (RP-8 §5).
    #[must_use]
    pub fn due_with(&self, now: u64) -> Option<Due> {
        let mut s = self.lock();
        let i = s.queue.iter().position(|o| o.not_before <= now)?;
        let outgoing = s.queue.remove(i)?;
        s.references += 1;
        let reference = format!("alert{}", s.references);
        if let Some(tag) = outgoing.alert
            && let Some(e) = s.entries.iter_mut().find(|e| e.out && e.tag == tag)
            && e.copies_until.is_none()
        {
            e.copies_until = e.outlast_s.map(|o| now + o);
        }
        let due = Due {
            reference: reference.clone(),
            frame: outgoing.frame.clone(),
            dial_hz: outgoing.dial_hz,
            tune: (outgoing.alert.is_some() || outgoing.report.is_some())
                && outgoing.dial_hz.is_some(),
        };
        s.flying.push(Flying {
            reference,
            outgoing,
            at: now,
        });
        Some(due)
    }

    /// Whether a frame waits for the modem now.
    #[must_use]
    pub fn wants(&self, now: u64) -> bool {
        self.lock().queue.iter().any(|o| o.not_before <= now)
    }

    /// The modem's `broadcast-sent`, or the `error` that answered the `broadcast`: an alert gone
    /// out waits for its answers; one refused is failed, with why.
    pub fn went(&self, reference: &str, sent: bool, text: Option<&str>, now: u64) {
        let mut s = self.lock();
        let Some(i) = s.flying.iter().position(|f| f.reference == reference) else {
            return;
        };
        let f = s.flying.remove(i);
        if let Some(tag) = f.outgoing.report {
            if let Some(r) = s.reports.iter_mut().find(|r| r.out && r.tag == tag) {
                r.state = if sent { "sent" } else { "failed" };
            }
            s.changed();
            return;
        }
        let Some(tag) = f.outgoing.alert else {
            return;
        };
        let mut copy = None;
        if let Some(e) = s.entries.iter_mut().find(|e| e.out && e.tag == tag) {
            if sent {
                e.sent_at = Some(now);
                // a copy more while the station alerted may still be scanning round to the
                // dial, unless a heard has come meanwhile (RP-8 §5)
                if e.state == "waiting" && e.copies_until.is_some_and(|until| now < until) {
                    copy = Some(Outgoing {
                        frame: e.frame.clone(),
                        not_before: now,
                        alert: Some(tag),
                        answers: None,
                        report: None,
                        dial_hz: e.dial_hz,
                    });
                } else if e.state == "waiting" {
                    e.state = "sent";
                }
            } else {
                e.state = "failed";
                e.why = Some(text.unwrap_or("the modem did not send it").to_owned());
            }
        }
        s.queue.extend(copy);
        s.changed();
    }

    /// What the clock moves on: an alert with no `heard` sent once more, then said not heard
    /// (RP-8 §5); a frame the modem never reported on, given up.
    pub fn tick(&self, now: u64) {
        let s = self.lock();
        let stale: Vec<String> = s
            .flying
            .iter()
            .filter(|f| now.saturating_sub(f.at) > FLYING_AT_MOST_S)
            .map(|f| f.reference.clone())
            .collect();
        drop(s);
        for r in stale {
            self.went(&r, false, Some("the modem never said it went out"), now);
        }
        let mut s = self.lock();
        let mut again = Vec::new();
        let mut changed = false;
        let flying: HashSet<[u8; 8]> = s.flying.iter().filter_map(|f| f.outgoing.alert).collect();
        let mut given_up = HashSet::new();
        for e in s
            .entries
            .iter_mut()
            .filter(|e| e.out && e.state == "waiting")
        {
            let since = e.sent_at.map_or(e.at, |t| t + RETRY_AFTER_S);
            if now.saturating_sub(since) > WAITING_AT_MOST_S && !flying.contains(&e.tag) {
                e.state = "failed";
                e.why = Some(
                    "it did not go out in five minutes: the station was busy, or its modem \
                     carries no alerts"
                        .into(),
                );
                given_up.insert(e.tag);
                changed = true;
            }
        }
        s.queue
            .retain(|o| o.alert.is_none_or(|t| !given_up.contains(&t)));
        // a report that waited five minutes is stale: the next carries the news
        let stale: HashSet<[u8; 8]> = s
            .reports
            .iter()
            .filter(|r| {
                r.out && r.state == "waiting" && now.saturating_sub(r.at) > WAITING_AT_MOST_S
            })
            .map(|r| r.tag)
            .collect();
        if !stale.is_empty() {
            s.queue
                .retain(|o| o.report.is_none_or(|t| !stale.contains(&t)));
            for r in s.reports.iter_mut().filter(|r| stale.contains(&r.tag)) {
                r.state = "failed";
            }
            changed = true;
        }
        for e in s.entries.iter_mut().filter(|e| e.out && e.state == "sent") {
            let Some(at) = e.sent_at else { continue };
            if now.saturating_sub(at) < RETRY_AFTER_S {
                continue;
            }
            if e.tries < TRIES {
                e.tries += 1;
                e.state = "waiting";
                e.copies_until = None;
                again.push(Outgoing {
                    frame: e.frame.clone(),
                    not_before: now,
                    alert: Some(e.tag),
                    answers: None,
                    report: None,
                    dial_hz: e.dial_hz,
                });
            } else {
                e.state = "not heard";
            }
            changed = true;
        }
        s.queue.extend(again);
        if changed {
            s.changed();
        }
    }
}

/// The tag a frame made by [`alert::make`] carries: its last 8 bytes, since key 7 sorts last of
/// the keys RP-8 writes.
fn tag_of_frame(frame: &[u8]) -> [u8; 8] {
    let mut tag = [0u8; 8];
    tag.copy_from_slice(&frame[frame.len() - 8..]);
    tag
}

/// The callsign a station answers an alert as: the one it was sent to, else its first.
fn answering_as(mine: &[String], frame: &Frame) -> String {
    frame
        .to
        .as_ref()
        .and_then(|to| {
            mine.iter()
                .find(|c| c.eq_ignore_ascii_case(to))
                .or_else(|| mine.iter().find(|c| radiopost::callsign::same_base(c, to)))
        })
        .or_else(|| mine.first())
        .cloned()
        .unwrap_or_default()
}

impl State {
    fn changed(&mut self) {
        self.seq += 1;
        if self.entries.len() > KEPT {
            let extra = self.entries.len() - KEPT;
            self.entries.drain(..extra);
        }
        if self.reports.len() > KEPT {
            let extra = self.reports.len() - KEPT;
            self.reports.drain(..extra);
        }
        let all: Vec<Value> = self.entries.iter().map(Entry::json).collect();
        if let Ok(text) = serde_json::to_string_pretty(&all)
            && let Err(e) = write(&self.base.join(LOG), text.as_bytes())
        {
            radiopost::logfile::line(&format!("alerts: {e}"));
        }
        let all: Vec<Value> = self.reports.iter().map(CircleReport::json).collect();
        if let Ok(text) = serde_json::to_string_pretty(&all)
            && let Err(e) = write(&self.base.join(REPORTS), text.as_bytes())
        {
            radiopost::logfile::line(&format!("alerts: {e}"));
        }
    }

    /// A report made and queued, kept among the reports sent: its tag.
    fn queue_report(
        &mut self,
        circle: &str,
        to: Option<&str>,
        form: &radiopost::forms::Form,
        from: &str,
        now: u64,
        how: Sending,
    ) -> Result<[u8; 8], Error> {
        let kept = self
            .circles
            .iter()
            .find(|c| c.name == circle)
            .ok_or_else(|| format!("no circle called {circle}"))?
            .clone();
        let frame =
            alert::report(&kept.circle, from, to, now, form).map_err(|why| match why.as_str() {
                "to: a callsign" => "to: a callsign, such as N0CALL".into(),
                "from: a callsign" => format!("{from} is not a callsign this station can send as"),
                w if w.starts_with("over ") => format!(
                    "too long for a broadcast: {} bytes at most; leave out a field or shorten one",
                    alert::FRAME_MAX
                ),
                "form: at least one value" => "fill in the form first".into(),
                _ => why,
            })?;
        let tag = tag_of_frame(&frame);
        self.reports.push(CircleReport {
            tag,
            out: true,
            circle: kept.name,
            from: from.to_owned(),
            to: to.map(str::to_owned),
            time: now,
            at: now,
            form: form.clone(),
            state: "waiting",
            dial_hz: how.dial_hz,
        });
        self.queue.push_back(Outgoing {
            frame,
            not_before: now,
            alert: None,
            answers: None,
            report: Some(tag),
            dial_hz: how.dial_hz,
        });
        Ok(tag)
    }

    fn save_circles(&self) -> Result<(), Error> {
        let all: Vec<Value> = self.circles.iter().map(Kept::json).collect();
        let text = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
        write(&self.base.join(CIRCLES), text.as_bytes())
    }

    fn circle_named_for(&self, id: [u8; 4]) -> Option<&Kept> {
        self.circles.iter().find(|c| c.circle.id == id)
    }

    /// The heard alert at `i` as a frame again, with its circle, to answer it.
    fn frame_of(&self, i: usize) -> Result<(Circle, Frame), Error> {
        let e = &self.entries[i];
        let kept = self
            .circles
            .iter()
            .find(|c| c.name == e.circle)
            .ok_or("its circle has been forgotten")?;
        Ok((
            kept.circle.clone(),
            Frame {
                kind: Kind::Alert,
                circle: kept.circle.id,
                from: e.from.clone(),
                to: e.to.clone(),
                time: e.time,
                text: Some(e.text.clone()),
                re: None,
                form: None,
                tag: e.tag,
            },
        ))
    }

    fn queue_answer(
        &mut self,
        circle: &Circle,
        frame: &Frame,
        kind: Kind,
        (me, dial_hz): (&str, Option<u64>),
        now: u64,
    ) {
        let reply = (kind == Kind::Reply)
            .then(|| {
                self.entries
                    .iter()
                    .find(|e| e.tag == frame.tag)
                    .and_then(|e| e.reply.clone())
            })
            .flatten();
        let Ok(raw) = alert::answer(circle, frame, kind, me, now, reply.as_deref()) else {
            return;
        };
        let spread = if frame.to.is_none() {
            random_below(CIRCLE_SPREAD_S + 1)
        } else {
            0
        };
        self.queue.push_back(Outgoing {
            frame: raw,
            not_before: now + spread,
            alert: None,
            answers: Some(frame.tag),
            report: None,
            dial_hz,
        });
    }

    /// Whether an answer to the alert `tag` is still to go out: queued, or with the modem.
    fn answer_pending(&self, tag: [u8; 8]) -> bool {
        self.queue.iter().any(|o| o.answers == Some(tag))
            || self.flying.iter().any(|f| f.outgoing.answers == Some(tag))
    }

    /// A new frame for this station: an alert kept and answered, or an answer to one of its own.
    fn take(
        &mut self,
        frame: Frame,
        mine: &[String],
        now: u64,
        dial_hz: Option<u64>,
    ) -> Option<String> {
        let kept = self.circle_named_for(frame.circle)?.clone();
        if frame.kind == Kind::Report {
            // kept for the map, and never answered (RP-8 §5)
            let form = frame.form.clone()?;
            self.reports.push(CircleReport {
                tag: frame.tag,
                out: false,
                circle: kept.name.clone(),
                from: frame.from.clone(),
                to: frame.to.clone(),
                time: frame.time,
                at: now,
                form,
                state: "heard",
                dial_hz,
            });
            self.changed();
            return Some(format!("report from {} in {}", frame.from, kept.name));
        }
        if frame.kind == Kind::Alert {
            let loud_lately = self
                .entries
                .iter()
                .filter(|e| {
                    !e.out && e.from == frame.from && now.saturating_sub(e.at) <= LOUD_WINDOW_S
                })
                .count();
            let quiet = loud_lately >= LOUD_AT_MOST || kept.level == Level::Silent;
            let me = answering_as(mine, &frame);
            if kept.answer {
                self.queue_answer(&kept.circle, &frame, Kind::Heard, (&me, dial_hz), now);
            }
            let line = format!(
                "alert from {} in {}{}: {}",
                frame.from,
                kept.name,
                if quiet { " (shown quietly)" } else { "" },
                frame.text.as_deref().unwrap_or("")
            );
            self.entries.push(Entry {
                tag: frame.tag,
                out: false,
                circle: kept.name,
                from: frame.from,
                to: frame.to,
                text: frame.text.unwrap_or_default(),
                time: frame.time,
                at: now,
                state: "new",
                level: kept.level,
                quiet,
                frame: Vec::new(),
                sent_at: None,
                tries: 0,
                answers: Vec::new(),
                reply: None,
                why: None,
                me,
                dial_hz,
                outlast_s: None,
                copies_until: None,
            });
            self.changed();
            return Some(line);
        }
        let re = frame.re?;
        // an answer has come: no more copies of the alert (RP-8 §5)
        self.queue.retain(|o| o.alert != Some(re));
        let e = self.entries.iter_mut().find(|e| e.out && e.tag == re)?;
        if e.state == "waiting" {
            e.state = "sent";
        }
        e.answers.push(Answer {
            kind: frame.kind,
            from: frame.from.clone(),
            time: frame.time,
            text: frame.text.clone(),
        });
        let rank = |s: &str| match s {
            "heard" => 1,
            "seen" => 2,
            "replied" => 3,
            _ => 0,
        };
        let now_state = match frame.kind {
            Kind::Heard => "heard",
            Kind::Seen => "seen",
            _ => "replied",
        };
        if rank(now_state) > rank(e.state) {
            e.state = now_state;
        }
        let line = match frame.kind {
            Kind::Reply => format!(
                "alert answered by {}: {}",
                frame.from,
                frame.text.unwrap_or_default()
            ),
            kind => format!("alert {} by {}", kind.name(), frame.from),
        };
        self.changed();
        Some(line)
    }

    /// An alert heard again: the sender is still sending, a copy or a try, so it has not heard
    /// the answers that went out (RP-8 §5). Answered again, unless the last answer is still to go:
    /// the copies a sender sends to outlast a scan come while it waits for the channel.
    fn again(
        &mut self,
        frame: &Frame,
        mine: &[String],
        now: u64,
        dial_hz: Option<u64>,
    ) -> Option<String> {
        if frame.kind != Kind::Alert {
            return None;
        }
        let kept = self.circle_named_for(frame.circle)?.clone();
        if !kept.answer || self.answer_pending(frame.tag) {
            return None;
        }
        let (state, was_on) = self
            .entries
            .iter()
            .find(|e| !e.out && e.tag == frame.tag)
            .map_or((None, None), |e| (Some(e.state), e.dial_hz));
        let me = answering_as(mine, frame);
        let on = (me.as_str(), dial_hz.or(was_on));
        self.queue_answer(&kept.circle, frame, Kind::Heard, on, now);
        match state {
            Some("seen") => self.queue_answer(&kept.circle, frame, Kind::Seen, on, now),
            Some("replied") => self.queue_answer(&kept.circle, frame, Kind::Reply, on, now),
            _ => {}
        }
        Some(format!(
            "alert from {} heard again: answered again",
            frame.from
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{Alerts, Level, Sending, callsigns_of};
    use radiopost::cbor::hex;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("radiopost-alerts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory");
        dir
    }

    const KEYWORD: &str = "kimo dali sune pato";
    const T: u64 = 1_794_667_200;

    /// Two computers in one circle, their broadcasts handed across by hand: an alert, its
    /// `heard`, `seen` and reply, and the sender trying again once when nothing is heard.
    #[test]
    fn an_alert_crosses_and_is_answered() {
        let (a, b) = (scratch("a"), scratch("b"));
        let facundo = Alerts::open(&a);
        let ana = Alerts::open(&b);
        facundo
            .add_circle("Family", KEYWORD, Level::TimeSensitive)
            .expect("kept");
        ana.add_circle("Home", "  KIMO dali\tSune PATO", Level::TimeSensitive)
            .expect("the same circle, typed another way");
        let mine_f = vec!["KK4ODA".to_owned()];
        let mine_a = vec!["N0CALL".to_owned()];
        let tag = facundo
            .send(
                "Family",
                Some("n0call"),
                "Please call me on 146.52 now",
                "KK4ODA",
                T,
            )
            .expect("written");
        let (r, frame) = facundo.due(T).expect("due at once");
        assert!(facundo.due(T).is_none());
        facundo.went(&r, true, None, T + 16);
        // Ana's station hears it, alarms, and says heard by itself
        let line = ana.heard(&frame, &mine_a, T + 17).expect("a line");
        assert!(line.contains("alert from KK4ODA in Home"), "{line}");
        assert!(ana.alarming());
        // the same broadcast handed over by a second connection is let go
        assert!(ana.heard(&frame, &mine_a, T + 17).is_none());
        let (r2, heard) = ana.due(T + 17).expect("heard, at once");
        ana.went(&r2, true, None, T + 28);
        facundo.heard(&heard, &mine_f, T + 28).expect("told");
        let log = facundo.json();
        assert_eq!(log["log"][0]["state"], "heard");
        // she acknowledges, and replies
        ana.acknowledge(&tag, T + 40).expect("acknowledged");
        assert!(!ana.alarming());
        let (_, seen) = ana.due(T + 40).expect("seen");
        facundo.heard(&seen, &mine_f, T + 52).expect("told");
        ana.reply(&tag, "On my way, 5 min", T + 55)
            .expect("replied");
        let (_, reply) = ana.due(T + 55).expect("reply");
        facundo.heard(&reply, &mine_f, T + 70).expect("told");
        let log = facundo.json();
        assert_eq!(log["log"][0]["state"], "replied");
        assert_eq!(log["log"][0]["answers"][2]["text"], "On my way, 5 min");
        assert_eq!(hex(&[0u8; 0]), "");
        let _ = (std::fs::remove_dir_all(&a), std::fs::remove_dir_all(&b));
    }

    /// A Position Report, its position and status only.
    fn position_report(lat: i64) -> radiopost::forms::Form {
        let t = &radiopost::forms::Template::published()[15];
        assert_eq!(t.name, "Position Report");
        radiopost::forms::Form {
            template: t.fingerprint(),
            values: vec![
                (
                    1,
                    radiopost::cbor::Value::Array(vec![
                        radiopost::cbor::Value::int(lat),
                        radiopost::cbor::Value::int(-843_876_500),
                    ]),
                ),
                (5, radiopost::cbor::Value::Uint(0)),
            ],
        }
    }

    /// RP-8 §3, §5, §7: a report reaches a station of the circle, which keeps it and answers
    /// nothing; one outside the circle keeps nothing; the sender's own is kept too.
    #[test]
    fn a_report_crosses_and_nobody_answers() {
        let (a, b, c) = (scratch("rep-a"), scratch("rep-b"), scratch("rep-c"));
        let facundo = Alerts::open(&a);
        let ana = Alerts::open(&b);
        let stranger = Alerts::open(&c);
        facundo
            .add_circle("Net", KEYWORD, Level::Normal)
            .expect("kept");
        ana.add_circle("Net", KEYWORD, Level::Normal).expect("kept");
        stranger
            .add_circle("Other", "a keyword of their own", Level::Normal)
            .expect("kept");
        facundo
            .report(
                "Net",
                None,
                position_report(337_812_300),
                "KK4ODA",
                T,
                Sending::default(),
                None,
                vec![1],
            )
            .expect("written");
        let due = facundo.due_with(T).expect("due at once");
        assert!(!due.tune, "no dial: where the radio is");
        facundo.went(&due.reference, true, None, T + 16);
        let line = ana
            .heard(&due.frame, &["N0CALL".to_owned()], T + 17)
            .expect("a line");
        assert_eq!(line, "report from KK4ODA in Net");
        assert!(ana.due(T + 30).is_none(), "nobody answers a report");
        assert!(!ana.alarming());
        assert!(
            ana.heard(&due.frame, &["N0CALL".to_owned()], T + 90)
                .is_none(),
            "heard again: nothing more"
        );
        assert_eq!(ana.reports().len(), 1);
        assert_eq!(ana.reports()[0].from, "KK4ODA");
        assert_eq!(ana.reports()[0].form, position_report(337_812_300));
        // a station outside the circle says once that it dropped one, and keeps nothing
        let said = stranger.heard(&due.frame, &["N9XYZ".to_owned()], T + 17);
        assert!(said.is_some_and(|l| l.contains("unknown circle")));
        assert_eq!(stranger.reports(), Vec::new());
        assert_eq!(facundo.reports()[0].state, "sent");
        // kept through a restart
        assert_eq!(Alerts::open(&b).reports(), ana.reports());
        let _ = [a, b, c].map(std::fs::remove_dir_all);
    }

    /// RP-8 §7: a report sent again every few minutes, its position brought up to date, until it
    /// is stopped, or for four hours at most; a frame too long is refused.
    #[test]
    fn a_report_goes_again_with_the_position_of_the_moment() {
        let a = scratch("rep-again");
        let facundo = Alerts::open(&a);
        facundo
            .add_circle("Net", KEYWORD, Level::Normal)
            .expect("kept");
        let how = Sending::default();
        assert!(
            facundo
                .report(
                    "Net",
                    None,
                    position_report(1),
                    "KK4ODA",
                    T,
                    how,
                    Some(700),
                    vec![1]
                )
                .is_err(),
            "every 700 s is not offered"
        );
        facundo
            .report(
                "Net",
                None,
                position_report(337_812_300),
                "KK4ODA",
                T,
                how,
                Some(600),
                vec![1],
            )
            .expect("written");
        let _ = facundo.due_with(T).expect("the first");
        assert_eq!(facundo.repeat_due(T + 599), None);
        assert_eq!(facundo.repeat_due(T + 600), Some(vec![1]));
        let moved = radiopost::cbor::Value::Array(vec![
            radiopost::cbor::Value::int(338_000_000),
            radiopost::cbor::Value::int(-843_876_500),
        ]);
        facundo.repeat_now(T + 600, Some(&moved)).expect("again");
        let again = facundo.due_with(T + 600).expect("the second");
        let frame = radiopost::cbor::decode(&again.frame).expect("CBOR");
        assert_eq!(
            frame.get(8).and_then(|f| f.get(2)).and_then(|v| v.get(1)),
            Some(&moved),
            "the position of the moment"
        );
        assert_eq!(facundo.repeat_due(T + 1199), None);
        assert_eq!(
            facundo.repeat_due(T + super::REPEAT_FOR_S),
            None,
            "four hours at most"
        );
        assert!(facundo.json()["repeat"].is_null());
        facundo
            .report(
                "Net",
                None,
                position_report(5),
                "KK4ODA",
                T,
                how,
                Some(900),
                vec![1],
            )
            .expect("written");
        facundo.stop_repeat();
        assert_eq!(facundo.repeat_due(T + 900), None);
        // a comment that makes the frame over 512 bytes
        let mut long = position_report(5);
        long.values
            .push((6, radiopost::cbor::Value::text(&"x".repeat(480))));
        let why = facundo
            .report("Net", None, long, "KK4ODA", T, how, None, vec![])
            .expect_err("refused");
        assert!(why.contains("too long for a broadcast"), "{why}");
        let _ = std::fs::remove_dir_all(&a);
    }

    #[test]
    fn an_alert_nobody_hears_goes_twice_then_is_not_heard() {
        let a = scratch("unheard");
        let facundo = Alerts::open(&a);
        facundo
            .add_circle("Family", KEYWORD, Level::TimeSensitive)
            .expect("kept");
        facundo
            .send("Family", None, "Net in 10 minutes", "KK4ODA", T)
            .expect("written");
        let (r, first) = facundo.due(T).expect("due");
        facundo.went(&r, true, None, T + 16);
        facundo.tick(T + 16 + 59);
        assert!(!facundo.wants(T + 16 + 59));
        facundo.tick(T + 16 + 60);
        let (r, second) = facundo.due(T + 76).expect("again");
        assert_eq!(first, second, "the same frame");
        facundo.went(&r, true, None, T + 92);
        facundo.tick(T + 92 + 60);
        assert!(facundo.due(T + 200).is_none());
        assert_eq!(facundo.json()["log"][0]["state"], "not heard");
        // the log is kept: a program opened again finds it
        let again = Alerts::open(&a);
        assert_eq!(again.json()["log"][0]["state"], "not heard");
        assert!(
            callsigns_of(&a).is_empty(),
            "no credential in a scratch home"
        );
        let _ = std::fs::remove_dir_all(&a);
    }

    #[test]
    fn a_sixth_alert_in_ten_minutes_is_shown_quietly() {
        let (a, b) = (scratch("loud-a"), scratch("loud-b"));
        let facundo = Alerts::open(&a);
        let ana = Alerts::open(&b);
        facundo
            .add_circle("Family", KEYWORD, Level::TimeSensitive)
            .expect("kept");
        ana.add_circle("Family", KEYWORD, Level::TimeSensitive)
            .expect("kept");
        let mine = vec!["N0CALL".to_owned()];
        for n in 0..6 {
            let t = T + n * 60;
            let tag = facundo
                .send(
                    "Family",
                    Some("N0CALL"),
                    &format!("number {n}"),
                    "KK4ODA",
                    t,
                )
                .expect("written");
            let (_, frame) = facundo.due(t).expect("due");
            ana.heard(&frame, &mine, t + 1).expect("heard");
            let quiet = ana.json()["log"][0]["quiet"].as_bool().expect("quiet");
            assert_eq!(quiet, n == 5, "alert {n}");
            ana.acknowledge(&tag, t + 2).expect("acknowledged");
        }
        let _ = (std::fs::remove_dir_all(&a), std::fs::remove_dir_all(&b));
    }

    const DIAL: u64 = 7_101_000;

    /// An alert to a station whose gateway scans (RP-8 0.2.0 §5): it goes on its dial, the radio
    /// tuned there, and copies go on until the gateway's `scan_s` has passed since the first,
    /// unless a `heard` comes first. Ana's station hears one partway through the copies; her
    /// `heard` goes on the dial she heard it on, without a tune, and stops the copies.
    #[test]
    fn copies_go_on_for_the_scan_and_a_heard_stops_them() {
        let (a, b) = (scratch("copies-a"), scratch("copies-b"));
        let facundo = Alerts::open(&a);
        let ana = Alerts::open(&b);
        facundo
            .add_circle("Family", KEYWORD, Level::TimeSensitive)
            .expect("kept");
        ana.add_circle("Family", KEYWORD, Level::TimeSensitive)
            .expect("kept");
        let how = Sending {
            dial_hz: Some(DIAL),
            outlast_s: Some(49),
        };
        facundo
            .send_on("Family", Some("N0CALL"), "Call me", "KK4ODA", T, how)
            .expect("written");
        let first = facundo.due_with(T).expect("due");
        assert_eq!((first.dial_hz, first.tune), (Some(DIAL), true));
        // gone out at T + 16: well inside the scan, so a copy follows at once
        facundo.went(&first.reference, true, None, T + 16);
        let second = facundo.due_with(T + 16).expect("a copy");
        assert_eq!(second.frame, first.frame, "a copy is the same frame");
        assert_eq!((second.dial_hz, second.tune), (Some(DIAL), true));
        facundo.went(&second.reference, true, None, T + 32);
        let third = facundo.due_with(T + 32).expect("another copy");
        // Ana's station came round to the dial during the second copy, and heard the third
        let mine = vec!["N0CALL".to_owned()];
        ana.heard_on(&third.frame, &mine, T + 48, Some(DIAL))
            .expect("heard");
        let heard = ana.due_with(T + 48).expect("heard, at once");
        assert_eq!((heard.dial_hz, heard.tune), (Some(DIAL), false));
        ana.went(&heard.reference, true, None, T + 59);
        facundo
            .heard(&heard.frame, &["KK4ODA".to_owned()], T + 59)
            .expect("told");
        // the copy on the air when the heard came is the last
        facundo.went(&third.reference, true, None, T + 60);
        assert!(facundo.due_with(T + 60).is_none(), "copied after a heard");
        assert_eq!(facundo.json()["log"][0]["state"], "heard");
        assert_eq!(facundo.json()["log"][0]["dial_hz"], DIAL);
        // the scan's time runs out with no heard: the copies stop, and the sending is over
        let lone = Alerts::open(&scratch("copies-lone"));
        lone.add_circle("Family", KEYWORD, Level::TimeSensitive)
            .expect("kept");
        lone.send_on("Family", Some("N0CALL"), "Anyone?", "KK4ODA", T, how)
            .expect("written");
        let mut copies = 0;
        let mut t = T;
        while let Some(d) = lone.due_with(t) {
            copies += 1;
            t += 16;
            lone.went(&d.reference, true, None, t);
        }
        assert_eq!(copies, 4, "copies until 49 s had passed since the first");
        assert_eq!(lone.json()["log"][0]["state"], "sent");
        let _ = (std::fs::remove_dir_all(&a), std::fs::remove_dir_all(&b));
    }

    /// Answers go on the dial the alert was heard on, though the scan has moved on by the time
    /// the operator acts; a copy heard while the `heard` still waits for the channel is not
    /// answered again, one heard after it went out is (RP-8 0.2.0 §5).
    #[test]
    fn answers_go_on_the_dial_heard_and_a_waiting_answer_is_not_doubled() {
        let (a, b) = (scratch("dial-a"), scratch("dial-b"));
        let facundo = Alerts::open(&a);
        let ana = Alerts::open(&b);
        facundo
            .add_circle("Family", KEYWORD, Level::TimeSensitive)
            .expect("kept");
        ana.add_circle("Family", KEYWORD, Level::TimeSensitive)
            .expect("kept");
        let mine = vec!["N0CALL".to_owned()];
        let tag = facundo
            .send("Family", Some("N0CALL"), "Call me", "KK4ODA", T)
            .expect("written");
        let (_, frame) = facundo.due(T).expect("due");
        ana.heard_on(&frame, &mine, T + 16, Some(DIAL))
            .expect("heard");
        // a copy heard while her heard waits: nothing more is queued
        assert!(ana.heard_on(&frame, &mine, T + 32, Some(DIAL)).is_none());
        let heard = ana.due_with(T + 32).expect("one heard");
        assert!(ana.due_with(T + 32).is_none(), "answered twice");
        ana.went(&heard.reference, true, None, T + 43);
        // heard again after it went: the sender did not hear it, so it goes again
        ana.heard_on(&frame, &mine, T + 60, Some(DIAL))
            .expect("answered again");
        let again = ana.due_with(T + 60).expect("again");
        assert_eq!(again.dial_hz, Some(DIAL));
        ana.went(&again.reference, true, None, T + 71);
        // minutes later, on another dial: seen and a reply go back to the alert's
        ana.acknowledge(&tag, T + 300).expect("acknowledged");
        let seen = ana.due_with(T + 300).expect("seen");
        assert_eq!((seen.dial_hz, seen.tune), (Some(DIAL), false));
        ana.reply(&tag, "On my way", T + 310).expect("replied");
        let reply = ana.due_with(T + 310).expect("reply");
        assert_eq!(reply.dial_hz, Some(DIAL));
        assert_eq!(ana.json()["log"][0]["dial_hz"], DIAL);
        // an alert sent with no dial names none and tunes nothing, and has no copies
        facundo
            .send_on(
                "Family",
                None,
                "Net in 10",
                "KK4ODA",
                T + 400,
                Sending {
                    dial_hz: None,
                    outlast_s: Some(49),
                },
            )
            .expect("written");
        let plain = facundo.due_with(T + 400).expect("due");
        assert_eq!((plain.dial_hz, plain.tune), (None, false));
        facundo.went(&plain.reference, true, None, T + 416);
        assert!(facundo.due_with(T + 416).is_none(), "copies with no dial");
        let _ = (std::fs::remove_dir_all(&a), std::fs::remove_dir_all(&b));
    }

    /// A circle's watch dial is kept with it, changed and cleared (RP-8 §7).
    #[test]
    fn a_circle_keeps_its_watch_dial() {
        let a = scratch("watch");
        let alerts = Alerts::open(&a);
        alerts
            .add_circle("Family", KEYWORD, Level::TimeSensitive)
            .expect("kept");
        assert!(alerts.circles()[0]["watch_hz"].is_null());
        alerts
            .set_circle("Family", None, None, Some(Some(DIAL)))
            .expect("set");
        assert_eq!(Alerts::open(&a).circles()[0]["watch_hz"], DIAL);
        alerts
            .set_circle("Family", None, Some(false), None)
            .expect("set");
        assert_eq!(
            Alerts::open(&a).circles()[0]["watch_hz"],
            DIAL,
            "left as it was"
        );
        alerts
            .set_circle("Family", None, None, Some(None))
            .expect("cleared");
        assert!(Alerts::open(&a).circles()[0]["watch_hz"].is_null());
        let _ = std::fs::remove_dir_all(&a);
    }

    /// RP-8 §2: the keyword is kept, normalised, through a restart; the page's list says only
    /// that it is kept, and the keyword comes when asked.
    #[test]
    fn a_circle_keeps_its_keyword_shown_only_when_asked() {
        let a = scratch("keyword");
        let alerts = Alerts::open(&a);
        alerts
            .add_circle("Home", "  KIMO dali\tSune PATO", Level::TimeSensitive)
            .expect("kept");
        let shown = &alerts.circles()[0];
        assert_eq!(shown["keyword_kept"], true);
        assert!(shown.get("keyword").is_none(), "not in the list: {shown}");
        assert!(
            !alerts.json().to_string().contains("kimo"),
            "nor in the view"
        );
        assert_eq!(alerts.keyword("Home").expect("kept"), KEYWORD);
        assert_eq!(Alerts::open(&a).keyword("Home").expect("kept"), KEYWORD);
        assert!(alerts.keyword("Away").is_err());
        let _ = std::fs::remove_dir_all(&a);
    }

    /// A circle kept before keywords were kept has none to show; typing its keyword again fills
    /// it in, whatever name is typed, and the circle keeps its name and settings.
    #[test]
    fn a_circle_kept_without_its_keyword_is_given_it_again() {
        let a = scratch("old");
        let key = radiopost::alert::Circle::from_keyword(KEYWORD)
            .expect("a circle")
            .key;
        let old = serde_json::json!([{"name": "Family", "key": hex(&key),
            "level": "important", "answer": false, "watch_hz": DIAL}]);
        std::fs::create_dir_all(&a).expect("made");
        std::fs::write(a.join(super::CIRCLES), old.to_string()).expect("written");
        let alerts = Alerts::open(&a);
        assert_eq!(alerts.circles()[0]["keyword_kept"], false);
        let why = alerts.keyword("Family").expect_err("none kept");
        assert!(why.contains("add it again"), "{why}");
        let filled = alerts
            .add_circle("Family", "Kimo Dali Sune Pato", Level::TimeSensitive)
            .expect("filled in");
        assert_eq!(filled["filled"], true);
        assert_eq!(filled["name"], "Family");
        let reopened = Alerts::open(&a);
        assert_eq!(reopened.keyword("Family").expect("kept"), KEYWORD);
        let c = &reopened.circles()[0];
        assert_eq!(c["level"], "important", "its settings stay");
        assert_eq!(c["answer"], false);
        assert_eq!(c["watch_hz"], DIAL);
        assert_eq!(reopened.circles().as_array().map(Vec::len), Some(1));
        let again = alerts
            .add_circle("Other", KEYWORD, Level::Normal)
            .expect_err("kept already");
        assert!(again.contains("already the circle Family"), "{again}");
        let _ = std::fs::remove_dir_all(&a);
    }

    /// Deleting a circle takes its keyword with it, and what was still to go for it does not go:
    /// an alert waiting, and a report sent again.
    #[test]
    fn deleting_a_circle_stops_what_was_to_go() {
        let a = scratch("delete");
        let alerts = Alerts::open(&a);
        alerts
            .add_circle("Family", KEYWORD, Level::TimeSensitive)
            .expect("kept");
        alerts
            .add_circle("Net", "riva toke lume sadi", Level::Normal)
            .expect("kept");
        alerts
            .send("Family", None, "Call me", "KK4ODA", T)
            .expect("written");
        alerts
            .report(
                "Family",
                None,
                position_report(337_812_300),
                "KK4ODA",
                T,
                Sending::default(),
                Some(600),
                vec![1],
            )
            .expect("written");
        alerts.remove_circle("Family").expect("deleted");
        assert!(alerts.due(T).is_none(), "nothing of Family goes");
        assert!(alerts.repeat_due(T + 600).is_none(), "the report stops");
        let log = alerts.json();
        assert_eq!(log["log"][0]["state"], "failed");
        assert_eq!(log["log"][0]["why"], "its circle was deleted");
        assert_eq!(alerts.reports()[0].state, "failed");
        assert!(alerts.keyword("Family").is_err());
        let left = Alerts::open(&a).circles();
        assert_eq!(left.as_array().map(Vec::len), Some(1));
        assert_eq!(left[0]["name"], "Net");
        assert!(alerts.remove_circle("Family").is_err(), "gone already");
        let _ = std::fs::remove_dir_all(&a);
    }
}
