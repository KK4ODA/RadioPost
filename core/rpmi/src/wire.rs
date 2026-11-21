// SPDX-License-Identifier: MIT OR Apache-2.0
//! RPMI's lines (RP-4 §2): one JSON object a line, its `op`, its `id`, its `data`, and the requests
//! of §4 read member by member.

use serde_json::{Map, Value};

use crate::base64;

/// One RPMI message: a JSON object. Messages compare as JSON values, never as text.
pub type Message = Map<String, Value>;

/// The protocol version this crate speaks (RP-4's header).
pub const VERSION: i64 = 1;

/// The adapter's port on the loopback interface (RP-4 §2).
pub const DEFAULT_PORT: u16 = 8600;

/// One line of JSON Lines, without its LF. Any spelling of the same JSON value is as good.
#[must_use]
pub fn line_of(message: &Message) -> String {
    Value::Object(message.clone()).to_string()
}

/// A message with `op`, then `id` when there is one.
#[must_use]
pub fn answer(op: &str, id: Option<&Value>) -> Message {
    let mut m = Message::new();
    m.insert("op".into(), op.into());
    if let Some(id) = id {
        m.insert("id".into(), id.clone());
    }
    m
}

/// `answer`, with more members after `op` and `id`.
#[must_use]
pub fn answer_with(op: &str, id: Option<&Value>, members: &[(&str, Value)]) -> Message {
    let mut m = answer(op, id);
    for (k, v) in members {
        m.insert((*k).into(), v.clone());
    }
    m
}

/// An `error` (RP-4 §5): its `code` from RP-4 §7 or RP-0 §9.3, and a `text` for people.
#[must_use]
pub fn error(id: Option<&Value>, code: &str, text: &str) -> Message {
    answer_with("error", id, &[("code", code.into()), ("text", text.into())])
}

fn is_id(v: &Value) -> bool {
    v.is_string() || integer(v).is_some()
}

/// The largest integer RP-4 §2 takes, either sign: I-JSON's exact range (RFC 7493 §2.2).
pub const MAX_INTEGER: i64 = (1 << 53) - 1;

/// A JSON value read as I-JSON (RFC 7493): `serde_json` already refuses a lone surrogate, `NaN`
/// and a number past double precision; this refuses a member named twice in one object too.
struct IJson(Value);

impl<'de> serde::Deserialize<'de> for IJson {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(IJsonVisitor).map(IJson)
    }
}

struct IJsonVisitor;

impl<'de> serde::de::Visitor<'de> for IJsonVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an I-JSON value")
    }
    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_f64<E>(self, v: f64) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_string<E>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut out = Vec::new();
        while let Some(IJson(v)) = seq.next_element()? {
            out.push(v);
        }
        Ok(Value::Array(out))
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut out = serde_json::Map::new();
        while let Some(name) = map.next_key::<String>()? {
            if out.contains_key(&name) {
                return Err(serde::de::Error::custom("a member named twice"));
            }
            let IJson(v) = map.next_value()?;
            out.insert(name, v);
        }
        Ok(Value::Object(out))
    }
}

/// RP-4 §2: the message a line holds, or the `error` `malformed` to answer it with. A line is
/// refused when it is not an I-JSON object with a string `op`, when its `id` is neither a string nor
/// an integer, or when its `data` is not standard base64 with padding; the `error` carries the
/// `id` when it could be read.
///
/// # Errors
///
/// The `error` message to answer the line with.
pub fn parse_line(line: &str) -> Result<Message, Message> {
    let Ok(IJson(value)) = serde_json::from_str::<IJson>(line) else {
        return Err(error(None, "malformed", "not I-JSON"));
    };
    let Value::Object(m) = value else {
        return Err(error(None, "malformed", "not a JSON object"));
    };
    let id = m.get("id");
    if id.is_some_and(|v| !is_id(v)) {
        return Err(error(
            None,
            "malformed",
            "`id` is neither a string nor an integer",
        ));
    }
    if !m.get("op").is_some_and(Value::is_string) {
        return Err(error(id, "malformed", "no `op`"));
    }
    if m.get("data")
        .is_some_and(|d| d.as_str().and_then(base64::decode).is_none())
    {
        return Err(error(
            id,
            "malformed",
            "`data` is not standard base64 with padding",
        ));
    }
    Ok(m)
}

