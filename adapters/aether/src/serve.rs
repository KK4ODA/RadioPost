// SPDX-License-Identifier: MIT OR Apache-2.0
//! The adapter's input and output: hosts on a TCP port (RP-4 §2), each given its own WebSocket
//! connection to Aether (RP-4 §9.1), and a [`Bridge`] between the two.
//!
//! A host's thread reads its lines; the connection's thread owns the bridge, the WebSocket and
//! the writer to the host, and wakes every few milliseconds to read Aether and the host's lines.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
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

/// Listens for hosts and serves each through its own connection to Aether at `aether`
/// (`ws://127.0.0.1:8515/v1`). Returns the address taken; the adapter runs on threads of its own.
///
/// # Errors
///
/// The address could not be taken.
pub fn listen(addr: impl ToSocketAddrs, aether: &str) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(addr)?;
    let local = listener.local_addr()?;
    let aether = aether.to_owned();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let aether = aether.clone();
            thread::spawn(move || serve(&stream, &aether));
        }
    });
    Ok(local)
}

fn open_aether(url: &str) -> Option<WebSocket<MaybeTlsStream<TcpStream>>> {
    let (ws, _) = tungstenite::connect(url).ok()?;
    if let MaybeTlsStream::Plain(s) = ws.get_ref() {
        s.set_read_timeout(Some(POLL)).ok()?;
        s.set_nodelay(true).ok()?;
    }
    Some(ws)
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

/// Serves one host until its connection closes. Its link or call is aborted first (RP-4 §1.1);
/// then Aether's connection closes, which ends the host's claim (ADR-0106).
pub fn serve(host: &TcpStream, aether: &str) {
    let _ = host.set_nodelay(true);
    let Ok(reader) = host.try_clone() else {
        return;
    };
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || read_host(reader, &tx));
    let mut ws = open_aether(aether);
    let mut bridge = if ws.is_some() {
        Bridge::new()
    } else {
        Bridge::without_modem()
    };
    run(&mut bridge, &mut ws, host, &rx);
    if let Some(mut ws) = ws {
        left(&mut bridge, &mut ws);
        let _ = ws.close(None);
        let _ = ws.flush();
    }
    let _ = host.shutdown(Shutdown::Both);
}

/// The host is gone: what it held on the air is ended, and Aether's answer awaited a moment so
/// the abort is done before the claim goes.
fn left(bridge: &mut Bridge, ws: &mut WebSocket<MaybeTlsStream<TcpStream>>) {
    for out in bridge.host_left() {
        if let Out::Aether(request) = out
            && ws.send(Ws::text(request.to_string())).is_err()
        {
            return;
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
            Err(_) => return,
        }
    }
}

fn run(
    bridge: &mut Bridge,
    ws: &mut Option<WebSocket<MaybeTlsStream<TcpStream>>>,
    host: &TcpStream,
    rx: &Receiver<FromHost>,
) {
    loop {
        let mut outs = Vec::new();
        if let Some(socket) = ws.as_mut() {
            match socket.read() {
                Ok(Ws::Text(text)) => outs.extend(bridge.from_aether(&text)),
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => {
                    *ws = None;
                    outs.extend(bridge.modem_lost());
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
                Ok(FromHost::Closed) | Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => break,
            }
        }
        for out in outs {
            match out {
                Out::Host(m) => {
                    let mut line = line_of(&m);
                    line.push('\n');
                    if (&*host).write_all(line.as_bytes()).is_err() {
                        return;
                    }
                }
                Out::Aether(request) => {
                    let sent = ws
                        .as_mut()
                        .is_some_and(|s| s.send(Ws::text(request.to_string())).is_ok());
                    if !sent && ws.is_some() {
                        *ws = None;
                        for m in bridge.modem_lost() {
                            if let Out::Host(m) = m {
                                let mut line = line_of(&m);
                                line.push('\n');
                                let _ = (&*host).write_all(line.as_bytes());
                            }
                        }
                    }
                }
                Out::CloseHost => return,
            }
        }
    }
}
