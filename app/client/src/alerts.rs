// SPDX-License-Identifier: MIT OR Apache-2.0
//! Alerts between stations (RP-8, wishlist W52), and the settings the panel sounds by (W51).
//!
//! * **Circles.** Each is a keyword the operator typed, kept as its key (never the keyword) in
//!   `circles.json` in the first home, with how loud its alerts are and whether the station says
//!   `heard` by itself. The computer's, not a callsign's: one alarm for every callsign it works.
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

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use radiopost::alert::{self, Circle, Draft, Frame, Heard, Kind};
use radiopost::cbor::hex;
use serde_json::{Value, json};

use crate::callsigns;
use crate::store::{Error, Store, unhex, write};

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
    /// How its alerts arrive.
    pub level: Level,
    /// The station says `heard` by itself (RP-8 §5, §8).
    pub answer: bool,
}

impl Kept {
    fn json(&self) -> Value {
        json!({"name": self.name, "key": hex(&self.circle.key), "level": self.level.name(),
               "answer": self.answer})
    }

    fn from_json(v: &Value) -> Option<Kept> {
        let key: [u8; 32] = unhex(v["key"].as_str()?)?.try_into().ok()?;
        Some(Kept {
            name: v["name"].as_str()?.to_owned(),
            circle: Circle::from_key(key),
            level: v["level"]
                .as_str()
                .and_then(Level::named)
                .unwrap_or(Level::TimeSensitive),
            answer: v["answer"].as_bool().unwrap_or(true),
        })
    }

    /// What the page shows of it: never the key.
    fn shown(&self) -> Value {
        json!({"name": self.name, "check": self.circle.check(), "level": self.level.name(),
               "answer": self.answer})
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

/// A frame waiting for the modem.
#[derive(Clone, Debug)]
struct Outgoing {
    frame: Vec<u8>,
    /// Not before this time: the spread of an answer to the whole circle.
    not_before: u64,
    /// The alert it is, when it is one this station sent (answers have none).
    alert: Option<[u8; 8]>,
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

    /// Keeps a circle from its keyword (RP-8 §2).
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
        let mut s = self.lock();
        if s.circles.iter().any(|c| c.name.eq_ignore_ascii_case(name)) {
            return Err(format!("a circle is already called {name}"));
        }
        if let Some(same) = s.circles.iter().find(|c| c.circle.id == circle.id) {
            return Err(format!("that keyword is already the circle {}", same.name));
        }
        let kept = Kept {
            name: name.to_owned(),
            circle,
            level,
            answer: true,
        };
        let shown = kept.shown();
        s.circles.push(kept);
        s.save_circles()?;
        s.seq += 1;
        Ok(shown)
    }

    /// Changes how a circle's alerts arrive, and whether the station says `heard` by itself.
    ///
    /// # Errors
    /// No such circle, or a file that cannot be written.
    pub fn set_circle(
        &self,
        name: &str,
        level: Option<Level>,
        answer: Option<bool>,
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
        s.save_circles()?;
        s.seq += 1;
        Ok(())
    }

    /// Forgets a circle: its alerts are no longer heard nor sent. The log keeps what it holds.
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
        s.seq += 1;
        Ok(())
    }

    /// Writes an alert from `from` to `to` (or the whole circle), queued for the modem.
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
        });
        s.queue.push_back(Outgoing {
            frame,
            not_before: now,
            alert: Some(tag),
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
        // a seen already sent is not sent again; a reply is sent each time it is written
        if kind == Kind::Reply || s.entries[i].state == "new" {
            let raw = alert::answer(&circle, &frame, kind, &me, now, text)?;
            s.queue.push_back(Outgoing {
                frame: raw,
                not_before: now,
                alert: None,
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
        })
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
                s.take(frame, mine, now)
            }
            Heard::HeardAgain(frame) => {
                let last = s.heard.insert(frame.tag, now);
                if last.is_some_and(|t| now.saturating_sub(t) < SAME_TRANSMISSION_S) {
                    return None;
                }
                s.again(&frame, mine, now)
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
        let mut s = self.lock();
        let i = s.queue.iter().position(|o| o.not_before <= now)?;
        let outgoing = s.queue.remove(i)?;
        s.references += 1;
        let reference = format!("alert{}", s.references);
        let frame = outgoing.frame.clone();
        s.flying.push(Flying {
            reference: reference.clone(),
            outgoing,
            at: now,
        });
        Some((reference, frame))
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
        let Some(tag) = f.outgoing.alert else {
            return;
        };
        if let Some(e) = s.entries.iter_mut().find(|e| e.out && e.tag == tag) {
            if sent {
                e.sent_at = Some(now);
                if e.state == "waiting" {
                    e.state = "sent";
                }
            } else {
                e.state = "failed";
                e.why = Some(text.unwrap_or("the modem did not send it").to_owned());
            }
        }
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
        for e in s.entries.iter_mut().filter(|e| e.out && e.state == "sent") {
            let Some(at) = e.sent_at else { continue };
            if now.saturating_sub(at) < RETRY_AFTER_S {
                continue;
            }
            if e.tries < TRIES {
                e.tries += 1;
                e.state = "waiting";
                again.push(Outgoing {
                    frame: e.frame.clone(),
                    not_before: now,
                    alert: Some(e.tag),
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
        let all: Vec<Value> = self.entries.iter().map(Entry::json).collect();
        if let Ok(text) = serde_json::to_string_pretty(&all)
            && let Err(e) = write(&self.base.join(LOG), text.as_bytes())
        {
            radiopost::logfile::line(&format!("alerts: {e}"));
        }
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
                tag: e.tag,
            },
        ))
    }

    fn queue_answer(&mut self, circle: &Circle, frame: &Frame, kind: Kind, me: &str, now: u64) {
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
        });
    }

    /// A new frame for this station: an alert kept and answered, or an answer to one of its own.
    fn take(&mut self, frame: Frame, mine: &[String], now: u64) -> Option<String> {
        let kept = self.circle_named_for(frame.circle)?.clone();
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
                self.queue_answer(&kept.circle, &frame, Kind::Heard, &me, now);
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
            });
            self.changed();
            return Some(line);
        }
        let re = frame.re?;
        let e = self.entries.iter_mut().find(|e| e.out && e.tag == re)?;
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

    /// An alert heard again: the sender is trying again, so its answers were lost (RP-8 §5).
    fn again(&mut self, frame: &Frame, mine: &[String], now: u64) -> Option<String> {
        if frame.kind != Kind::Alert {
            return None;
        }
        let kept = self.circle_named_for(frame.circle)?.clone();
        if !kept.answer {
            return None;
        }
        let state = self
            .entries
            .iter()
            .find(|e| !e.out && e.tag == frame.tag)
            .map(|e| e.state);
        let me = answering_as(mine, frame);
        self.queue_answer(&kept.circle, frame, Kind::Heard, &me, now);
        match state {
            Some("seen") => self.queue_answer(&kept.circle, frame, Kind::Seen, &me, now),
            Some("replied") => self.queue_answer(&kept.circle, frame, Kind::Reply, &me, now),
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
    use super::{Alerts, Level, callsigns_of};
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
}
