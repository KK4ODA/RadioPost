// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host's side of RPMI: requests numbered on one connection, the adapter's lines read as
//! events (RP-4 §3 to §6), and a carrier that joins one RP-3 session to one link.
//!
//! Like the session engines, none of it does input or output: the program reads lines from its
//! connection, hands each to [`Event::read`] and its carrier, and writes the requests it is given
//! back.

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::base64;
use crate::wire::{Caps, Message, VERSION, answer_with, op_of};

/// Numbers the requests on one connection to an adapter, from 1.
#[derive(Debug)]
pub struct Host {
    next_id: i64,
}

impl Default for Host {
    fn default() -> Self {
        Host { next_id: 1 }
    }
}

impl Host {
    /// A host with no requests yet.
    #[must_use]
    pub fn new() -> Host {
        Host::default()
    }

    fn with_id(&mut self, op: &str, members: &[(&str, Value)]) -> Message {
        let id = Value::from(self.next_id);
        self.next_id += 1;
        answer_with(op, Some(&id), members)
    }

    /// The `id` the next request will carry.
    #[must_use]
    pub fn next_id(&self) -> Value {
        Value::from(self.next_id)
    }

    /// `hello`: the host speaks version 1 only (RP-4 §3).
    pub fn hello(&mut self, app: &str) -> Message {
        self.with_id(
            "hello",
            &[
                ("version", serde_json::json!([VERSION, VERSION])),
                ("app", app.into()),
            ],
        )
    }

    /// `callsigns`: the first is the default.
    pub fn callsigns(&mut self, callsigns: &[&str]) -> Message {
        self.with_id("callsigns", &[("callsigns", callsigns.into())])
    }

    /// `listen`.
    pub fn listen(&mut self, on: bool) -> Message {
        self.with_id("listen", &[("on", on.into())])
    }

    /// `connect`: `connected` or `failed` will carry its `id`.
    pub fn connect(
        &mut self,
        remote: &str,
        as_call: Option<&str>,
        bandwidth: Option<i64>,
    ) -> Message {
        self.connect_lasting(remote, as_call, bandwidth, None)
    }

    /// `connect` whose tries keep starting for at least `call_s` seconds, with `caps.call_s`
    /// (§4): a call to a gateway that scans (RP-5 §4.4, `scan_s`).
    pub fn connect_lasting(
        &mut self,
        remote: &str,
        as_call: Option<&str>,
        bandwidth: Option<i64>,
        call_s: Option<u64>,
    ) -> Message {
        let mut members = vec![("remote", Value::from(remote))];
        if let Some(a) = as_call {
            members.push(("as", a.into()));
        }
        if let Some(b) = bandwidth {
            members.push(("bandwidth", b.into()));
        }
        if let Some(s) = call_s {
            members.push(("call_s", s.into()));
        }
        self.with_id("connect", &members)
    }

    /// `send`, with no `id`: its answers are `queued` and, with `reference`, `delivered`.
    #[must_use]
    pub fn send(&self, link: &str, data: &[u8], reference: Option<&str>) -> Message {
        let mut m = answer_with(
            "send",
            None,
            &[("link", link.into()), ("data", base64::encode(data).into())],
        );
        if let Some(r) = reference {
            m.insert("ref".into(), r.into());
        }
        m
    }

    /// `disconnect`: what is queued goes first.
    pub fn disconnect(&mut self, link: &str) -> Message {
        self.with_id("disconnect", &[("link", link.into())])
    }

    /// `abort`: a link, or with none the call under way.
    pub fn abort(&mut self, link: Option<&str>) -> Message {
        match link {
            Some(l) => self.with_id("abort", &[("link", l.into())]),
            None => self.with_id("abort", &[]),
        }
    }

    /// `broadcast` (RP-4 §6).
    pub fn broadcast(&mut self, data: &[u8], reference: Option<&str>) -> Message {
        self.broadcast_on(data, reference, None)
    }

    /// `broadcast` naming the dial it belongs on (§6): an answer to one heard there.
    pub fn broadcast_on(
        &mut self,
        data: &[u8],
        reference: Option<&str>,
        dial_hz: Option<u64>,
    ) -> Message {
        let mut members = vec![("data", Value::from(base64::encode(data)))];
        if let Some(r) = reference {
            members.push(("ref", r.into()));
        }
        if let Some(hz) = dial_hz {
            members.push(("dial_hz", hz.into()));
        }
        self.with_id("broadcast", &members)
    }

