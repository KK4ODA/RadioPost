// SPDX-License-Identifier: MIT OR Apache-2.0
//! The adapter's input and output: hosts on a TCP port (RP-4 §2), Mercury's control and data
//! ports, and a [`Bridge`] between them.
//!
//! Mercury gives its control port to the newest program that connects and closes the one before,
//! so the adapter connects only when a host has said `hello`, and one host has it at a time
//! (`shared` false): a second is turned away `busy` at its `hello` (RP-4 §3). When the host
//! leaves, the adapter ends what it left on the air, stops Mercury answering, and closes its
//! connections, so that another program may have Mercury.
//!
//! Each host has a thread that reads its lines; one thread, the station's, owns the bridge and
//! Mercury's two connections.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use rpmi::tcp::MAX_LINE;
use rpmi::wire::{Message, Request, VERSION, admit, error, line_of, parse_line, speaks};

use crate::bridge::{Bridge, Out};

/// Mercury's control port when none is named; its data port is the next.
pub const DEFAULT_MERCURY: &str = "127.0.0.1:8300";

/// How long a read of Mercury's control port waits before the host's lines are looked at again.
const POLL: Duration = Duration::from_millis(10);
/// How long a read of the data port waits when it has nothing.
const DATA_POLL: Duration = Duration::from_millis(1);
/// How long Mercury is given to answer a connection, or to take what is written.
const REACH: Duration = Duration::from_secs(5);
/// The longest the adapter keeps Mercury after its host has gone, ending what the host left.
const LEAVING: Duration = Duration::from_secs(90);
/// The longest line Mercury writes is far shorter.
const MODEM_LINE: usize = 512;

/// Where the adapter says what it saw of Mercury that its host is not told in full: why it could
/// not be reached, and why a connection to it ended.
pub type Note = Arc<dyn Fn(&str) + Send + Sync>;

/// A [`Note`] that says nothing.
#[must_use]
pub fn quiet() -> Note {
    Arc::new(|_| {})
}

/// What a host's reader thread hands the station's thread.
enum FromHost {
    Line(Vec<u8>),
    TooLong,
    Closed,
}

/// A host that has said `hello` and has the modem.
struct Arrival {
    stream: TcpStream,
    lines: Receiver<FromHost>,
}

/// Listens for hosts and serves them, one at a time, through Mercury's TNC interface at
/// `mercury`, its control port (`127.0.0.1:8300`; the data port is the next). Returns the address
/// taken; the adapter runs on threads of its own.
///
/// # Errors
///
/// The address could not be taken.
pub fn listen(addr: impl ToSocketAddrs, mercury: &str) -> io::Result<SocketAddr> {
    listen_noting(addr, mercury, quiet())
}

/// [`listen`], saying what it saw of Mercury on `note`.
///
/// # Errors
///
/// The address could not be taken.
pub fn listen_noting(
    addr: impl ToSocketAddrs,
    mercury: &str,
    note: Note,
) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(addr)?;
    let local = listener.local_addr()?;
    let occupied = Arc::new(AtomicBool::new(false));
    let (arrivals, arrived) = mpsc::channel();
    let mercury = mercury.to_owned();
    thread::spawn(move || Station::new(&mercury, &note).run(&arrived));
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let occupied = Arc::clone(&occupied);
            let arrivals = arrivals.clone();
            thread::spawn(move || lobby(&stream, &occupied, &arrivals));
        }
    });
    Ok(local)
}

fn write_line(mut stream: &TcpStream, m: &Message) -> io::Result<()> {
    let mut line = line_of(m);
    line.push('\n');
    stream.write_all(line.as_bytes())
}

