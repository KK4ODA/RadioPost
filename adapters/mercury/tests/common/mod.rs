// SPDX-License-Identifier: MIT OR Apache-2.0
//! A stand-in for two Mercury modems on one channel: the TNC interface of Mercury's
//! `docs/TNC.md`, with what its source does where the document is silent, and a channel on which
//! every frame takes a tenth of a second or so.
//!
//! It is what the adapter was written against, not Mercury: `tests/mercury.rs` runs the real
//! modem. What it keeps to, because the adapter leans on each:
//!
//! * one program at a time, and the control port goes to the newest;
//! * a CALL carries `MYCALL`'s first callsign, whatever `CONNECT` names;
//! * `LISTEN OFF` ends a call or a session without a word to the other station, which then waits
//!   for its watchdog;
//! * `DISCONNECT` delivers what is queued first, and `DISCONNECTED` comes when the session has
//!   closed; with no session it comes at once, as it does to every `ABORT`;
//! * `ABORT`'s DISCONNECT frame goes out after its `DISCONNECTED`;
//! * `BUFFER` is what the other station has not confirmed, counted every tenth of a second, and
//!   once on every write to the data port before the bytes written are taken in;
//! * `CQFRAME` keys at once, whatever is on the air, between `PENDING` and `CANCELPENDING`, and
//!   the other modem says `CQFRAME <callsign> <bandwidth>` when it has heard it; a CALL waits
//!   while the other station is on the air.
//!
//! It logs two transmissions at once when either is a `CQFRAME`, and keeps the `CQFRAME`s heard
//! ([`Air::idents`]).
//!
//! What a host must not do to a real Mercury is kept in [`Air::faults`].

#![allow(dead_code, reason = "each test file uses its own part")]

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

const CALL_MS: u64 = 80;
const ACCEPT_MS: u64 = 80;
const GIVE_UP_MS: u64 = 700;
const TAKE_IN_MS: u64 = 20;
const ROUND_MS: u64 = 120;
const ACK_MS: u64 = 80;
const FRAME_MS: u64 = 150;
const WATCHDOG_MS: u64 = 4000;
const BUFFER_MS: u64 = 100;
const ROUND_BYTES: usize = 4096;
const CQ_MS: u64 = 120;

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Idle,
    Calling {
        remote: String,
        src: String,
    },
    Answering {
        caller: usize,
    },
    Up {
        peer: usize,
        draining: bool,
    },
    /// The session's host has its `DISCONNECTED`; the DISCONNECT frame is still going out.
    Tail {
        peer: usize,
    },
}

#[derive(Debug)]
enum Ev {
    Heard {
        caller: usize,
    },
    Accepted {
        caller: usize,
    },
    GaveUp {
        caller: usize,
    },
    TakeIn {
        at: usize,
        bytes: Vec<u8>,
    },
    Arrive {
        to: usize,
        bytes: Vec<u8>,
    },
    Confirm {
        at: usize,
        n: usize,
    },
    Unkey {
        at: usize,
    },
    FrameOut {
        at: usize,
    },
    Watchdog {
        at: usize,
    },
    CallKey {
        caller: usize,
    },
    CqOut {
        at: usize,
        callsign: String,
        bw: i64,
    },
}

struct Tnc {
    control: Option<TcpStream>,
    control_no: u64,
    data: Option<TcpStream>,
    mycall: Vec<String>,
    listen: bool,
    bw: i64,
    phase: Phase,
    /// Counts calls and sessions, so that a timer set for one is not another's.
    serial: u64,
    session_bw: i64,
    tx: VecDeque<u8>,
    flying: usize,
    in_round: bool,
    buffer_said: Option<u64>,
    buffer_next: u64,
    redial: Option<(String, String)>,
    /// The session's host has gone: nobody is told of its end.
    orphaned: bool,
}

impl Tnc {
    fn new() -> Tnc {
        Tnc {
            control: None,
            control_no: 0,
            data: None,
            mycall: Vec::new(),
            listen: false,
            bw: 2300,
            phase: Phase::Idle,
            serial: 0,
            session_bw: 2300,
            tx: VecDeque::new(),
            flying: 0,
            in_round: false,
            buffer_said: None,
            buffer_next: 0,
            redial: None,
            orphaned: false,
        }
    }

    fn say(&mut self, line: &str) {
        if let Some(c) = self.control.as_mut() {
            let _ = c.write_all(format!("{line}\r").as_bytes());
        }
    }

