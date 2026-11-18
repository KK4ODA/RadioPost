// SPDX-License-Identifier: MIT OR Apache-2.0
//! Alerts (RP-8): a circle's key from its keyword (§2), an alert's frame and its tag (§3), what a
//! station makes of a broadcast it hears (§4), and the answers (§5).
//!
//! Sans-IO: the clock, the circles a station knows and the tags it has heard come in as arguments,
//! so `tests/vectors.rs` replays `vectors/alerts.json` exactly.

use std::collections::HashSet;
use std::hash::BuildHasher;

use unicode_normalization::UnicodeNormalization as _;

use crate::callsign;
use crate::cbor::{self, Value};
use crate::crypto::{self, context};

/// PBKDF2's iterations for a circle's key (RP-8 §2).
pub const ITERATIONS: u32 = 600_000;
/// The shortest keyword a program keeps, in characters once normalised (RP-8 §2).
pub const KEYWORD_MIN: usize = 8;
/// The longest line an alert or a reply carries, in bytes of UTF-8 (RP-8 §3).
pub const TEXT_MAX: usize = 100;
/// How far an alert's time may be from the receiver's clock, either way (RP-8 §4, step 5).
pub const WINDOW_S: u64 = 600;
/// How long a tag heard is kept, so a frame heard again is known for one (RP-8 §4, step 7).
pub const REMEMBER_S: u64 = 2 * WINDOW_S;

/// The frame's keys (RP-8 §3).
mod key {
    pub const T: i64 = 0;
    pub const CIRCLE: i64 = 1;
    pub const FROM: i64 = 2;
    pub const TO: i64 = 3;
    pub const TIME: i64 = 4;
    pub const TEXT: i64 = 5;
    pub const RE: i64 = 6;
    pub const TAG: i64 = 7;
}

/// The kinds of frame RP-8 defines: key 0, from RP-0 §9.5's broadcast types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// An alert.
    Alert,
    /// The station alerted has heard it.
    Heard,
    /// Its operator has seen it.
    Seen,
    /// Its operator's line in answer.
    Reply,
}

impl Kind {
    fn number(self) -> u64 {
        match self {
            Kind::Alert => 1,
            Kind::Heard => 2,
            Kind::Seen => 3,
            Kind::Reply => 4,
        }
    }

    fn of(n: u64) -> Option<Kind> {
        match n {
            1 => Some(Kind::Alert),
            2 => Some(Kind::Heard),
            3 => Some(Kind::Seen),
            4 => Some(Kind::Reply),
            _ => None,
        }
    }

    /// Its name in RP-8 §3: `alert`, `heard`, `seen` or `reply`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Kind::Alert => "alert",
            Kind::Heard => "heard",
            Kind::Seen => "seen",
            Kind::Reply => "reply",
        }
    }

    /// A kind by its name.
    #[must_use]
    pub fn named(name: &str) -> Option<Kind> {
        [Kind::Alert, Kind::Heard, Kind::Seen, Kind::Reply]
            .into_iter()
            .find(|k| k.name() == name)
    }
}

/// RP-8 §2: the keyword normalised: NFC, the ASCII capitals in lower case, every run of spaces
/// and tabs one space, none at either end.
#[must_use]
pub fn normalise_keyword(keyword: &str) -> String {
    let lowered: String = keyword
        .nfc()
        .map(|c| c.to_ascii_lowercase())
        .map(|c| if c == '\t' { ' ' } else { c })
        .collect();
    lowered
        .split(' ')
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Why a program refuses a keyword (RP-8 §2), or `None`.
#[must_use]
pub fn keyword_problem(keyword: &str) -> Option<String> {
    (normalise_keyword(keyword).chars().count() < KEYWORD_MIN)
        .then(|| format!("shorter than {KEYWORD_MIN} characters"))
}

/// An alert's or a reply's line as it goes on the air: NFC (RP-0 §4.3).
#[must_use]
pub fn normalise_text(text: &str) -> String {
    text.nfc().collect()
}

/// A circle: its key, and the ID its alerts name it by (RP-8 §2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Circle {
    /// PBKDF2-HMAC-SHA256 of the normalised keyword.
    pub key: [u8; 32],
    /// The first 4 bytes of the key's tag over its ID context.
    pub id: [u8; 4],
}

