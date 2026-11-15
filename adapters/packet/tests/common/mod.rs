// SPDX-License-Identifier: MIT OR Apache-2.0
//! A stand-in for two TNCs on one channel, each with an AGWPE port, after SV2AGW's *AGWPE TCP/IP
//! API* and what Dire Wolf's `server.c` answers where the document is silent:
//!
//! * `R` is answered with the version, `G` with one port, `X` with one byte, 1 (`x` has none);
//! * a connection to a callsign a program registered at the other TNC comes up after a frame's
//!   time: the caller's program hears `C` `*** CONNECTED With Station <called>`, the called one's
//!   `C` `*** CONNECTED To Station <caller>`; a connection to a callsign nobody registered there
//!   ends after the retries with `d` `*** DISCONNECTED RETRYOUT With <called>`;
//! * `D` reaches the other end after a frame's time and is acknowledged a little after; `Y` counts
//!   the frames sent on a link and not yet acknowledged;
//! * `d` ends the link: the asker hears `d` `*** DISCONNECTED From Station <other>` at once, the
//!   other end after a frame's time; a program that closes its connection ends its links and lets
//!   its callsigns go.
//!
//! What a program must not do to a TNC is kept in [`Air::faults`].

#![allow(dead_code, reason = "each test file uses its own part")]

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use radiopost_packet::frame::{Frame, Reader};

const CONNECT_MS: u64 = 120;
const RETRY_MS: u64 = 1500;
const FRAME_MS: u64 = 60;
const ACK_MS: u64 = 60;

struct Client {
    id: u64,
    stream: TcpStream,
    registered: Vec<String>,
}

#[derive(Clone, Debug)]
struct End {
    client: u64,
    local: String,
    remote: String,
    flying: usize,
}

#[derive(Debug)]
enum Ev {
    /// A connection asked for by `client` at `station`, from `local` to `remote`.
    Connect {
        station: usize,
        client: u64,
        local: String,
        remote: String,
    },
    /// Data from `local` reaches `remote` at the other station.
    Arrive {
        station: usize,
        local: String,
        remote: String,
        data: Vec<u8>,
    },
    /// The frame from `local` to `remote`, sent at `station`, acknowledged.
    Ack {
        station: usize,
        local: String,
        remote: String,
    },
    /// The other end of the link from `local` to `remote` at `station` hears it ended.
    Ended {
        station: usize,
        local: String,
        remote: String,
    },
}

#[derive(Default)]
struct Tnc {
    clients: Vec<Client>,
    links: Vec<End>,
}