    fn unconfirmed(&self) -> u64 {
        (self.tx.len() + self.flying) as u64
    }

    fn say_buffer(&mut self, n: u64) {
        if self.buffer_said != Some(n) {
            self.buffer_said = Some(n);
            self.say(&format!("BUFFER {n}"));
        }
    }

    fn idle(&mut self) {
        self.phase = Phase::Idle;
        self.serial += 1;
        self.tx.clear();
        self.flying = 0;
        self.in_round = false;
        self.orphaned = false;
    }

    /// The session is over for this station's host.
    fn ended(&mut self) {
        let told = !self.orphaned;
        self.idle();
        if told {
            self.say("DISCONNECTED");
        }
    }
}

struct Inner {
    start: Instant,
    tnc: [Tnc; 2],
    timers: Vec<(u64, u64, Ev)>,
    faults: Vec<String>,
    /// Until when each station is keyed, and keyed for a `CQFRAME`.
    air: [u64; 2],
    cq_air: [u64; 2],
    idents: Vec<(usize, String)>,
}

impl Inner {
    fn now(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn after(&mut self, ms: u64, at: usize, ev: Ev) {
        let serial = self.tnc[at].serial;
        let when = self.now() + ms;
        self.timers.push((when, serial, ev));
    }

    fn fault(&mut self, at: usize, what: &str) {
        self.faults.push(format!("station {at}: {what}"));
    }

    /// Keys station `at` for `ms`.
    fn key(&mut self, at: usize, ms: u64) {
        let now = self.now();
        if self.cq_air[1 - at] > now {
            self.fault(at, "keyed while the other station's CQFRAME was on the air");
        }
        self.air[at] = now + ms;
        self.tnc[at].say("PTT ON");
    }

    fn place(&mut self, at: usize, src: &str, remote: &str) {
        let t = &mut self.tnc[at];
        t.serial += 1;
        t.phase = Phase::Calling {
            remote: remote.to_owned(),
            src: src.to_owned(),
        };
        self.call_key(at);
    }

    /// The CALL, once the other station is off the air: Mercury holds a CALL while a frame is
    /// arriving.
    fn call_key(&mut self, at: usize) {
        let now = self.now();
        let busy = self.air[1 - at];
        if busy > now {
            self.after(busy - now + 5, at, Ev::CallKey { caller: at });
            return;
        }
        self.key(at, CALL_MS);
        self.after(CALL_MS, at, Ev::Unkey { at });
        self.after(CALL_MS, at, Ev::Heard { caller: at });
        self.after(GIVE_UP_MS, at, Ev::GaveUp { caller: at });
    }

    /// The caller has heard the ACCEPT: the session is up at both ends.
    #[allow(clippy::similar_names, reason = "a call's two ends")]
    fn accepted(&mut self, caller: usize) {
        let callee = 1 - caller;
        let Phase::Calling { remote, src } = self.tnc[caller].phase.clone() else {
            return;
        };
        if self.tnc[callee].phase != (Phase::Answering { caller }) {
            return;
        }
        let (a, b) = (self.tnc[caller].bw, self.tnc[callee].bw);
        let bw = if a == 500 || b == 500 { 500 } else { a.min(b) };
        // the CALL carried the caller's first callsign, not CONNECT's
        let on_air = self.tnc[caller].mycall.first().cloned().unwrap_or_default();
        let dialed = self.tnc[callee]
            .mycall
            .iter()
            .find(|m| m.eq_ignore_ascii_case(&remote))
            .cloned()
            .unwrap_or_default();
        for (at, peer, line) in [
            (caller, callee, format!("CONNECTED {src} {remote} {bw}")),
            (callee, caller, format!("CONNECTED {on_air} {dialed} {bw}")),
        ] {
            let t = &mut self.tnc[at];
            t.phase = Phase::Up {
                peer,
                draining: false,
            };
            t.session_bw = bw;
            t.say(&line);
        }
    }

    /// The CQ frame is out: the other modem has heard it.
    fn cq_out(&mut self, at: usize, callsign: String, bw: i64) {
        self.tnc[at].say("PTT OFF");
        self.tnc[at].say("CANCELPENDING");
        self.tnc[1 - at].say(&format!("CQFRAME {callsign} {bw}"));
        self.idents.push((at, callsign));
    }

    /// `CQFRAME <callsign> <bandwidth>`: keyed at once.
    fn cq(&mut self, at: usize, callsign: &str, bw: i64) {
        if self.tnc[at].phase != Phase::Idle {
            self.fault(at, "CQFRAME with a call or session under way");
        }
        let now = self.now();
        if self.air[1 - at] > now {
            self.fault(at, "CQFRAME while the other station was on the air");
        }
        self.tnc[at].say("PENDING");
        self.key(at, CQ_MS);
        self.cq_air[at] = now + CQ_MS;
        let ev = Ev::CqOut {
            at,
            callsign: callsign.to_ascii_uppercase(),
            bw,
        };
        self.after(CQ_MS, at, ev);
    }

    /// The host's `ABORT`, or a second `DISCONNECT`: `DISCONNECTED` at once, the frame after.
    fn abort(&mut self, at: usize) {
        self.tnc[at].redial = None;
        match self.tnc[at].phase.clone() {
            Phase::Up { peer, .. } => {
                let t = &mut self.tnc[at];
                t.tx.clear();
                t.flying = 0;
                t.phase = Phase::Tail { peer };
                t.say("DISCONNECTED");
                self.key(at, FRAME_MS);
                self.after(FRAME_MS, at, Ev::FrameOut { at });
            }
            Phase::Calling { .. } => {
                self.tnc[at].ended();
                let other = 1 - at;
                if self.tnc[other].phase == (Phase::Answering { caller: at }) {
                    self.tnc[other].say("CANCELPENDING");
                    self.tnc[other].idle();
                }
            }
            Phase::Answering { .. } => {
                self.tnc[at].say("CANCELPENDING");
                self.tnc[at].ended();
            }
            Phase::Idle | Phase::Tail { .. } => self.tnc[at].say("DISCONNECTED"),
        }
    }

    /// `LISTEN OFF`: Mercury lets go of the radio, whatever it was doing.
    fn release(&mut self, at: usize) {
        self.tnc[at].listen = false;
        match self.tnc[at].phase.clone() {
            Phase::Up { peer, .. } => {
                self.fault(
                    at,
                    "LISTEN OFF with a session up: the other station is not told",
                );
                self.tnc[at].ended();
                self.after(WATCHDOG_MS, peer, Ev::Watchdog { at: peer });
            }
            Phase::Tail { peer } => {
                self.fault(at, "LISTEN OFF before the DISCONNECT frame was out");
                self.tnc[at].idle();
                self.after(WATCHDOG_MS, peer, Ev::Watchdog { at: peer });
            }
            Phase::Calling { .. } => {
                self.fault(at, "LISTEN OFF with a call under way");
                self.abort(at);
            }
            Phase::Answering { .. } => {
                self.tnc[at].say("CANCELPENDING");
                self.tnc[at].ended();
            }
            Phase::Idle => {}
        }
    }

    fn command(&mut self, at: usize, line: &str) {
        let mut words = line.split_whitespace();
        let word = words.next().unwrap_or("");
        let args: Vec<&str> = words.collect();
        match (word, args.as_slice()) {
            ("MYCALL", [first, more @ ..]) => {
                let mut calls = vec![(*first).to_owned()];
                calls.extend(more.iter().take(4).map(|c| (*c).to_owned()));
                let t = &mut self.tnc[at];
                t.say("OK");
                for c in &calls {
                    t.say(&format!("REGISTERED {c}"));
                }
                t.mycall = calls;
            }
            ("LISTEN", ["ON" | "CQ"]) => {
                self.tnc[at].listen = true;
                self.tnc[at].say("OK");
            }
            ("LISTEN", ["OFF"]) => {
                self.tnc[at].say("OK");
                self.release(at);
            }
            ("BW500" | "BW2300" | "BW2750", []) => {
                self.tnc[at].bw = word[2..].parse().unwrap_or(2300);
                self.tnc[at].say("OK");
            }
            ("BUSYDETECT", []) => self.tnc[at].say("BUSYDETECT OFF"),
            ("CQFRAME", [callsign, bw]) => match bw.parse::<i64>() {
                Ok(bw @ (500 | 2300 | 2750)) => {
                    self.tnc[at].say("OK");
                    self.cq(at, callsign, bw);
                }
                _ => self.tnc[at].say("WRONG"),
            },
            ("CONNECT", [src, remote]) => {
                self.tnc[at].say("OK");
                match self.tnc[at].phase.clone() {
                    Phase::Idle => self.place(at, src, remote),
                    Phase::Tail { .. } => {
                        self.tnc[at].redial = Some(((*src).to_owned(), (*remote).to_owned()));
                    }
                    Phase::Answering { .. } => {
                        self.tnc[at].say("CANCELPENDING");
                        self.tnc[at].idle();
                        self.place(at, src, remote);
                    }
                    _ => self.fault(at, "CONNECT with a call or session up: ignored"),
                }
            }
            ("DISCONNECT", []) => {
                self.tnc[at].say("OK");
                match self.tnc[at].phase.clone() {
                    Phase::Up {
                        peer,
                        draining: false,
                    } => {
                        self.tnc[at].phase = Phase::Up {
                            peer,
                            draining: true,
                        };
                    }
                    Phase::Up { draining: true, .. } => {
                        self.fault(at, "a second DISCONNECT: what was queued is dropped");
                        self.abort(at);
                    }
                    Phase::Idle => self.tnc[at].say("DISCONNECTED"),
                    _ => self.abort(at),
                }
            }
            ("ABORT", []) => {
                self.tnc[at].say("OK");
                self.abort(at);
            }
            _ => self.tnc[at].say("WRONG"),
        }
    }

    fn data(&mut self, at: usize, bytes: &[u8]) {
        if !matches!(self.tnc[at].phase, Phase::Up { .. }) {
            self.fault(at, "bytes on the data port with no session up");
            return;
        }
        // the count is taken before the bytes are in it
        let count = self.tnc[at].unconfirmed();
        self.tnc[at].say_buffer(count);
        self.after(
            TAKE_IN_MS,
            at,
            Ev::TakeIn {
                at,
                bytes: bytes.to_vec(),
            },
        );
    }

    /// The program on the control port left, or was put out by another.
    fn host_gone(&mut self, at: usize) {
        if let Some(d) = self.tnc[at].data.take() {
            let _ = d.shutdown(Shutdown::Both);
        }
        self.tnc[at].control = None;
        self.tnc[at].buffer_said = None;
        match self.tnc[at].phase.clone() {
            Phase::Up { peer, .. } => {
                self.tnc[at].orphaned = true;
                self.tnc[at].phase = Phase::Up {
                    peer,
                    draining: true,
                };
            }
            Phase::Calling { .. } | Phase::Answering { .. } => self.abort(at),
            _ => {}
        }
    }

    fn fire(&mut self, ev: Ev) {
        match ev {
            Ev::Heard { caller } => {
                let callee = 1 - caller;
                let Phase::Calling { remote, .. } = self.tnc[caller].phase.clone() else {
                    return;
                };
                let c = &self.tnc[callee];
                let answers = c.listen
                    && c.phase == Phase::Idle
                    && c.mycall.iter().any(|m| m.eq_ignore_ascii_case(&remote));
                if answers {
                    let c = &mut self.tnc[callee];
                    c.serial += 1;
                    c.phase = Phase::Answering { caller };
                    c.say("PENDING");
                    self.key(callee, ACCEPT_MS);
                    self.after(ACCEPT_MS, callee, Ev::Unkey { at: callee });
                    self.after(ACCEPT_MS, caller, Ev::Accepted { caller });
                }
            }
            Ev::Accepted { caller } => self.accepted(caller),
            Ev::GaveUp { caller } => {
                if matches!(self.tnc[caller].phase, Phase::Calling { .. }) {
                    self.tnc[caller].ended();
                }
            }
            Ev::TakeIn { at, bytes } => self.tnc[at].tx.extend(bytes),
            Ev::Arrive { to, bytes } => {
                let t = &mut self.tnc[to];
                if matches!(t.phase, Phase::Up { .. }) && !t.orphaned {
                    if let Some(d) = t.data.as_mut() {
                        let _ = d.write_all(&bytes);
                    }
                    t.say("SN 12.3");
                    t.say("BITRATE (4) 87 BPS");
                }
            }
            Ev::Confirm { at, n } => {
                let t = &mut self.tnc[at];
                t.flying = t.flying.saturating_sub(n);
                t.in_round = false;
                let count = t.unconfirmed();
                t.say_buffer(count);
            }
            Ev::Unkey { at } => self.tnc[at].say("PTT OFF"),
            Ev::FrameOut { at } => {
                let (Phase::Tail { peer } | Phase::Up { peer, .. }) = self.tnc[at].phase else {
                    return;
                };
                let orderly = matches!(self.tnc[at].phase, Phase::Up { .. });
                self.tnc[at].say("PTT OFF");
                if matches!(self.tnc[peer].phase, Phase::Up { peer: p, .. } | Phase::Tail { peer: p } if p == at)
                {
                    self.tnc[peer].ended();
                }
                if orderly {
                    self.tnc[at].ended();
                } else {
                    self.tnc[at].idle();
                }
                if let Some((src, remote)) = self.tnc[at].redial.take() {
                    self.place(at, &src, &remote);
                }
            }
            Ev::CallKey { caller } => {
                if matches!(self.tnc[caller].phase, Phase::Calling { .. }) {
                    self.call_key(caller);
                }
            }
            Ev::CqOut { at, callsign, bw } => self.cq_out(at, callsign, bw),
            Ev::Watchdog { at } => {
                if matches!(self.tnc[at].phase, Phase::Up { .. }) {
                    self.tnc[at].ended();
                }
            }
        }
    }

    fn tick(&mut self) {
        let now = self.now();
        let mut due = Vec::new();
        let mut i = 0;
        while i < self.timers.len() {
            if self.timers[i].0 <= now {
                due.push(self.timers.remove(i));
            } else {
                i += 1;
            }
        }
        due.sort_by_key(|(when, _, _)| *when);
        for (_, serial, ev) in due {
            let at = match &ev {
                Ev::Heard { caller }
                | Ev::Accepted { caller }
                | Ev::GaveUp { caller }
                | Ev::CallKey { caller } => *caller,
                Ev::CqOut { at, .. }
                | Ev::TakeIn { at, .. }
                | Ev::Confirm { at, .. }
                | Ev::Unkey { at }
                | Ev::FrameOut { at }
                | Ev::Watchdog { at } => *at,
                Ev::Arrive { to, .. } => 1 - *to,
            };
            // PTT OFF and a CQ frame belong to the transmitter, not to a session
            if serial == self.tnc[at].serial || matches!(ev, Ev::Unkey { .. } | Ev::CqOut { .. }) {
                self.fire(ev);
            }
        }
        for at in 0..2 {
            let t = &mut self.tnc[at];
            if t.control.is_some() && now >= t.buffer_next {
                t.buffer_next = now + BUFFER_MS;
                let count = t.unconfirmed();
                t.say_buffer(count);
            }
            let Phase::Up { peer, draining } = t.phase.clone() else {
                continue;
            };
            if !t.in_round && !t.tx.is_empty() {
                let n = ROUND_BYTES.min(t.tx.len());
                let bytes: Vec<u8> = t.tx.drain(..n).collect();
                t.flying += n;
                t.in_round = true;
                self.key(at, ROUND_MS);
                self.after(ROUND_MS, at, Ev::Unkey { at });
                self.after(ROUND_MS, at, Ev::Arrive { to: peer, bytes });
                self.after(ROUND_MS + ACK_MS, at, Ev::Confirm { at, n });
            } else if draining && !t.in_round && t.unconfirmed() == 0 {
                // everything is delivered: the DISCONNECT frame, and the session closes
                t.phase = Phase::Up {
                    peer,
                    draining: false,
                };
                t.in_round = true;
                self.key(at, FRAME_MS);
                self.after(FRAME_MS, at, Ev::FrameOut { at });
            }
        }
    }
}

/// Two stand-in Mercury modems on one channel.
#[derive(Clone)]
pub struct Air {
    inner: Arc<Mutex<Inner>>,
}

fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    inner
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A control port and the data port after it, both free.
fn two_ports() -> (TcpListener, TcpListener) {
    for _ in 0..200 {
        let control = TcpListener::bind("127.0.0.1:0").expect("a port");
        let port = control.local_addr().expect("bound").port();
        if let Some(next) = port.checked_add(1)
            && let Ok(data) = TcpListener::bind(("127.0.0.1", next))
        {
            return (control, data);
        }
    }
    panic!("no two ports in a row are free");
}

impl Air {
    /// The channel and its two modems: the address of each one's control port.
    pub fn new() -> (Air, [SocketAddr; 2]) {
        let air = Air {
            inner: Arc::new(Mutex::new(Inner {
                start: Instant::now(),
                tnc: [Tnc::new(), Tnc::new()],
                timers: Vec::new(),
                faults: Vec::new(),
                air: [0; 2],
                cq_air: [0; 2],
                idents: Vec::new(),
            })),
        };
        let mut at = Vec::new();
        for station in 0..2 {
            let (control, data) = two_ports();
            at.push(control.local_addr().expect("bound"));
            let inner = Arc::clone(&air.inner);
            thread::spawn(move || serve_ports(&control, &data, &inner, station));
        }
        let inner = Arc::clone(&air.inner);
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_millis(5));
                lock(&inner).tick();
            }
        });
        (air, [at[0], at[1]])
    }

    /// What a host did that a real Mercury would have answered badly.
    pub fn faults(&self) -> Vec<String> {
        lock(&self.inner).faults.clone()
    }

    /// The `CQFRAME`s sent on the channel: the station and the callsign.
    pub fn idents(&self) -> Vec<(usize, String)> {
        lock(&self.inner).idents.clone()
    }

    /// Whether a program holds the modem's control port.
    pub fn held(&self, station: usize) -> bool {
        lock(&self.inner).tnc[station].control.is_some()
    }

    /// Whether the modem answers calls.
    pub fn listening(&self, station: usize) -> bool {
        lock(&self.inner).tnc[station].listen
    }

    /// The modem's `MYCALL`, the first callsign first.
    pub fn mycall(&self, station: usize) -> Vec<String> {
        lock(&self.inner).tnc[station].mycall.clone()
    }

    /// Stops the modem: the program on its ports sees them close.
    pub fn stop(&self, station: usize) {
        let mut g = lock(&self.inner);
        for s in [g.tnc[station].control.take(), g.tnc[station].data.take()]
            .into_iter()
            .flatten()
        {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

/// The two ports, accepted in one loop as Mercury's are: a data connection waiting now came
/// after any control connection waiting, since a program connects to the control port first, so
/// the control port is taken first. Accepted apart, a new program's data connection could be
/// closed as the last program's.
fn serve_ports(
    control: &TcpListener,
    data: &TcpListener,
    inner: &Arc<Mutex<Inner>>,
    station: usize,
) {
    control.set_nonblocking(true).expect("a listener");
    data.set_nonblocking(true).expect("a listener");
    loop {
        let new_data = data.accept().ok().map(|(s, _)| s);
        if let Ok((s, _)) = control.accept() {
            take_control(s, inner, station);
        }
        if let Some(s) = new_data {
            take_data(s, inner, station);
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn take_control(stream: TcpStream, inner: &Arc<Mutex<Inner>>, station: usize) {
    let _ = stream.set_nonblocking(false);
    let Ok(writer) = stream.try_clone() else {
        return;
    };
    let no = {
        let mut g = lock(inner);
        if let Some(old) = g.tnc[station].control.take() {
            // Mercury gives the port to the newest program. One that had already left, its
            // leaving not yet seen here, was put out by nobody.
            let _ = old.set_nonblocking(true);
            if !matches!(old.peek(&mut [0u8; 1]), Ok(0)) {
                g.fault(station, "a second program took the control port");
            }
            let _ = old.shutdown(Shutdown::Both);
            g.host_gone(station);
        }
        let t = &mut g.tnc[station];
        t.control_no += 1;
        t.control = Some(writer);
        t.buffer_said = None;
        if let Some(first) = t.mycall.first().cloned() {
            t.say(&format!("REGISTERED {first}"));
        }
        t.control_no
    };
    let inner = Arc::clone(inner);
    thread::spawn(move || {
        let mut stream = stream;
        let mut line = Vec::new();
        let mut buf = [0u8; 256];
        loop {
            let n = match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            for &b in &buf[..n] {
                if b == b'\r' {
                    let text = String::from_utf8_lossy(&line).into_owned();
                    line.clear();
                    let mut g = lock(&inner);
                    if g.tnc[station].control_no == no {
                        g.command(station, &text);
                    }
                } else {
                    line.push(b);
                }
            }
        }
        let mut g = lock(&inner);
        if g.tnc[station].control_no == no && g.tnc[station].control.is_some() {
            g.host_gone(station);
        }
    });
}

fn take_data(stream: TcpStream, inner: &Arc<Mutex<Inner>>, station: usize) {
    let _ = stream.set_nonblocking(false);
    let Ok(writer) = stream.try_clone() else {
        return;
    };
    if let Some(old) = lock(inner).tnc[station].data.replace(writer) {
        let _ = old.shutdown(Shutdown::Both);
    }
    let inner = Arc::clone(inner);
    thread::spawn(move || {
        let mut stream = stream;
        let mut buf = [0u8; 128];
        while let Ok(n) = stream.read(&mut buf) {
            if n == 0 {
                break;
            }
            lock(&inner).data(station, &buf[..n]);
        }
    });
}