/// The `op` of a message read by [`parse_line`].
#[must_use]
pub fn op_of(m: &Message) -> &str {
    m.get("op").and_then(Value::as_str).unwrap_or("")
}

/// A JSON integer, of either sign, within RP-4 §2's range: a number written with no fraction or
/// exponent (`serde_json` reads those as floats), at most 2^53-1 from zero.
fn integer(v: &Value) -> Option<i128> {
    v.as_i64()
        .map(i128::from)
        .or_else(|| v.as_u64().map(i128::from))
        .filter(|n| n.unsigned_abs() <= MAX_INTEGER.unsigned_abs().into())
}

/// An integer of RP-4 §2's range, 0 or above.
fn natural(v: &Value) -> Option<u64> {
    integer(v).and_then(|n| u64::try_from(n).ok())
}

/// The requests of RP-4 §4, read member by member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// §3: the versions the host speaks, lowest and highest, and its name.
    Hello {
        /// The lowest version.
        low: i128,
        /// The highest version.
        high: i128,
        /// The host program's name and version.
        app: String,
    },
    /// The callsigns this host answers to and calls as; the first is the default.
    Callsigns(Vec<String>),
    /// Whether calls to this host's callsigns are answered.
    Listen(bool),
    /// A call.
    Connect {
        /// The station called.
        remote: String,
        /// The host's callsign to call as; the first when absent.
        as_call: Option<String>,
        /// A bandwidth in Hz from `caps.bandwidths`.
        bandwidth: Option<i128>,
        /// How long the call's tries keep starting, in seconds, with `caps.call_s` (§4): a call
        /// to a gateway that scans.
        call_s: Option<u64>,
    },
    /// Bytes to queue on a link.
    Send {
        /// The link.
        link: String,
        /// The bytes.
        data: Vec<u8>,
        /// The `ref` to report `delivered` with.
        reference: Option<Value>,
    },
    /// Close a link in order.
    Disconnect {
        /// The link.
        link: String,
    },
    /// End a link, or the call under way, now.
    Abort {
        /// The link; with none, the call under way.
        link: Option<String>,
    },
    /// A frame to everyone listening (§6).
    Broadcast {
        /// The bytes.
        data: Vec<u8>,
        /// The `ref` to report `broadcast-sent` with.
        reference: Option<Value>,
        /// The dial it belongs on: a scanning modem goes there to send it (§6).
        dial_hz: Option<u64>,
    },
    /// The adapter's `status`.
    Status,
    /// The radio's dial (§4.1).
    Dial,
    /// Set the radio's dial, in hertz (§4.1).
    Tune(u64),
    /// Listen on each of these dials in turn, in hertz, the first the home; none stops the
    /// host's scan (§4.2).
    Scan(Vec<u64>),
    /// Key a steady tone for an antenna tuner for this many seconds, 1 to [`TONE_MAX_S`]; 0 stops
    /// one (§4.3).
    Tone(u64),
}

/// The longest tuner tone a host may ask for, in seconds (RP-4 §4.3).
pub const TONE_MAX_S: u64 = 10;

fn text(m: &Message, key: &str) -> Result<String, String> {
    match m.get(key) {
        None => Err(format!("no `{key}`")),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(format!("`{key}` is not a string")),
    }
}

fn optional_text(m: &Message, key: &str) -> Result<Option<String>, String> {
    if m.contains_key(key) {
        text(m, key).map(Some)
    } else {
        Ok(None)
    }
}

/// An optional `dial_hz` (§4, §6): absent, or an integer above 0.
fn optional_dial(m: &Message) -> Result<Option<u64>, String> {
    match m.get("dial_hz") {
        None => Ok(None),
        Some(v) => natural(v)
            .filter(|&hz| hz > 0)
            .map(Some)
            .ok_or_else(|| "`dial_hz` is not an integer above 0".into()),
    }
}

