// SPDX-License-Identifier: MIT OR Apache-2.0
//! RPMI over TCP (RP-4 §2): a [`Channel`]'s stations served on loopback ports, one host a
//! connection, and a host's connection to any adapter.
//!
//! The server keeps the channel behind one lock and gives each connection a reader thread and a
//! writer thread, so a host that stops reading holds up nobody else. A call nobody answers is
//! given up after the server's call timeout.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread;
use std::time::{Duration, Instant};

use crate::channel::{Channel, Out};
use crate::wire::{Message, line_of};

/// The most a line may hold: a `send` of `max_queue` bytes is well under it.
pub const MAX_LINE: usize = 1 << 20;

/// What one connection is given to write.
enum ToWrite {
    Line(String),
    Close,
}

struct Inner {
    channel: Channel,
    writers: HashMap<(usize, String), Sender<ToWrite>>,
    streams: HashMap<(usize, String), TcpStream>,
    calls: Vec<(u64, Instant)>,
    connections: u64,
}

impl Inner {
    /// Sends what the channel said to the connections it was said on.
    fn route(&mut self) {
        for out in self.channel.take() {
            match out {
                Out::Line {
                    station,
                    host,
                    message,
                } => {
                    if let Some(w) = self.writers.get(&(station, host)) {
                        let _ = w.send(ToWrite::Line(line_of(&message)));
                    }
                }
                Out::Close { station, host } => {
                    let key = (station, host);
                    if let Some(w) = self.writers.remove(&key) {
                        let _ = w.send(ToWrite::Close);
                    }
                    self.streams.remove(&key);
                }
            }
        }
        let now = Instant::now();
        let pending = self.channel.pending_calls();
        self.calls.retain(|(serial, _)| pending.contains(serial));
        for serial in pending {
            if !self.calls.iter().any(|(s, _)| *s == serial) {
                self.calls.push((serial, now));
            }
        }
    }
}

/// A channel's stations served over TCP: the loopback adapter.
#[derive(Clone)]
pub struct Server {
    inner: Arc<Mutex<Inner>>,
}

fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    inner
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Server {
    /// Serves `channel`. A call nobody answers fails `timeout` after `call_timeout`.
    #[must_use]
    pub fn new(channel: Channel, call_timeout: Duration) -> Server {
        let inner = Arc::new(Mutex::new(Inner {
            channel,
            writers: HashMap::new(),
            streams: HashMap::new(),
            calls: Vec::new(),
            connections: 0,
        }));
        let weak = Arc::downgrade(&inner);
        thread::spawn(move || give_up_late_calls(&weak, call_timeout));
        Server { inner }
    }

    /// Listens for hosts of one station; returns the address taken (port 0 picks a free one).
    ///
    /// # Errors
    ///
    /// The address could not be taken.
    pub fn listen(&self, station: usize, addr: impl ToSocketAddrs) -> io::Result<SocketAddr> {
        let listener = TcpListener::bind(addr)?;
        let local = listener.local_addr()?;
        let inner = Arc::downgrade(&self.inner);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Some(strong) = inner.upgrade() else {
                    return;
                };
                if let Ok(stream) = stream {
                    serve(&strong, station, stream);
                }
            }
        });
        Ok(local)
    }

    /// Runs `f` on the channel, then sends what it said: how a test drives the modem.
    pub fn with_channel<T>(&self, f: impl FnOnce(&mut Channel) -> T) -> T {
        let mut inner = lock(&self.inner);
        let t = f(&mut inner.channel);
        inner.route();
        t
    }

    /// A host inside this process on one of the channel's stations, as if it had connected: the
    /// conformance tester's own side of the air.
    #[must_use]
    pub fn attach(&self, station: usize, name: &str) -> Local {
        let (tx, rx) = channel::<ToWrite>();
        let mut inner = lock(&self.inner);
        inner.channel.open(station, name);
        inner.writers.insert((station, name.to_owned()), tx);
        Local {
            server: self.clone(),
            station,
            name: name.to_owned(),
            rx,
        }
    }
}

/// A host inside the adapter's own process ([`Server::attach`]).
pub struct Local {
    server: Server,
    station: usize,
    name: String,
    rx: std::sync::mpsc::Receiver<ToWrite>,
}

impl Local {
    /// Sends a request, as a host's line.
    pub fn send(&self, m: &Message) {
        let mut inner = lock(&self.server.inner);
        inner.channel.request(self.station, &self.name, m);
        inner.route();
    }

    /// The adapter's next message to this host, waiting at most `timeout`; `None` when none came.
    #[must_use]
    pub fn next(&self, timeout: Duration) -> Option<Message> {
        match self.rx.recv_timeout(timeout) {
            Ok(ToWrite::Line(line)) => match serde_json::from_str(&line) {
                Ok(serde_json::Value::Object(m)) => Some(m),
                _ => None,
            },
            Ok(ToWrite::Close) | Err(_) => None,
        }
    }
}

