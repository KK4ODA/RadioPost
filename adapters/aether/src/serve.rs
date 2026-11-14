// SPDX-License-Identifier: MIT OR Apache-2.0
//! The adapter's input and output: hosts on a TCP port (RP-4 §2), each given its own WebSocket
//! connection to Aether (RP-4 §9.1), and a [`Bridge`] between the two.
//!
//! A host's thread reads its lines; the connection's thread owns the bridge, the WebSocket and
//! the writer to the host, and wakes every few milliseconds to read Aether and the host's lines.
//!
//! The WebSocket is read through a [`PolledStream`], a thread of its own doing plain blocking
//! reads, never with a read timeout on its socket: on Windows a read whose timeout runs out can
//! fail with os error 997 ("Overlapped I/O operation is in progress"), which ended RadioPost
//! Gateway's connection to Aether, and its claim, four times on 2026-11-12.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use rpmi::polled::PolledStream;
use rpmi::tcp::MAX_LINE;
use rpmi::wire::line_of;
use tungstenite::client::IntoClientRequest;
use tungstenite::{Message as Ws, WebSocket};

use crate::bridge::{Bridge, Out};

/// How long a read of Aether waits before the host's lines are looked at again.
const POLL: Duration = Duration::from_millis(10);
/// How long Aether has to answer the WebSocket's opening handshake.
const HANDSHAKE: Duration = Duration::from_secs(10);
/// How long a host that left waits for Aether to take its abort.
const LEAVING: Duration = Duration::from_secs(2);

/// What a host's reader thread hands the connection's thread.
enum FromHost {
    Line(Vec<u8>),
    TooLong,
    Closed,
}

/// Where the adapter says what it saw of Aether that its host is not told in full: why Aether
/// could not be reached, and why a connection to it ended. A program built around the adapter
/// sends these to its log; on 2026-11-12 a client's connection to Aether ended twice with no
/// word of how at either end.
pub type Note = Arc<dyn Fn(&str) + Send + Sync>;

/// A [`Note`] that says nothing.
#[must_use]
pub fn quiet() -> Note {
    Arc::new(|_| {})
}

/// Why Aether could not be reached, as last said, for the hosts of one listener. A host that
/// opens the modem again every few seconds while Aether is away (a gateway waiting for it, the
/// client's connection between calls) is answered `no-modem` each time, but the note says it once
/// for each reason, and says when Aether is reached again.
#[derive(Default)]
struct Reach(Mutex<Option<String>>);

impl Reach {
    /// Aether could not be reached, for `why`: whether to say so.
    fn missed(&self, why: &str) -> bool {
        let mut last = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if last.as_deref() == Some(why) {
            return false;
        }
        *last = Some(why.to_owned());
        true
    }

    /// Aether was reached: whether it had not been, last time.
    fn reached(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .is_some()
    }
}

/// Listens for hosts and serves each through its own connection to Aether at `aether`
/// (`ws://127.0.0.1:8515/v1`). Returns the address taken; the adapter runs on threads of its own.
///
/// # Errors
///
/// The address could not be taken.
pub fn listen(addr: impl ToSocketAddrs, aether: &str) -> io::Result<SocketAddr> {
    listen_noting(addr, aether, quiet(), None)
}

/// [`listen`], saying what it saw of Aether on `note`, and offering Aether `log`, the program's
/// own log file, for the files the station shares ([`Bridge::offering_log`]): a program built
/// around the adapter names it; the adapter on its own has none.
///
/// # Errors
///
/// The address could not be taken.
pub fn listen_noting(
    addr: impl ToSocketAddrs,
    aether: &str,
    note: Note,
    log: Option<PathBuf>,
) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(addr)?;
    let local = listener.local_addr()?;
    let aether = aether.to_owned();
    let reach = Arc::new(Reach::default());
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let aether = aether.clone();
            let note = Arc::clone(&note);
            let log = log.clone();
            let reach = Arc::clone(&reach);
            thread::spawn(move || serve_reaching(&stream, &aether, &note, log.as_deref(), &reach));
        }
    });
    Ok(local)
}

/// Opens the WebSocket to Aether's control API at `url` (`ws://`, on this machine or the
/// station's network): the handshake waits up to [`HANDSHAKE`] for each read, and every read after
/// it up to [`POLL`].
fn open_aether(url: &str) -> Result<WebSocket<PolledStream>, String> {
    let request = url.into_client_request().map_err(|e| e.to_string())?;
    if request.uri().scheme_str() != Some("ws") {
        return Err(format!("{url} is not a ws:// address"));
    }
    let host = request
        .uri()
        .host()
        .ok_or("the address names no host")?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let port = request.uri().port_u16().unwrap_or(80);
    let tcp = TcpStream::connect((host.as_str(), port))
        .map_err(|e| format!("cannot connect to {host}:{port}: {e}"))?;
    tcp.set_nodelay(true).map_err(|e| e.to_string())?;
    let stream =
        PolledStream::new(&tcp, Some(HANDSHAKE), "aether-ws-reader").map_err(|e| e.to_string())?;
    let (ws, _) = tungstenite::client(request, stream).map_err(|e| e.to_string())?;
    ws.get_ref().set_wait(Some(POLL));
    Ok(ws)
}

fn read_host(stream: TcpStream, to: &mpsc::Sender<FromHost>) {
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let read = (&mut reader)
            .take(MAX_LINE as u64 + 1)
            .read_until(b'\n', &mut buf);
        let said = match read {
            Ok(0) | Err(_) => FromHost::Closed,
            Ok(_) if buf.len() > MAX_LINE => FromHost::TooLong,
            Ok(_) => {
                if buf.last() == Some(&b'\n') {
                    buf.pop();
                }
                FromHost::Line(buf.clone())
            }
        };
        let last = !matches!(said, FromHost::Line(_));
        if to.send(said).is_err() || last {
            return;
        }
    }
}

