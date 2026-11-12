// SPDX-License-Identifier: MIT OR Apache-2.0
//! One RPMI host on Mercury's TNC interface: RP-4 §9.6's table, with no input or output of its
//! own.
//!
//! The program gives the bridge each line the host writes ([`Bridge::from_host`]), each line
//! Mercury writes on its control port ([`Bridge::from_modem`]), the bytes of its data port
//! ([`Bridge::from_data`]) and the time ([`Bridge::tick`]), and does what it answers: lines to
//! the host, commands and bytes to Mercury, or closing the host's connection.
//!
//! What the bridge keeps Mercury to, that its commands do not do by themselves:
//!
//! * `LISTEN OFF` ends a call or a session in Mercury, as it does in VARA. RPMI's `listen` off
//!   only stops answering, so the command waits until nothing of the host's is up.
//! * A call goes out under the first callsign of `MYCALL`, whatever `CONNECT` names: the callsign
//!   a `connect` calls as is put first.
//! * `DISCONNECTED` says neither which session ended nor why. The bridge sends no `CONNECT` for a
//!   second after a command that can still be answered with one.
//! * `BUFFER` counts the bytes the other station has not confirmed, among those Mercury has taken
//!   in: `delivered` and `queued` follow from it, and a `BUFFER 0` that may have been counted
//!   before Mercury took the last bytes in is not believed until they have had time to show.
//! * Mercury does not identify the station: the called station's callsign goes on the air only as
//!   a CRC. After each session, and after any answer Mercury sent that came to no session, the
//!   bridge sends `CQFRAME` with the callsign the station used, which every Mercury decodes. The
//!   called station goes first; the caller waits to hear it, so the two do not key at once.

use std::collections::VecDeque;

use rpmi::base64;
use rpmi::wire::{
    Caps, Message, Request, VERSION, admit, answer, answer_with, error, parse_line, speaks,
};
use serde_json::Value;

/// The bandwidths Mercury takes (`BW500`, `BW2300`, `BW2750`).
pub const BANDWIDTHS: [i64; 3] = [500, 2300, 2750];
/// How many callsigns Mercury answers for: `MYCALL`'s first and four more.
pub const MAX_CALLSIGNS: usize = 5;
/// The longest callsign Mercury keeps.
pub const MAX_CALLSIGN: usize = 15;
/// The longest callsign Mercury's CALL frame carries, the caller's.
pub const MAX_CALLER: usize = 14;
/// The most bytes a host should keep queued (`caps.max_queue`).
pub const MAX_QUEUE: u64 = 65536;
/// The most bytes Mercury holds unconfirmed at once: half its own queue of 64 KiB, which drops
/// what does not fit. The rest waits in the bridge.
const FEED: u64 = 32 * 1024;
/// How long after the last bytes went to Mercury a `BUFFER 0` is believed: Mercury counts every
/// second, so bytes it has taken in show within two.
const SETTLE_MS: u64 = 2500;
/// How long after `DISCONNECT`, `ABORT` or `LISTEN OFF` no `CONNECT` is sent: Mercury answers each
/// with `DISCONNECTED` when it finds no session, within a pass of its loop (100 ms).
const QUIET_MS: u64 = 1000;
/// The longest an `ABORT`'s DISCONNECT frame is waited for before `LISTEN OFF` may follow: 30 s
/// for its slot, and the frame itself (27 s on the 500 Hz floor).
const TAIL_MS: u64 = 60_000;
/// How long bytes that came before their `CONNECTED` are kept for it.
const EARLY_MS: u64 = 1000;
/// The most such bytes kept.
const EARLY_MAX: usize = 65536;

/// How long the called station waits after a session's end before it identifies: the other
/// station's last answer, a reply pattern of 0.64 s, comes 0.7 s after ours.
const GUARD_MS: u64 = 2000;
/// How long the caller waits after it has heard the called station identify: a frame decodes about
/// 0.2 s before its sender unkeys, and a radio takes about 0.34 s to turn from sending to hearing.
/// Mercury waits as long before it answers a frame (`ARQ_CHANNEL_GUARD_MS`). Found on two Mercury
/// processes: without it, the caller keyed 0.13 s before the called station had unkeyed.
const HEARD_MS: u64 = 700;
/// How long the caller waits to hear the called station identify before it identifies anyway: a
/// CQ frame, the size of a CALL, is 3.74 s on DATAC16 and 27 s on the 500 Hz floor of a station
/// that sends only MFSK (Mercury's `docs/TNC.md`).
const PEER_ID_MS: u64 = 35_000;
/// How long a `CQFRAME` is given to be sent: Mercury says nothing of one it could not build.
const IDENT_MS: u64 = 60_000;
/// The bandwidth a `CQFRAME` names when the session's is not known.
const IDENT_BW: i64 = 2300;

/// What a host is told when the modem there does not answer as Mercury does.
const NOT_MERCURY: &str =
    "the modem there is not Mercury 1.9.23 or later: it does not know BUSYDETECT";

/// What the bridge asks of the program that drives it.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    /// A line to the host.
    Host(Message),
    /// A command for Mercury's control port, without its CR.
    Command(String),
    /// Bytes for Mercury's data port.
    Data(Vec<u8>),
    /// Close the host's connection.
    CloseHost,
}