impl Drop for Local {
    fn drop(&mut self) {
        let mut inner = lock(&self.server.inner);
        inner.writers.remove(&(self.station, self.name.clone()));
        inner.channel.close(self.station, &self.name);
        inner.route();
    }
}

fn give_up_late_calls(inner: &Weak<Mutex<Inner>>, timeout: Duration) {
    loop {
        thread::sleep(Duration::from_millis(50).min(timeout));
        let Some(inner) = inner.upgrade() else {
            return;
        };
        let mut inner = lock(&inner);
        let late: Vec<u64> = inner
            .calls
            .iter()
            .filter(|(_, t)| t.elapsed() >= timeout)
            .map(|(s, _)| *s)
            .collect();
        for serial in late {
            inner.channel.give_up_call(serial);
        }
        inner.route();
    }
}

fn serve(inner: &Arc<Mutex<Inner>>, station: usize, stream: TcpStream) {
    let Ok(writer) = stream.try_clone() else {
        return;
    };
    let Ok(closer) = stream.try_clone() else {
        return;
    };
    let (tx, rx) = channel::<ToWrite>();
    let name = {
        let mut g = lock(inner);
        g.connections += 1;
        let name = format!("connection {}", g.connections);
        g.channel.open(station, &name);
        g.writers.insert((station, name.clone()), tx);
        g.streams.insert((station, name.clone()), closer);
        name
    };
    thread::spawn(move || {
        let mut writer = writer;
        for w in rx {
            match w {
                ToWrite::Line(line) => {
                    if writer
                        .write_all(line.as_bytes())
                        .and_then(|()| writer.write_all(b"\n"))
                        .is_err()
                    {
                        return;
                    }
                }
                ToWrite::Close => {
                    let _ = writer.shutdown(Shutdown::Both);
                    return;
                }
            }
        }
    });
    let inner = Arc::clone(inner);
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            let read = (&mut reader)
                .take(MAX_LINE as u64 + 1)
                .read_until(b'\n', &mut buf);
            let mut g = lock(&inner);
            let key = (station, name.clone());
            if !g.writers.contains_key(&key) {
                return; // the adapter closed this connection
            }
            match read {
                Ok(0) | Err(_) => {
                    g.channel.close(station, &name);
                    g.writers.remove(&key);
                    g.streams.remove(&key);
                    g.route();
                    return;
                }
                Ok(_) if buf.len() > MAX_LINE => {
                    g.channel
                        .refuse(station, &name, "a line longer than the adapter takes");
                    g.route();
                    if let Some(s) = g.streams.get(&key) {
                        let _ = s.shutdown(Shutdown::Read);
                    }
                }
                Ok(_) => {
                    if buf.last() == Some(&b'\n') {
                        buf.pop();
                    }
                    g.channel.request_bytes(station, &name, &buf);
                    g.route();
                }
            }
        }
    });
}

/// A host's connection to an adapter.
pub struct Connection {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Connection {
    /// Connects to an adapter.
    ///
    /// # Errors
    ///
    /// The adapter could not be reached.
    pub fn open(addr: impl ToSocketAddrs) -> io::Result<Connection> {
        let stream = TcpStream::connect(addr)?;
        stream.set_nodelay(true)?;
        Ok(Connection {
            writer: stream.try_clone()?,
            reader: BufReader::new(stream),
        })
    }

    /// Waits at most this long for each line; `None` waits for ever.
    ///
    /// # Errors
    ///
    /// The socket refused the setting.
    pub fn set_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.writer.set_read_timeout(timeout)
    }

    /// Writes one request.
    ///
    /// # Errors
    ///
    /// The connection failed.
    pub fn send(&mut self, m: &Message) -> io::Result<()> {
        let mut line = line_of(m);
        line.push('\n');
        self.writer.write_all(line.as_bytes())
    }

    /// Writes one line as it is, JSON or not, and its LF: how a test sends what a host should not.
    ///
    /// # Errors
    ///
    /// The connection failed.
    pub fn send_line(&mut self, line: &str) -> io::Result<()> {
        self.writer.write_all(line.as_bytes())?;
        self.writer.write_all(b"\n")
    }

    /// The adapter's next message; `None` when it has closed the connection.
    ///
    /// # Errors
    ///
    /// The connection failed, or the adapter wrote a line that is not a JSON object.
    pub fn read_message(&mut self) -> io::Result<Option<Message>> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        match serde_json::from_str(&line) {
            Ok(serde_json::Value::Object(m)) => Ok(Some(m)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the adapter wrote a line that is not a JSON object",
            )),
        }
    }

    /// Closes the connection: the adapter lets the host's callsigns go (RP-4 §1.1).
    pub fn close(self) {
        let _ = self.writer.shutdown(Shutdown::Both);
    }
}
