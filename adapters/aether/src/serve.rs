// SPDX-License-Identifier: MIT OR Apache-2.0
//! The adapter's input and output: hosts on a TCP port (RP-4 §2), each given its own WebSocket
//! connection to Aether (RP-4 §9.1), and a [`Bridge`] between the two.
//!
//! A host's thread reads its lines; the connection's thread owns the bridge, the WebSocket and
//! the writer to the host, and wakes every few milliseconds to read Aether and the host's lines.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use rpmi::tcp::MAX_LINE;
use rpmi::wire::line_of;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message as Ws, WebSocket};

use crate::bridge::{Bridge, Out};

/// How long a read of Aether waits before the host's lines are looked at again.
const POLL: Duration = Duration::from_millis(10);
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

/// Listens for hosts and serves each through its own connection to Aether at `aether`
/// (`ws://127.0.0.1:8515/v1`). Returns the address taken; the adapter runs on threads of its own.
///
/// # Errors
///
/// The address could not be taken.
pub fn listen(addr: impl ToSocketAddrs, aether: &str) -> io::Result<SocketAddr> {
    listen_noting(addr, aether, quiet())
}

/// [`listen`], saying what it saw of Aether on `note`.
///
/// # Errors
///
/// The address could not be taken.
pub fn listen_noting(addr: impl ToSocketAddrs, aether: &str, note: Note) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(addr)?;
    let local = listener.local_addr()?;
    let aether = aether.to_owned();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let aether = aether.clone();
            let note = Arc::clone(&note);
            thread::spawn(move || serve_noting(&stream, &aether, &note));
        }
    });
    Ok(local)
}

fn open_aether(url: &str) -> Result<WebSocket<MaybeTlsStream<TcpStream>>, String> {
    let (ws, _) = tungstenite::connect(url).map_err(|e| e.to_string())?;
    if let MaybeTlsStream::Plain(s) = ws.get_ref() {
        s.set_read_timeout(Some(POLL)).map_err(|e| e.to_string())?;
        s.set_nodelay(true).map_err(|e| e.to_string())?;
    }
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
    serve_noting(host, aether, &quiet());
}

/// [`serve`], saying on `note` why Aether could not be reached, or why the connection to it
/// ended, and when a host left with a link or call up.
pub fn serve_noting(host: &TcpStream, aether: &str, note: &Note) {
    let _ = host.set_nodelay(true);
    let Ok(reader) = host.try_clone() else {
        return;
    };
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || read_host(reader, &tx));
    let mut ws = match open_aether(aether) {
        Ok(ws) => Some(ws),
        Err(why) => {
            note(&format!("Aether at {aether} could not be reached: {why}"));
            None
        }
    };
    let mut bridge = if ws.is_some() {
        Bridge::new()
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
fn left(bridge: &mut Bridge, ws: &mut WebSocket<MaybeTlsStream<TcpStream>>) -> bool {
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
    ws: &mut Option<WebSocket<MaybeTlsStream<TcpStream>>>,
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