/// What a command to Mercury is waiting to answer. Mercury answers commands in order.
#[derive(Debug)]
enum Waiting {
    Probe {
        rid: Option<Value>,
    },
    Callsigns {
        rid: Option<Value>,
        wanted: Vec<String>,
    },
    Step {
        call: u64,
    },
    Connect {
        call: u64,
    },
    Ident,
    Plain(&'static str),
}

/// A command sent, and the host it was sent for.
#[derive(Debug)]
struct Asked {
    host: u64,
    what: Waiting,
}

/// What a call still has to send Mercury.
#[derive(Debug)]
enum Step {
    Mycall(Vec<String>),
    Bandwidth(i64),
    Connect,
}

/// A call the host placed, not yet connected or failed.
#[derive(Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "how far the call has come, each step its own flag"
)]
struct Call {
    serial: u64,
    id: Value,
    remote: String,
    local: String,
    steps: VecDeque<Step>,
    /// A `MYCALL` or `BW` of this call's is unanswered.
    in_flight: bool,
    /// `CONNECT` has been sent.
    placed: bool,
    /// The host has its `ok`.
    told: bool,
    aborted: bool,
}

/// The host's link while it is up.
#[derive(Debug)]
struct Link {
    id: String,
    remote: String,
    local: String,
    direction: &'static str,
    bandwidth: Option<i64>,
    /// Bytes the host has sent on the link.
    given: u64,
    /// Of those, the bytes written to Mercury's data port.
    fed: u64,
    /// Of those, the bytes the other station is known to hold.
    acked: u64,
    /// The bytes Mercury is known to have taken in.
    taken: u64,
    held: VecDeque<u8>,
    refs: VecDeque<(u64, Value)>,
    fed_at: u64,
    buffer: Option<u64>,
    queued_told: u64,
    /// The host asked for the link to end: its end is `local`.
    ending: bool,
    /// `disconnect`: `DISCONNECT` follows the last byte.
    closing: bool,
    /// `DISCONNECT` or `ABORT` has been sent.
    asked: bool,
    quality_told: Option<(Option<f64>, Option<u64>)>,
}

/// An `ABORT` whose DISCONNECT frame may still be going out.
#[derive(Debug)]
struct Tail {
    until: u64,
    keyed: bool,
}

/// A `CQFRAME` sent: Mercury brackets its transmission with `PENDING` and `CANCELPENDING`.
#[derive(Debug)]
struct Ident {
    until: u64,
    keyed: bool,
}

/// One host's side of the adapter. It outlives its host by the end of what the host left on the
/// air ([`Bridge::settled`]), and may be given the next host meanwhile.
#[derive(Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "the host's state and Mercury's, each its own flag"
)]
pub struct Bridge {
    now: u64,
    host: u64,
    hello: bool,
    modem_lost: bool,
    lost_why: String,
    /// Whether Mercury's busy detector was on when asked; `None` until it has answered.
    detector: Option<bool>,
    callsigns: Vec<String>,
    listening: bool,
    /// The callsigns of the last `MYCALL`, the first first.
    mycall: Vec<String>,
    /// What Mercury was last told of listening. Unknown at first, so taken as on.
    modem_listening: bool,
    bandwidth: Option<i64>,
    waiting: VecDeque<Asked>,
    quiet_until: u64,
    tail: Option<Tail>,
    /// Mercury is answering a call.
    pending: bool,
    /// A session Mercury brought up that is not the host's, being ended: the callsign it was for.
    ghost: Option<String>,
    call: Option<Call>,
    calls_made: u64,
    link: Option<Link>,
    links_made: u64,
    early: VecDeque<(u64, Vec<u8>)>,
    snr_db: Option<f64>,
    rate_bps: Option<u64>,
    channel_busy: bool,
    busy_told: bool,
    /// The callsigns the station has transmitted under and not yet identified with.
    owed: Vec<String>,
    owed_bw: i64,
    /// No `CQFRAME` before this.
    ident_after: u64,
    /// The other station, whose `CQFRAME` heard ends the wait.
    ident_peer: Option<String>,
    ident: Option<Ident>,
    /// Mercury keyed while answering a call that has not come up.
    answer_keyed: bool,
}

impl Default for Bridge {
    fn default() -> Bridge {
        Bridge::new()
    }
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Why Mercury cannot hold `callsign` for a host, if it cannot.
#[must_use]
pub fn callsign_fault(callsign: &str) -> Option<&'static str> {
    if callsign.is_empty() || callsign.len() > MAX_CALLSIGN {
        return Some("Mercury keeps callsigns of 1 to 15 characters");
    }
    let carried = |b: u8| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-';
    if !callsign.bytes().all(carried) {
        return Some("Mercury's CALL carries capitals, digits and hyphens only");
    }
    None
}

/// Why Mercury cannot call `remote`, if it cannot.
#[must_use]
pub fn remote_fault(remote: &str) -> Option<&'static str> {
    if remote.is_empty() || remote.len() > MAX_CALLSIGN {
        return Some("Mercury calls callsigns of 1 to 15 characters");
    }
    if !remote.bytes().all(|b| b.is_ascii_graphic()) {
        return Some("a callsign has no spaces");
    }
    None
}

