// SPDX-License-Identifier: MIT OR Apache-2.0
//! A stand-in for two ARDOP TNCs on one channel, each with a command port and the data port after
//! it, after ardopcf's *TCP Host Interface Commands* and what its source answers:
//!
//! * a command is answered by its echo (`MYCALL now …`, `LISTEN now TRUE`, `ARQCALL <call> <n>`,
//!   `DISCONNECT NOW TRUE`, `ABORT`) or `FAULT`; `MYAUX` refused clears the list;
//! * a call reaches the other TNC when it listens for that callsign (`MYCALL` or `MYAUX`): it
//!   says `PENDING`, `TARGET <call>` and `CONNECTED <caller> <bw>`, the caller `CONNECTED
//!   <called> <bw>`; with nobody there, `STATUS CONNECT TO <call> FAILED!`;
//! * bytes on the data port are blocks of a two-byte length and the data; the TNC says `BUFFER
//!   <n>` when it takes them in and when asked, and hands them over as blocks tagged `ARQ`, a
//!   frame's worth at a time, counted out of `BUFFER` once acknowledged;
//! * `DISCONNECT` ends the session, both hosts told `DISCONNECTED`; `ABORT` ends it saying nothing
//!   more, and the other station times out.
//!
//! What a host must not do is kept in [`Air::faults`].

#![allow(dead_code, reason = "each test file uses its own part")]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

const CALL_MS: u64 = 150;
const RETRY_MS: u64 = 1500;
const FRAME_MS: u64 = 80;
const TIMEOUT_MS: u64 = 2000;
const FRAME_BYTES: usize = 256;

#[derive(Default)]
struct Tnc {
    control: Option<TcpStream>,
    data: Option<TcpStream>,
    partial: Vec<u8>,
    mycall: String,
    aux: Vec<String>,
    listen: bool,
    bw: u32,
    /// The other station of the session, and when this station's next frame goes.
    session: Option<String>,
    calling: Option<(String, u64)>,
    buffer: Vec<u8>,
    /// Bytes sent and not yet acknowledged, counted in `BUFFER`.
    flying: usize,
    next_frame: u64,
    /// The other station stopped answering: the session times out then.
    silent_since: Option<u64>,
}

impl Tnc {
    fn say(&mut self, line: &str) {
        if let Some(c) = self.control.as_mut() {
            let _ = c.write_all(format!("{line}\r").as_bytes());
        }
    }

    fn give(&mut self, data: &[u8]) {
        if let Some(d) = self.data.as_mut() {
            let len = u16::try_from(data.len() + 3).unwrap_or(u16::MAX);
            let mut block = len.to_be_bytes().to_vec();
            block.extend_from_slice(b"ARQ");
            block.extend_from_slice(data);
            let _ = d.write_all(&block);
        }
    }

    fn answers_to(&self, call: &str) -> bool {
        self.listen
            && (self.mycall.eq_ignore_ascii_case(call)
                || self.aux.iter().any(|a| a.eq_ignore_ascii_case(call)))
    }

    fn held(&self) -> usize {
        self.buffer.len() + self.flying
    }
}

struct Inner {
    start: Instant,
    tnc: [Tnc; 2],
    faults: Vec<String>,
}

fn valid(call: &str) -> bool {
    let (base, ssid) = call.split_once('-').unwrap_or((call, "0"));
    (3..=7).contains(&base.len())
        && base.bytes().all(|b| b.is_ascii_alphanumeric())
        && (ssid.len() == 1 && ssid.bytes().all(|b| b.is_ascii_alphabetic())
            || ssid.parse::<u8>().is_ok_and(|n| n <= 15))
}