    /// `status`.
    pub fn status(&mut self) -> Message {
        self.with_id("status", &[])
    }

    /// `dial`: the radio's dial (RP-4 §4.1).
    pub fn dial(&mut self) -> Message {
        self.with_id("dial", &[])
    }

    /// `tune`: the radio's dial set, in hertz (RP-4 §4.1).
    pub fn tune(&mut self, dial_hz: u64) -> Message {
        self.with_id("tune", &[("dial_hz", dial_hz.into())])
    }

    /// `scan`: listen on each dial in turn, the first the home; none stops the scan (§4.2).
    pub fn scan(&mut self, dials: &[u64]) -> Message {
        self.with_id("scan", &[("dials", dials.to_vec().into())])
    }

    /// `tone`: a steady tone for an antenna tuner, `seconds` long; 0 stops one (§4.3).
    pub fn tone(&mut self, seconds: u64) -> Message {
        self.with_id("tone", &[("seconds", seconds.into())])
    }

    /// `stations`: the stations the modem has heard (RP-4 §4.4).
    pub fn stations(&mut self) -> Message {
        self.with_id("stations", &[])
    }

    /// `beacon`: one beacon carrying `as_call` (the first callsign when none), with `reference`
    /// for its `beacon-sent`, waiting at most `within_s` for a clear channel, on `dial_hz`, and
    /// with `position` when the modem carries one (RP-4 §4.4).
    pub fn beacon(
        &mut self,
        as_call: Option<&str>,
        reference: Option<&str>,
        within_s: Option<u64>,
        dial_hz: Option<u64>,
        position: Option<&crate::position::Position>,
    ) -> Message {
        self.beacon_saying(as_call, reference, within_s, dial_hz, position, None)
    }

    /// [`Host::beacon`], with `modem_position` (§4.4): `Some(false)` says the station's position
    /// goes in no form, the modem's own included.
    pub fn beacon_saying(
        &mut self,
        as_call: Option<&str>,
        reference: Option<&str>,
        within_s: Option<u64>,
        dial_hz: Option<u64>,
        position: Option<&crate::position::Position>,
        modem_position: Option<bool>,
    ) -> Message {
        let mut members: Vec<(&str, Value)> = Vec::new();
        if let Some(c) = as_call {
            members.push(("as", c.into()));
        }
        if let Some(r) = reference {
            members.push(("ref", r.into()));
        }
        if let Some(s) = within_s {
            members.push(("within_s", s.into()));
        }
        if let Some(hz) = dial_hz {
            members.push(("dial_hz", hz.into()));
        }
        if let Some(p) = position {
            members.push(("position", p.to_json()));
        }
        if let Some(own) = modem_position {
            members.push(("modem_position", own.into()));
        }
        self.with_id("beacon", &members)
    }
}

/// Which station placed the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// A call this host answered.
    In,
    /// A call this host placed.
    Out,
}

/// A station heard (RP-4 §4.4): a `heard` event, or one entry of `stations`.
#[derive(Clone, Debug, PartialEq)]
pub struct Sighting {
    /// Its callsign.
    pub callsign: String,
    /// The SNR it was heard at.
    pub snr_db: Option<f64>,
    /// The bandwidth it runs, in Hz.
    pub bandwidth: Option<i64>,
    /// What was heard: `beacon`, `call`, `answer`, `probe`, `session`, `datagram` or `id`.
    pub kind: Option<String>,
    /// Whom a call, an answer or a probe was addressed to.
    pub to: Option<String>,
    /// For a beacon: whether calls to its callsign are answered now.
    pub answers: Option<bool>,
    /// The radio's dial when it was heard.
    pub dial_hz: Option<u64>,
    /// Where its beacon says its station is.
    pub position: Option<crate::position::Position>,
    /// When it was heard, Unix seconds.
    pub at: Option<u64>,
}

impl Sighting {
    /// A station heard with nothing but its callsign.
    #[must_use]
    pub fn of(callsign: &str) -> Sighting {
        Sighting {
            callsign: callsign.into(),
            snr_db: None,
            bandwidth: None,
            kind: None,
            to: None,
            answers: None,
            dial_hz: None,
            position: None,
            at: None,
        }
    }