impl Bridge {
    /// A host on a Mercury that has just been reached.
    #[must_use]
    pub fn new() -> Bridge {
        Bridge {
            now: 0,
            host: 0,
            hello: false,
            modem_lost: false,
            lost_why: String::new(),
            detector: None,
            callsigns: Vec::new(),
            listening: false,
            mycall: Vec::new(),
            modem_listening: true,
            bandwidth: None,
            waiting: VecDeque::new(),
            quiet_until: 0,
            tail: None,
            pending: false,
            ghost: None,
            call: None,
            calls_made: 0,
            link: None,
            links_made: 0,
            early: VecDeque::new(),
            snr_db: None,
            rate_bps: None,
            channel_busy: false,
            busy_told: false,
            owed: Vec::new(),
            owed_bw: IDENT_BW,
            ident_after: 0,
            ident_peer: None,
            ident: None,
            answer_keyed: false,
        }
    }

    /// A host connected while Mercury cannot be reached: every request is answered `no-modem`.
    #[must_use]
    pub fn without_modem(why: &str) -> Bridge {
        Bridge {
            modem_lost: true,
            lost_why: why.to_owned(),
            ..Bridge::new()
        }
    }

    fn command(&mut self, line: &str, what: Waiting, out: &mut Vec<Out>) {
        if matches!(line, "DISCONNECT" | "ABORT" | "LISTEN OFF") {
            self.quiet_until = self.now + QUIET_MS;
        }
        self.waiting.push_back(Asked {
            host: self.host,
            what,
        });
        out.push(Out::Command(line.to_owned()));
    }

    fn ok(rid: Option<&Value>, out: &mut Vec<Out>) {
        if rid.is_some() {
            out.push(Out::Host(answer("ok", rid)));
        }
    }

    fn update_busy(&mut self, out: &mut Vec<Out>) {
        // while the host's own link is up, what the detector hears is mostly the other station
        let busy = self.channel_busy && self.link.is_none();
        if self.hello && self.detector == Some(true) && busy != self.busy_told {
            self.busy_told = busy;
            out.push(Out::Host(answer_with(
                "busy",
                None,
                &[("busy", busy.into())],
            )));
        }
    }

    /// Whether nothing of the host's is on the air, so that `LISTEN OFF` ends nothing.
    fn may_release(&self) -> bool {
        self.link.is_none()
            && self.ghost.is_none()
            && self.tail.is_none()
            && !self.call.as_ref().is_some_and(|c| c.placed)
    }

    /// Brings Mercury's answering in line with the host's `listen`, when it may.
    fn settle_listen(&mut self, out: &mut Vec<Out>) {
        let wanted = self.hello && self.listening;
        if wanted {
            if !self.modem_listening {
                self.modem_listening = true;
                self.command("LISTEN ON", Waiting::Plain("LISTEN ON"), out);
            }
        } else if self.modem_listening
            && self.detector.is_some()
            && (self.pending || self.may_release())
        {
            self.modem_listening = false;
            self.command("LISTEN OFF", Waiting::Plain("LISTEN OFF"), out);
        }
    }

    fn say_hello(&mut self, rid: Option<&Value>, out: &mut Vec<Out>) {
        self.hello = true;
        let caps = Caps {
            bandwidths: BANDWIDTHS.to_vec(),
            max_links: 1,
            delivery: true,
            broadcast: false,
            quality: true,
            busy: self.detector == Some(true),
            regulatory: false,
            max_queue: MAX_QUEUE,
            shared: false,
        };
        out.push(Out::Host(answer_with(
            "hello",
            rid,
            &[
                ("version", VERSION.into()),
                ("modem", "Mercury".into()),
                ("caps", caps.to_json()),
            ],
        )));
        self.busy_told = false;
        self.update_busy(out);
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
        if self.modem_lost {
            return vec![Out::Host(error(rid_ref, "no-modem", &self.lost_why))];
        }
        match request {
            Request::Hello { .. } => {
                if self.detector.is_some() {
                    self.say_hello(rid_ref, &mut out);
                } else {
                    // only Mercury knows this command, and only from 1.9.23
                    self.command("BUSYDETECT", Waiting::Probe { rid }, &mut out);
                }
            }
            Request::Callsigns(wanted) => self.on_callsigns(wanted, rid, &mut out),
            Request::Listen(on) => {
                if self.callsigns.is_empty() {
                    out.push(Out::Host(error(rid_ref, "refused", "no callsigns yet")));
                } else {
                    self.listening = on;
                    Self::ok(rid_ref, &mut out);
                    self.settle_listen(&mut out);
                }
            }
            Request::Connect {
                remote,
                as_call,
                bandwidth,
            } => self.on_connect(&remote, as_call, bandwidth, rid, &mut out),
            Request::Send {
                link,
                data,
                reference,
            } => self.on_send(&link, &data, reference, rid_ref, &mut out),
            Request::Disconnect { link } => self.on_disconnect(&link, rid_ref, &mut out),
            Request::Abort { link } => self.on_abort(link.as_deref(), rid_ref, &mut out),
            Request::Broadcast { .. } => {
                let text = "Mercury's broadcasts are not carried by this adapter";
                out.push(Out::Host(error(rid_ref, "unsupported", text)));
            }
            Request::Status => out.push(Out::Host(self.status(rid_ref))),
        }
        out
    }