impl Circle {
    /// The circle a keyword makes; a fraction of a second's work, done once when it is typed.
    ///
    /// # Errors
    /// A keyword too short to keep (RP-8 §2).
    pub fn from_keyword(keyword: &str) -> Result<Circle, String> {
        if let Some(why) = keyword_problem(keyword) {
            return Err(format!("the keyword is {why}"));
        }
        let key = crypto::pbkdf2_sha256(
            normalise_keyword(keyword).as_bytes(),
            context::CIRCLE.as_bytes(),
            ITERATIONS,
        );
        Ok(Circle::from_key(key))
    }

    /// The circle of a key kept.
    #[must_use]
    pub fn from_key(key: [u8; 32]) -> Circle {
        let h = crypto::mac(&key, context::CIRCLE_ID, &[]);
        Circle {
            key,
            id: [h[0], h[1], h[2], h[3]],
        }
    }

    /// The ID as operators read it to each other: two groups of four hex digits (RP-8 §2).
    #[must_use]
    pub fn check(&self) -> String {
        check_text(&self.id)
    }
}

/// RP-8 §2: a circle's ID in two groups of four hex digits.
#[must_use]
pub fn check_text(id: &[u8; 4]) -> String {
    let h = cbor::hex(id);
    format!("{} {}", &h[..4], &h[4..])
}

/// A frame, read (RP-8 §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// What it is.
    pub kind: Kind,
    /// Its circle's ID.
    pub circle: [u8; 4],
    /// The sender's callsign.
    pub from: String,
    /// The station it is for; `None` for an alert to the whole circle.
    pub to: Option<String>,
    /// When it was sent, Unix seconds.
    pub time: u64,
    /// An alert's or a reply's line.
    pub text: Option<String>,
    /// The tag of the alert an answer answers.
    pub re: Option<[u8; 8]>,
    /// Its tag, which names it.
    pub tag: [u8; 8],
}

/// What goes in a frame, before its tag.
#[derive(Clone, Debug)]
pub struct Draft<'a> {
    /// What it is.
    pub kind: Kind,
    /// The sender's callsign.
    pub from: &'a str,
    /// The station it is for; `None` for an alert to the whole circle.
    pub to: Option<&'a str>,
    /// When it is sent.
    pub time: u64,
    /// An alert's or a reply's line.
    pub text: Option<&'a str>,
    /// The alert an answer answers.
    pub re: Option<[u8; 8]>,
}

/// The tag of a map's encoding without key 7 (RP-8 §3).
fn tag_of(circle: &Circle, untagged: &[u8]) -> [u8; 8] {
    let h = crypto::mac(&circle.key, context::ALERT, untagged);
    let mut tag = [0u8; 8];
    tag.copy_from_slice(&h[..8]);
    tag
}

/// Makes a frame, checked against RP-8 §3 as a receiver will check it.
///
/// # Errors
/// A draft a receiver would refuse as malformed: says which rule.
pub fn make(circle: &Circle, draft: &Draft<'_>) -> Result<Vec<u8>, String> {
    let mut entries = vec![
        (key::T, Value::Uint(draft.kind.number())),
        (key::CIRCLE, Value::bytes(&circle.id)),
        (key::FROM, Value::text(draft.from)),
        (key::TIME, Value::Uint(draft.time)),
    ];
    if let Some(to) = draft.to {
        entries.push((key::TO, Value::text(to)));
    }
    if let Some(text) = draft.text {
        entries.push((key::TEXT, Value::text(&normalise_text(text))));
    }
    if let Some(re) = draft.re {
        entries.push((key::RE, Value::bytes(&re)));
    }
    let untagged = Value::int_map(entries.clone());
    let tag = tag_of(circle, &untagged.encode().map_err(|e| e.to_string())?);
    entries.push((key::TAG, Value::bytes(&tag)));
    let frame = Value::int_map(entries);
    if let Some(why) = shape(&frame).err() {
        return Err(why.to_owned());
    }
    frame.encode().map_err(|e| e.to_string())
}

