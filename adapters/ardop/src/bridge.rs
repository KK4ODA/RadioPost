// SPDX-License-Identifier: MIT OR Apache-2.0
//! One RPMI host on an ARDOP TNC's host interface: RP-4 §9.3's table, with no input or output of
//! its own. Written from ardopcf's *TCP Host Interface Commands* and its source (MIT).
//!
//! The program gives the bridge each line the host writes ([`Bridge::from_host`]), each line the
//! TNC writes on its command port ([`Bridge::from_modem`]), the bytes of its data port
//! ([`Bridge::from_data`]) and the time ([`Bridge::tick`]), and does what it answers.
//!
//! What the bridge keeps ARDOP to, that its commands do not do by themselves:
//!
//! * ARDOP answers a command by echoing its name (`MYCALL now …`, `LISTEN now TRUE`), or with
//!   `FAULT`; other lines come of its own accord. The bridge matches echoes to its commands in
//!   order.
//! * `ARQCALL` goes out under `MYCALL`: the callsign a `connect` calls as is made `MYCALL` first,
//!   the host's others `MYAUX`.
//! * `CONNECTED` carries no direction: a call the host placed is the one to the station it named;
//!   any other, after `PENDING` and `TARGET`, is a call answered.
//! * `DISCONNECT` is sent once `BUFFER` says the TNC holds nothing, and the bytes the bridge holds
//!   have gone to it. ARDOP's `BUFFER` counts what is not yet sent or acknowledged; whether it is
//!   acknowledged is not published, so no `delivered` is reported (`delivery` false).
//! * `ABORT` ends a session with nothing more said to the host: the bridge reports the link's end
//!   itself. The other station hears nothing and times out.
//! * No `ARQCALL` for a second after `DISCONNECT` or `ABORT`, so that a `DISCONNECTED` of the old
//!   session is not read as the new call's end.

use std::collections::VecDeque;

use radiopost_mercury::bridge::Out;
use radiopost_mercury::serve::TncBridge;
use rpmi::base64;
use rpmi::wire::{
    Caps, Message, Request, VERSION, admit, answer, answer_with, error, parse_line, speaks,
};
use serde_json::Value;

/// ARDOP's ARQ bandwidths (`ARQBW`).
pub const BANDWIDTHS: [i64; 4] = [200, 500, 1000, 2000];
/// The most bytes a host should keep queued (`caps.max_queue`).
pub const MAX_QUEUE: u64 = 65536;
/// The most bytes in one block on the data port.
pub const BLOCK: usize = 1024;
/// The most bytes the bridge has given the TNC that it still holds.
const FEED: u64 = 8192;
/// How many CONREQ frames a call sends (`ARQCALL <call> <n>`, 2 to 15).
const ATTEMPTS: u32 = 5;
/// How long the TNC is given to answer a command.
const REPLY_MS: u64 = 10_000;
/// How often `BUFFER` is asked while the TNC holds bytes of the link's.
const ASK_MS: u64 = 1000;
/// How long an orderly `disconnect` waits for the TNC to have sent everything.
const DRAIN_MS: u64 = 120_000;
/// How long after `DISCONNECT` or `ABORT` no `ARQCALL` is sent.
const QUIET_MS: u64 = 1000;
/// How long a `DISCONNECT` is given to end the session before the host is told it has.
const CLOSE_MS: u64 = 60_000;

/// What a host is told when the modem there does not answer as ARDOP does.
const NOT_ARDOP: &str = "the modem there did not answer VERSION as an ARDOP TNC does";

/// Why a callsign cannot be an ARDOP station ID: 3 to 7 capitals and digits, then `-1` to `-15`
/// or one capital. So a callsign with `/` (RP-0 §6.1) cannot be used.
#[must_use]
pub fn callsign_fault(callsign: &str) -> Option<&'static str> {
    let (base, ssid) = match callsign.split_once('-') {
        Some((base, ssid)) => (base, Some(ssid)),
        None => (callsign, None),
    };
    let base_ok = (3..=7).contains(&base.len())
        && base
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
    let ssid_ok = ssid.is_none_or(|s| {
        (s.len() == 1 && s.bytes().all(|b| b.is_ascii_uppercase()))
            || (!s.starts_with('0') && s.parse::<u8>().is_ok_and(|n| (1..=15).contains(&n)))
    });
    (!(base_ok && ssid_ok))
        .then_some("an ARDOP station is 3 to 7 capitals and digits, then -1 to -15 or a letter")
}