    fn on_callsigns(&mut self, wanted: Vec<String>, rid: Option<Value>, out: &mut Vec<Out>) {
        let fault = if wanted.len() > MAX_CALLSIGNS {
            Some("Mercury answers for five callsigns at most".to_owned())
        } else {
            wanted
                .iter()
                .find_map(|c| callsign_fault(c).map(|why| format!("{c}: {why}")))
        };
        if let Some(why) = fault {
            out.push(Out::Host(error(rid.as_ref(), "refused", &why)));
            return;
        }
        if wanted.is_empty() {
            // Mercury has no command that takes its callsigns away: it is kept from answering
            self.callsigns.clear();
            self.listening = false;
            Self::ok(rid.as_ref(), out);
            self.settle_listen(out);
            return;
        }
        self.mycall.clone_from(&wanted);
        let line = format!("MYCALL {}", wanted.join(" "));
        self.command(&line, Waiting::Callsigns { rid, wanted }, out);
    }

    fn status(&self, rid: Option<&Value>) -> Message {
        let links: Vec<Value> = self
            .link
            .iter()
            .map(|l| {
                serde_json::json!({"link": l.id, "remote": l.remote, "direction": l.direction,
                                   "queued": l.given - l.acked})
            })
            .collect();
        answer_with(
            "status",
            rid,
            &[
                ("listening", self.listening.into()),
                ("callsigns", self.callsigns.clone().into()),
                ("busy", self.busy_told.into()),
                ("links", links.into()),
            ],
        )
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
        let bandwidth = match bandwidth.map(i64::try_from) {
            None => None,
            Some(Ok(bw)) if BANDWIDTHS.contains(&bw) => Some(bw),
            Some(_) => {
                out.push(refuse("unsupported", "Mercury has 500, 2300 and 2750 Hz"));
                return;
            }
        };
        if self.call.is_some() || self.link.is_some() {
            out.push(refuse("busy", "a link or a call is up"));
            return;
        }
        if self.pending || self.ghost.is_some() {
            out.push(refuse("busy", "Mercury is answering a call"));
            return;
        }
        if let Some(why) = remote_fault(remote) {
            out.push(refuse("refused", why));
            return;
        }
        if local.len() > MAX_CALLER {
            let text = "Mercury's CALL carries a caller's callsign of 14 characters at most";
            out.push(refuse("refused", text));
            return;
        }
        let mut steps = VecDeque::new();
        if self.mycall.first() != Some(&local) {
            // the CALL carries MYCALL's first callsign, whatever CONNECT names
            let mut order = vec![local.clone()];
            order.extend(self.callsigns.iter().filter(|c| **c != local).cloned());
            steps.push_back(Step::Mycall(order));
        }
        if let Some(bw) = bandwidth
            && self.bandwidth != Some(bw)
        {
            steps.push_back(Step::Bandwidth(bw));
        }
        steps.push_back(Step::Connect);
        // the call is under way: what it waits for (the station's identification, a quiet second)
        // may take longer than a host waits for an answer
        Self::ok(rid.as_ref(), out);
        self.calls_made += 1;
        self.call = Some(Call {
            serial: self.calls_made,
            id: rid.unwrap_or(Value::Null),
            remote: remote.to_owned(),
            local,
            steps,
            in_flight: false,
            placed: false,
            told: true,
            aborted: false,
        });
        self.advance_call(out);
    }

    /// Sends the call's next command, once the last is answered; `CONNECT` once no late
    /// `DISCONNECTED` can follow it.
    fn advance_call(&mut self, out: &mut Vec<Out>) {
        let Some(call) = self.call.as_mut() else {
            return;
        };
        if call.in_flight || call.placed {
            return;
        }
        let identifying = !self.owed.is_empty() || self.ident.is_some();
        if matches!(call.steps.front(), Some(Step::Connect))
            && (self.now < self.quiet_until || identifying)
        {
            return;
        }
        let serial = call.serial;
        match call.steps.pop_front() {
            Some(Step::Mycall(order)) => {
                call.in_flight = true;
                self.mycall.clone_from(&order);
                let line = format!("MYCALL {}", order.join(" "));
                self.command(&line, Waiting::Step { call: serial }, out);
            }
            Some(Step::Bandwidth(bw)) => {
                call.in_flight = true;
                self.bandwidth = Some(bw);
                self.command(&format!("BW{bw}"), Waiting::Step { call: serial }, out);
            }
            Some(Step::Connect) => {
                call.placed = true;
                let line = format!("CONNECT {} {}", call.local, call.remote);
                self.command(&line, Waiting::Connect { call: serial }, out);
            }
            None => {}
        }
    }

    /// Ends the host's call with no link: `failed` once it had its `ok`, `error` before.
    fn end_call(&mut self, code: &str, text: &str, out: &mut Vec<Out>) {
        let Some(call) = self.call.take() else {
            return;
        };
        let members = [("code", code.into()), ("text", text.into())];
        let op = if call.told { "failed" } else { "error" };
        out.push(Out::Host(answer_with(op, Some(&call.id), &members)));
    }

    fn on_send(
        &mut self,
        link: &str,
        data: &[u8],
        reference: Option<Value>,
        rid: Option<&Value>,
        out: &mut Vec<Out>,
    ) {
        let Some(l) = self.link.as_mut().filter(|l| l.id == link) else {
            let text = "no such link of this host's";
            out.push(Out::Host(error(rid, "not-connected", text)));
            return;
        };
        if l.ending {
            out.push(Out::Host(error(rid, "refused", "the link is ending")));
            return;
        }
        l.given += data.len() as u64;
        l.held.extend(data);
        if let Some(r) = reference {
            l.refs.push_back((l.given, r));
        }
        Self::ok(rid, out);
        self.progress(out);
    }