/// An answer to an alert (RP-8 §5): `heard`, `seen` or a `reply`, from `me` at `now`.
///
/// # Errors
/// An alert for nobody in particular cannot be answered; a reply needs a line.
pub fn answer(
    circle: &Circle,
    alert: &Frame,
    kind: Kind,
    me: &str,
    now: u64,
    text: Option<&str>,
) -> Result<Vec<u8>, String> {
    if kind == Kind::Alert {
        return Err("an answer is heard, seen or a reply".into());
    }
    make(
        circle,
        &Draft {
            kind,
            from: me,
            to: Some(&alert.from),
            time: now,
            text,
            re: Some(alert.tag),
        },
    )
}

fn sent_callsign(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_text).filter(|c| callsign::valid(c))
}

/// RP-8 §3's rules on a map whose key 0 is one of its kinds: the frame, or the rule broken.
fn shape(m: &Value) -> Result<Frame, &'static str> {
    let kind = m
        .get(key::T)
        .and_then(Value::as_u64)
        .and_then(Kind::of)
        .ok_or("t: an RP-8 kind")?;
    let circle: [u8; 4] = m
        .get(key::CIRCLE)
        .and_then(Value::as_bytes)
        .and_then(|b| b.try_into().ok())
        .ok_or("circle: 4 bytes")?;
    let from = sent_callsign(m.get(key::FROM)).ok_or("from: a callsign")?;
    let time = m
        .get(key::TIME)
        .and_then(Value::as_u64)
        .ok_or("time: Unix seconds")?;
    let to = match m.get(key::TO) {
        None => None,
        Some(v) => Some(sent_callsign(Some(v)).ok_or("to: a callsign")?),
    };
    if to.is_none() && kind != Kind::Alert {
        return Err("to: only an alert may leave it out");
    }
    let text = match (kind, m.get(key::TEXT)) {
        (Kind::Alert | Kind::Reply, Some(v)) => {
            let t = v.as_text().ok_or("text: needed")?;
            if !(1..=TEXT_MAX).contains(&t.len()) {
                return Err("text: 1 to 100 bytes");
            }
            if t.nfc().collect::<String>() != t {
                return Err("text: NFC");
            }
            if t.chars().any(char::is_control) {
                return Err("text: no control characters");
            }
            Some(t.to_owned())
        }
        (Kind::Alert | Kind::Reply, None) => return Err("text: needed"),
        (_, Some(_)) => return Err("text: only an alert or a reply"),
        (_, None) => None,
    };
    let re = match (kind, m.get(key::RE)) {
        (Kind::Alert, None) => None,
        (Kind::Alert, Some(_)) => return Err("re: not on an alert"),
        (_, v) => Some(
            v.and_then(Value::as_bytes)
                .and_then(|b| b.try_into().ok())
                .ok_or("re: 8 bytes")?,
        ),
    };
    let tag: [u8; 8] = m
        .get(key::TAG)
        .and_then(Value::as_bytes)
        .and_then(|b| b.try_into().ok())
        .ok_or("tag: 8 bytes")?;
    Ok(Frame {
        kind,
        circle,
        from: from.to_owned(),
        to: to.map(str::to_owned),
        time,
        text,
        re,
        tag,
    })
}

/// What a station makes of a broadcast (RP-8 §4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Heard {
    /// Not an RP-8 frame at all: another program's broadcast, or a kind not known.
    NotAnAlert,
    /// Its keys break RP-8 §3's rules.
    Malformed(&'static str),
    /// It names a circle the station does not know.
    UnknownCircle,
    /// Its tag is not its circle's.
    TagDoesNotCheck,
    /// Its time is more than [`WINDOW_S`] from the station's clock.
    OutOfTime,
    /// It is for another station.
    ForAnotherStation(Frame),
    /// Its tag was heard in the last [`REMEMBER_S`].
    HeardAgain(Frame),
    /// An alert, a receipt or a reply for this station, or an alert for the circle.
    New(Frame),
}

