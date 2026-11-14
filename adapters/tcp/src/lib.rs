// SPDX-License-Identifier: MIT OR Apache-2.0
//! The TCP adapter (RP-4 §9.5): RPMI to its host, and links that are TCP connections to other
//! adapters, TLS on the Internet and plain on an amateur mesh, opened with one line each way.
//!
//! One host has the adapter (`shared` false): a second is turned away at its `hello`. It keeps one
//! link or call at a time. Each link has a thread that writes what the host sends and reads what
//! arrives; the host's state is behind one lock.
//!
//! A link's socket is read through an [`rpmi::polled::PolledStream`], never with a read timeout
//! on the socket: on Windows such a read can fail with os error 997 and lose what it had.

pub mod tls;

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use rpmi::polled::{Closer, PolledStream};
use rpmi::wire::{Caps, Message, Request, admit, answer, answer_with, error, line_of, parse_line};
use rustls::{ClientConfig, ClientConnection, ServerConfig, ServerConnection, StreamOwned};
use serde_json::{Value, json};

/// The port an adapter takes calls on (RP-4 §9.5).
pub const DEFAULT_PORT: u16 = 8774;
/// The longest opening line (RP-4 §9.5).
pub const MAX_OPENING: usize = 200;
/// How long a caller waits for the connection, and for the answering adapter's line.
pub const ANSWER_WAIT: Duration = Duration::from_secs(30);
/// The most a host's line may hold.
const MAX_LINE: usize = 1 << 20;
/// How long a link's thread waits for bytes before it looks at what the host sent.
const POLL: Duration = Duration::from_millis(20);

/// How the links are carried.
pub enum Security {
    /// Plain TCP: an amateur mesh network, which is on the air (RP-0 §5).
    Plain,
    /// TLS: the Internet.
    Tls {
        /// For calls taken: this adapter's certificate. None: it takes no calls.
        server: Option<Arc<ServerConfig>>,
        /// For calls made: how the other adapter's certificate is checked.
        client: Arc<ClientConfig>,
    },
}

/// The adapter's settings.
pub struct Config {
    /// TLS or plain.
    pub security: Security,
    /// Names the operator maps to an address: `N1CALL-10` to `gw.example.net:8774`.
    pub peers: HashMap<String, String>,
    /// The name `hello` gives the modem.
    pub name: String,
}

/// What the adapter can do (RP-4 §3).
#[must_use]
pub fn caps() -> Caps {
    Caps {
        bandwidths: Vec::new(),
        max_links: 1,
        delivery: false,
        broadcast: false,
        quality: false,
        busy: false,
        regulatory: false,
        max_queue: 1 << 20,
        shared: false,
    }
}

/// A link's connection, plain or TLS.
enum Stream {
    Plain(PolledStream),
    Client(Box<StreamOwned<ClientConnection, PolledStream>>),
    Server(Box<StreamOwned<ServerConnection, PolledStream>>),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Client(s) => s.read(buf),
            Stream::Server(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Client(s) => s.write(buf),
            Stream::Server(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Client(s) => s.flush(),
            Stream::Server(s) => s.flush(),
        }
    }
}

impl Stream {
    /// Closes in order: TLS says so first, then the connection's sending half shuts.
    fn close(&mut self) {
        let raw = match self {
            Stream::Plain(s) => s,
            Stream::Client(s) => {
                s.conn.send_close_notify();
                let _ = s.flush();
                &s.sock
            }
            Stream::Server(s) => {
                s.conn.send_close_notify();
                let _ = s.flush();
                &s.sock
            }
        };
        let _ = raw.shutdown_write();
    }

    fn raw(&self) -> &PolledStream {
        match self {
            Stream::Plain(s) => s,
            Stream::Client(s) => &s.sock,
            Stream::Server(s) => &s.sock,
        }
    }
}

/// One line of the opening (RP-4 §9.5), read a byte at a time so nothing after it is taken.
fn read_opening(s: &mut Stream) -> io::Result<String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if s.read(&mut byte)? == 0 {
            return Err(ErrorKind::UnexpectedEof.into());
        }
        if byte[0] == b'\n' {
            return String::from_utf8(line).map_err(|_| ErrorKind::InvalidData.into());
        }
        line.push(byte[0]);
        if line.len() > MAX_OPENING {
            return Err(ErrorKind::InvalidData.into());
        }
    }
}