struct Inner {
    start: Instant,
    next_client: u64,
    tnc: [Tnc; 2],
    timers: Vec<(u64, Ev)>,
    faults: Vec<String>,
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl Inner {
    fn now(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn after(&mut self, ms: u64, ev: Ev) {
        let when = self.now() + ms;
        self.timers.push((when, ev));
    }

    fn fault(&mut self, station: usize, what: &str) {
        self.faults.push(format!("station {station}: {what}"));
    }

    fn send(&mut self, station: usize, client: u64, f: &Frame) {
        if let Some(c) = self.tnc[station]
            .clients
            .iter_mut()
            .find(|c| c.id == client)
        {
            let _ = c.stream.write_all(&f.encode());
        }
    }

    fn text(kind: u8, from: &str, to: &str, text: &str) -> Frame {
        let mut data = text.as_bytes().to_vec();
        data.extend_from_slice(b"\r\0");
        Frame::new(0, kind).calls(from, to).data(&data)
    }

    fn end_of(&self, station: usize, local: &str, remote: &str) -> Option<usize> {
        self.tnc[station]
            .links
            .iter()
            .position(|l| same(&l.local, local) && same(&l.remote, remote))
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one arm a kind of frame, as the API lists them"
    )]
    fn on_frame(&mut self, station: usize, client: u64, f: &Frame) {
        match f.kind {
            b'R' => {
                let mut data = 2005u32.to_le_bytes().to_vec();
                data.extend_from_slice(&127u32.to_le_bytes());
                self.send(station, client, &Frame::new(0, b'R').data(&data));
            }
            b'G' => {
                let g = Frame::new(0, b'G').data(b"1;Port1 stand-in TNC;\0");
                self.send(station, client, &g);
            }
            b'X' => {
                if let Some(c) = self.tnc[station]
                    .clients
                    .iter_mut()
                    .find(|c| c.id == client)
                {
                    c.registered.push(f.from.clone());
                }
                let x = Frame::new(f.port, b'X').calls(&f.from, "").data(&[1]);
                self.send(station, client, &x);
            }
            b'x' => {
                if let Some(c) = self.tnc[station]
                    .clients
                    .iter_mut()
                    .find(|c| c.id == client)
                {
                    c.registered.retain(|r| !same(r, &f.from));
                }
            }
            b'C' => {
                if self.end_of(station, &f.from, &f.to).is_some() {
                    self.fault(station, "a connection asked for while that link is up");
                    return;
                }
                self.after(
                    CONNECT_MS,
                    Ev::Connect {
                        station,
                        client,
                        local: f.from.clone(),
                        remote: f.to.clone(),
                    },
                );
            }
            b'D' => {
                let Some(i) = self.end_of(station, &f.from, &f.to) else {
                    self.fault(station, "data on no link");
                    return;
                };
                if f.data.len() > 256 {
                    self.fault(station, "an I frame longer than PACLEN");
                }
                self.tnc[station].links[i].flying += 1;
                self.after(
                    FRAME_MS,
                    Ev::Arrive {
                        station,
                        local: f.from.clone(),
                        remote: f.to.clone(),
                        data: f.data.clone(),
                    },
                );
            }
            b'Y' => {
                let n = self
                    .end_of(station, &f.from, &f.to)
                    .map_or(0, |i| self.tnc[station].links[i].flying);
                let n = u32::try_from(n).unwrap_or(u32::MAX);
                let y = Frame::new(f.port, b'Y')
                    .calls(&f.from, &f.to)
                    .data(&n.to_le_bytes());
                self.send(station, client, &y);
            }
            b'd' => {
                // a connection still being asked for is given up
                let pending = self.timers.iter().position(|(_, e)| {
                    matches!(e, Ev::Connect { station: s, local, remote, .. }
                        if *s == station && same(local, &f.from) && same(remote, &f.to))
                });
                if let Some(p) = pending {
                    self.timers.remove(p);
                    let d = Self::text(
                        b'd',
                        &f.to,
                        &f.from,
                        &format!("*** DISCONNECTED From Station {}", f.to),
                    );
                    self.send(station, client, &d);
                    return;
                }
                let Some(i) = self.end_of(station, &f.from, &f.to) else {
                    return;
                };
                let end = self.tnc[station].links.remove(i);
                let d = Self::text(
                    b'd',
                    &end.remote,
                    &end.local,
                    &format!("*** DISCONNECTED From Station {}", end.remote),
                );
                self.send(station, client, &d);
                self.after(
                    FRAME_MS,
                    Ev::Ended {
                        station: 1 - station,
                        local: end.remote,
                        remote: end.local,
                    },
                );
            }
            k => self.fault(
                station,
                &format!("a frame of kind {:?} the stand-in does not know", k as char),
            ),
        }
    }

    #[allow(clippy::too_many_lines, reason = "one arm an event on the channel")]
    fn fire(&mut self, ev: Ev) {
        match ev {
            Ev::Connect {
                station,
                client,
                local,
                remote,
            } => {
                let other = 1 - station;
                let callee = self.tnc[other]
                    .clients
                    .iter()
                    .find(|c| c.registered.iter().any(|r| same(r, &remote)))
                    .map(|c| c.id);
                let Some(callee) = callee else {
                    let left = RETRY_MS.saturating_sub(CONNECT_MS);
                    let d = Self::text(
                        b'd',
                        &remote,
                        &local,
                        &format!("*** DISCONNECTED RETRYOUT With {remote}"),
                    );
                    let stream = self.tnc[station]
                        .clients
                        .iter()
                        .find(|c| c.id == client)
                        .and_then(|c| c.stream.try_clone().ok());
                    thread::spawn(move || {
                        thread::sleep(Duration::from_millis(left));
                        if let Some(mut s) = stream {
                            let _ = s.write_all(&d.encode());
                        }
                    });
                    return;
                };
                self.tnc[station].links.push(End {
                    client,
                    local: local.clone(),
                    remote: remote.clone(),
                    flying: 0,
                });
                self.tnc[other].links.push(End {
                    client: callee,
                    local: remote.clone(),
                    remote: local.clone(),
                    flying: 0,
                });
                let mine = Self::text(
                    b'C',
                    &remote,
                    &local,
                    &format!("*** CONNECTED With Station {remote}"),
                );
                self.send(station, client, &mine);
                let theirs = Self::text(
                    b'C',
                    &local,
                    &remote,
                    &format!("*** CONNECTED To Station {local}"),
                );
                self.send(other, callee, &theirs);
            }
            Ev::Arrive {
                station,
                local,
                remote,
                data,
            } => {
                let other = 1 - station;
                if let Some(i) = self.end_of(other, &remote, &local) {
                    let c = self.tnc[other].links[i].client;
                    let d = Frame::new(0, b'D').calls(&local, &remote).data(&data);
                    self.send(other, c, &d);
                    self.after(
                        ACK_MS,
                        Ev::Ack {
                            station,
                            local,
                            remote,
                        },
                    );
                }
            }
            Ev::Ack {
                station,
                local,
                remote,
            } => {
                if let Some(i) = self.end_of(station, &local, &remote) {
                    let l = &mut self.tnc[station].links[i];
                    l.flying = l.flying.saturating_sub(1);
                }
            }
            Ev::Ended {
                station,
                local,
                remote,
            } => {
                if let Some(i) = self.end_of(station, &local, &remote) {
                    let end = self.tnc[station].links.remove(i);
                    let d = Self::text(
                        b'd',
                        &remote,
                        &local,
                        &format!("*** DISCONNECTED From Station {remote}"),
                    );
                    self.send(station, end.client, &d);
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
                due.push(self.timers.remove(i).1);
            } else {
                i += 1;
            }
        }
        for ev in due {
            self.fire(ev);
        }
    }

    /// A program closed its connection: its links end, its callsigns go.
    fn gone(&mut self, station: usize, client: u64) {
        self.tnc[station].clients.retain(|c| c.id != client);
        let ends: Vec<End> = self.tnc[station]
            .links
            .iter()
            .filter(|l| l.client == client)
            .cloned()
            .collect();
        self.tnc[station].links.retain(|l| l.client != client);
        for e in ends {
            self.after(
                FRAME_MS,
                Ev::Ended {
                    station: 1 - station,
                    local: e.remote,
                    remote: e.local,
                },
            );
        }
    }
}

/// Two stand-in TNCs on one channel.
#[derive(Clone)]
pub struct Air {
    inner: Arc<Mutex<Inner>>,
}

fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Air {
    /// The channel and its two TNCs: each one's AGWPE port.
    pub fn new() -> (Air, [SocketAddr; 2]) {
        let air = Air {
            inner: Arc::new(Mutex::new(Inner {
                start: Instant::now(),
                next_client: 0,
                tnc: [Tnc::default(), Tnc::default()],
                timers: Vec::new(),
                faults: Vec::new(),
            })),
        };
        let mut at = Vec::new();
        for station in 0..2 {
            let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
            at.push(listener.local_addr().expect("bound"));
            let inner = Arc::clone(&air.inner);
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let inner = Arc::clone(&inner);
                    thread::spawn(move || serve(stream, &inner, station));
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

    /// What a program did that a TNC would have answered badly.
    pub fn faults(&self) -> Vec<String> {
        lock(&self.inner).faults.clone()
    }

    /// The callsigns registered at a station's TNC.
    pub fn registered(&self, station: usize) -> Vec<String> {
        lock(&self.inner).tnc[station]
            .clients
            .iter()
            .flat_map(|c| c.registered.clone())
            .collect()
    }

    /// How many programs are connected to a station's TNC.
    pub fn programs(&self, station: usize) -> usize {
        lock(&self.inner).tnc[station].clients.len()
    }
}

fn serve(stream: TcpStream, inner: &Arc<Mutex<Inner>>, station: usize) {
    let Ok(writer) = stream.try_clone() else {
        return;
    };
    let id = {
        let mut g = lock(inner);
        g.next_client += 1;
        let id = g.next_client;
        g.tnc[station].clients.push(Client {
            id,
            stream: writer,
            registered: Vec::new(),
        });
        id
    };
    let mut reader = Reader::new();
    let mut s = stream;
    let mut buf = [0u8; 4096];
    loop {
        match s.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => reader.push(&buf[..n]),
        }
        loop {
            match reader.next_frame() {
                Ok(Some(f)) => lock(inner).on_frame(station, id, &f),
                Ok(None) => break,
                Err(e) => {
                    lock(inner).fault(station, &e);
                    let _ = s.shutdown(Shutdown::Both);
                    break;
                }
            }
        }
    }
    lock(inner).gone(station, id);
}