    fn on_disconnect(&mut self, link: &str, rid: Option<&Value>, out: &mut Vec<Out>) {
        let Some(l) = self.link.as_mut().filter(|l| l.id == link) else {
            let text = "no such link of this host's";
            out.push(Out::Host(error(rid, "not-connected", text)));
            return;
        };
        l.ending = true;
        l.closing = true;
        Self::ok(rid, out);
        self.progress(out);
    }

    fn on_abort(&mut self, link: Option<&str>, rid: Option<&Value>, out: &mut Vec<Out>) {
        if let Some(id) = link {
            let Some(l) = self.link.as_mut().filter(|l| l.id == id) else {
                let text = "no such link of this host's";
                out.push(Out::Host(error(rid, "not-connected", text)));
                return;
            };
            l.ending = true;
            l.closing = false;
            l.asked = true;
            l.held.clear();
            Self::ok(rid, out);
            self.abort(out);
            return;
        }
        let Some(call) = self.call.as_mut() else {
            out.push(Out::Host(error(rid, "not-connected", "no call under way")));
            return;
        };
        call.aborted = true;
        if call.placed {
            Self::ok(rid, out);
            self.abort(out);
        } else {
            // no CONNECT has gone to Mercury: the call ends here
            if !call.told {
                call.told = true;
                out.push(Out::Host(answer("ok", Some(&call.id))));
            }
            Self::ok(rid, out);
            self.end_call("aborted", "the host aborted", out);
        }
    }

    /// `ABORT`: Mercury says `DISCONNECTED` at once, and its DISCONNECT frame goes out after.
    fn abort(&mut self, out: &mut Vec<Out>) {
        self.tail = Some(Tail {
            until: self.now + TAIL_MS,
            keyed: false,
        });
        self.command("ABORT", Waiting::Plain("ABORT"), out);
    }

    /// Feeds Mercury, and tells the host what the other station holds.
    fn progress(&mut self, out: &mut Vec<Out>) {
        let now = self.now;
        let Some(l) = self.link.as_mut() else {
            return;
        };
        while !l.held.is_empty() && l.fed - l.acked < FEED {
            let room = usize::try_from(FEED - (l.fed - l.acked)).unwrap_or(usize::MAX);
            let n = room.min(l.held.len());
            let bytes: Vec<u8> = l.held.drain(..n).collect();
            l.fed += n as u64;
            l.fed_at = now;
            out.push(Out::Data(bytes));
        }
        while l.refs.front().is_some_and(|(end, _)| *end <= l.acked) {
            if let Some((_, reference)) = l.refs.pop_front() {
                out.push(Out::Host(answer_with(
                    "delivered",
                    None,
                    &[("link", l.id.clone().into()), ("ref", reference)],
                )));
            }
        }
        let queued = l.given - l.acked;
        if queued != l.queued_told {
            l.queued_told = queued;
            out.push(Out::Host(answer_with(
                "queued",
                None,
                &[("link", l.id.clone().into()), ("bytes", queued.into())],
            )));
        }
        if l.closing && !l.asked && l.held.is_empty() {
            // Mercury delivers what it holds before its DISCONNECT frame goes out
            l.asked = true;
            self.command("DISCONNECT", Waiting::Plain("DISCONNECT"), out);
        }
    }

    /// A `BUFFER 0` is believed once the last bytes have had time to show in Mercury's count.
    fn settle(&mut self) {
        let now = self.now;
        if let Some(l) = self.link.as_mut()
            && l.buffer == Some(0)
            && l.acked < l.fed
            && now >= l.fed_at + SETTLE_MS
        {
            l.acked = l.fed;
            l.taken = l.fed;
        }
    }

    fn on_buffer(&mut self, unconfirmed: u64, out: &mut Vec<Out>) {
        if let Some(l) = self.link.as_mut() {
            l.buffer = Some(unconfirmed);
            // Mercury holds `unconfirmed` bytes beyond those confirmed: it has taken that many in
            l.taken = l.taken.max(l.acked + unconfirmed).min(l.fed);
            l.acked = l.acked.max(l.taken.saturating_sub(unconfirmed));
        }
        self.settle();
        self.progress(out);
    }

    fn tell_quality(&mut self, out: &mut Vec<Out>) {
        let now = (self.snr_db, self.rate_bps);
        let Some(l) = self.link.as_mut() else {
            return;
        };
        if l.quality_told == Some(now) {
            return;
        }
        l.quality_told = Some(now);
        let mut q = answer_with("quality", None, &[("link", l.id.clone().into())]);
        if let Some(snr) = now.0 {
            q.insert("snr_db".into(), snr.into());
        }
        if let Some(rate) = now.1 {
            q.insert("rate_bps".into(), rate.into());
        }
        out.push(Out::Host(q));
    }