impl Heard {
    /// The verdict's words, as `vectors/alerts.json` writes them.
    #[must_use]
    pub fn verdict(&self) -> &'static str {
        match self {
            Heard::NotAnAlert => "not an alert",
            Heard::Malformed(_) => "malformed",
            Heard::UnknownCircle => "unknown circle",
            Heard::TagDoesNotCheck => "the tag does not check",
            Heard::OutOfTime => "out of time",
            Heard::ForAnotherStation(_) => "for another station",
            Heard::HeardAgain(_) => "heard again",
            Heard::New(f) => match (f.kind, &f.to) {
                (Kind::Alert, None) => "an alert for the circle",
                (Kind::Alert, Some(_)) => "an alert for this station",
                (kind, _) => kind.name(),
            },
        }
    }
}

/// RP-8 §4: what a station with the callsigns `mine`, knowing `circles`, at `now`, having heard
/// the tags in `heard` within [`REMEMBER_S`], makes of a broadcast's data. The checks in the
/// specification's order: the first that fails decides.
#[must_use]
pub fn hear<S: BuildHasher>(
    data: &[u8],
    mine: &[String],
    circles: &[Circle],
    now: u64,
    heard: &HashSet<[u8; 8], S>,
) -> Heard {
    let Ok(map) = cbor::decode(data) else {
        return Heard::NotAnAlert;
    };
    if map.as_map().is_none()
        || map
            .get(key::T)
            .and_then(Value::as_u64)
            .and_then(Kind::of)
            .is_none()
    {
        return Heard::NotAnAlert;
    }
    let frame = match shape(&map) {
        Ok(f) => f,
        Err(why) => return Heard::Malformed(why),
    };
    let Some(circle) = circles.iter().find(|c| c.id == frame.circle) else {
        return Heard::UnknownCircle;
    };
    let untagged = match &map {
        Value::Map(entries) => Value::Map(
            entries
                .iter()
                .filter(|(k, _)| k.as_i64() != Some(key::TAG))
                .cloned()
                .collect(),
        ),
        _ => return Heard::NotAnAlert,
    };
    let Ok(untagged) = untagged.encode() else {
        return Heard::NotAnAlert;
    };
    let tag = tag_of(circle, &untagged);
    if !same_in_constant_time(tag, frame.tag) {
        return Heard::TagDoesNotCheck;
    }
    if now.abs_diff(frame.time) > WINDOW_S {
        return Heard::OutOfTime;
    }
    if let Some(to) = &frame.to
        && !mine.iter().any(|c| callsign::same_base(c, to))
    {
        return Heard::ForAnotherStation(frame);
    }
    if heard.contains(&frame.tag) {
        return Heard::HeardAgain(frame);
    }
    Heard::New(frame)
}

fn same_in_constant_time(a: [u8; 8], b: [u8; 8]) -> bool {
    a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::{Circle, Draft, Heard, Kind, hear, make, normalise_keyword};
    use std::collections::HashSet;

    #[test]
    fn a_keyword_typed_two_ways_is_one_circle() {
        assert_eq!(
            normalise_keyword("  KIMO  Dali\tsune pato "),
            "kimo dali sune pato"
        );
        assert_eq!(normalise_keyword("Ñandú corre lejos"), "Ñandú corre lejos");
        assert!(Circle::from_keyword("eagle").is_err());
    }

    #[test]
    fn a_frame_made_is_heard_and_a_draft_a_receiver_would_refuse_is_not_made() {
        let circle = Circle::from_key([7; 32]);
        let draft = Draft {
            kind: Kind::Alert,
            from: "KK4ODA",
            to: Some("N0CALL"),
            time: 1000,
            text: Some("Call me"),
            re: None,
        };
        let raw = make(&circle, &draft).expect("a good draft");
        let mine = vec!["N0CALL-7".to_owned()];
        let heard = hear(
            &raw,
            &mine,
            std::slice::from_ref(&circle),
            1100,
            &HashSet::new(),
        );
        assert_eq!(heard.verdict(), "an alert for this station");
        let Heard::New(frame) = heard else {
            panic!("new")
        };
        assert_eq!(frame.text.as_deref(), Some("Call me"));
        let long = "x".repeat(101);
        let bad = Draft {
            text: Some(&long),
            ..draft
        };
        assert!(make(&circle, &bad).is_err());
    }
}