enum Out {
    Line(Message),
    Close,
}

enum ToLink {
    Data(Vec<u8>),
    Close,
}

struct Call {
    id: Value,
    cancelled: Arc<AtomicBool>,
}

struct Link {
    name: String,
    remote: String,
    direction: &'static str,
    queued: usize,
    tx: Sender<ToLink>,
    /// Ends the link from the host's side (`abort`): see [`Closer`].
    raw: Closer,
}

struct Conn {
    out: Sender<Out>,
    hello: bool,
    callsigns: Vec<String>,
    listening: bool,
    call: Option<Call>,
    link: Option<Link>,
}

impl Conn {
    fn emit(&self, m: Message) {
        let _ = self.out.send(Out::Line(m));
    }
}

#[derive(Default)]
struct Shared {
    conns: HashMap<u64, Conn>,
    owner: Option<u64>,
    next_conn: u64,
    next_link: u64,
}

/// The adapter: its host's state and its settings.
#[derive(Clone)]
pub struct Adapter {
    shared: Arc<Mutex<Shared>>,
    cfg: Arc<Config>,
}

fn link_status(l: &Link) -> Value {
    json!({"link": l.name, "remote": l.remote, "direction": l.direction, "queued": l.queued})
}

impl Adapter {
    /// An adapter with these settings.
    #[must_use]
    pub fn new(cfg: Config) -> Adapter {
        Adapter {
            shared: Arc::new(Mutex::new(Shared::default())),
            cfg: Arc::new(cfg),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Serves hosts: RPMI on `listener` (RP-4 §2).
    pub fn serve_hosts(&self, listener: TcpListener) {
        let me = self.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let me = me.clone();
                thread::spawn(move || me.host(stream));
            }
        });
    }

    /// Takes calls from other adapters on `listener` (RP-4 §9.5).
    pub fn serve_calls(&self, listener: TcpListener) {
        let me = self.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let me = me.clone();
                thread::spawn(move || me.answer(&stream));
            }
        });
    }

    // -- the host's side ------------------------------------------------------------------------

    fn host(&self, stream: TcpStream) {
        let Ok(mut writer) = stream.try_clone() else {
            return;
        };
        let (out, rx) = channel::<Out>();
        thread::spawn(move || {
            for o in rx {
                match o {
                    Out::Line(m) => {
                        let line = line_of(&m) + "\n";
                        if writer.write_all(line.as_bytes()).is_err() {
                            return;
                        }
                    }
                    Out::Close => {
                        let _ = writer.shutdown(Shutdown::Both);
                        return;
                    }
                }
            }
        });
        let c = {
            let mut g = self.lock();
            g.next_conn += 1;
            let c = g.next_conn;
            g.conns.insert(
                c,
                Conn {
                    out,
                    hello: false,
                    callsigns: Vec::new(),
                    listening: false,
                    call: None,
                    link: None,
                },
            );
            c
        };
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match (&mut reader)
                .take(MAX_LINE as u64 + 1)
                .read_until(b'\n', &mut buf)
            {
                Ok(0) | Err(_) => break,
                Ok(_) if buf.len() > MAX_LINE => {
                    let g = self.lock();
                    if let Some(conn) = g.conns.get(&c) {
                        conn.emit(error(
                            None,
                            "malformed",
                            "a line longer than the adapter takes",
                        ));
                        let _ = conn.out.send(Out::Close);
                    }
                    break;
                }
                Ok(_) => {
                    if buf.last() == Some(&b'\n') {
                        buf.pop();
                    }
                    self.line(c, &buf);
                }
            }
        }
        self.host_left(c);
    }

    fn host_left(&self, c: u64) {
        let mut g = self.lock();
        if g.owner == Some(c) {
            g.owner = None;
        }
        if let Some(conn) = g.conns.remove(&c) {
            if let Some(call) = conn.call {
                call.cancelled.store(true, Ordering::SeqCst);
            }
            if let Some(link) = conn.link {
                // the other side hears `remote`; nobody is left here to tell
                let _ = link.tx.send(ToLink::Close);
            }
            let _ = conn.out.send(Out::Close);
        }
    }

    fn line(&self, c: u64, bytes: &[u8]) {
        let mut g = self.lock();
        let Some(conn) = g.conns.get(&c) else {
            return;
        };
        let Ok(text) = std::str::from_utf8(bytes) else {
            conn.emit(error(None, "malformed", "not JSON"));
            return;
        };
        let m = match parse_line(text) {
            Ok(m) => m,
            Err(refusal) => {
                conn.emit(refusal);
                return;
            }
        };
        let id = m.get("id").cloned();
        match admit(&m, conn.hello) {
            Ok(request) => self.request(&mut g, c, request, id.as_ref()),
            Err(refusal) => conn.emit(refusal),
        }
    }

    fn request(&self, g: &mut Shared, c: u64, request: Request, id: Option<&Value>) {
        if let Request::Hello { low, high, .. } = request {
            self.hello(g, c, low, high, id);
            return;
        }
        let Some(conn) = g.conns.get_mut(&c) else {
            return;
        };
        match request {
            Request::Hello { .. } => {}
            Request::Callsigns(list) => {
                conn.callsigns = list;
                conn.emit(answer("ok", id));
            }
            Request::Listen(on) => {
                if conn.callsigns.is_empty() {
                    conn.emit(error(id, "refused", "no callsigns yet"));
                } else {
                    conn.listening = on;
                    conn.emit(answer("ok", id));
                }
            }
            Request::Connect {
                remote,
                as_call,
                bandwidth,
            } => self.connect(c, conn, &remote, as_call, bandwidth, id),
            Request::Send { link, data, .. } => match conn.link.as_mut() {
                Some(l) if l.name == link => {
                    l.queued += data.len();
                    let queued = l.queued;
                    let _ = l.tx.send(ToLink::Data(data));
                    if id.is_some() {
                        conn.emit(answer("ok", id));
                    }
                    conn.emit(answer_with(
                        "queued",
                        None,
                        &[("link", link.into()), ("bytes", queued.into())],
                    ));
                }
                _ => conn.emit(error(id, "not-connected", "no such link of this host's")),
            },
            Request::Disconnect { link } => match conn.link.as_ref() {
                Some(l) if l.name == link => {
                    let _ = l.tx.send(ToLink::Close);
                    conn.emit(answer("ok", id));
                }
                _ => conn.emit(error(id, "not-connected", "no such link of this host's")),
            },
            Request::Abort { link: Some(link) } => match conn.link.take() {
                Some(l) if l.name == link => {
                    l.raw.close();
                    conn.emit(answer("ok", id));
                    conn.emit(answer_with(
                        "disconnected",
                        None,
                        &[("link", link.into()), ("reason", "local".into())],
                    ));
                }
                other => {
                    conn.link = other;
                    conn.emit(error(id, "not-connected", "no such link of this host's"));
                }
            },
            Request::Abort { link: None } => match conn.call.take() {
                Some(call) => {
                    call.cancelled.store(true, Ordering::SeqCst);
                    conn.emit(answer("ok", id));
                    conn.emit(answer_with(
                        "failed",
                        Some(&call.id),
                        &[
                            ("code", "aborted".into()),
                            ("text", "the host aborted".into()),
                        ],
                    ));
                }
                None => conn.emit(error(id, "not-connected", "no call under way")),
            },
            Request::Broadcast { .. } => {
                conn.emit(error(id, "unsupported", "no broadcasts over TCP"));
            }
            Request::Status => {
                let links: Vec<Value> = conn.link.iter().map(link_status).collect();
                conn.emit(answer_with(
                    "status",
                    id,
                    &[
                        ("listening", conn.listening.into()),
                        ("callsigns", json!(conn.callsigns)),
                        ("busy", false.into()),
                        ("links", Value::Array(links)),
                    ],
                ));
            }
        }
    }

    fn hello(&self, g: &mut Shared, c: u64, low: i128, high: i128, id: Option<&Value>) {
        let owner = g.owner;
        let Some(conn) = g.conns.get_mut(&c) else {
            return;
        };
        if !rpmi::wire::speaks(low, high) {
            conn.emit(error(id, "unsupported-version", "this adapter speaks 1"));
            let _ = conn.out.send(Out::Close);
            return;
        }
        if owner.is_some_and(|o| o != c) {
            conn.emit(error(id, "busy", "another program has this modem"));
            let _ = conn.out.send(Out::Close);
            return;
        }
        conn.hello = true;
        conn.emit(answer_with(
            "hello",
            id,
            &[
                ("version", rpmi::wire::VERSION.into()),
                ("modem", self.cfg.name.clone().into()),
                ("caps", caps().to_json()),
            ],
        ));
        g.owner = Some(c);
    }

    fn connect(
        &self,
        c: u64,
        conn: &mut Conn,
        remote: &str,
        as_call: Option<String>,
        bandwidth: Option<i128>,
        id: Option<&Value>,
    ) {
        let Some(first) = conn.callsigns.first() else {
            conn.emit(error(id, "refused", "no callsigns yet"));
            return;
        };
        let local = as_call.unwrap_or_else(|| first.clone());
        if !conn.callsigns.contains(&local) {
            conn.emit(error(
                id,
                "not-authorized",
                &format!("{local} is not this host's"),
            ));
            return;
        }
        if let Some(bw) = bandwidth {
            conn.emit(error(
                id,
                "unsupported",
                &format!("no {bw} Hz on this modem"),
            ));
            return;
        }
        if conn.call.is_some() || conn.link.is_some() {
            conn.emit(error(id, "busy", "a link or a call is up"));
            return;
        }
        let id = id.cloned().unwrap_or(Value::Null);
        let cancelled = Arc::new(AtomicBool::new(false));
        conn.call = Some(Call {
            id: id.clone(),
            cancelled: cancelled.clone(),
        });
        conn.emit(answer("ok", Some(&id)));
        let me = self.clone();
        let remote = remote.to_owned();
        thread::spawn(move || {
            let outcome = me.dial(&remote, &local);
            me.called(c, &id, &cancelled, &local, outcome);
        });
    }

    // -- calls out ------------------------------------------------------------------------------

    /// Where `remote` is: the callsign called (or `*`) and the address (RP-4 §9.5).
    fn resolve(&self, remote: &str) -> Result<(String, String), String> {
        if let Some((call, addr)) = remote.split_once('@') {
            return Ok((call.to_owned(), addr.to_owned()));
        }
        if let Some(addr) = self.cfg.peers.get(remote) {
            return Ok(match addr.split_once('@') {
                Some((call, addr)) => (call.to_owned(), addr.to_owned()),
                None => (remote.to_owned(), addr.clone()),
            });
        }
        let port = remote.rsplit_once(':').map(|(_, p)| p);
        if port.is_some_and(|p| p.parse::<u16>().is_ok()) {
            return Ok(("*".to_owned(), remote.to_owned()));
        }
        Err(format!("no address for {remote}"))
    }

    /// Places the call: the link's stream and the callsign that answered, or `failed`'s code and
    /// text.
    fn dial(&self, remote: &str, local: &str) -> Result<(Stream, String), (&'static str, String)> {
        let (called, addr) = self.resolve(remote).map_err(|e| ("refused", e))?;
        let addrs: Vec<SocketAddr> = addr
            .to_socket_addrs()
            .map_err(|e| ("refused", format!("{addr}: {e}")))?
            .collect();
        let mut last = ("refused", format!("{addr}: no address"));
        let mut tcp = None;
        for a in addrs {
            match TcpStream::connect_timeout(&a, ANSWER_WAIT) {
                Ok(s) => {
                    tcp = Some(s);
                    break;
                }
                Err(e) if e.kind() == ErrorKind::TimedOut => {
                    last = ("timeout", format!("{addr}: no answer"));
                }
                Err(e) => last = ("refused", format!("{addr}: {e}")),
            }
        }
        let tcp = tcp.ok_or(last)?;
        let _ = tcp.set_nodelay(true);
        let tcp = PolledStream::new(&tcp, Some(ANSWER_WAIT), "rptcp-link")
            .map_err(|e| ("refused", format!("{addr}: {e}")))?;
        let mut s = match &self.cfg.security {
            Security::Plain => Stream::Plain(tcp),
            Security::Tls { client, .. } => {
                let name = tls::server_name(&addr).map_err(|e| ("refused", e))?;
                let conn = ClientConnection::new(client.clone(), name)
                    .map_err(|e| ("refused", e.to_string()))?;
                Stream::Client(Box::new(StreamOwned::new(conn, tcp)))
            }
        };
        let opening = format!("RPTCP 1 {local} {called}\n");
        s.write_all(opening.as_bytes())
            .and_then(|()| s.flush())
            .map_err(|e| ("refused", format!("{addr}: {e}")))?;
        let reply = read_opening(&mut s).map_err(|e| match e.kind() {
            ErrorKind::WouldBlock | ErrorKind::TimedOut => ("timeout", "no answer".to_owned()),
            _ => ("refused", format!("{addr}: {e}")),
        })?;
        let mut words = reply.split(' ');
        match (words.next(), words.next(), words.next()) {
            (Some("OK"), Some(call), None) if !call.is_empty() => Ok((s, call.to_owned())),
            (Some("NO"), Some("busy"), None) => Err(("busy", format!("{remote} is busy"))),
            (Some("NO"), Some(_), None) => Err(("refused", format!("{remote} refused the call"))),
            _ => Err((
                "refused",
                format!("{addr} does not answer as RP-4 §9.5 says"),
            )),
        }
    }

    /// The call placed: the link up, or `failed`; nothing when the host aborted it or left.
    fn called(
        &self,
        c: u64,
        id: &Value,
        cancelled: &Arc<AtomicBool>,
        local: &str,
        outcome: Result<(Stream, String), (&'static str, String)>,
    ) {
        let mut g = self.lock();
        let g = &mut *g;
        let ours = g.conns.get(&c).is_some_and(|conn| {
            conn.call
                .as_ref()
                .is_some_and(|call| Arc::ptr_eq(&call.cancelled, cancelled))
        });
        if !ours || cancelled.load(Ordering::SeqCst) {
            if let Ok((mut s, _)) = outcome {
                s.close();
            }
            return;
        }
        g.next_link += 1;
        let name = format!("t{}", g.next_link);
        let Some(conn) = g.conns.get_mut(&c) else {
            return;
        };
        conn.call = None;
        match outcome {
            Err((code, text)) => conn.emit(answer_with(
                "failed",
                Some(id),
                &[("code", code.into()), ("text", text.into())],
            )),
            Ok((s, remote)) => {
                if !self.start_link(c, conn, &name, &remote, "out", s) {
                    return;
                }
                conn.emit(answer_with(
                    "connected",
                    Some(id),
                    &[
                        ("link", name.into()),
                        ("remote", remote.into()),
                        ("local", local.into()),
                        ("direction", "out".into()),
                    ],
                ));
            }
        }
    }

    // -- calls in -------------------------------------------------------------------------------

    fn answer(&self, tcp: &TcpStream) {
        let _ = tcp.set_nodelay(true);
        let Ok(tcp) = PolledStream::new(tcp, Some(ANSWER_WAIT), "rptcp-link") else {
            return;
        };
        let mut s = match &self.cfg.security {
            Security::Plain => Stream::Plain(tcp),
            Security::Tls {
                server: Some(server),
                ..
            } => match ServerConnection::new(server.clone()) {
                Ok(conn) => Stream::Server(Box::new(StreamOwned::new(conn, tcp))),
                Err(_) => return,
            },
            Security::Tls { server: None, .. } => return,
        };
        let Ok(opening) = read_opening(&mut s) else {
            return;
        };
        let mut words = opening.split(' ');
        let (Some("RPTCP"), Some("1"), Some(caller), Some(wanted), None) = (
            words.next(),
            words.next(),
            words.next(),
            words.next(),
            words.next(),
        ) else {
            return;
        };
        let mut g = self.lock();
        let g = &mut *g;
        let host = g.owner.and_then(|o| g.conns.get(&o).map(|conn| (o, conn)));
        let verdict = match host {
            Some((_, conn)) if conn.listening && !conn.callsigns.is_empty() => {
                let local = if wanted == "*" {
                    Some(conn.callsigns[0].clone())
                } else {
                    conn.callsigns.iter().find(|x| *x == wanted).cloned()
                };
                match local {
                    None => Err("refused"),
                    Some(_) if conn.call.is_some() || conn.link.is_some() => Err("busy"),
                    Some(local) => Ok(local),
                }
            }
            _ => Err("refused"),
        };
        let local = match verdict {
            Ok(local) => local,
            Err(code) => {
                let _ = s.write_all(format!("NO {code}\n").as_bytes());
                s.close();
                return;
            }
        };
        if s.write_all(format!("OK {local}\n").as_bytes())
            .and_then(|()| s.flush())
            .is_err()
        {
            return;
        }
        let Some(o) = g.owner else {
            return;
        };
        g.next_link += 1;
        let name = format!("t{}", g.next_link);
        let Some(conn) = g.conns.get_mut(&o) else {
            return;
        };
        if !self.start_link(o, conn, &name, caller, "in", s) {
            return;
        }
        conn.emit(answer_with(
            "incoming",
            None,
            &[("remote", caller.into()), ("local", local.clone().into())],
        ));
        conn.emit(answer_with(
            "connected",
            None,
            &[
                ("link", name.into()),
                ("remote", caller.into()),
                ("local", local.into()),
                ("direction", "in".into()),
            ],
        ));
    }

    // -- links ----------------------------------------------------------------------------------

    /// Records the link on host `c` and starts its thread; false when the link could not start.
    fn start_link(
        &self,
        c: u64,
        conn: &mut Conn,
        name: &str,
        remote: &str,
        direction: &'static str,
        s: Stream,
    ) -> bool {
        let raw = s.raw().closer();
        s.raw().set_wait(Some(POLL));
        let (tx, rx) = channel::<ToLink>();
        conn.link = Some(Link {
            name: name.to_owned(),
            remote: remote.to_owned(),
            direction,
            queued: 0,
            tx,
            raw,
        });
        let me = self.clone();
        let name = name.to_owned();
        thread::spawn(move || me.run_link(c, &name, s, &rx));
        true
    }

    /// A link's life: what the host sent goes out, what arrives goes to the host, until one side
    /// closes it.
    fn run_link(&self, c: u64, name: &str, mut s: Stream, rx: &Receiver<ToLink>) {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let mut written = 0;
            loop {
                match rx.try_recv() {
                    Ok(ToLink::Data(d)) => {
                        if s.write_all(&d).and_then(|()| s.flush()).is_err() {
                            self.end_link(c, name, "failure");
                            return;
                        }
                        written += d.len();
                    }
                    Ok(ToLink::Close) => {
                        s.close();
                        self.end_link(c, name, "local");
                        return;
                    }
                    Err(TryRecvError::Empty) => break,
                    // aborted, or the host left: the link is already gone from its state
                    Err(TryRecvError::Disconnected) => {
                        s.raw().closer().close();
                        return;
                    }
                }
            }
            if written > 0 {
                self.drained(c, name, written);
            }
            match s.read(&mut buf) {
                Ok(0) => {
                    self.end_link(c, name, "remote");
                    return;
                }
                Ok(n) => {
                    let data = rpmi::base64::encode(&buf[..n]);
                    self.to_host(c, name, |_| {
                        Some(answer_with(
                            "received",
                            None,
                            &[("link", name.into()), ("data", data.into())],
                        ))
                    });
                }
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        ErrorKind::UnexpectedEof
                            | ErrorKind::ConnectionReset
                            | ErrorKind::ConnectionAborted
                            | ErrorKind::BrokenPipe
                    ) =>
                {
                    self.end_link(c, name, "remote");
                    return;
                }
                Err(_) => {
                    self.end_link(c, name, "failure");
                    return;
                }
            }
        }
    }

    /// Tells host `c` what `say` makes of its link `name`, while the host has that link.
    fn to_host(&self, c: u64, name: &str, say: impl FnOnce(&mut Link) -> Option<Message>) {
        let mut g = self.lock();
        let Some(conn) = g.conns.get_mut(&c) else {
            return;
        };
        if let Some(l) = conn.link.as_mut().filter(|l| l.name == name)
            && let Some(m) = say(l)
        {
            conn.emit(m);
        }
    }

    fn drained(&self, c: u64, name: &str, written: usize) {
        self.to_host(c, name, |l| {
            l.queued = l.queued.saturating_sub(written);
            Some(answer_with(
                "queued",
                None,
                &[("link", name.into()), ("bytes", l.queued.into())],
            ))
        });
    }

    fn end_link(&self, c: u64, name: &str, reason: &str) {
        let mut g = self.lock();
        let Some(conn) = g.conns.get_mut(&c) else {
            return;
        };
        if conn.link.as_ref().is_some_and(|l| l.name == name) {
            conn.link = None;
            conn.emit(answer_with(
                "disconnected",
                None,
                &[("link", name.into()), ("reason", reason.into())],
            ));
        }
    }
}
