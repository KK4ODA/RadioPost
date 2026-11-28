// SPDX-License-Identifier: MIT OR Apache-2.0
//! One RPMI host on a TNC's AGWPE port: RP-4 §9.4's table, with no input or output of its own.
//!
//! The program gives the bridge each line the host writes ([`Bridge::from_host`]), each frame the
//! TNC writes ([`Bridge::from_tnc`]) and the time ([`Bridge::tick`]), and does what it answers:
//! lines to the host, frames to the TNC, or closing the host's connection.
//!
//! What the bridge keeps to, that AGWPE does not do by itself:
//!
//! * A TNC answers every connection to a callsign registered with it (`X`). So callsigns are
//!   registered only while the host listens, and a call the host places while it does not has
//!   its own callsign registered for the call alone. A connection the TNC took that the host
//!   cannot have (a link already up, a call under way) is ended at once (`d`).
//! * AGWPE's disconnect (`d`) drops what the TNC still holds. An orderly `disconnect` waits until
//!   the TNC reports no frames outstanding on the link (`Y`), as a TNC such as Dire Wolf counts
//!   frames sent but not yet acknowledged among them.
//! * Bytes go to the TNC a frame of [`PACLEN`] at a time, no more than [`WINDOW`] frames ahead of
//!   what it has sent, so the host's `queued` means something and an `abort` drops little.
//! * Whether a frame outstanding has been acknowledged or only sent differs between TNCs, so the
//!   bridge reports no `delivered` (`delivery` false).

use std::collections::{BTreeSet, VecDeque};

use rpmi::base64;
use rpmi::wire::{
    Caps, Message, Request, VERSION, admit, answer, answer_with, error, parse_line, speaks,
};
use serde_json::Value;

use crate::frame::Frame;

/// The most data an I frame carries here: AX.25's default (N1, PACLEN) of 256 bytes.
pub const PACLEN: usize = 256;
/// The most frames the bridge has with the TNC unsent or unacknowledged at once.
pub const WINDOW: usize = 7;
/// The most bytes a host should keep queued (`caps.max_queue`).
pub const MAX_QUEUE: u64 = 65536;
/// How long the TNC is given to answer the version, the ports and a registration.
const REPLY_MS: u64 = 5000;
/// How often the frames outstanding on a link are asked for while some are.
const ASK_MS: u64 = 500;
/// How long an orderly `disconnect` waits for the TNC to have nothing outstanding.
const DRAIN_MS: u64 = 120_000;
/// How long a `d` the bridge sent is given to be confirmed before the link is taken as ended.
const CLOSE_MS: u64 = 30_000;
/// How long a call given up is remembered, so its late answer is ended and not taken for a call.
const GIVEN_UP_MS: u64 = 120_000;

/// What the bridge asks of the program that drives it.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    /// A line to the host.
    Host(Message),
    /// A frame to the TNC.
    Tnc(Frame),
    /// Close the host's connection.
    CloseHost,
}

/// Why a callsign cannot be an AX.25 address: one to six capitals and digits, then `-1` to
/// `-15`. So a callsign with `/` (RP-0 §6.1) cannot be used.
#[must_use]
pub fn callsign_fault(callsign: &str) -> Option<&'static str> {
    let (base, ssid) = match callsign.split_once('-') {
        Some((base, ssid)) => (base, Some(ssid)),
        None => (callsign, None),
    };
    let base_ok = (1..=6).contains(&base.len())
        && base
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
    let ssid_ok = ssid.is_none_or(|s| {
        !s.starts_with('0') && s.parse::<u8>().is_ok_and(|n| (1..=15).contains(&n))
    });
    (!(base_ok && ssid_ok))
        .then_some("an AX.25 address is 1 to 6 capitals and digits, then -1 to -15")
}

/// The opening: what the bridge has asked the TNC on the host's `hello`.
#[derive(Debug)]
enum Opening {
    /// Nothing asked yet.
    Idle,
    /// `R`, its version: anything that answers it speaks AGWPE.
    Version { rid: Option<Value>, since: u64 },
    /// `G`, its ports: the one the adapter uses must be among them.
    Ports { rid: Option<Value>, since: u64 },
    /// Answered.
    Done,
}