fn read_line(reader: &mut BufReader<TcpStream>, buf: &mut Vec<u8>) -> FromHost {
    buf.clear();
    match reader
        .by_ref()
        .take(MAX_LINE as u64 + 1)
        .read_until(b'\n', buf)
    {
        Ok(0) | Err(_) => FromHost::Closed,
        Ok(_) if buf.len() > MAX_LINE => FromHost::TooLong,
        Ok(_) => {
            if buf.last() == Some(&b'\n') {
                buf.pop();
            }
            FromHost::Line(buf.clone())
        }
    }
}

/// One host's connection until its `hello` (RP-4 §3): what is not a `hello` is answered here,
/// and a `hello` takes the modem if no host has it. From then on its lines go to the station.
fn lobby(stream: &TcpStream, occupied: &AtomicBool, arrivals: &Sender<Arrival>) {
    let _ = stream.set_nodelay(true);
    let Ok(reading) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(reading);
    let mut buf = Vec::new();
    let lines = loop {
        let line = match read_line(&mut reader, &mut buf) {
            FromHost::Line(line) => line,
            FromHost::TooLong => {
                let refusal = error(None, "malformed", "a line longer than the adapter takes");
                let _ = write_line(stream, &refusal);
                let _ = stream.shutdown(Shutdown::Both);
                return;
            }
            FromHost::Closed => return,
        };
        let text = std::str::from_utf8(&line).unwrap_or("\u{0}");
        let refusal = match parse_line(text).and_then(|m| {
            let id = m.get("id").cloned();
            admit(&m, false).map(|request| (request, id))
        }) {
            Err(refusal) => refusal,
            Ok((Request::Hello { low, high, .. }, id)) => {
                let closing = if !speaks(low, high) {
                    let text = format!("this adapter speaks {VERSION}");
                    error(id.as_ref(), "unsupported-version", &text)
                } else if occupied.swap(true, Ordering::SeqCst) {
                    error(id.as_ref(), "busy", "another program has this modem")
                } else {
                    let (tx, rx) = mpsc::channel();
                    let _ = tx.send(FromHost::Line(line));
                    let Ok(writing) = stream.try_clone() else {
                        occupied.store(false, Ordering::SeqCst);
                        return;
                    };
                    let arrival = Arrival {
                        stream: writing,
                        lines: rx,
                    };
                    if arrivals.send(arrival).is_err() {
                        occupied.store(false, Ordering::SeqCst);
                        return;
                    }
                    break tx;
                };
                let _ = write_line(stream, &closing);
                let _ = stream.shutdown(Shutdown::Both);
                return;
            }
            Ok(_) => continue, // `admit` lets nothing but `hello` through before `hello`
        };
        if write_line(stream, &refusal).is_err() {
            return;
        }
    };
    // The modem is free once this host's connection has closed, whoever closed it: the station
    // ends what the host left on the air before it takes the next host, who waits in `arrivals`.
    // Freed here, not when the station next looks, so that a host that leaves and another that
    // comes at once are not taken for two at a time (RP-4 §3).
    loop {
        let said = read_line(&mut reader, &mut buf);
        let last = !matches!(said, FromHost::Line(_));
        if lines.send(said).is_err() || last {
            occupied.store(false, Ordering::SeqCst);
            return;
        }
    }
}

/// The adapter's two connections to Mercury.
struct Modem {
    control: TcpStream,
    data: TcpStream,
    partial: Vec<u8>,
}

impl Modem {
    /// Connects to the control port, then to the data port, the next one: Mercury closes the data
    /// connection of the program it takes the control port from.
    fn open(mercury: &str) -> Result<Modem, String> {
        let addr = mercury
            .to_socket_addrs()
            .map_err(|e| e.to_string())?
            .next()
            .ok_or("no such address")?;
        let port = addr.port().checked_add(1).ok_or("no port for the data")?;
        let data_addr = SocketAddr::new(addr.ip(), port);
        let control = TcpStream::connect_timeout(&addr, REACH)
            .map_err(|e| format!("its control port: {e}"))?;
        let data = TcpStream::connect_timeout(&data_addr, REACH)
            .map_err(|e| format!("its data port: {e}"))?;
        let set = |s: &TcpStream, wait: Duration| -> io::Result<()> {
            s.set_nodelay(true)?;
            s.set_read_timeout(Some(wait))?;
            s.set_write_timeout(Some(REACH))
        };
        set(&control, POLL).map_err(|e| e.to_string())?;
        set(&data, DATA_POLL).map_err(|e| e.to_string())?;
        Ok(Modem {
            control,
            data,
            partial: Vec::new(),
        })
    }