impl Inner {
    fn now(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn fault(&mut self, at: usize, what: &str) {
        self.faults.push(format!("station {at}: {what}"));
    }

    fn command(&mut self, at: usize, line: &str) {
        let now = self.now();
        let mut words = line.split_whitespace();
        let word = words.next().unwrap_or("").to_ascii_uppercase();
        let args: Vec<&str> = words.collect();
        let t = &mut self.tnc[at];
        match (word.as_str(), args.as_slice()) {
            ("VERSION", []) => t.say("VERSION ardopcf_stand-in"),
            ("PROTOCOLMODE", [m]) => t.say(&format!("PROTOCOLMODE now {m}")),
            ("LISTEN", [v]) => {
                t.listen = v.eq_ignore_ascii_case("TRUE");
                t.say(&format!(
                    "LISTEN now {}",
                    if t.listen { "TRUE" } else { "FALSE" }
                ));
            }
            ("MYCALL", [c]) if valid(c) => {
                t.mycall = c.to_ascii_uppercase();
                t.say(&format!("MYCALL now {}", t.mycall));
            }
            ("MYAUX", [list]) => {
                let calls: Vec<String> = list.split(',').map(str::to_ascii_uppercase).collect();
                if calls.iter().all(|c| valid(c)) {
                    t.say(&format!("MYAUX now {}", calls.join(",")));
                    t.aux = calls;
                } else {
                    t.aux.clear();
                    t.say("FAULT Syntax Err: MYAUX");
                }
            }
            ("ARQBW", [bw]) => {
                t.bw = bw
                    .trim_end_matches("MAX")
                    .trim_end_matches("FORCE")
                    .parse()
                    .unwrap_or(2000);
                t.say(&format!("ARQBW now {bw}"));
            }
            ("ARQCALL", [call, n]) => {
                if t.session.is_some() || t.calling.is_some() {
                    self.fault(at, "ARQCALL with a session or a call up");
                    self.tnc[at].say("FAULT not from state");
                    return;
                }
                if t.mycall.is_empty() {
                    t.say("FAULT MYCALL not set");
                    return;
                }
                t.say(&format!("ARQCALL {call} {n}"));
                t.calling = Some((call.to_ascii_uppercase(), now + CALL_MS));
            }
            ("BUFFER", []) => {
                let n = t.held();
                t.say(&format!("BUFFER {n}"));
            }
            ("DISCONNECT", []) => {
                if t.session.is_some() {
                    t.say("DISCONNECT NOW TRUE");
                    if t.held() > 0 {
                        self.fault(at, "DISCONNECT with bytes still to send");
                    }
                    let other = 1 - at;
                    for s in [at, other] {
                        let tnc = &mut self.tnc[s];
                        tnc.session = None;
                        tnc.buffer.clear();
                        tnc.flying = 0;
                        tnc.silent_since = None;
                        tnc.say("DISCONNECTED");
                    }
                } else {
                    t.say("DISCONNECT IGNORED");
                }
            }
            ("ABORT", []) => {
                t.say("ABORT");
                t.calling = None;
                t.buffer.clear();
                t.flying = 0;
                if t.session.take().is_some() {
                    self.tnc[1 - at].silent_since = Some(now);
                }
            }
            _ => {
                self.fault(at, &format!("a command ardopcf does not take: {line}"));
                self.tnc[at].say("FAULT unknown command");
            }
        }
    }

    fn data(&mut self, at: usize, bytes: &[u8]) {
        self.tnc[at].partial.extend_from_slice(bytes);
        loop {
            let t = &mut self.tnc[at];
            if t.partial.len() < 2 {
                return;
            }
            let len = usize::from(u16::from_be_bytes([t.partial[0], t.partial[1]]));
            if t.partial.len() < 2 + len {
                return;
            }
            let block: Vec<u8> = t.partial.drain(..2 + len).skip(2).collect();
            if t.session.is_none() {
                self.fault(at, "bytes on the data port with no session up");
                return;
            }
            let t = &mut self.tnc[at];
            t.buffer.extend_from_slice(&block);
            let n = t.held();
            t.say(&format!("BUFFER {n}"));
        }
    }

    fn tick(&mut self) {
        let now = self.now();
        for at in 0..2 {
            let other = 1 - at;
            // a call heard, or given up
            if let Some((call, when)) = self.tnc[at].calling.clone()
                && now >= when
            {
                if self.tnc[other].answers_to(&call) && self.tnc[other].session.is_none() {
                    let caller = self.tnc[at].mycall.clone();
                    let bw = self.tnc[at].bw.min(self.tnc[other].bw).max(200);
                    let them = &mut self.tnc[other];
                    them.say("PENDING");
                    them.say(&format!("TARGET {call}"));
                    them.say(&format!("CONNECTED {caller} {bw}"));
                    them.session = Some(caller);
                    let me = &mut self.tnc[at];
                    me.calling = None;
                    me.say(&format!("CONNECTED {call} {bw}"));
                    me.session = Some(call);
                } else if now >= when + RETRY_MS {
                    let me = &mut self.tnc[at];
                    me.calling = None;
                    me.say(&format!("STATUS CONNECT TO {call} FAILED!"));
                    me.say("NEWSTATE DISC ");
                }
            }
            // a session whose other end has gone quiet times out
            if let Some(since) = self.tnc[at].silent_since
                && now >= since + TIMEOUT_MS
            {
                let me = &mut self.tnc[at];
                me.silent_since = None;
                me.session = None;
                me.buffer.clear();
                me.flying = 0;
                me.say("STATUS ARQ Timeout from Protocol State:  IRS");
                me.say("DISCONNECTED");
            }
            // a frame's worth of data across, acknowledged a frame later
            if self.tnc[at].session.is_some()
                && self.tnc[other].session.is_some()
                && now >= self.tnc[at].next_frame
            {
                self.tnc[at].next_frame = now + FRAME_MS;
                let acked = std::mem::take(&mut self.tnc[at].flying);
                if acked > 0 {
                    let n = self.tnc[at].held();
                    self.tnc[at].say(&format!("BUFFER {n}"));
                }
                let n = self.tnc[at].buffer.len().min(FRAME_BYTES);
                if n > 0 {
                    let frame: Vec<u8> = self.tnc[at].buffer.drain(..n).collect();
                    self.tnc[at].flying = n;
                    self.tnc[other].give(&frame);
                }
            }
        }
    }
}

/// Two stand-in ARDOP TNCs on one channel.
#[derive(Clone)]
pub struct Air {
    inner: Arc<Mutex<Inner>>,
}

fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A command port and the data port after it, both free.
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
    /// The channel and its two TNCs: each one's command port.
    pub fn new() -> (Air, [SocketAddr; 2]) {
        let air = Air {
            inner: Arc::new(Mutex::new(Inner {
                start: Instant::now(),
                tnc: [
                    Tnc {
                        bw: 2000,
                        ..Tnc::default()
                    },
                    Tnc {
                        bw: 2000,
                        ..Tnc::default()
                    },
                ],
                faults: Vec::new(),
            })),
        };
        let mut at = Vec::new();
        for station in 0..2 {
            let (control, data) = two_ports();
            at.push(control.local_addr().expect("bound"));
            let inner = Arc::clone(&air.inner);
            thread::spawn(move || {
                for s in control.incoming().flatten() {
                    let inner = Arc::clone(&inner);
                    thread::spawn(move || read_control(s, &inner, station));
                }
            });
            let inner = Arc::clone(&air.inner);
            thread::spawn(move || {
                for s in data.incoming().flatten() {
                    let inner = Arc::clone(&inner);
                    thread::spawn(move || read_data(s, &inner, station));
                }
            });
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

    /// What a host did that a real ARDOP TNC would have answered badly.
    pub fn faults(&self) -> Vec<String> {
        lock(&self.inner).faults.clone()
    }

    /// Whether the TNC answers calls.
    pub fn listening(&self, station: usize) -> bool {
        lock(&self.inner).tnc[station].listen
    }
}

fn read_control(stream: TcpStream, inner: &Arc<Mutex<Inner>>, station: usize) {
    let Ok(writer) = stream.try_clone() else {
        return;
    };
    lock(inner).tnc[station].control = Some(writer);
    let mut s = stream;
    let mut line = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = match s.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        for &b in &buf[..n] {
            if b == b'\r' {
                let text = String::from_utf8_lossy(&line).into_owned();
                line.clear();
                lock(inner).command(station, &text);
            } else {
                line.push(b);
            }
        }
    }
}

fn read_data(stream: TcpStream, inner: &Arc<Mutex<Inner>>, station: usize) {
    let Ok(writer) = stream.try_clone() else {
        return;
    };
    lock(inner).tnc[station].data = Some(writer);
    let mut s = stream;
    let mut buf = [0u8; 4096];
    loop {
        match s.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => lock(inner).data(station, &buf[..n]),
        }
    }
}