/// A host's answer that waits for the TNC's replies to registrations.
#[derive(Debug)]
struct Waiter {
    rid: Option<Value>,
    since: u64,
}

#[derive(Debug)]
struct Call {
    id: Value,
    remote: String,
    local: String,
}

#[derive(Debug)]
struct Link {
    id: String,
    remote: String,
    local: String,
    direction: &'static str,
    held: VecDeque<u8>,
    /// The sizes of the frames given to the TNC that it may still have, oldest first.
    flying: VecDeque<usize>,
    /// The frames given since the last `Y` was asked.
    since_ask: usize,
    /// A `Y` asked and not answered, and when.
    asked: Option<u64>,
    /// When the last `Y` was answered.
    answered: Option<u64>,
    queued_told: Option<u64>,
    /// The host asked for an orderly end, and until when it waits.
    ending: Option<u64>,
    /// A `d` was sent, and when.
    closing: Option<u64>,
}

impl Link {
    fn queued(&self) -> u64 {
        (self.held.len() + self.flying.iter().sum::<usize>()) as u64
    }
}

/// One host on one AGWPE connection.
#[derive(Debug)]
pub struct Bridge {
    port: u8,
    now: u64,
    opening: Opening,
    hello: bool,
    lost: Option<String>,
    callsigns: Vec<String>,
    listening: bool,
    /// What the TNC holds registered.
    registered: BTreeSet<String>,
    /// Registrations sent, their replies awaited, in order.
    registering: VecDeque<String>,
    refused: Vec<String>,
    waiter: Option<Waiter>,
    call: Option<Call>,
    link: Option<Link>,
    links_made: u64,
    /// Calls given up, and links refused, whose late frames are ended or passed over.
    given_up: Vec<(String, u64)>,
}