    /// What Mercury has written: the data port's bytes first, then the control port's lines. The
    /// two ports are read apart, and a session's last bytes are written before the line that
    /// says it ended.
    fn read(&mut self, bridge: &mut Bridge, outs: &mut Vec<Out>) -> Result<(), String> {
        let waiting = |e: &io::Error| {
            matches!(
                e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
            )
        };
        let mut buf = [0u8; 4096];
        let mut lines = Vec::new();
        match self.control.read(&mut buf) {
            Ok(0) => return Err("Mercury closed its control port".into()),
            Ok(n) => {
                for &b in &buf[..n] {
                    if b == b'\r' || b == b'\n' {
                        if !self.partial.is_empty() {
                            lines.push(String::from_utf8_lossy(&self.partial).into_owned());
                            self.partial.clear();
                        }
                    } else if self.partial.len() < MODEM_LINE {
                        self.partial.push(b);
                    }
                }
            }
            Err(e) if waiting(&e) => {}
            Err(e) => return Err(format!("a read of its control port failed: {e}")),
        }
        loop {
            match self.data.read(&mut buf) {
                Ok(0) => return Err("Mercury closed its data port".into()),
                Ok(n) => outs.extend(bridge.from_data(&buf[..n])),
                Err(e) if waiting(&e) => break,
                Err(e) => return Err(format!("a read of its data port failed: {e}")),
            }
        }
        for line in lines {
            outs.extend(bridge.from_modem(&line));
        }
        Ok(())
    }

    fn write(&mut self, out: &Out) -> Result<(), String> {
        match out {
            Out::Command(line) => {
                let mut bytes = line.clone().into_bytes();
                bytes.push(b'\r');
                self.control.write_all(&bytes)
            }
            Out::Data(bytes) => self.data.write_all(bytes),
            Out::Host(_) | Out::CloseHost => Ok(()),
        }
        .map_err(|e| format!("a write to Mercury failed: {e}"))
    }
}

/// The station: one bridge, Mercury's connections while they are needed, and the host that has
/// them.
struct Station<'a> {
    mercury: &'a str,
    note: &'a Note,
    start: Instant,
    modem: Option<Modem>,
    bridge: Bridge,
    host: Option<Arrival>,
    /// When the last host left, while Mercury is still kept for what it left on the air.
    left: Option<Instant>,
}