    /// A sighting from a `heard` message or a `stations` entry. A member of the wrong type is
    /// left out, as one a host does not know would be (§2); only the callsign is needed.
    ///
    /// # Errors
    ///
    /// When the callsign is missing.
    pub fn read(m: &Message) -> Result<Sighting, String> {
        Ok(Sighting {
            callsign: text(m, "callsign")?,
            snr_db: m.get("snr_db").and_then(Value::as_f64),
            bandwidth: m.get("bandwidth").and_then(Value::as_i64),
            kind: optional_text(m, "kind"),
            to: optional_text(m, "to"),
            answers: m.get("answers").and_then(Value::as_bool),
            dial_hz: m
                .get("dial_hz")
                .and_then(Value::as_u64)
                .filter(|&hz| hz > 0),
            position: m
                .get("position")
                .and_then(crate::position::Position::from_json),
            at: m.get("at").and_then(Value::as_u64),
        })
    }

    /// Its members as a `heard` message or a `stations` entry carries them, `op` aside.
    #[must_use]
    pub fn to_message(&self) -> Message {
        let mut m = Message::new();
        m.insert("callsign".into(), self.callsign.clone().into());
        if let Some(v) = self.snr_db {
            m.insert("snr_db".into(), serde_json::json!(v));
        }
        if let Some(v) = self.bandwidth {
            m.insert("bandwidth".into(), v.into());
        }
        if let Some(v) = &self.kind {
            m.insert("kind".into(), v.clone().into());
        }
        if let Some(v) = &self.to {
            m.insert("to".into(), v.clone().into());
        }
        if let Some(v) = self.answers {
            m.insert("answers".into(), v.into());
        }
        if let Some(v) = self.dial_hz {
            m.insert("dial_hz".into(), v.into());
        }
        if let Some(v) = &self.position {
            m.insert("position".into(), v.to_json());
        }
        if let Some(v) = self.at {
            m.insert("at".into(), v.into());
        }
        m
    }
}

/// One of the host's links in `status`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkStatus {
    /// The link.
    pub link: String,
    /// The station at the other end.
    pub remote: String,
    /// Who called.
    pub direction: Direction,
    /// Bytes still queued.
    pub queued: u64,
}