/// What a command is waiting to answer.
#[derive(Debug)]
enum Waiting {
    /// `VERSION`, asked at the host's `hello`.
    Probe { rid: Option<Value> },
    /// `MYCALL` or `MYAUX` for the host's `callsigns`; the last one answers it.
    Callsigns {
        rid: Option<Value>,
        last: bool,
        wanted: Vec<String>,
    },
    /// `LISTEN` for the host's `listen`.
    Listen { rid: Option<Value> },
    /// `ARQCALL` for the host's call.
    Call,
    /// `MYAUX -`, which clears the auxiliary callsigns: ARDOP refuses it with `FAULT` and
    /// clears them, so either answer will do.
    AuxCleared {
        rid: Option<Value>,
        wanted: Vec<String>,
    },
    /// One whose answer only needs reading.
    Plain,
}

#[derive(Debug)]
struct Asked {
    name: &'static str,
    what: Waiting,
    at: u64,
}

#[derive(Debug)]
enum Step {
    Mycall(Vec<String>),
    Bandwidth(i64),
    Call,
}

#[derive(Debug)]
struct Call {
    id: Value,
    remote: String,
    local: String,
    steps: VecDeque<Step>,
    placed: bool,
}

#[derive(Debug)]
struct Link {
    id: String,
    remote: String,
    held: VecDeque<u8>,
    /// What the TNC said it holds, and whether bytes went to it since.
    buffer: u64,
    fed_since: u64,
    asked: Option<u64>,
    queued_told: Option<u64>,
    ending: Option<u64>,
    closing: Option<u64>,
}

/// One host on one ARDOP TNC.
#[derive(Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each a separate thing the host or the TNC said"
)]
pub struct Bridge {
    now: u64,
    hello: bool,
    lost: Option<String>,
    probed: bool,
    callsigns: Vec<String>,
    /// MYCALL and MYAUX as the TNC holds them.
    mycall: Vec<String>,
    listening: bool,
    waiting: VecDeque<Asked>,
    call: Option<Call>,
    link: Option<Link>,
    links_made: u64,
    pending: bool,
    target: Option<String>,
    busy: bool,
    busy_told: Option<bool>,
    quiet_until: u64,
    /// Data-port bytes of a block not yet whole.
    partial: Vec<u8>,
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl Default for Bridge {
    fn default() -> Self {
        Bridge::new()
    }
}

impl Bridge {
    /// A bridge on a TNC reached.
    #[must_use]
    pub fn new() -> Bridge {
        Bridge {
            now: 0,
            hello: false,
            lost: None,
            probed: false,
            callsigns: Vec::new(),
            mycall: Vec::new(),
            listening: false,
            waiting: VecDeque::new(),
            call: None,
            link: None,
            links_made: 0,
            pending: false,
            target: None,
            busy: false,
            busy_told: None,
            quiet_until: 0,
            partial: Vec::new(),
        }
    }

    /// A bridge whose TNC could not be reached: the host's `hello` gets `no-modem` with `why`.
    #[must_use]
    pub fn without_tnc(why: &str) -> Bridge {
        let mut b = Bridge::new();
        b.lost = Some(why.to_owned());
        b
    }