/// Two callsigns the same, whatever their case.
fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl Bridge {
    /// A bridge on the TNC's port `port` (0 for the first).
    #[must_use]
    pub fn new(port: u8) -> Bridge {
        Bridge {
            port,
            now: 0,
            opening: Opening::Idle,
            hello: false,
            lost: None,
            callsigns: Vec::new(),
            listening: false,
            registered: BTreeSet::new(),
            registering: VecDeque::new(),
            refused: Vec::new(),
            waiter: None,
            call: None,
            link: None,
            links_made: 0,
            given_up: Vec::new(),
        }
    }

    /// A bridge whose TNC could not be reached: the host's `hello` gets `no-modem` with `why`.
    #[must_use]
    pub fn without_tnc(port: u8, why: &str) -> Bridge {
        let mut b = Bridge::new(port);
        b.lost = Some(why.to_owned());
        b
    }

    fn frame(&self, kind: u8) -> Frame {
        Frame::new(self.port, kind)
    }

    fn ok(rid: Option<&Value>, out: &mut Vec<Out>) {
        if rid.is_some() {
            out.push(Out::Host(answer("ok", rid)));
        }
    }

    /// A line the host wrote, without its LF.
    pub fn from_host(&mut self, line: &str) -> Vec<Out> {
        let mut out = Vec::new();
        let m = match parse_line(line) {
            Ok(m) => m,
            Err(refusal) => return vec![Out::Host(refusal)],
        };
        let rid = m.get("id").cloned();
        let request = match admit(&m, self.hello) {
            Ok(r) => r,
            Err(refusal) => return vec![Out::Host(refusal)],
        };
        let rid_ref = rid.as_ref();
        if let Request::Hello { low, high, .. } = &request
            && !speaks(*low, *high)
        {
            let text = format!("this adapter speaks {VERSION}");
            return vec![
                Out::Host(error(rid_ref, "unsupported-version", &text)),
                Out::CloseHost,
            ];
        }
        if let Some(why) = &self.lost {
            return vec![Out::Host(error(rid_ref, "no-modem", why))];
        }
        match request {
            Request::Hello { .. } => {
                if let Opening::Done = self.opening {
                    self.say_hello(rid_ref, &mut out);
                } else {
                    self.opening = Opening::Version {
                        rid,
                        since: self.now,
                    };
                    out.push(Out::Tnc(self.frame(b'R')));
                }
            }
            Request::Callsigns(wanted) => self.on_callsigns(wanted, rid, &mut out),
            Request::Listen(on) => {
                if self.callsigns.is_empty() {
                    out.push(Out::Host(error(rid_ref, "refused", "no callsigns yet")));
                } else {
                    self.listening = on;
                    self.settle(rid, &mut out);
                }
            }
            Request::Connect {
                remote,
                as_call,
                bandwidth,
                call_s: _,
            } => self.on_connect(&remote, as_call, bandwidth, rid, &mut out),
            Request::Send {
                link,
                data,
                reference: _,
            } => self.on_send(&link, &data, rid_ref, &mut out),
            Request::Disconnect { link } => self.on_disconnect(&link, rid_ref, &mut out),
            Request::Abort { link } => self.on_abort(link.as_deref(), rid_ref, &mut out),
            Request::Broadcast { .. } => {
                let text = "AX.25 UI frames are not carried by this adapter";
                out.push(Out::Host(error(rid_ref, "unsupported", text)));
            }
            Request::Dial | Request::Tune(_) | Request::Scan(_) => {
                let text = "AGWPE has no dial";
                out.push(Out::Host(error(rid_ref, "unsupported", text)));
            }
            Request::Tone(_) => {
                let text = "AGWPE has no tuner tone";
                out.push(Out::Host(error(rid_ref, "unsupported", text)));
            }
            Request::Stations | Request::Beacon { .. } => {
                let text = "this adapter reports no stations heard and sends no beacons yet";
                out.push(Out::Host(error(rid_ref, "unsupported", text)));
            }
            Request::Status => out.push(Out::Host(self.status(rid_ref))),
        }
        out
    }

    fn say_hello(&mut self, rid: Option<&Value>, out: &mut Vec<Out>) {
        self.hello = true;
        let caps = Caps {
            // the TNC's speed is its own setting: a call names none
            bandwidths: Vec::new(),
            max_links: 1,
            delivery: false,
            broadcast: false,
            quality: false,
            busy: false,
            regulatory: false,
            max_queue: MAX_QUEUE,
            shared: false,
            dial: false,
            scan: false,
            call_s: false,
            tone: false,
            heard: false,
            beacon: false,
            position: false,
        };
        out.push(Out::Host(answer_with(
            "hello",
            rid,
            &[
                ("version", VERSION.into()),
                ("modem", "AX.25 packet".into()),
                ("caps", caps.to_json()),
            ],
        )));
    }

    fn on_callsigns(&mut self, wanted: Vec<String>, rid: Option<Value>, out: &mut Vec<Out>) {
        if let Some(why) = wanted
            .iter()
            .find_map(|c| callsign_fault(c).map(|why| format!("{c}: {why}")))
        {
            out.push(Out::Host(error(rid.as_ref(), "refused", &why)));
            return;
        }
        self.callsigns = wanted;
        if self.callsigns.is_empty() {
            self.listening = false;
        }
        self.settle(rid, out);
    }

    /// The callsigns the TNC should hold: the host's while it listens, and a call's or link's own.
    fn wanted(&self) -> BTreeSet<String> {
        let mut w: BTreeSet<String> = if self.listening {
            self.callsigns.iter().cloned().collect()
        } else {
            BTreeSet::new()
        };
        if let Some(c) = &self.call {
            w.insert(c.local.clone());
        }
        if let Some(l) = &self.link {
            w.insert(l.local.clone());
        }
        w
    }

    /// Registers and unregisters as [`Bridge::wanted`] says; the host's `ok` waits for the TNC's
    /// replies to the registrations sent.
    fn settle(&mut self, rid: Option<Value>, out: &mut Vec<Out>) {
        let wanted = self.wanted();
        for gone in self
            .registered
            .difference(&wanted)
            .cloned()
            .collect::<Vec<_>>()
        {
            self.registered.remove(&gone);
            out.push(Out::Tnc(self.frame(b'x').calls(&gone, "")));
        }
        for new in wanted
            .difference(&self.registered)
            .cloned()
            .collect::<Vec<_>>()
        {
            if self.registering.contains(&new) {
                continue;
            }
            self.registering.push_back(new.clone());
            out.push(Out::Tnc(self.frame(b'X').calls(&new, "")));
        }
        if self.registering.is_empty() {
            Self::ok(rid.as_ref(), out);
        } else if rid.is_some() {
            if let Some(w) = self.waiter.take() {
                // a later request's answer covers the earlier one's
                Self::ok(w.rid.as_ref(), out);
            }
            self.waiter = Some(Waiter {
                rid,
                since: self.now,
            });
        }
    }

    fn on_connect(
        &mut self,
        remote: &str,
        as_call: Option<String>,
        bandwidth: Option<i128>,
        rid: Option<Value>,
        out: &mut Vec<Out>,
    ) {
        let rid_ref = rid.as_ref();
        let refuse = |code: &str, text: &str| Out::Host(error(rid_ref, code, text));
        let Some(first) = self.callsigns.first() else {
            out.push(refuse("refused", "no callsigns yet"));
            return;
        };
        let local = as_call.unwrap_or_else(|| first.clone());
        if !self.callsigns.contains(&local) {
            out.push(refuse(
                "not-authorized",
                &format!("{local} is not this host's"),
            ));
            return;
        }
        if bandwidth.is_some() {
            out.push(refuse(
                "unsupported",
                "a packet link's speed is the TNC's setting",
            ));
            return;
        }
        if self.call.is_some() || self.link.is_some() {
            out.push(refuse("busy", "a link or a call is up"));
            return;
        }
        if let Some(why) = callsign_fault(remote) {
            out.push(refuse("refused", why));
            return;
        }
        let Some(id) = rid else {
            out.push(refuse("malformed", "a `connect` needs its `id`"));
            return;
        };
        Self::ok(Some(&id), out);
        self.call = Some(Call {
            id,
            remote: remote.to_owned(),
            local: local.clone(),
        });
        self.settle(None, out);
        out.push(Out::Tnc(self.frame(b'C').calls(&local, remote)));
    }

    fn on_send(&mut self, link: &str, data: &[u8], rid: Option<&Value>, out: &mut Vec<Out>) {
        let Some(l) = self.link.as_mut().filter(|l| l.id == link) else {
            out.push(Out::Host(error(
                rid,
                "not-connected",
                "no such link of this host's",
            )));
            return;
        };
        if l.ending.is_some() || l.closing.is_some() {
            out.push(Out::Host(error(rid, "refused", "the link is ending")));
            return;
        }
        l.held.extend(data);
        Self::ok(rid, out);
        self.progress(out);
    }

    fn on_disconnect(&mut self, link: &str, rid: Option<&Value>, out: &mut Vec<Out>) {
        let now = self.now;
        let Some(l) = self.link.as_mut().filter(|l| l.id == link) else {
            out.push(Out::Host(error(
                rid,
                "not-connected",
                "no such link of this host's",
            )));
            return;
        };
        if l.ending.is_none() {
            l.ending = Some(now + DRAIN_MS);
        }
        Self::ok(rid, out);
        self.progress(out);
    }

    fn on_abort(&mut self, link: Option<&str>, rid: Option<&Value>, out: &mut Vec<Out>) {
        let Some(id) = link else {
            self.abort_call(rid, out);
            return;
        };
        let now = self.now;
        let port = self.port;
        let Some(l) = self.link.as_mut().filter(|l| l.id == id) else {
            out.push(Out::Host(error(
                rid,
                "not-connected",
                "no such link of this host's",
            )));
            return;
        };
        l.held.clear();
        l.ending.get_or_insert(now);
        if l.closing.is_none() {
            l.closing = Some(now);
            out.push(Out::Tnc(Frame::new(port, b'd').calls(&l.local, &l.remote)));
        }
        Self::ok(rid, out);
    }

    /// The call under way given up: `failed` `aborted` at once, and its late answer ended.
    fn abort_call(&mut self, rid: Option<&Value>, out: &mut Vec<Out>) {
        let Some(call) = self.call.take() else {
            out.push(Out::Host(error(rid, "not-connected", "no call under way")));
            return;
        };
        Self::ok(rid, out);
        out.push(Out::Tnc(self.frame(b'd').calls(&call.local, &call.remote)));
        self.given_up
            .push((call.remote.clone(), self.now + GIVEN_UP_MS));
        out.push(Out::Host(answer_with(
            "failed",
            Some(&call.id),
            &[
                ("code", "aborted".into()),
                ("text", "the host's abort".into()),
            ],
        )));
        self.settle(None, out);
    }

    fn status(&self, rid: Option<&Value>) -> Message {
        let links: Vec<Value> = self
            .link
            .iter()
            .map(|l| {
                serde_json::json!({"link": l.id, "remote": l.remote, "direction": l.direction,
                                   "queued": l.queued()})
            })
            .collect();
        answer_with(
            "status",
            rid,
            &[
                ("listening", self.listening.into()),
                ("callsigns", self.callsigns.clone().into()),
                ("busy", false.into()),
                ("links", links.into()),
            ],
        )
    }

    /// Feeds the link's bytes to the TNC, asks what is outstanding, says what is queued, and ends
    /// the link once an orderly end finds nothing left.
    fn progress(&mut self, out: &mut Vec<Out>) {
        let now = self.now;
        let port = self.port;
        let Some(l) = self.link.as_mut() else {
            return;
        };
        if l.closing.is_none() {
            while !l.held.is_empty() && l.flying.len() < WINDOW {
                let n = l.held.len().min(PACLEN);
                let chunk: Vec<u8> = l.held.drain(..n).collect();
                out.push(Out::Tnc(
                    Frame::new(port, b'D')
                        .calls(&l.local, &l.remote)
                        .data(&chunk),
                ));
                l.flying.push_back(n);
                l.since_ask += 1;
            }
            // asked again no sooner than ASK_MS after the last answer
            let due = l.answered.is_none_or(|at| now >= at + ASK_MS);
            let ask = l.asked.is_none() && due && (!l.flying.is_empty() || l.ending.is_some());
            if ask {
                l.asked = Some(now);
                l.since_ask = 0;
                out.push(Out::Tnc(Frame::new(port, b'Y').calls(&l.local, &l.remote)));
            }
        }
        let queued = l.queued();
        if l.queued_told != Some(queued) {
            l.queued_told = Some(queued);
            out.push(Out::Host(answer_with(
                "queued",
                None,
                &[("link", l.id.clone().into()), ("bytes", queued.into())],
            )));
        }
    }

    /// The TNC's answer to `Y`: `n` frames outstanding on the link.
    fn on_outstanding(&mut self, n: usize, out: &mut Vec<Out>) {
        let port = self.port;
        let now = self.now;
        let Some(l) = self.link.as_mut() else {
            return;
        };
        if l.asked.is_none() {
            return;
        }
        l.asked = None;
        l.answered = Some(now);
        // frames given since the question are outstanding whatever it says; of the others,
        // the oldest went first
        let keep = n.min(l.flying.len()) + l.since_ask.min(l.flying.len());
        while l.flying.len() > keep.min(l.flying.len()) {
            l.flying.pop_front();
        }
        let drained = l.held.is_empty() && l.flying.is_empty() && n == 0 && l.since_ask == 0;
        if l.closing.is_none() && l.ending.is_some_and(|until| drained || now >= until) {
            l.closing = Some(now);
            out.push(Out::Tnc(Frame::new(port, b'd').calls(&l.local, &l.remote)));
        }
        self.progress(out);
    }

    fn link_up(
        &mut self,
        remote: &str,
        local: &str,
        direction: &'static str,
        id: Option<&Value>,
        out: &mut Vec<Out>,
    ) {
        self.links_made += 1;
        let link = format!("p{}", self.links_made);
        out.push(Out::Host(answer_with(
            "connected",
            id,
            &[
                ("link", link.clone().into()),
                ("remote", remote.into()),
                ("local", local.into()),
                ("direction", direction.into()),
            ],
        )));
        self.link = Some(Link {
            id: link,
            remote: remote.to_owned(),
            local: local.to_owned(),
            direction,
            held: VecDeque::new(),
            flying: VecDeque::new(),
            since_ask: 0,
            asked: None,
            answered: None,
            queued_told: None,
            ending: None,
            closing: None,
        });
    }

    /// A frame the TNC wrote.
    pub fn from_tnc(&mut self, f: &Frame) -> Vec<Out> {
        let mut out = Vec::new();
        match f.kind {
            b'R' => {
                if let Opening::Version { rid, .. } =
                    std::mem::replace(&mut self.opening, Opening::Idle)
                {
                    self.opening = Opening::Ports {
                        rid,
                        since: self.now,
                    };
                    out.push(Out::Tnc(self.frame(b'G')));
                }
            }
            b'G' => {
                if let Opening::Ports { rid, .. } =
                    std::mem::replace(&mut self.opening, Opening::Idle)
                {
                    // "2;Port1 …;Port2 …;": the number of ports first
                    let ports: usize = f
                        .text()
                        .split(';')
                        .next()
                        .and_then(|n| n.trim().parse().ok())
                        .unwrap_or(0);
                    if usize::from(self.port) < ports {
                        self.opening = Opening::Done;
                        self.say_hello(rid.as_ref(), &mut out);
                    } else {
                        let why = format!(
                            "the TNC has {ports} port(s), and port {} is not one of them",
                            self.port
                        );
                        out.push(Out::Host(error(rid.as_ref(), "no-modem", &why)));
                        self.lost = Some(why);
                    }
                }
            }
            b'X' => self.on_registered(f, &mut out),
            b'C' => self.on_connected(f, &mut out),
            b'D' => {
                if let Some(l) = self.link.as_ref().filter(|l| same(&l.remote, &f.from)) {
                    out.push(Out::Host(answer_with(
                        "received",
                        None,
                        &[
                            ("link", l.id.clone().into()),
                            ("data", base64::encode(&f.data).into()),
                        ],
                    )));
                }
            }
            b'd' => self.on_disconnected(f, &mut out),
            b'Y' if f.data.len() >= 4 => {
                let mut n = [0u8; 4];
                n.copy_from_slice(&f.data[..4]);
                let n = usize::try_from(u32::from_le_bytes(n)).unwrap_or(usize::MAX);
                self.on_outstanding(n, &mut out);
            }
            _ => {} // monitoring, heard lists, and what a later TNC adds
        }
        out
    }

    fn on_registered(&mut self, f: &Frame, out: &mut Vec<Out>) {
        let Some(call) = self.registering.pop_front() else {
            return;
        };
        if f.data.first() == Some(&1) {
            self.registered.insert(call);
        } else {
            self.refused.push(call);
        }
        if self.registering.is_empty() {
            let refused = std::mem::take(&mut self.refused);
            if let Some(w) = self.waiter.take() {
                if refused.is_empty() {
                    Self::ok(w.rid.as_ref(), out);
                } else {
                    let text = format!("the TNC would not register {}", refused.join(", "));
                    self.listening = false;
                    out.push(Out::Host(error(w.rid.as_ref(), "refused", &text)));
                }
            }
        }
    }

    fn on_connected(&mut self, f: &Frame, out: &mut Vec<Out>) {
        let remote = f.from.clone();
        let local = f.to.clone();
        let ours = self
            .call
            .as_ref()
            .is_some_and(|c| same(&c.remote, &remote) && same(&c.local, &local));
        if ours {
            let Some(call) = self.call.take() else {
                return;
            };
            self.link_up(&call.remote, &call.local, "out", Some(&call.id), out);
            return;
        }
        let given_up = self.given_up.iter().any(|(r, _)| same(r, &remote));
        let answerable = !given_up
            && self.hello
            && self.listening
            && self.call.is_none()
            && self.link.is_none()
            && self.callsigns.iter().any(|c| same(c, &local));
        if answerable {
            let local = self
                .callsigns
                .iter()
                .find(|c| same(c, &local))
                .cloned()
                .unwrap_or(local);
            out.push(Out::Host(answer_with(
                "incoming",
                None,
                &[
                    ("remote", remote.clone().into()),
                    ("local", local.clone().into()),
                ],
            )));
            self.link_up(&remote, &local, "in", None, out);
        } else {
            // a connection the host cannot have, or a call it gave up answered late: ended now
            out.push(Out::Tnc(self.frame(b'd').calls(&local, &remote)));
            self.given_up.push((remote, self.now + GIVEN_UP_MS));
        }
    }

    fn on_disconnected(&mut self, f: &Frame, out: &mut Vec<Out>) {
        let remote = f.from.clone();
        let text = f.text();
        if self.link.as_ref().is_some_and(|l| same(&l.remote, &remote)) {
            let Some(l) = self.link.take() else {
                return;
            };
            let (reason, said) = if l.ending.is_some() || l.closing.is_some() {
                ("local", "the host ended the link".to_owned())
            } else if text.contains("RETRYOUT") {
                ("timeout", format!("the TNC lost the link: {text}"))
            } else {
                ("remote", format!("the TNC says: {text}"))
            };
            out.push(Out::Host(answer_with(
                "disconnected",
                None,
                &[
                    ("link", l.id.into()),
                    ("reason", reason.into()),
                    ("text", said.into()),
                ],
            )));
            self.settle(None, out);
            return;
        }
        if self.call.as_ref().is_some_and(|c| same(&c.remote, &remote)) {
            let Some(call) = self.call.take() else {
                return;
            };
            let code = if text.contains("RETRYOUT") {
                "timeout"
            } else {
                "refused"
            };
            out.push(Out::Host(answer_with(
                "failed",
                Some(&call.id),
                &[
                    ("code", code.into()),
                    ("text", format!("the TNC says: {text}").into()),
                ],
            )));
            self.settle(None, out);
            return;
        }
        // a call given up, or a connection refused, now ended: nothing to tell
        self.given_up.retain(|(r, _)| !same(r, &remote));
    }

    /// The time, in milliseconds from any start.
    pub fn tick(&mut self, now: u64) -> Vec<Out> {
        self.now = now;
        let mut out = Vec::new();
        let late = |since: u64| now >= since + REPLY_MS;
        match &self.opening {
            Opening::Version { rid, since } | Opening::Ports { rid, since } if late(*since) => {
                let why = "the TNC there does not answer AGWPE's requests".to_owned();
                out.push(Out::Host(error(rid.as_ref(), "no-modem", &why)));
                self.lost = Some(why);
                self.opening = Opening::Idle;
            }
            _ => {}
        }
        if self.waiter.as_ref().is_some_and(|w| late(w.since)) {
            if let Some(w) = self.waiter.take() {
                let text = "the TNC did not answer a registration";
                out.push(Out::Host(error(w.rid.as_ref(), "refused", text)));
            }
            self.registering.clear();
        }
        self.given_up.retain(|(_, until)| now < *until);
        let port = self.port;
        let mut unconfirmed = false;
        if let Some(l) = self.link.as_mut() {
            if l.asked.is_some_and(|at| now >= at + REPLY_MS) {
                l.asked = None; // not answered: asked again
            }
            if l.closing.is_none() && l.ending.is_some_and(|until| now >= until) {
                l.closing = Some(now);
                out.push(Out::Tnc(Frame::new(port, b'd').calls(&l.local, &l.remote)));
            }
            unconfirmed = l.closing.is_some_and(|at| now >= at + CLOSE_MS);
        }
        if unconfirmed && let Some(l) = self.link.take() {
            // the TNC never confirmed the end: the host is told it is over
            out.push(Out::Host(answer_with(
                "disconnected",
                None,
                &[
                    ("link", l.id.into()),
                    ("reason", "local".into()),
                    ("text", "the TNC did not confirm the end".into()),
                ],
            )));
            self.settle(None, &mut out);
        }
        self.progress(&mut out);
        out
    }

    /// The host has gone (RP-4 §1.1): its link or call ended, its callsigns let go.
    pub fn host_left(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        if let Some(l) = self.link.take() {
            out.push(Out::Tnc(self.frame(b'd').calls(&l.local, &l.remote)));
        }
        if let Some(c) = self.call.take() {
            out.push(Out::Tnc(self.frame(b'd').calls(&c.local, &c.remote)));
        }
        for call in std::mem::take(&mut self.registered) {
            out.push(Out::Tnc(self.frame(b'x').calls(&call, "")));
        }
        out
    }

    /// The TNC's connection failed: the host is told what that ended, and closed (RP-4 §2.1).
    pub fn tnc_lost(&mut self, why: &str) -> Vec<Out> {
        let text = format!("the TNC went away: {why}");
        let mut out = Vec::new();
        self.lost = Some(text.clone());
        if let Some(l) = self.link.take() {
            out.push(Out::Host(answer_with(
                "disconnected",
                None,
                &[
                    ("link", l.id.into()),
                    ("reason", "failure".into()),
                    ("text", text.clone().into()),
                ],
            )));
        }
        if let Some(c) = self.call.take() {
            out.push(Out::Host(answer_with(
                "failed",
                Some(&c.id),
                &[("code", "refused".into()), ("text", text.clone().into())],
            )));
        }
        out.push(Out::CloseHost);
        out
    }
}