    /// The time, in milliseconds from any start: what was waiting for it is done.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Out> {
        let mut out = Vec::new();
        self.now = self.now.max(now_ms);
        let now = self.now;
        if self.tail.as_ref().is_some_and(|t| now >= t.until) {
            self.tail = None;
            self.settle_listen(&mut out);
        }
        while self
            .early
            .front()
            .is_some_and(|(at, _)| now > at + EARLY_MS)
        {
            self.early.pop_front();
        }
        if self.ident.as_ref().is_some_and(|i| now >= i.until) {
            self.ident = None;
        }
        self.identify(&mut out);
        self.advance_call(&mut out);
        self.settle();
        self.progress(&mut out);
        out
    }

    /// The station has transmitted under `callsign`: it identifies once the air is its own, after
    /// the other station, `peer`, has identified if this station called it.
    fn owe(&mut self, callsign: &str, peer: Option<&str>, bandwidth: Option<i64>) {
        if !self.owed.iter().any(|c| same(c, callsign)) {
            self.owed.push(callsign.to_owned());
        }
        self.owed_bw = bandwidth.unwrap_or(IDENT_BW);
        let wait = if let Some(p) = peer {
            self.ident_peer = Some(p.to_owned());
            PEER_ID_MS
        } else {
            GUARD_MS
        };
        self.ident_after = self.ident_after.max(self.now + wait);
    }

    /// Sends the next `CQFRAME` owed, when nothing is on the air for the station and its turn has
    /// come.
    fn identify(&mut self, out: &mut Vec<Out>) {
        let clear = self.link.is_none()
            && self.ghost.is_none()
            && self.tail.is_none()
            && !self.pending
            && !self.call.as_ref().is_some_and(|c| c.placed);
        if self.owed.is_empty()
            || self.ident.is_some()
            || !clear
            || self.modem_lost
            || self.detector.is_none()
            || self.now < self.ident_after
        {
            return;
        }
        let callsign = self.owed.remove(0);
        if self.owed.is_empty() {
            self.ident_peer = None;
        }
        self.ident = Some(Ident {
            until: self.now + IDENT_MS,
            keyed: false,
        });
        let line = format!("CQFRAME {callsign} {}", self.owed_bw);
        self.command(&line, Waiting::Ident, out);
    }

    /// A line Mercury wrote on its control port, without its CR.
    pub fn from_modem(&mut self, line: &str) -> Vec<Out> {
        let mut out = Vec::new();
        let line = line.trim();
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        let rest = rest.trim();
        match word {
            "OK" => self.answered(true, &mut out),
            "WRONG" => self.answered(false, &mut out),
            "BUSYDETECT" => self.on_probe(rest == "ON", &mut out),
            "CONNECTED" => self.on_connected(rest, &mut out),
            "DISCONNECTED" => self.on_disconnected(&mut out),
            "PENDING" => {
                if let Some(i) = self.ident.as_mut().filter(|i| !i.keyed) {
                    // the CQ frame is on the air
                    i.keyed = true;
                    return out;
                }
                // Mercury is back to answering: the last session's teardown is over
                self.pending = true;
                self.answer_keyed = false;
                self.tail = None;
                self.settle_listen(&mut out);
            }
            "CANCELPENDING" => {
                if self.ident.as_ref().is_some_and(|i| i.keyed) {
                    self.ident = None;
                } else {
                    self.pending = false;
                    if std::mem::take(&mut self.answer_keyed) {
                        // Mercury's ACCEPTs went out, and carried no callsign
                        let answered = self.callsigns.first().or(self.mycall.first()).cloned();
                        if let Some(c) = answered {
                            self.owe(&c, None, self.bandwidth);
                        }
                    }
                    self.settle_listen(&mut out);
                }
                self.identify(&mut out);
                self.advance_call(&mut out);
            }
            "CQFRAME" => {
                // a station identifying, or calling CQ: the one this station waits for, perhaps
                let heard = rest.split_whitespace().next().unwrap_or("");
                if self.ident_peer.as_deref().is_some_and(|p| same(p, heard)) {
                    self.ident_peer = None;
                    self.ident_after = self.now + HEARD_MS;
                }
            }
            "BUFFER" => {
                if let Ok(n) = rest.parse::<u64>() {
                    self.on_buffer(n, &mut out);
                }
            }
            "PTT" => self.on_ptt(rest == "ON", &mut out),
            "SN" => {
                if let Ok(snr) = rest.parse::<f64>()
                    && snr.is_finite()
                {
                    self.snr_db = Some(snr);
                    self.tell_quality(&mut out);
                }
            }
            "BITRATE" => {
                // BITRATE (<mode>) <bps> BPS
                if let Some(bps) = rest.split_whitespace().nth(1).and_then(|t| t.parse().ok()) {
                    self.rate_bps = Some(bps);
                    self.tell_quality(&mut out);
                }
            }
            "BUSY" => {
                self.channel_busy = rest == "ON";
                self.update_busy(&mut out);
            }
            _ => {} // REGISTERED, IAMALIVE, CQFRAME, MONITOR and what a later Mercury adds
        }
        out
    }

    /// `BUSYDETECT ON` or `OFF`: Mercury 1.9.23 or later answering the question `hello` asked.
    fn on_probe(&mut self, detector: bool, out: &mut Vec<Out>) {
        let probing = |a: &Asked| matches!(a.what, Waiting::Probe { .. });
        if !self.waiting.front().is_some_and(probing) {
            return;
        }
        let Some(Asked { host, what }) = self.waiting.pop_front() else {
            return;
        };
        self.detector = Some(detector);
        // whatever the last program left it doing, Mercury answers nobody until a host listens
        self.settle_listen(out);
        if let Waiting::Probe { rid } = what
            && host == self.host
        {
            self.say_hello(rid.as_ref(), out);
        }
    }

    fn on_ptt(&mut self, on: bool, out: &mut Vec<Out>) {
        if on && self.pending && self.link.is_none() && self.ident.is_none() {
            self.answer_keyed = true;
        }
        let Some(tail) = self.tail.as_mut() else {
            return;
        };
        if on {
            tail.keyed = true;
        } else if tail.keyed {
            // the transmission keyed after the ABORT is over: its DISCONNECT frame is out
            self.tail = None;
            self.settle_listen(out);
            self.identify(out);
        }
    }

    /// `OK` or `WRONG`, for the oldest command still unanswered.
    fn answered(&mut self, ok: bool, out: &mut Vec<Out>) {
        let Some(asked) = self.waiting.pop_front() else {
            return;
        };
        let mine = asked.host == self.host;
        match asked.what {
            Waiting::Probe { rid } => {
                // any other answer than BUSYDETECT's own: not a Mercury this adapter knows
                self.modem_lost = true;
                self.lost_why.clear();
                self.lost_why.push_str(NOT_MERCURY);
                if mine {
                    out.push(Out::Host(error(rid.as_ref(), "no-modem", &self.lost_why)));
                }
            }
            Waiting::Callsigns { rid, wanted } => {
                if !ok {
                    self.mycall.clear();
                }
                if !mine {
                    return;
                }
                if ok {
                    self.callsigns = wanted;
                    Self::ok(rid.as_ref(), out);
                } else {
                    let text = "Mercury refused the callsigns";
                    out.push(Out::Host(error(rid.as_ref(), "refused", text)));
                }
            }
            Waiting::Step { call } => {
                if self.call.as_ref().is_none_or(|c| c.serial != call) {
                    return;
                }
                if ok {
                    if let Some(c) = self.call.as_mut() {
                        c.in_flight = false;
                    }
                    self.advance_call(out);
                } else {
                    self.mycall.clear();
                    self.bandwidth = None;
                    self.end_call("refused", "Mercury refused what the call needs", out);
                }
            }
            Waiting::Connect { call } => {
                let Some(c) = self.call.as_mut().filter(|c| c.serial == call) else {
                    return;
                };
                if ok {
                    if !c.told {
                        c.told = true;
                        out.push(Out::Host(answer("ok", Some(&c.id))));
                    }
                } else {
                    self.end_call("refused", "Mercury refused the call", out);
                    self.settle_listen(out);
                }
            }
            Waiting::Ident => {
                if !ok {
                    // a callsign or bandwidth Mercury will not send: nothing comes of it
                    self.ident = None;
                    self.identify(out);
                    self.advance_call(out);
                }
            }
            Waiting::Plain(what) => {
                if !ok && mine && self.hello {
                    let text = format!("Mercury refused {what}");
                    out.push(Out::Host(error(None, "refused", &text)));
                }
            }
        }
    }

    /// `CONNECTED <caller> <callsign called> <bandwidth>`.
    fn on_connected(&mut self, rest: &str, out: &mut Vec<Out>) {
        let mut parts = rest.split_whitespace();
        let (Some(caller), Some(dialed)) = (parts.next(), parts.next()) else {
            return;
        };
        let bandwidth = parts.next().and_then(|b| b.parse::<i64>().ok());
        self.pending = false;
        self.answer_keyed = false;
        if self.link.is_some() {
            return;
        }
        // Mercury gives CONNECT's callsigns back as they were written, the caller first: a
        // session whose caller is the host's own callsign is the call the host placed
        let placed = self
            .call
            .as_ref()
            .is_some_and(|c| c.placed && same(caller, &c.local));
        if placed {
            let Some(call) = self.call.take() else {
                return;
            };
            if call.aborted {
                // answered as the host gave it up: its ABORT is ending the session
                self.call = Some(call);
                return;
            }
            if !call.told {
                out.push(Out::Host(answer("ok", Some(&call.id))));
            }
            let (remote, local) = (call.remote.clone(), call.local.clone());
            self.link_up(&remote, &local, "out", Some(&call.id), bandwidth, out);
            return;
        }
        // a call to this station was answered: a call of the host's own has lost to it
        self.end_call("busy", "Mercury answered a call first", out);
        let local = self.callsigns.iter().find(|c| same(c, dialed)).cloned();
        match local {
            Some(local) if self.hello && self.listening => {
                out.push(Out::Host(answer_with(
                    "incoming",
                    None,
                    &[("remote", caller.into()), ("local", local.clone().into())],
                )));
                self.link_up(caller, &local, "in", None, bandwidth, out);
            }
            _ => {
                // answered for nobody: the host does not listen, or it is not its callsign; or a
                // call the host gave up as it left. The station's callsign is the one it holds.
                let ours = [dialed, caller]
                    .into_iter()
                    .find(|c| self.mycall.iter().any(|m| same(m, c)))
                    .unwrap_or(dialed);
                self.ghost = Some(ours.to_owned());
                self.owed_bw = bandwidth.unwrap_or(IDENT_BW);
                self.abort(out);
            }
        }
    }

    fn link_up(
        &mut self,
        remote: &str,
        local: &str,
        direction: &'static str,
        id: Option<&Value>,
        bandwidth: Option<i64>,
        out: &mut Vec<Out>,
    ) {
        self.links_made += 1;
        let link = format!("m{}", self.links_made);
        let mut m = answer_with(
            "connected",
            id,
            &[
                ("link", link.clone().into()),
                ("remote", remote.into()),
                ("local", local.into()),
                ("direction", direction.into()),
            ],
        );
        if let Some(hz) = bandwidth {
            m.insert("bandwidth".into(), hz.into());
        }
        out.push(Out::Host(m));
        self.tail = None;
        self.snr_db = None;
        self.rate_bps = None;
        self.link = Some(Link {
            id: link.clone(),
            remote: remote.to_owned(),
            local: local.to_owned(),
            direction,
            bandwidth,
            given: 0,
            fed: 0,
            acked: 0,
            taken: 0,
            held: VecDeque::new(),
            refs: VecDeque::new(),
            fed_at: self.now,
            buffer: None,
            queued_told: 0,
            ending: false,
            closing: false,
            asked: false,
            quality_told: None,
        });
        // bytes read from the data port just before CONNECTED was read from the control port
        let now = self.now;
        for (at, bytes) in std::mem::take(&mut self.early) {
            if now <= at + EARLY_MS {
                out.push(Out::Host(answer_with(
                    "received",
                    None,
                    &[
                        ("link", link.clone().into()),
                        ("data", base64::encode(&bytes).into()),
                    ],
                )));
            }
        }
        self.update_busy(out);
    }

    fn on_disconnected(&mut self, out: &mut Vec<Out>) {
        self.pending = false;
        self.early.clear();
        if let Some(l) = self.link.take() {
            self.owe_for(&l);
            let (reason, text) = if l.ending {
                ("local", "the host ended the link")
            } else {
                // Mercury does not say why: the other station closed it, or it was lost
                ("remote", "Mercury says the session ended")
            };
            out.push(Out::Host(answer_with(
                "disconnected",
                None,
                &[
                    ("link", l.id.into()),
                    ("reason", reason.into()),
                    ("text", text.into()),
                ],
            )));
            self.update_busy(out);
        } else if let Some(ours) = self.ghost.take() {
            let bw = self.owed_bw;
            self.owe(&ours, None, Some(bw));
        } else if self.call.as_ref().is_some_and(|c| c.placed) {
            if self.call.as_ref().is_some_and(|c| c.aborted) {
                self.end_call("aborted", "the host aborted", out);
            } else {
                self.end_call("timeout", "no answer", out);
            }
        }
        // else: Mercury's answer to a DISCONNECT, an ABORT or a LISTEN OFF that found no session
        self.settle_listen(out);
        self.identify(out);
    }

    /// A link has ended: the station identifies with the callsign it had on it, the caller after
    /// the station it called.
    fn owe_for(&mut self, l: &Link) {
        let peer = (l.direction == "out").then_some(l.remote.as_str());
        self.owe(&l.local, peer, l.bandwidth);
    }

    /// Bytes read from Mercury's data port.
    pub fn from_data(&mut self, bytes: &[u8]) -> Vec<Out> {
        if bytes.is_empty() {
            return Vec::new();
        }
        if let Some(l) = &self.link {
            return vec![Out::Host(answer_with(
                "received",
                None,
                &[
                    ("link", l.id.clone().into()),
                    ("data", base64::encode(bytes).into()),
                ],
            ))];
        }
        // the two ports are read apart: these may be a link's first bytes, ahead of its CONNECTED
        let kept: usize = self.early.iter().map(|(_, b)| b.len()).sum();
        if self.hello && self.ghost.is_none() && kept + bytes.len() <= EARLY_MAX {
            self.early.push_back((self.now, bytes.to_vec()));
        }
        Vec::new()
    }

    /// The host's connection closed (RP-4 §1.1): its link and its call under way end as `abort`
    /// would end them, and Mercury stops answering once that is done ([`Bridge::settled`]).
    pub fn host_left(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        let link = self.link.take();
        let call = self.call.take();
        self.host += 1;
        self.hello = false;
        self.callsigns.clear();
        self.listening = false;
        self.early.clear();
        self.busy_told = false;
        if self.modem_lost {
            return out;
        }
        if let Some(l) = &link {
            self.owe_for(l);
        }
        let aborting = link.as_ref().is_some_and(|l| l.asked && !l.closing);
        if (link.is_some() && !aborting) || call.is_some_and(|c| c.placed && !c.aborted) {
            self.abort(&mut out);
        }
        self.settle_listen(&mut out);
        out
    }

    /// Whether nothing is left to do for a host that has gone: Mercury no longer answers, and
    /// every command has its answer. The program then closes its connections to Mercury.
    #[must_use]
    pub fn settled(&self) -> bool {
        let quiet = self.detector.is_none() || !self.modem_listening;
        let identified = self.owed.is_empty() && self.ident.is_none();
        self.modem_lost || (quiet && identified && self.tail.is_none() && self.waiting.is_empty())
    }

    /// Mercury's connection closed, for the reason `why` (RP-4 §2.1): the host is told its link
    /// and its call are over, and its connection is closed.
    pub fn modem_lost(&mut self, why: &str) -> Vec<Out> {
        let text = format!("Mercury went away: {why}");
        let mut out = Vec::new();
        self.modem_lost = true;
        self.lost_why.clone_from(&text);
        self.owed.clear();
        self.ident = None;
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
        if let Some(call) = self.call.as_mut() {
            call.told = true;
        }
        self.end_call("refused", &text, &mut out);
        for asked in std::mem::take(&mut self.waiting) {
            if asked.host != self.host {
                continue;
            }
            if let Waiting::Probe { rid } | Waiting::Callsigns { rid, .. } = asked.what
                && rid.is_some()
            {
                out.push(Out::Host(error(rid.as_ref(), "no-modem", &text)));
            }
        }
        out.push(Out::CloseHost);
        out
    }
}