    fn command(&mut self, line: &str, what: Waiting, out: &mut Vec<Out>) {
        let name = match line.split_whitespace().next().unwrap_or("") {
            "MYCALL" => "MYCALL",
            "MYAUX" => "MYAUX",
            "LISTEN" => "LISTEN",
            "ARQBW" => "ARQBW",
            "ARQCALL" => "ARQCALL",
            "DISCONNECT" => "DISCONNECT",
            "ABORT" => "ABORT",
            "VERSION" => "VERSION",
            "PROTOCOLMODE" => "PROTOCOLMODE",
            "BUFFER" => "BUFFER",
            _ => "",
        };
        self.waiting.push_back(Asked {
            name,
            what,
            at: self.now,
        });
        out.push(Out::Command(line.to_owned()));
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
                if self.probed {
                    self.say_hello(rid_ref, &mut out);
                } else {
                    self.command("VERSION", Waiting::Probe { rid }, &mut out);
                }
            }
            Request::Callsigns(wanted) => self.on_callsigns(&wanted, rid, &mut out),
            Request::Listen(on) => {
                if self.callsigns.is_empty() {
                    out.push(Out::Host(error(rid_ref, "refused", "no callsigns yet")));
                } else {
                    self.listening = on;
                    let line = if on { "LISTEN TRUE" } else { "LISTEN FALSE" };
                    self.command(line, Waiting::Listen { rid }, &mut out);
                }
            }
            Request::Connect {
                remote,
                as_call,
                bandwidth,
                call_s: _,
            } => self.on_connect(&remote, as_call, bandwidth, rid, &mut out),
            Request::Send { link, data, .. } => self.on_send(&link, &data, rid_ref, &mut out),
            Request::Disconnect { link } => self.on_disconnect(&link, rid_ref, &mut out),
            Request::Abort { link } => self.on_abort(link.as_deref(), rid_ref, &mut out),
            Request::Broadcast { .. } => {
                let text = "ARDOP's FEC mode is not carried by this adapter";
                out.push(Out::Host(error(rid_ref, "unsupported", text)));
            }
            Request::Dial | Request::Tune(_) | Request::Scan(_) => {
                let text = "ARDOP's host commands have no dial";
                out.push(Out::Host(error(rid_ref, "unsupported", text)));
            }
            Request::Tone(_) => {
                let text = "ARDOP's host commands have no tuner tone";
                out.push(Out::Host(error(rid_ref, "unsupported", text)));
            }
            Request::Status => out.push(Out::Host(self.status(rid_ref))),
        }
        self.advance_call(&mut out);
        out
    }

    fn say_hello(&mut self, rid: Option<&Value>, out: &mut Vec<Out>) {
        self.hello = true;
        let caps = Caps {
            bandwidths: BANDWIDTHS.to_vec(),
            max_links: 1,
            delivery: false,
            broadcast: false,
            quality: false,
            busy: true,
            regulatory: false,
            max_queue: MAX_QUEUE,
            shared: false,
            dial: false,
            scan: false,
            call_s: false,
            tone: false,
        };
        out.push(Out::Host(answer_with(
            "hello",
            rid,
            &[
                ("version", VERSION.into()),
                ("modem", "ARDOP".into()),
                ("caps", caps.to_json()),
            ],
        )));
        self.busy_told = None;
        self.update_busy(out);
    }

    fn update_busy(&mut self, out: &mut Vec<Out>) {
        let busy = self.busy && self.link.is_none();
        if self.hello && self.busy_told != Some(busy) {
            self.busy_told = Some(busy);
            out.push(Out::Host(answer_with(
                "busy",
                None,
                &[("busy", busy.into())],
            )));
        }
    }

    fn on_callsigns(&mut self, wanted: &[String], rid: Option<Value>, out: &mut Vec<Out>) {
        if let Some(why) = wanted
            .iter()
            .find_map(|c| callsign_fault(c).map(|why| format!("{c}: {why}")))
        {
            out.push(Out::Host(error(rid.as_ref(), "refused", &why)));
            return;
        }
        if wanted.is_empty() {
            // ARDOP has no command that clears MYCALL: it is kept from answering instead
            self.callsigns.clear();
            self.listening = false;
            self.command("LISTEN FALSE", Waiting::Listen { rid }, out);
            return;
        }
        self.set_mycall(wanted, rid, out);
    }

    /// `MYCALL` the first, `MYAUX` the others; `rid` answered with the last reply.
    fn set_mycall(&mut self, order: &[String], rid: Option<Value>, out: &mut Vec<Out>) {
        let aux = order[1..].join(",");
        let had_aux = self.mycall.len() > 1;
        let with_aux = !aux.is_empty() || had_aux;
        self.mycall = order.to_vec();
        self.command(
            &format!("MYCALL {}", order[0]),
            Waiting::Callsigns {
                rid: if with_aux { None } else { rid.clone() },
                last: !with_aux,
                wanted: order.to_vec(),
            },
            out,
        );
        if aux.is_empty() && had_aux {
            let wanted = order.to_vec();
            self.command("MYAUX -", Waiting::AuxCleared { rid, wanted }, out);
        } else if with_aux {
            let what = Waiting::Callsigns {
                rid,
                last: true,
                wanted: order.to_vec(),
            };
            self.command(&format!("MYAUX {aux}"), what, out);
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
        let bandwidth = match bandwidth.map(i64::try_from) {
            None => None,
            Some(Ok(bw)) if BANDWIDTHS.contains(&bw) => Some(bw),
            Some(_) => {
                out.push(refuse(
                    "unsupported",
                    "ARDOP has 200, 500, 1000 and 2000 Hz",
                ));
                return;
            }
        };
        if self.call.is_some() || self.link.is_some() || self.pending {
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
        let mut steps = VecDeque::new();
        if self.mycall.first() != Some(&local) {
            let mut order = vec![local.clone()];
            order.extend(self.callsigns.iter().filter(|c| **c != local).cloned());
            steps.push_back(Step::Mycall(order));
        }
        if let Some(bw) = bandwidth {
            steps.push_back(Step::Bandwidth(bw));
        }
        steps.push_back(Step::Call);
        Self::ok(Some(&id), out);
        self.call = Some(Call {
            id,
            remote: remote.to_owned(),
            local,
            steps,
            placed: false,
        });
    }

    /// The call's next step, when the TNC is ready for it.
    fn advance_call(&mut self, out: &mut Vec<Out>) {
        let ready = self.waiting.is_empty() && self.now >= self.quiet_until;
        let Some(call) = self.call.as_mut() else {
            return;
        };
        if !ready || call.placed {
            return;
        }
        match call.steps.pop_front() {
            Some(Step::Mycall(order)) => self.set_mycall(&order, None, out),
            Some(Step::Bandwidth(bw)) => {
                self.command(&format!("ARQBW {bw}MAX"), Waiting::Plain, out);
            }
            Some(Step::Call) => {
                call.placed = true;
                let line = format!("ARQCALL {} {ATTEMPTS}", call.remote);
                self.command(&line, Waiting::Call, out);
            }
            None => {}
        }
    }

    fn end_call(&mut self, code: &str, text: &str, out: &mut Vec<Out>) {
        if let Some(call) = self.call.take() {
            out.push(Out::Host(answer_with(
                "failed",
                Some(&call.id),
                &[("code", code.into()), ("text", text.into())],
            )));
        }
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
        if l.ending.is_some() {
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
        l.ending.get_or_insert(now + DRAIN_MS);
        Self::ok(rid, out);
        self.progress(out);
    }

    fn on_abort(&mut self, link: Option<&str>, rid: Option<&Value>, out: &mut Vec<Out>) {
        match link {
            Some(id) if self.link.as_ref().is_some_and(|l| l.id == id) => {
                Self::ok(rid, out);
                self.command("ABORT", Waiting::Plain, out);
                self.quiet_until = self.now + QUIET_MS;
                self.link_ended("local", "the host aborted the link", out);
            }
            Some(_) => {
                out.push(Out::Host(error(
                    rid,
                    "not-connected",
                    "no such link of this host's",
                )));
            }
            None => {
                if self.call.is_none() {
                    out.push(Out::Host(error(rid, "not-connected", "no call under way")));
                    return;
                }
                Self::ok(rid, out);
                if self.call.as_ref().is_some_and(|c| c.placed) {
                    self.command("ABORT", Waiting::Plain, out);
                    self.quiet_until = self.now + QUIET_MS;
                }
                self.end_call("aborted", "the host's abort", out);
            }
        }
    }

    fn link_ended(&mut self, reason: &str, text: &str, out: &mut Vec<Out>) {
        if let Some(l) = self.link.take() {
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
        }
    }

    fn status(&self, rid: Option<&Value>) -> Message {
        let links: Vec<Value> = self
            .link
            .iter()
            .map(|l| {
                serde_json::json!({"link": l.id, "remote": l.remote,
                                   "queued": l.held.len() as u64 + l.buffer})
            })
            .collect();
        answer_with(
            "status",
            rid,
            &[
                ("listening", self.listening.into()),
                ("callsigns", self.callsigns.clone().into()),
                ("busy", self.busy.into()),
                ("links", links.into()),
            ],
        )
    }

    /// Feeds the link's bytes to the TNC, says what is queued, and ends an orderly link once the
    /// TNC holds nothing.
    fn progress(&mut self, out: &mut Vec<Out>) {
        let now = self.now;
        let mut disconnect = false;
        if let Some(l) = self.link.as_mut() {
            if l.closing.is_none() {
                while !l.held.is_empty() && l.buffer + l.fed_since < FEED {
                    let n = l.held.len().min(BLOCK);
                    let block: Vec<u8> = l.held.drain(..n).collect();
                    let len = u16::try_from(n).unwrap_or(u16::MAX);
                    let mut bytes = len.to_be_bytes().to_vec();
                    bytes.extend_from_slice(&block);
                    out.push(Out::Data(bytes));
                    l.fed_since += n as u64;
                }
                let drained = l.held.is_empty() && l.buffer == 0 && l.fed_since == 0;
                if let Some(until) = l.ending
                    && (drained || now >= until)
                {
                    disconnect = true;
                    l.closing = Some(now);
                }
            }
            let queued = l.held.len() as u64 + l.buffer + l.fed_since;
            if l.queued_told != Some(queued) {
                l.queued_told = Some(queued);
                out.push(Out::Host(answer_with(
                    "queued",
                    None,
                    &[("link", l.id.clone().into()), ("bytes", queued.into())],
                )));
            }
        }
        if disconnect {
            self.command("DISCONNECT", Waiting::Plain, out);
            self.quiet_until = now + QUIET_MS;
        }
    }

    /// The modem's `BUFFER <n>`.
    fn on_buffer(&mut self, n: u64, out: &mut Vec<Out>) {
        if let Some(l) = self.link.as_mut() {
            l.buffer = n;
            // the TNC reports when it takes bytes in: what was fed is in this count
            l.fed_since = 0;
            l.asked = None;
        }
        self.progress(out);
    }

    /// A line the TNC wrote on its command port, without its CR.
    pub fn from_modem(&mut self, line: &str) -> Vec<Out> {
        let mut out = Vec::new();
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        let rest = rest.trim();
        match word {
            "FAULT" => {
                if let Some(asked) = self.waiting.pop_front() {
                    self.answered(asked, false, &mut out);
                }
            }
            "BUFFER" => {
                if self.waiting.front().is_some_and(|a| a.name == "BUFFER") {
                    self.waiting.pop_front();
                }
                self.on_buffer(rest.parse().unwrap_or(0), &mut out);
            }
            "BUSY" => {
                self.busy = rest.eq_ignore_ascii_case("TRUE");
                self.update_busy(&mut out);
            }
            "PENDING" => self.pending = true,
            "CANCELPENDING" => {
                self.pending = false;
                self.target = None;
            }
            "TARGET" => self.target = Some(rest.to_owned()),
            "CONNECTED" => self.on_connected(rest, &mut out),
            "DISCONNECTED" => {
                self.pending = false;
                self.target = None;
                let ending = self.link.as_ref().is_some_and(|l| l.ending.is_some());
                if ending {
                    self.link_ended("local", "the host ended the link", &mut out);
                } else {
                    self.link_ended("remote", "ARDOP says the session ended", &mut out);
                }
            }
            "REJECTEDBW" => {
                self.end_call("refused", "the other station's bandwidth differs", &mut out);
            }
            "REJECTEDBUSY" => self.end_call("busy", "the channel was busy", &mut out),
            "STATUS" => {
                let upper = rest.to_ascii_uppercase();
                let failed = upper.starts_with("CONNECT TO") && upper.contains("FAILED")
                    || upper.starts_with("END ARQ CALL");
                if failed && self.call.as_ref().is_some_and(|c| c.placed) {
                    self.end_call("timeout", "no answer", &mut out);
                }
            }
            _ => {
                // an echo answers the oldest command, which has that name
                if self
                    .waiting
                    .front()
                    .is_some_and(|a| !a.name.is_empty() && a.name == word)
                    && let Some(asked) = self.waiting.pop_front()
                {
                    self.answered(asked, true, &mut out);
                }
            }
        }
        self.advance_call(&mut out);
        out
    }

    fn answered(&mut self, asked: Asked, ok: bool, out: &mut Vec<Out>) {
        match asked.what {
            Waiting::Probe { rid } => {
                if ok {
                    self.probed = true;
                    // what an ARQ host needs: ARQ mode, and no answering until it listens
                    self.command("PROTOCOLMODE ARQ", Waiting::Plain, out);
                    self.command("LISTEN FALSE", Waiting::Plain, out);
                    self.say_hello(rid.as_ref(), out);
                } else {
                    self.lost = Some(NOT_ARDOP.into());
                    out.push(Out::Host(error(rid.as_ref(), "no-modem", NOT_ARDOP)));
                }
            }
            Waiting::Callsigns { rid, last, wanted } => {
                if !ok {
                    self.mycall.clear();
                    if rid.is_some() {
                        let text = "the TNC refused the callsigns";
                        out.push(Out::Host(error(rid.as_ref(), "refused", text)));
                    } else {
                        self.end_call("refused", "the TNC refused the callsign", out);
                    }
                } else if last && rid.is_some() {
                    self.callsigns = wanted;
                    Self::ok(rid.as_ref(), out);
                }
            }
            Waiting::Listen { rid } => {
                if ok {
                    Self::ok(rid.as_ref(), out);
                } else {
                    self.listening = false;
                    out.push(Out::Host(error(
                        rid.as_ref(),
                        "refused",
                        "the TNC refused LISTEN",
                    )));
                }
            }
            Waiting::AuxCleared { rid, wanted } => {
                if rid.is_some() {
                    self.callsigns = wanted;
                    Self::ok(rid.as_ref(), out);
                }
            }
            Waiting::Call => {
                if !ok {
                    self.end_call("refused", "the TNC refused the call", out);
                }
            }
            Waiting::Plain => {}
        }
    }

    fn on_connected(&mut self, rest: &str, out: &mut Vec<Out>) {
        let mut parts = rest.split_whitespace();
        let Some(remote) = parts.next() else {
            return;
        };
        let bandwidth = parts.next().and_then(|b| b.parse::<i64>().ok());
        self.pending = false;
        let target = self.target.take();
        if self.link.is_some() {
            return;
        }
        let ours = self
            .call
            .as_ref()
            .is_some_and(|c| c.placed && same(&c.remote, remote));
        if ours {
            let Some(call) = self.call.take() else {
                return;
            };
            self.link_up(remote, &call.local, "out", Some(&call.id), bandwidth, out);
            return;
        }
        // a call answered: one the host placed has lost to it
        self.end_call("busy", "ARDOP answered a call first", out);
        let local = target
            .and_then(|t| self.callsigns.iter().find(|c| same(c, &t)).cloned())
            .or_else(|| self.mycall.first().cloned());
        match local {
            Some(local) if self.hello && self.listening && self.callsigns.contains(&local) => {
                out.push(Out::Host(answer_with(
                    "incoming",
                    None,
                    &[("remote", remote.into()), ("local", local.clone().into())],
                )));
                self.link_up(remote, &local, "in", None, bandwidth, out);
            }
            _ => {
                // answered for nobody: ended at once
                self.command("ABORT", Waiting::Plain, out);
                self.quiet_until = self.now + QUIET_MS;
            }
        }
    }

    fn link_up(
        &mut self,
        remote: &str,
        local: &str,
        direction: &str,
        id: Option<&Value>,
        bandwidth: Option<i64>,
        out: &mut Vec<Out>,
    ) {
        self.links_made += 1;
        let link = format!("a{}", self.links_made);
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
        self.link = Some(Link {
            id: link,
            remote: remote.to_owned(),
            held: VecDeque::new(),
            buffer: 0,
            fed_since: 0,
            asked: None,
            queued_told: None,
            ending: None,
            closing: None,
        });
        self.update_busy(out);
    }

    /// Bytes the TNC wrote on its data port: blocks of a two-byte length, a three-letter tag and
    /// the data. `ARQ` blocks are the link's.
    pub fn from_data(&mut self, bytes: &[u8]) -> Vec<Out> {
        self.partial.extend_from_slice(bytes);
        let mut out = Vec::new();
        while self.partial.len() >= 2 {
            let len = usize::from(u16::from_be_bytes([self.partial[0], self.partial[1]]));
            if self.partial.len() < 2 + len {
                break;
            }
            let block: Vec<u8> = self.partial.drain(..2 + len).skip(2).collect();
            if block.len() >= 3
                && &block[..3] == b"ARQ"
                && let Some(l) = &self.link
            {
                out.push(Out::Host(answer_with(
                    "received",
                    None,
                    &[
                        ("link", l.id.clone().into()),
                        ("data", base64::encode(&block[3..]).into()),
                    ],
                )));
            }
        }
        out
    }

    /// The time, in milliseconds from any start.
    pub fn tick(&mut self, now: u64) -> Vec<Out> {
        self.now = now;
        let mut out = Vec::new();
        if let Some(asked) = self.waiting.front()
            && now >= asked.at + REPLY_MS
            && let Some(asked) = self.waiting.pop_front()
        {
            self.answered(asked, false, &mut out);
        }
        let mut closed = false;
        let mut ask = false;
        if let Some(l) = self.link.as_mut() {
            closed = l.closing.is_some_and(|at| now >= at + CLOSE_MS);
            let holding = l.buffer > 0 || l.ending.is_some();
            if l.closing.is_none() && holding && l.asked.is_none_or(|at| now >= at + ASK_MS) {
                l.asked = Some(now);
                ask = true;
            }
        }
        if closed {
            self.link_ended("local", "ARDOP did not confirm the end", &mut out);
        }
        if ask {
            self.command("BUFFER", Waiting::Plain, &mut out);
        }
        self.progress(&mut out);
        self.advance_call(&mut out);
        out
    }

    /// The host has gone (RP-4 §1.1): its link or call aborted, answering off.
    pub fn host_left(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        if self.link.take().is_some() || self.call.take().is_some_and(|c| c.placed) {
            out.push(Out::Command("ABORT".into()));
        }
        out.push(Out::Command("LISTEN FALSE".into()));
        out
    }

    /// Whether nothing of the last host's is left on the air.
    #[must_use]
    pub fn settled(&self) -> bool {
        self.link.is_none() && self.call.is_none()
    }

    /// The TNC's connection failed: the host is told what that ended, and closed (RP-4 §2.1).
    pub fn modem_lost(&mut self, why: &str) -> Vec<Out> {
        let text = format!("ARDOP went away: {why}");
        let mut out = Vec::new();
        self.lost = Some(text.clone());
        self.link_ended("failure", &text, &mut out);
        self.end_call("refused", &text, &mut out);
        for asked in std::mem::take(&mut self.waiting) {
            if let Waiting::Probe { rid } | Waiting::Callsigns { rid, .. } | Waiting::Listen { rid } =
                asked.what
                && rid.is_some()
            {
                out.push(Out::Host(error(rid.as_ref(), "no-modem", &text)));
            }
        }
        out.push(Out::CloseHost);
        out
    }
}

impl TncBridge for Bridge {
    fn tick(&mut self, now: u64) -> Vec<Out> {
        Bridge::tick(self, now)
    }
    fn host_line(&mut self, line: &str) -> Vec<Out> {
        Bridge::from_host(self, line)
    }
    fn modem_line(&mut self, line: &str) -> Vec<Out> {
        Bridge::from_modem(self, line)
    }
    fn modem_data(&mut self, bytes: &[u8]) -> Vec<Out> {
        Bridge::from_data(self, bytes)
    }
    fn modem_lost(&mut self, why: &str) -> Vec<Out> {
        Bridge::modem_lost(self, why)
    }
    fn host_left(&mut self) -> Vec<Out> {
        Bridge::host_left(self)
    }
    fn settled(&self) -> bool {
        Bridge::settled(self)
    }
}
