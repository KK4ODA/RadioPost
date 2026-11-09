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
    v.is_string() || v.is_i64() || v.is_u64()
}

/// RP-4 §2: the message a line holds, or the `error` `malformed` to answer it with. A line is
/// refused when it is not a JSON object with a string `op`, when its `id` is neither a string nor
/// an integer, or when its `data` is not standard base64 with padding; the `error` carries the
/// `id` when it could be read.
///
/// # Errors
///
/// The `error` message to answer the line with.
pub fn parse_line(line: &str) -> Result<Message, Message> {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Err(error(None, "malformed", "not JSON"));
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

/// A JSON integer, of either sign.
fn integer(v: &Value) -> Option<i128> {
    v.as_i64()
        .map(i128::from)
        .or_else(|| v.as_u64().map(i128::from))
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
    },
    /// The adapter's `status`.
    Status,
}

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
                Request::Connect {
                    remote: text(m, "remote")?,
                    as_call: optional_text(m, "as")?,
                    bandwidth,
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
            },
            "status" => Request::Status,
            _ => return Ok(None),
        };
        Ok(Some(request))
    }
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
}

impl Caps {
    /// The `caps` member of `hello`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "bandwidths": self.bandwidths,
            "max_links": self.max_links,
            "delivery": self.delivery,
            "broadcast": self.broadcast,
            "quality": self.quality,
            "busy": self.busy,
            "regulatory": self.regulatory,
            "max_queue": self.max_queue,
            "shared": self.shared,
        })
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
                bandwidth: Some(2300)
            }))
        );
        assert_eq!(read(r#"{"op":"tune","id":9}"#), Ok(None));
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
        ] {
            assert!(read(bad).is_err(), "{bad}");
        }
    }
}