/// What an adapter says (RP-4 §3, §4, §5, §6).
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The answer to `hello`.
    Hello {
        /// The request's `id`.
        id: Option<Value>,
        /// The version chosen.
        version: i64,
        /// The modem's name.
        modem: String,
        /// What it can do.
        caps: Caps,
    },
    /// A request done.
    Ok {
        /// The request's `id`.
        id: Value,
    },
    /// A request failed, or something went wrong.
    Error {
        /// The request's `id`, when it had one that could be read.
        id: Option<Value>,
        /// The code (RP-4 §7, RP-0 §9.3).
        code: String,
        /// For people.
        text: Option<String>,
    },
    /// A call to one of the host's callsigns is being answered.
    Incoming {
        /// The caller.
        remote: String,
        /// The callsign called.
        local: String,
    },
    /// A link is up.
    Connected {
        /// The `connect`'s `id`, for a call the host placed.
        id: Option<Value>,
        /// The link.
        link: String,
        /// The station at the other end.
        remote: String,
        /// The host's callsign on this link.
        local: String,
        /// Who called.
        direction: Direction,
        /// The bandwidth in Hz, when the modem says.
        bandwidth: Option<i64>,
    },
    /// A call ended without a link.
    Failed {
        /// The `connect`'s `id`.
        id: Option<Value>,
        /// `timeout`, `refused`, `busy` or `aborted`.
        code: String,
        /// For people.
        text: Option<String>,
        /// Whether the station called was heard during the call; `None`, the adapter cannot
        /// tell (RP-4 1.4.0).
        heard: Option<bool>,
        /// The strongest SNR it was heard at.
        snr_db: Option<f64>,
    },
    /// Bytes arrived on a link, in order, with no `send` boundaries.
    Received {
        /// The link.
        link: String,
        /// The bytes.
        data: Vec<u8>,
    },
    /// Bytes still queued on a link.
    Queued {
        /// The link.
        link: String,
        /// How many.
        bytes: u64,
    },
    /// The other station holds every byte up to the end of a `send`.
    Delivered {
        /// The link.
        link: String,
        /// The `send`'s `ref`.
        reference: Value,
    },
    /// A link is gone.
    Disconnected {
        /// The link.
        link: String,
        /// `local`, `remote`, `timeout` or `failure`.
        reason: String,
        /// For people.
        text: Option<String>,
    },
    /// What the modem knows about the path.
    Quality {
        /// The link.
        link: Option<String>,
        /// The last SNR heard.
        snr_db: Option<f64>,
        /// The data rate in use.
        rate_bps: Option<f64>,
        /// Whether it is receiving now.
        receiving: Option<bool>,
        /// What the other station reports hearing this one at (RP-4 1.4.0).
        peer_snr_db: Option<f64>,
    },
    /// Whether others use the channel.
    Busy(bool),
    /// A station was heard (§4.4).
    Heard(Sighting),
    /// The answer to `stations` (§4.4): what the modem has heard, the most recently heard first.
    Stations {
        /// The request's `id`.
        id: Option<Value>,
        /// Each station once, as the modem last heard it.
        stations: Vec<Sighting>,
    },
    /// A beacon of this host's has gone out, or will not (§4.4).
    BeaconSent {
        /// The `beacon`'s `ref`.
        reference: Option<Value>,
        /// Whether it went out.
        sent: bool,
        /// The dial it went on, with `caps.dial`.
        dial_hz: Option<u64>,
        /// For people: why it did not go.
        text: Option<String>,
    },
    /// A broadcast heard (§6).
    Broadcast {
        /// Who sent it.
        source: String,
        /// Its bytes.
        data: Vec<u8>,
        /// Its SNR.
        snr_db: Option<f64>,
        /// The dial it was heard on, with `caps.dial`: where an answer to it belongs.
        dial_hz: Option<u64>,
    },
    /// A broadcast of this host's has gone out, or will not.
    BroadcastSent {
        /// The `broadcast`'s `ref`.
        reference: Option<Value>,
        /// Whether it went out.
        sent: bool,
        /// For people.
        text: Option<String>,
    },
    /// The answer to `status`.
    Status {
        /// The request's `id`.
        id: Option<Value>,
        /// Whether the host's `listen` is on.
        listening: bool,
        /// The host's callsigns.
        callsigns: Vec<String>,
        /// Whether others use the channel.
        busy: bool,
        /// The host's links.
        links: Vec<LinkStatus>,
        /// The station's dial, when the adapter can say (§4.1).
        dial_hz: Option<u64>,
        /// The bandwidths a call to the host is answered in now, when the modem has a choice
        /// (§1.1); empty when the adapter does not say.
        answers: Vec<u64>,
        /// The dials the station scans, whichever host asked (§4.2); empty when it scans none.
        scanning: Vec<u64>,
        /// The interval of a beacon the modem repeats by itself, in seconds (§4.4).
        beaconing: Option<u64>,
    },
    /// The host's scan (§4.2): the answer to `scan`, or, with no `id`, its dials changed or,
    /// with none and a `why`, its end.
    Scan {
        /// The request's `id`.
        id: Option<Value>,
        /// The dials scanned, the first the home; none once it has ended.
        dials: Vec<u64>,
        /// How long a call's tries must keep starting to be heard on any of them, seconds.
        scan_s: Option<u64>,
        /// Why it ended: `asked`, `left`, `tuned`, `knob`, `operator` or `radio`.
        why: Option<String>,
    },
    /// The radio's dial (§4.1): the answer to `dial`, or, with no `id`, a change of it.
    Dial {
        /// The request's `id`; none when the dial changed.
        id: Option<Value>,
        /// The dial, in hertz.
        dial_hz: u64,
    },
    /// An `op` this host does not know: ignored (RP-4 §2).
    Unknown(String),
}

fn text(m: &Message, key: &str) -> Result<String, String> {
    m.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("no `{key}` string"))
}

fn optional_text(m: &Message, key: &str) -> Option<String> {
    m.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn flag(m: &Message, key: &str) -> Result<bool, String> {
    m.get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("no `{key}` true or false"))
}

fn direction(m: &Message) -> Result<Direction, String> {
    match m.get("direction").and_then(Value::as_str) {
        Some("in") => Ok(Direction::In),
        Some("out") => Ok(Direction::Out),
        _ => Err("`direction` is neither `in` nor `out`".into()),
    }
}