fn data(m: &Message) -> Result<Vec<u8>, String> {
    let t = text(m, "data")?;
    base64::decode(&t).ok_or_else(|| "`data` is not standard base64 with padding".into())
}

fn reference(m: &Message) -> Result<Option<Value>, String> {
    match m.get("ref") {
        None => Ok(None),
        Some(v) if is_id(v) => Ok(Some(v.clone())),
        Some(_) => Err("`ref` is neither a string nor an integer".into()),
    }
}

/// The radio's requests of RP-4 §4.1 to §4.3, read member by member: the dial, a scan, the tone.
fn radio(m: &Message) -> Result<Request, String> {
    Ok(match op_of(m) {
        "dial" => Request::Dial,
        "tune" => Request::Tune(
            m.get("dial_hz")
                .ok_or("no `dial_hz`")
                .map(natural)?
                .filter(|&hz| hz > 0)
                .ok_or("`dial_hz` is not an integer above 0")?,
        ),
        "scan" => {
            if !m.contains_key("id") {
                return Err("a `scan` needs an `id`".into());
            }
            let dials = m
                .get("dials")
                .ok_or("no `dials`")?
                .as_array()
                .and_then(|a| {
                    a.iter()
                        .map(|v| natural(v).filter(|&hz| hz > 0))
                        .collect::<Option<Vec<_>>>()
                })
                .ok_or("`dials` is not a list of integers above 0")?;
            Request::Scan(dials)
        }
        "tone" => Request::Tone(
            m.get("seconds")
                .ok_or("no `seconds`")
                .map(natural)?
                .filter(|&s| s <= TONE_MAX_S)
                .ok_or("`seconds` is not an integer from 0 to 10")?,
        ),
        _ => return Err(format!("no op {}", op_of(m))),
    })
}

impl Request {
    /// The request a message holds: `Ok(None)` for an `op` RP-4 §4 does not name, and
    /// `Err(text)` for a member the request needs that is missing or of the wrong type, which the
    /// adapter answers with `error` `malformed` (RP-4 §2).
    ///
    /// # Errors
    ///
    /// What is wrong with the request, for the `error`'s `text`.
    pub fn read(m: &Message) -> Result<Option<Request>, String> {
        let request = match op_of(m) {
            "hello" => {
                let version = m.get("version").ok_or("no `version`")?;
                let pair = version
                    .as_array()
                    .filter(|a| a.len() == 2)
                    .and_then(|a| Some((integer(&a[0])?, integer(&a[1])?)))
                    .ok_or("`version` is not two integers")?;
                Request::Hello {
                    low: pair.0,
                    high: pair.1,
                    app: text(m, "app")?,
                }
            }
            "callsigns" => {
                let list = m
                    .get("callsigns")
                    .ok_or("no `callsigns`")?
                    .as_array()
                    .and_then(|a| {
                        a.iter()
                            .map(|c| c.as_str().map(str::to_owned))
                            .collect::<Option<Vec<_>>>()
                    })
                    .ok_or("`callsigns` is not a list of strings")?;
                Request::Callsigns(list)
            }
            "listen" => Request::Listen(
                m.get("on")
                    .ok_or("no `on`")?
                    .as_bool()
                    .ok_or("`on` is not true or false")?,
            ),
            "connect" => {
                if !m.contains_key("id") {
                    return Err("a `connect` needs an `id`".into());
                }
                let bandwidth = match m.get("bandwidth") {
                    None => None,
                    Some(v) => Some(integer(v).ok_or("`bandwidth` is not an integer")?),
                };
                let call_s = match m.get("call_s") {
                    None => None,
                    Some(v) => Some(natural(v).ok_or("`call_s` is not an integer, 0 or above")?),
                };
                Request::Connect {
                    remote: text(m, "remote")?,
                    as_call: optional_text(m, "as")?,
                    bandwidth,
                    call_s,
                }
            }
            "send" => Request::Send {
                link: text(m, "link")?,
                data: data(m)?,
                reference: reference(m)?,
            },
            "disconnect" => Request::Disconnect {
                link: text(m, "link")?,
            },
            "abort" => Request::Abort {
                link: optional_text(m, "link")?,
            },
            "broadcast" => Request::Broadcast {
                data: data(m)?,
                reference: reference(m)?,
                dial_hz: optional_dial(m)?,
            },
            "status" => Request::Status,
            "dial" | "tune" | "scan" | "tone" => radio(m)?,
            _ => return Ok(None),
        };
        Ok(Some(request))
    }
}

