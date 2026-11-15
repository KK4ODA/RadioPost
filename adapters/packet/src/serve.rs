// SPDX-License-Identifier: MIT OR Apache-2.0
//! The adapter's input and output: hosts on a TCP port (RP-4 §2), a TNC's AGWPE port, and a
//! [`Bridge`] between them.
//!
//! One host has the adapter at a time (`shared` false): a second is turned away `busy` at its
//! `hello` (RP-4 §3, [`rpmi::lobby`]). The adapter connects to the TNC when a host says `hello`,
//! and when the host leaves it ends what the host left on the air, lets its callsigns go and
//! closes that connection, so the TNC's other programs have the callsigns back.
//!
//! One thread, the station's, owns the bridge and the TNC's connection, read through a
//! [`PolledStream`], never with a read timeout on its socket: on Windows such a read can fail with
//! os error 997 and lose what it had.

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use rpmi::lobby::{Arrival, FromHost, hosts, write_line};
use rpmi::polled::PolledStream;
use rpmi::wire::Message;

use crate::bridge::{Bridge, Out};
use crate::frame::Reader;

/// AGWPE's port when none is named, as Dire Wolf and AGWPE itself take it.
pub const DEFAULT_TNC: &str = "127.0.0.1:8000";

/// How long a read of the TNC waits before the host's lines are looked at again.
const POLL: Duration = Duration::from_millis(10);
/// How long the TNC is given to answer a connection, or to take what is written.
const REACH: Duration = Duration::from_secs(5);

/// Where the adapter says what it saw of the TNC that its host is not told in full.
pub type Note = Arc<dyn Fn(&str) + Send + Sync>;

/// A [`Note`] that says nothing.
#[must_use]
pub fn quiet() -> Note {
    Arc::new(|_| {})
}

/// Listens for hosts and serves them, one at a time, through the AGWPE port at `tnc`
/// (`127.0.0.1:8000`), on the TNC's port `port` (0 for the first). Returns the address taken; the
/// adapter runs on threads of its own.
///
/// # Errors
///
/// The address could not be taken.
pub fn listen(addr: impl ToSocketAddrs, tnc: &str, port: u8, note: Note) -> io::Result<SocketAddr> {
    let (local, arrived) = hosts(addr)?;
    let tnc = tnc.to_owned();
    thread::spawn(move || Station::new(&tnc, port, &note).run(&arrived));
    Ok(local)
}

/// The adapter's connection to the TNC.
struct Tnc {
    stream: PolledStream,
    reader: Reader,
}

impl Tnc {
    fn open(tnc: &str) -> Result<Tnc, String> {
        let addr = tnc
            .to_socket_addrs()
            .map_err(|e| e.to_string())?
            .next()
            .ok_or("no such address")?;
        let s = TcpStream::connect_timeout(&addr, REACH).map_err(|e| e.to_string())?;
        s.set_nodelay(true).map_err(|e| e.to_string())?;
        s.set_write_timeout(Some(REACH))
            .map_err(|e| e.to_string())?;
        Ok(Tnc {
            stream: PolledStream::new(&s, Some(POLL), "tnc").map_err(|e| e.to_string())?,
            reader: Reader::new(),
        })
    }

    /// What the TNC has written, given to the bridge.
    fn read(&mut self, bridge: &mut Bridge, outs: &mut Vec<Out>) -> Result<(), String> {
        let mut buf = [0u8; 4096];
        match self.stream.read(&mut buf) {
            Ok(0) => return Err("the TNC closed its AGWPE port".into()),
            Ok(n) => self.reader.push(&buf[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(format!("a read of the TNC failed: {e}")),
        }
        while let Some(f) = self.reader.next_frame()? {
            outs.extend(bridge.from_tnc(&f));
        }
        Ok(())
    }

    fn write(&mut self, out: &Out) -> Result<(), String> {
        match out {
            Out::Tnc(f) => self
                .stream
                .write_all(&f.encode())
                .map_err(|e| format!("a write to the TNC failed: {e}")),
            Out::Host(_) | Out::CloseHost => Ok(()),
        }
    }
}

/// The station: one bridge, the TNC's connection while a host has it, and that host.
struct Station<'a> {
    tnc_at: &'a str,
    port: u8,
    note: &'a Note,
    start: Instant,
    tnc: Option<Tnc>,
    bridge: Bridge,
    host: Option<Arrival>,
}

impl<'a> Station<'a> {
    fn new(tnc_at: &'a str, port: u8, note: &'a Note) -> Station<'a> {
        Station {
            tnc_at,
            port,
            note,
            start: Instant::now(),
            tnc: None,
            bridge: Bridge::new(port),
            host: None,
        }
    }

    fn run(mut self, arrived: &Receiver<Arrival>) {
        loop {
            if self.host.is_none() {
                match arrived.recv() {
                    Ok(arrival) => self.take(arrival),
                    Err(_) => return,
                }
            }
            self.turn();
        }
    }

    /// A host that has said `hello`: the TNC is reached for it.
    fn take(&mut self, arrival: Arrival) {
        match Tnc::open(self.tnc_at) {
            Ok(t) => {
                self.tnc = Some(t);
                self.bridge = Bridge::new(self.port);
            }
            Err(why) => {
                let why = format!(
                    "the TNC's AGWPE port at {} could not be reached: {why}",
                    self.tnc_at
                );
                (self.note)(&why);
                self.bridge = Bridge::without_tnc(self.port, &why);
            }
        }
        self.host = Some(arrival);
    }

    /// One turn: the time, what the TNC wrote, what the host wrote, and what the bridge said of
    /// them done.
    fn turn(&mut self) {
        let now = u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut outs = self.bridge.tick(now);
        let mut lost = if let Some(t) = self.tnc.as_mut() {
            t.read(&mut self.bridge, &mut outs).err()
        } else {
            thread::sleep(POLL);
            None
        };
        let mut gone = self.host_lines(&mut outs);
        if let Some(why) = &lost {
            outs = self.bridge.tnc_lost(why);
        }
        for out in outs {
            match &out {
                Out::Host(m) => gone |= !self.tell(m),
                Out::Tnc(_) => {
                    if lost.is_none()
                        && let Some(t) = self.tnc.as_mut()
                        && let Err(why) = t.write(&out)
                    {
                        for told in self.bridge.tnc_lost(&why) {
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
                "the connection to the TNC at {} ended: {why}; the program's connection is closed",
                self.tnc_at
            ));
            self.tnc = None;
            gone = true;
        }
        if gone {
            self.release();
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

    /// The host has gone (RP-4 §1.1): what it left on the air is ended, its callsigns let go, and
    /// the TNC's connection closed.
    fn release(&mut self) {
        let Some(h) = self.host.take() else {
            return;
        };
        let _ = h.stream.shutdown(Shutdown::Both);
        let asked = self.bridge.host_left();
        if let Some(mut t) = self.tnc.take() {
            for out in &asked {
                if t.write(out).is_err() {
                    break;
                }
            }
            // what was written is the TNC's before it sees the connection close
            let _ = t.stream.flush();
        }
        self.bridge = Bridge::new(self.port);
    }
}