/// Serves one host until its connection closes, or Aether's does. A host that leaves has its
/// link or call aborted first (RP-4 §1.1); then Aether's connection closes, which ends the
/// host's claim (ADR-0106). Aether going away is told to the host and closes its connection
/// (RP-4 §2.1).
pub fn serve(host: &TcpStream, aether: &str) {
    serve_noting(host, aether, &quiet(), None);
}

/// [`serve`], saying on `note` why Aether could not be reached, or why the connection to it
/// ended, and when a host left with a link or call up; and offering Aether `log`, the
/// program's own log file, once the host has said hello.
pub fn serve_noting(host: &TcpStream, aether: &str, note: &Note, log: Option<&Path>) {
    serve_reaching(host, aether, note, log, &Reach::default());
}

/// [`serve_noting`], with what was last said of reaching Aether shared with the listener's other
/// hosts ([`Reach`]).
fn serve_reaching(host: &TcpStream, aether: &str, note: &Note, log: Option<&Path>, reach: &Reach) {
    let _ = host.set_nodelay(true);
    let Ok(reader) = host.try_clone() else {
        return;
    };
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || read_host(reader, &tx));
    let mut ws = match open_aether(aether) {
        Ok(ws) => {
            if reach.reached() {
                note(&format!("Aether at {aether} can be reached again"));
            }
            Some(ws)
        }
        Err(why) => {
            if reach.missed(&why) {
                note(&format!("Aether at {aether} could not be reached: {why}"));
            }
            None
        }
    };
    let mut bridge = if ws.is_some() {
        Bridge::new().offering_log(log.map(|p| p.display().to_string()))
    } else {
        Bridge::without_modem()
    };
    if let Some(why) = run(&mut bridge, &mut ws, host, &rx) {
        note(&format!(
            "the connection to Aether at {aether} ended: {why}; the program's connection is closed"
        ));
    }
    if let Some(mut ws) = ws {
        if left(&mut bridge, &mut ws) {
            note("the program left with a call or link up: asked Aether to abort it");
        }
        let _ = ws.close(None);
        let _ = ws.flush();
    }
    let _ = host.shutdown(Shutdown::Both);
}

/// The host is gone: what it held on the air is ended, and Aether's answer awaited a moment so
/// the abort is done before the claim goes. Whether there was anything to end.
fn left(bridge: &mut Bridge, ws: &mut WebSocket<PolledStream>) -> bool {
    let asked = bridge.host_left();
    let held = !asked.is_empty();
    for out in asked {
        if let Out::Aether(request) = out
            && ws.send(Ws::text(request.to_string())).is_err()
        {
            return held;
        }
    }
    let deadline = Instant::now() + LEAVING;
    while bridge.awaiting() && Instant::now() < deadline {
        match ws.read() {
            Ok(Ws::Text(text)) => {
                let _ = bridge.from_aether(&text);
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return held,
        }
    }
    held
}

/// Carries one host until it leaves, or until Aether goes away under it: then why.
///
/// Aether going away ends the host's connection too, once the host has been told its link and
/// its call are over (RP-4 §2.1). Aether restarts whenever its settings are saved, and a claim
/// lasts only as long as its WebSocket (ADR-0106): kept open, the host's connection would have a
/// gateway listening for calls that Aether no longer answers for it, and nothing would tell it to
/// claim again. Closed, it is the host's cue to open the modem afresh.
fn run(
    bridge: &mut Bridge,
    ws: &mut Option<WebSocket<PolledStream>>,
    host: &TcpStream,
    rx: &Receiver<FromHost>,
) -> Option<String> {
    loop {
        let mut outs = Vec::new();
        let mut lost: Option<String> = None;
        if let Some(socket) = ws.as_mut() {
            match socket.read() {
                Ok(Ws::Text(text)) => outs.extend(bridge.from_aether(&text)),
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(error) => {
                    *ws = None;
                    let why = format!("a read failed: {error}");
                    outs.extend(bridge.modem_lost(&why));
                    lost = Some(why);
                }
            }
        } else {
            thread::sleep(POLL);
        }
        loop {
            match rx.try_recv() {
                Ok(FromHost::Line(line)) => match std::str::from_utf8(&line) {
                    Ok(text) => outs.extend(bridge.from_host(text)),
                    Err(_) => outs.extend(bridge.from_host("\u{0}")),
                },
                Ok(FromHost::TooLong) => {
                    outs.extend(bridge.from_host("a line longer than the adapter takes"));
                    outs.push(Out::CloseHost);
                }
                Ok(FromHost::Closed) | Err(TryRecvError::Disconnected) => return None,
                Err(TryRecvError::Empty) => break,
            }
        }
        for out in outs {
            match out {
                Out::Host(m) => {
                    let mut line = line_of(&m);
                    line.push('\n');
                    if (&*host).write_all(line.as_bytes()).is_err() {
                        return lost;
                    }
                }
                Out::Aether(request) => {
                    let sent = ws.as_mut().map(|s| s.send(Ws::text(request.to_string())));
                    if let Some(Err(error)) = sent {
                        *ws = None;
                        let why = format!("a request could not be written: {error}");
                        for m in bridge.modem_lost(&why) {
                            if let Out::Host(m) = m {
                                let mut line = line_of(&m);
                                line.push('\n');
                                let _ = (&*host).write_all(line.as_bytes());
                            }
                        }
                        lost = Some(why);
                    }
                }
                Out::CloseHost => return lost,
            }
        }
        if lost.is_some() {
            return lost;
        }
    }
}