fn bytes_of(m: &Message) -> Result<Vec<u8>, String> {
    m.get("data")
        .and_then(Value::as_str)
        .and_then(base64::decode)
        .ok_or_else(|| "no `data` in standard base64".into())
}

/// A broadcast heard (§6), with the dial it was heard on when the modem has one.
fn broadcast(m: &Message) -> Result<Event, String> {
    Ok(Event::Broadcast {
        source: text(m, "source")?,
        data: bytes_of(m)?,
        snr_db: m.get("snr_db").and_then(Value::as_f64),
        dial_hz: m
            .get("dial_hz")
            .and_then(Value::as_u64)
            .filter(|&hz| hz > 0),
    })
}

/// The answer to `stations` (§4.4): an entry without a callsign is left out.
fn stations(m: &Message, id: Option<Value>) -> Result<Event, String> {
    Ok(Event::Stations {
        id,
        stations: m
            .get("stations")
            .and_then(Value::as_array)
            .ok_or("no `stations`")?
            .iter()
            .filter_map(|e| e.as_object().and_then(|o| Sighting::read(o).ok()))
            .collect(),
    })
}

/// What became of a beacon (§4.4).
fn beacon_sent(m: &Message) -> Result<Event, String> {
    Ok(Event::BeaconSent {
        reference: m.get("ref").cloned(),
        sent: flag(m, "sent")?,
        dial_hz: m
            .get("dial_hz")
            .and_then(Value::as_u64)
            .filter(|&hz| hz > 0),
        text: optional_text(m, "text"),
    })
}