/// RP-4 §2, §3: what an adapter checks of a message [`parse_line`] read before it acts on it, in
/// order: `hello` first (`refused`), an `op` it knows (`unsupported`), and the members its request
/// needs, of their types (`malformed`).
///
/// # Errors
///
/// The `error` to answer the message with.
pub fn admit(m: &Message, hello_said: bool) -> Result<Request, Message> {
    let op = op_of(m);
    let id = m.get("id");
    if !hello_said && op != "hello" {
        return Err(error(id, "refused", "hello first"));
    }
    match Request::read(m) {
        Ok(Some(request)) => Ok(request),
        Ok(None) => Err(error(id, "unsupported", &format!("no op {op}"))),
        Err(why) => Err(error(id, "malformed", &why)),
    }
}

/// Whether a host speaking versions `low` to `high` and this crate have one in common (§3).
#[must_use]
pub fn speaks(low: i128, high: i128) -> bool {
    (low..=high).contains(&i128::from(VERSION))
}

/// What a modem can do (RP-4 §3).
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools, reason = "RP-4 §3's own flags")]
pub struct Caps {
    /// Bandwidths in Hz a `connect` may ask for.
    pub bandwidths: Vec<i64>,
    /// How many links may be up at once.
    pub max_links: usize,
    /// `delivered` is reported.
    pub delivery: bool,
    /// Broadcasts are carried.
    pub broadcast: bool,
    /// `quality` is reported.
    pub quality: bool,
    /// `busy` is reported.
    pub busy: bool,
    /// The modem checks the band rules (§8).
    pub regulatory: bool,
    /// The most bytes a host should keep queued on a link.
    pub max_queue: u64,
    /// Other programs may use the modem under §1.1's rules.
    pub shared: bool,
    /// The adapter reads and sets the radio's dial (§4.1).
    pub dial: bool,
    /// The adapter scans the radio over a list of dials (§4.2).
    pub scan: bool,
    /// The adapter keeps a call going as long as `connect`'s `call_s` asks (§4).
    pub call_s: bool,
    /// The adapter keys a tuner tone (§4.3).
    pub tone: bool,
}

