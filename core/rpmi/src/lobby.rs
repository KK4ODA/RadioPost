// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host side of an adapter that serves one host at a time (`shared` false, RP-4 §1.1, §3):
//! each connection is answered here until its `hello`; the first to say it while no host has the
//! modem is handed on as an [`Arrival`], and the others are turned away `busy`. Shared by the
//! adapters for modems that serve one program (Mercury, VARA, packet through AGWPE).

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use crate::tcp::MAX_LINE;
use crate::wire::{Message, Request, VERSION, admit, error, line_of, parse_line, speaks};

/// How long a `hello` that finds the modem taken waits for its host to be gone before it is
/// turned away `busy`. RP-4 §3 turns a second host away only while the first is still connected,
/// and a host that leaves and comes back at once (or a program that hands the modem to the next)
/// has closed its first connection before it says `hello` on the second, but the thread reading
/// the first may not have read that close yet. It always arrives first, so a moment is enough; a
/// second host that is really there waits that moment longer for its `busy`.
pub const LEAVING: Duration = Duration::from_secs(1);

/// Whether a host has the modem.
#[derive(Default)]
pub struct Occupied {
    taken: Mutex<bool>,
    freed: Condvar,
}

impl Occupied {
    /// Takes the modem for a host that has said `hello`, waiting up to [`LEAVING`] for the host
    /// that has it to be gone. False: it is still taken.
    pub fn take(&self) -> bool {
        let taken = self.taken.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut taken, _) = self
            .freed
            .wait_timeout_while(taken, LEAVING, |taken| *taken)
            .unwrap_or_else(PoisonError::into_inner);
        if *taken {
            return false;
        }
        *taken = true;
        true
    }

    /// The host that had the modem is gone.
    pub fn free(&self) {
        *self.taken.lock().unwrap_or_else(PoisonError::into_inner) = false;
        self.freed.notify_all();
    }
}

/// Listens for hosts at `addr`: each that says `hello` while no other has the modem arrives on
/// the receiver. Returns the address taken; the lobby runs on threads of its own.
///
/// # Errors
///
/// The address could not be taken.
pub fn hosts(addr: impl ToSocketAddrs) -> io::Result<(SocketAddr, Receiver<Arrival>)> {
    let listener = TcpListener::bind(addr)?;
    let local = listener.local_addr()?;
    let occupied = Arc::new(Occupied::default());
    let (arrivals, arrived) = mpsc::channel();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let occupied = Arc::clone(&occupied);
            let arrivals = arrivals.clone();
            thread::spawn(move || lobby(&stream, &occupied, &arrivals));
        }
    });
    Ok((local, arrived))
}

/// What a host's reader thread hands the station's thread.
pub enum FromHost {
    /// A line, without its LF.
    Line(Vec<u8>),
    /// A line longer than RP-4 allows.
    TooLong,
    /// The connection closed.
    Closed,
}

/// A host that has said `hello` and has the modem.
pub struct Arrival {
    /// Its connection, to write to.
    pub stream: TcpStream,
    /// Its lines, its `hello` first.
    pub lines: Receiver<FromHost>,
}

/// Writes one line to a host.
///
/// # Errors
///
/// The connection failed.
pub fn write_line(mut stream: &TcpStream, m: &Message) -> io::Result<()> {
    let mut line = line_of(m);
    line.push('\n');
    stream.write_all(line.as_bytes())
}

/// Reads one line from a host.
pub fn read_line(reader: &mut BufReader<TcpStream>, buf: &mut Vec<u8>) -> FromHost {
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
pub fn lobby(stream: &TcpStream, occupied: &Occupied, arrivals: &Sender<Arrival>) {
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
                } else if !occupied.take() {
                    error(id.as_ref(), "busy", "another program has this modem")
                } else {
                    let (tx, rx) = mpsc::channel();
                    let _ = tx.send(FromHost::Line(line));
                    let Ok(writing) = stream.try_clone() else {
                        occupied.free();
                        return;
                    };
                    let arrival = Arrival {
                        stream: writing,
                        lines: rx,
                    };
                    if arrivals.send(arrival).is_err() {
                        occupied.free();
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
            occupied.free();
            return;
        }
    }
}