fn status(m: &Message, id: Option<Value>) -> Result<Event, String> {
    let links = m
        .get("links")
        .and_then(Value::as_array)
        .ok_or("no `links`")?
        .iter()
        .map(|l| {
            let l = l.as_object().ok_or("a link that is not an object")?;
            Ok(LinkStatus {
                link: text(l, "link")?,
                remote: text(l, "remote")?,
                direction: direction(l)?,
                queued: l.get("queued").and_then(Value::as_u64).unwrap_or(0),
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(Event::Status {
        id,
        listening: flag(m, "listening")?,
        callsigns: m
            .get("callsigns")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .ok_or("no `callsigns`")?,
        busy: flag(m, "busy")?,
        links,
        dial_hz: m
            .get("dial_hz")
            .and_then(Value::as_u64)
            .filter(|&hz| hz > 0),
        answers: numbers(m, "answers"),
        scanning: numbers(m, "scanning"),
        beaconing: m.get("beaconing").and_then(Value::as_u64),
    })
}

/// A list of whole numbers above 0, as `status`'s `answers` and `scanning` carry them: empty
/// when absent, anything else in it left out.
fn numbers(m: &Message, key: &str) -> Vec<u64> {
    m.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_u64)
                .filter(|&n| n > 0)
                .collect()
        })
        .unwrap_or_default()
}

impl Event {
    /// The event an adapter's message holds. An `op` RP-4 does not name is
    /// [`Event::Unknown`], and members it does not name are ignored (RP-4 §2).
    ///
    /// # Errors
    ///
    /// A known event that lacks a member it needs, or has one of the wrong type.
    pub fn read(m: &Message) -> Result<Event, String> {
        let id = m.get("id").cloned();
        Ok(match op_of(m) {
            "hello" => Event::Hello {
                id,
                version: m
                    .get("version")
                    .and_then(Value::as_i64)
                    .ok_or("no `version`")?,
                modem: text(m, "modem")?,
                caps: Caps::from_json(m.get("caps").unwrap_or(&Value::Null)),
            },
            "ok" => Event::Ok {
                id: id.ok_or("an `ok` with no `id`")?,
            },
            "error" => Event::Error {
                id,
                code: text(m, "code")?,
                text: optional_text(m, "text"),
            },
            "incoming" => Event::Incoming {
                remote: text(m, "remote")?,
                local: text(m, "local")?,
            },
            "connected" => Event::Connected {
                id,
                link: text(m, "link")?,
                remote: text(m, "remote")?,
                local: text(m, "local")?,
                direction: direction(m)?,
                bandwidth: m.get("bandwidth").and_then(Value::as_i64),
            },
            "failed" => Event::Failed {
                id,
                code: text(m, "code")?,
                text: optional_text(m, "text"),
                heard: m.get("heard").and_then(Value::as_bool),
                snr_db: m.get("snr_db").and_then(Value::as_f64),
            },
            "received" => Event::Received {
                link: text(m, "link")?,
                data: bytes_of(m)?,
            },
            "queued" => Event::Queued {
                link: text(m, "link")?,
                bytes: m.get("bytes").and_then(Value::as_u64).ok_or("no `bytes`")?,
            },
            "delivered" => Event::Delivered {
                link: text(m, "link")?,
                reference: m.get("ref").cloned().ok_or("no `ref`")?,
            },
            "disconnected" => Event::Disconnected {
                link: text(m, "link")?,
                reason: text(m, "reason")?,
                text: optional_text(m, "text"),
            },
            "quality" => Event::Quality {
                link: optional_text(m, "link"),
                snr_db: m.get("snr_db").and_then(Value::as_f64),
                rate_bps: m.get("rate_bps").and_then(Value::as_f64),
                receiving: m.get("receiving").and_then(Value::as_bool),
                peer_snr_db: m.get("peer_snr_db").and_then(Value::as_f64),
            },
            "busy" => Event::Busy(flag(m, "busy")?),
            "heard" => Event::Heard(Sighting::read(m)?),
            "stations" => stations(m, id)?,
            "beacon-sent" => beacon_sent(m)?,
            "broadcast" => broadcast(m)?,
            "broadcast-sent" => Event::BroadcastSent {
                reference: m.get("ref").cloned(),
                sent: flag(m, "sent")?,
                text: optional_text(m, "text"),
            },
            "status" => status(m, id)?,
            "dial" => Event::Dial {
                id,
                dial_hz: m
                    .get("dial_hz")
                    .and_then(Value::as_u64)
                    .ok_or("no `dial_hz`")?,
            },
            "scan" => Event::Scan {
                id,
                dials: m
                    .get("dials")
                    .and_then(Value::as_array)
                    .and_then(|a| a.iter().map(Value::as_u64).collect::<Option<Vec<_>>>())
                    .ok_or("no `dials`")?,
                scan_s: m.get("scan_s").and_then(Value::as_u64),
                why: optional_text(m, "why"),
            },
            other => Event::Unknown(other.into()),
        })
    }
}

/// RP-3 §8's silence on a session's link: the time since bytes last arrived, stopped while the
/// modem's last `quality` says it is receiving. Other events (`busy`, `queued`, a `quality`
/// saying `receiving` false) do not restart it; with no `quality` at all, nothing stops it.
#[derive(Debug)]
pub struct Silence {
    since: Instant,
    receiving: bool,
}

impl Default for Silence {
    fn default() -> Self {
        Silence {
            since: Instant::now(),
            receiving: false,
        }
    }
}

impl Silence {
    /// A session whose link has just come up.
    #[must_use]
    pub fn new() -> Silence {
        Silence::default()
    }

    /// One event from the adapter.
    pub fn heard(&mut self, event: &Event) {
        match event {
            Event::Received { .. } => self.since = Instant::now(),
            Event::Quality {
                receiving: Some(now),
                ..
            } => {
                if *now || self.receiving {
                    // while it receives, and from the moment it stops, the count starts afresh
                    self.since = Instant::now();
                }
                self.receiving = *now;
            }
            _ => {}
        }
    }

    /// Whether the session has been silent for `quiet` (RP-3 §8: 300 seconds).
    #[must_use]
    pub fn over(&self, quiet: Duration) -> bool {
        !self.receiving && self.since.elapsed() >= quiet
    }
}

/// What a session engine offers the link it rides on: RP-3's engines, seen from RP-4.
pub trait Engine {
    /// What the engine reports.
    type Event;
    /// The first transmission, when this side speaks first (the caller's `HELLO`).
    fn opening(&mut self) -> Option<Vec<u8>>;
    /// Bytes the link delivered.
    fn receive(&mut self, bytes: &[u8]) -> Vec<Self::Event>;
    /// The next transmission, when one is due.
    fn transmit(&mut self) -> Option<Vec<u8>>;
    /// Whether the session is over.
    fn is_over(&self) -> bool;
    /// The link is gone.
    fn link_lost(&mut self) -> Vec<Self::Event>;
}

/// One RP-3 session on one link: each transmission is one `send`, whose `ref` is `t` and the
/// transmission's number in the session (RP-3 §5.7: the stations take turns, the caller first).
/// The caller hangs up once its engine is over and the adapter has handed over every byte
/// (RP-3 §8).
#[derive(Debug)]
pub struct Carrier<E: Engine> {
    engine: E,
    link: Option<String>,
    caller: bool,
    sent: u64,
    queued: u64,
    hanging_up: bool,
    ended: bool,
}

impl<E: Engine> Carrier<E> {
    /// A carrier waiting for its link's `connected`.
    pub fn new(engine: E) -> Carrier<E> {
        Carrier {
            engine,
            link: None,
            caller: false,
            sent: 0,
            queued: 0,
            hanging_up: false,
            ended: false,
        }
    }

    /// The engine.
    pub fn engine(&self) -> &E {
        &self.engine
    }

    /// The engine, to change it.
    pub fn engine_mut(&mut self) -> &mut E {
        &mut self.engine
    }

    /// The link, once up.
    #[must_use]
    pub fn link(&self) -> Option<&str> {
        self.link.as_deref()
    }

    /// Whether the link has come and gone.
    #[must_use]
    pub fn is_ended(&self) -> bool {
        self.ended
    }

    fn send(&mut self, bytes: &[u8], host: &Host, out: &mut Vec<Message>) {
        self.sent += 1;
        let n = if self.caller {
            2 * self.sent - 1
        } else {
            2 * self.sent
        };
        let link = self.link.clone().unwrap_or_default();
        self.queued += bytes.len() as u64;
        out.push(host.send(&link, bytes, Some(&format!("t{n}"))));
    }

    /// One event from the adapter: the requests to write, and what the engine said. The first
    /// `connected` is taken as this carrier's link; events about other links are ignored.
    pub fn on_event(&mut self, host: &mut Host, event: &Event) -> (Vec<Message>, Vec<E::Event>) {
        let mut out = Vec::new();
        let mut said = Vec::new();
        let mine = |link: &str, me: &Option<String>| me.as_deref() == Some(link);
        match event {
            Event::Connected {
                link, direction, ..
            } if self.link.is_none() && !self.ended => {
                self.link = Some(link.clone());
                self.caller = *direction == Direction::Out;
                if let Some(bytes) = self.engine.opening() {
                    self.send(&bytes, host, &mut out);
                }
            }
            Event::Received { link, data } if mine(link, &self.link) => {
                said = self.engine.receive(data);
                while let Some(bytes) = self.engine.transmit() {
                    self.send(&bytes, host, &mut out);
                }
            }
            Event::Queued { link, bytes } if mine(link, &self.link) => self.queued = *bytes,
            Event::Disconnected { link, .. } if mine(link, &self.link) => {
                self.link = None;
                self.ended = true;
                said = self.engine.link_lost();
            }
            _ => {}
        }
        if self.caller
            && !self.hanging_up
            && self.queued == 0
            && self.engine.is_over()
            && let Some(link) = self.link.clone()
        {
            self.hanging_up = true;
            out.push(host.disconnect(&link));
        }
        (out, said)
    }
}

#[cfg(test)]
mod silence {
    use super::*;

    #[test]
    fn only_bytes_or_receiving_hold_off_the_silence() {
        let quiet = Duration::from_millis(40);
        let mut s = Silence::new();
        let receiving = |r| Event::Quality {
            link: None,
            snr_db: None,
            rate_bps: None,
            receiving: Some(r),
            peer_snr_db: None,
        };
        std::thread::sleep(quiet);
        s.heard(&Event::Busy(false));
        s.heard(&receiving(false));
        assert!(s.over(quiet), "busy and not receiving restart nothing");
        s.heard(&receiving(true));
        std::thread::sleep(quiet);
        assert!(!s.over(quiet), "held while the modem receives");
        s.heard(&receiving(false));
        assert!(!s.over(quiet), "counted afresh once it stops");
        std::thread::sleep(quiet);
        assert!(s.over(quiet));
        s.heard(&Event::Received {
            link: "c1".into(),
            data: vec![1],
        });
        assert!(!s.over(quiet), "bytes restart it");
    }
}