impl Caps {
    /// The `caps` member of `hello`; `dial`, `scan`, `call_s` and `tone` only when the adapter
    /// has them, as §3 lets them be absent.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut caps = serde_json::json!({
            "bandwidths": self.bandwidths,
            "max_links": self.max_links,
            "delivery": self.delivery,
            "broadcast": self.broadcast,
            "quality": self.quality,
            "busy": self.busy,
            "regulatory": self.regulatory,
            "max_queue": self.max_queue,
            "shared": self.shared,
        });
        for (key, set) in [
            ("dial", self.dial),
            ("scan", self.scan),
            ("call_s", self.call_s),
            ("tone", self.tone),
        ] {
            if set {
                caps[key] = Value::Bool(true);
            }
        }
        caps
    }

    /// Caps from a `hello`'s `caps`, ignoring members it does not know (RP-4 §2). A member that is
    /// absent reads as false or empty; `shared` absent is false (§3).
    #[must_use]
    pub fn from_json(v: &Value) -> Caps {
        let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        Caps {
            bandwidths: v
                .get("bandwidths")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_i64).collect())
                .unwrap_or_default(),
            max_links: v
                .get("max_links")
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(1),
            delivery: flag("delivery"),
            broadcast: flag("broadcast"),
            quality: flag("quality"),
            busy: flag("busy"),
            regulatory: flag("regulatory"),
            max_queue: v.get("max_queue").and_then(Value::as_u64).unwrap_or(0),
            shared: flag("shared"),
            dial: flag("dial"),
            scan: flag("scan"),
            call_s: flag("call_s"),
            tone: flag("tone"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Request, parse_line};

    fn read(line: &str) -> Result<Option<Request>, String> {
        Request::read(&parse_line(line).expect("a line"))
    }

    #[test]
    fn members_are_read_by_type() {
        assert_eq!(
            read(r#"{"op":"connect","id":3,"remote":"N1CALL-10","bandwidth":2300}"#),
            Ok(Some(Request::Connect {
                remote: "N1CALL-10".into(),
                as_call: None,
                bandwidth: Some(2300),
                call_s: None,
            }))
        );
        assert_eq!(
            read(r#"{"op":"connect","id":3,"remote":"N1CALL-10","call_s":14}"#),
            Ok(Some(Request::Connect {
                remote: "N1CALL-10".into(),
                as_call: None,
                bandwidth: None,
                call_s: Some(14),
            }))
        );
        assert_eq!(
            read(r#"{"op":"scan","id":4,"dials":[3590000,7101000]}"#),
            Ok(Some(Request::Scan(vec![3_590_000, 7_101_000])))
        );
        assert_eq!(
            read(r#"{"op":"scan","id":7,"dials":[]}"#),
            Ok(Some(Request::Scan(Vec::new())))
        );
        assert_eq!(
            read(r#"{"op":"tune","id":9,"dial_hz":7101000}"#),
            Ok(Some(Request::Tune(7_101_000)))
        );
        assert_eq!(
            read(r#"{"op":"tone","id":9,"seconds":10}"#),
            Ok(Some(Request::Tone(10)))
        );
        assert_eq!(
            read(r#"{"op":"tone","seconds":0}"#),
            Ok(Some(Request::Tone(0)))
        );
        assert_eq!(read(r#"{"op":"rig","id":9}"#), Ok(None));
        for bad in [
            r#"{"op":"connect","remote":"N1CALL-10"}"#,
            r#"{"op":"connect","id":1,"remote":7}"#,
            r#"{"op":"connect","id":1,"remote":"X","bandwidth":"2300"}"#,
            r#"{"op":"callsigns","id":1}"#,
            r#"{"op":"callsigns","id":1,"callsigns":["A",1]}"#,
            r#"{"op":"listen","id":1,"on":"yes"}"#,
            r#"{"op":"hello","id":1,"version":1,"app":"x"}"#,
            r#"{"op":"hello","id":1,"version":[1,1]}"#,
            r#"{"op":"send","id":1,"link":1,"data":"AQ=="}"#,
            r#"{"op":"send","id":1,"link":"c1"}"#,
            r#"{"op":"send","id":1,"link":"c1","data":"AQ==","ref":1.5}"#,
            r#"{"op":"disconnect","id":1}"#,
            r#"{"op":"abort","id":1,"link":false}"#,
            r#"{"op":"tune","id":1}"#,
            r#"{"op":"tune","id":1,"dial_hz":0}"#,
            r#"{"op":"tune","id":1,"dial_hz":-7101000}"#,
            r#"{"op":"tune","id":1,"dial_hz":7101000.5}"#,
            r#"{"op":"tune","id":1,"dial_hz":"7101000"}"#,
            r#"{"op":"connect","id":1,"remote":"X","call_s":-1}"#,
            r#"{"op":"connect","id":1,"remote":"X","call_s":14.5}"#,
            r#"{"op":"scan","dials":[3590000]}"#,
            r#"{"op":"scan","id":1}"#,
            r#"{"op":"scan","id":1,"dials":3590000}"#,
            r#"{"op":"scan","id":1,"dials":[3590000,0]}"#,
            r#"{"op":"scan","id":1,"dials":[3590000,"7101000"]}"#,
            r#"{"op":"tone","id":1}"#,
            r#"{"op":"tone","id":1,"seconds":11}"#,
            r#"{"op":"tone","id":1,"seconds":-1}"#,
            r#"{"op":"tone","id":1,"seconds":2.5}"#,
            r#"{"op":"tone","id":1,"seconds":"10"}"#,
        ] {
            assert!(read(bad).is_err(), "{bad}");
        }
    }
}