impl<'a> Station<'a> {
    fn new(mercury: &'a str, note: &'a Note) -> Station<'a> {
        Station {
            mercury,
            note,
            start: Instant::now(),
            modem: None,
            bridge: Bridge::new(),
            host: None,
            left: None,
        }
    }

    fn run(mut self, arrived: &Receiver<Arrival>) {
        loop {
            if self.host.is_none() {
                // with nothing of Mercury's to watch, the station waits for a host
                let next = if self.modem.is_some() {
                    match arrived.try_recv() {
                        Ok(arrival) => Some(arrival),
                        Err(TryRecvError::Empty) => None,
                        Err(TryRecvError::Disconnected) => return,
                    }
                } else {
                    match arrived.recv() {
                        Ok(arrival) => Some(arrival),
                        Err(_) => return,
                    }
                };
                if let Some(arrival) = next {
                    self.take(arrival);
                }
            }
            self.turn();
        }
    }

    /// A host that has said `hello`: Mercury is reached for it, unless the last host's teardown
    /// still holds it, and then this host joins that bridge.
    fn take(&mut self, arrival: Arrival) {
        if self.modem.is_none() {
            match Modem::open(self.mercury) {
                Ok(m) => {
                    self.modem = Some(m);
                    self.bridge = Bridge::new();
                }
                Err(why) => {
                    let why = format!("Mercury at {} could not be reached: {why}", self.mercury);
                    (self.note)(&why);
                    self.bridge = Bridge::without_modem(&why);
                }
            }
        }
        self.host = Some(arrival);
        self.left = None;
    }

    /// One turn: the time, what Mercury wrote, what the host wrote, and what the bridge said of
    /// them done.
    fn turn(&mut self) {
        let now = u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut outs = self.bridge.tick(now);
        let mut lost: Option<String> = None;
        match self.modem.as_mut() {
            Some(m) => lost = m.read(&mut self.bridge, &mut outs).err(),
            None => thread::sleep(POLL),
        }
        let mut gone = self.host_lines(&mut outs);
        if let Some(why) = &lost {
            // the host's link and call are over, and it is told so before it is closed
            outs = self.bridge.modem_lost(why);
        }
        for out in outs {
            match &out {
                Out::Host(m) => gone |= !self.tell(m),
                Out::Command(_) | Out::Data(_) => {
                    if lost.is_none()
                        && let Some(m) = self.modem.as_mut()
                        && let Err(why) = m.write(&out)
                    {
                        for told in self.bridge.modem_lost(&why) {
                            if let Out::Host(m) = &told {
                                self.tell(m);
                            }
                        }
                        lost = Some(why);
                    }
                }
                Out::CloseHost => gone = true,
            }
        }
        if let Some(why) = lost {
            (self.note)(&format!(
                "the connection to Mercury at {} ended: {why}; the program's connection is closed",
                self.mercury
            ));
            self.modem = None;
            gone = true;
        }
        if gone {
            self.release();
        }
        if self.host.is_none() {
            let overdue = self.left.is_some_and(|at| at.elapsed() > LEAVING);
            if self.modem.is_none() || self.bridge.settled() || overdue {
                // closing the control port is how another program is let have Mercury
                self.modem = None;
                self.bridge = Bridge::new();
            }
        }
    }

    /// The host's lines so far, given to the bridge: whether the host has gone.
    fn host_lines(&mut self, outs: &mut Vec<Out>) -> bool {
        let Some(h) = &self.host else {
            return false;
        };
        loop {
            match h.lines.try_recv() {
                Ok(FromHost::Line(line)) => {
                    let text = std::str::from_utf8(&line).unwrap_or("\u{0}");
                    outs.extend(self.bridge.from_host(text));
                }
                Ok(FromHost::TooLong) => {
                    outs.extend(
                        self.bridge
                            .from_host("a line longer than the adapter takes"),
                    );
                    outs.push(Out::CloseHost);
                }
                Ok(FromHost::Closed) | Err(TryRecvError::Disconnected) => return true,
                Err(TryRecvError::Empty) => return false,
            }
        }
    }

    /// A line to the host: whether it could be written.
    fn tell(&self, m: &Message) -> bool {
        self.host
            .as_ref()
            .is_none_or(|h| write_line(&h.stream, m).is_ok())
    }

    /// The host has gone (RP-4 §1.1): what it left on the air is ended, and another host may come.
    fn release(&mut self) {
        let Some(h) = self.host.take() else {
            return;
        };
        let _ = h.stream.shutdown(Shutdown::Both);
        let asked = self.bridge.host_left();
        if asked.contains(&Out::Command("ABORT".into())) {
            (self.note)("the program left with a call or link up: Mercury asked to abort it");
        }
        for out in asked {
            if let Some(m) = self.modem.as_mut()
                && m.write(&out).is_err()
            {
                self.modem = None;
            }
        }
        self.left = Some(Instant::now());
    }
}
