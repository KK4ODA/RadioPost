// SPDX-License-Identifier: MIT OR Apache-2.0
//! A TCP connection read a while at a time, without a read timeout on its socket.
//!
//! A host's connection to its adapter ([`crate::tcp::Connection`]) and the Aether adapter's
//! WebSocket to Aether are each served by one thread that waits a short while for input and then
//! does its other work. That wait was a read timeout on the socket (`SO_RCVTIMEO`), and on Windows
//! it is not safe: Rust's sockets there are overlapped, and a read whose time runs out can fail
//! with `ERROR_IO_PENDING`, os error 997, "Overlapped I/O operation is in progress", rather than as
//! a timeout, which every loop took for a broken connection. Microsoft's documentation of
//! `SO_RCVTIMEO` says a socket whose receive timed out is in an indeterminate state and may lose
//! data, so reading that error as a timeout would be no fix. RadioPost Gateway's Aether adapter
//! lost its connection to Aether this way four times on 2026-11-12, and its claim with it, each
//! time for about five seconds; Aether's own control server, which read the same way, lost five
//! (Aether's `core/aetherd/src/polled.rs`, which this follows).
//!
//! So a thread of the connection's own reads the socket with plain blocking reads, which no
//! platform times out, and hands what it reads over a channel. [`PolledStream`] reads from that
//! channel as the socket read with its timeout: what has come in at once, else
//! [`std::io::ErrorKind::TimedOut`] after the wait, `Ok(0)` once the other end has closed. It
//! writes straight to the socket.
//!
//! **Ending a connection from another thread goes through its [`Closer`]**, never a bare
//! `shutdown` on a copy of the socket: on Windows a shutdown on one copy of a socket does not end
//! a blocking read waiting on another. The closer marks the connection ended, which the next read
//! sees within its wait, and shuts the socket down so the other end sees it close. Dropped, a
//! `PolledStream` closes itself the same way. Its reading thread ends when its read does: at once
//! on Linux, on Windows when the other end, told by the shutdown, closes.

use std::cell::Cell;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::time::Duration;

/// The most the reading thread takes off the socket at a time.
const CHUNK: usize = 8192;
/// Chunks read ahead before the reading thread waits for the serving one: past that the bytes
/// stay in the socket's own buffer, and a peer that sends faster than it is served is slowed by
/// TCP, as before.
const READ_AHEAD: usize = 16;

/// What the reading thread hands over.
enum Input {
    Bytes(Vec<u8>),
    /// The other end closed the connection.
    Closed,
    Failed(io::Error),
}

/// How the connection ended, for every read after the first that said so.
enum Ended {
    Closed,
    Failed(io::ErrorKind, String),
}

/// What ends a connection from another thread: see the module's note.
#[derive(Debug, Clone)]
pub struct Closer {
    closed: Arc<AtomicBool>,
    stream: Arc<TcpStream>,
}

impl Closer {
    /// Ends the connection: its next read says so, and the other end sees it close.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

/// A TCP connection whose reads wait at most a set time, read by a thread of its own.
pub struct PolledStream {
    /// The socket, for writes.
    stream: TcpStream,
    closer: Closer,
    input: Receiver<Input>,
    /// Bytes handed over and not read yet, from `at`.
    held: Vec<u8>,
    at: usize,
    /// How long a read waits; `None`, for as long as it takes.
    wait: Cell<Option<Duration>>,
    ended: Option<Ended>,
}

impl PolledStream {
    /// Reads `stream` from a thread named `name`, each read waiting at most `wait` (`None`: for as
    /// long as it takes).
    ///
    /// # Errors
    ///
    /// The socket cannot be cloned or its read timeout cleared, or the thread cannot start.
    pub fn new(stream: &TcpStream, wait: Option<Duration>, name: &str) -> io::Result<PolledStream> {
        let mut reading = stream.try_clone()?;
        // the reading thread waits for as long as the connection is quiet; a timeout left on the
        // socket (it is the clones' too) would bring back what this is here to remove
        reading.set_read_timeout(None)?;
        let writing = stream.try_clone()?;
        let closer = Closer {
            closed: Arc::new(AtomicBool::new(false)),
            stream: Arc::new(stream.try_clone()?),
        };
        let (hand, input) = mpsc::sync_channel(READ_AHEAD);
        std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || read_into(&mut reading, &hand))?;
        Ok(PolledStream {
            stream: writing,
            closer,
            input,
            held: Vec::new(),
            at: 0,
            wait: Cell::new(wait),
            ended: None,
        })
    }

    /// How long each read waits from now on; `None`, for as long as it takes.
    pub fn set_wait(&self, wait: Option<Duration>) {
        self.wait.set(wait);
    }

    /// What ends this connection from another thread.
    #[must_use]
    pub fn closer(&self) -> Closer {
        self.closer.clone()
    }

    /// Shuts the sending half, as a close in order does: the other end reads the end of what was
    /// sent, and may still send.
    ///
    /// # Errors
    ///
    /// The socket refused.
    pub fn shutdown_write(&self) -> io::Result<()> {
        self.stream.shutdown(Shutdown::Write)
    }

    fn closed(&self) -> bool {
        self.closer.closed.load(Ordering::SeqCst)
    }

    /// The next thing the reading thread hands over, within the wait.
    fn next_input(&self) -> Result<Input, RecvTimeoutError> {
        match self.wait.get() {
            Some(wait) => self.input.recv_timeout(wait),
            None => self
                .input
                .recv()
                .map_err(|_| RecvTimeoutError::Disconnected),
        }
    }
}

/// The reading thread: the socket's bytes into the channel until the connection ends, or the
/// serving side has gone.
fn read_into(stream: &mut TcpStream, hand: &SyncSender<Input>) {
    let mut buffer = vec![0u8; CHUNK];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => {
                let _ = hand.send(Input::Closed);
                return;
            }
            Ok(count) => {
                if hand.send(Input::Bytes(buffer[..count].to_vec())).is_err() {
                    return;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                let _ = hand.send(Input::Failed(error));
                return;
            }
        }
    }
}

impl Read for PolledStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.closed() {
            // let go from another thread: what it had not read yet goes with the connection
            self.ended = Some(Ended::Closed);
            self.held.clear();
            self.at = 0;
        }
        if self.at >= self.held.len() {
            match &self.ended {
                Some(Ended::Closed) => return Ok(0),
                Some(Ended::Failed(kind, said)) => return Err(io::Error::new(*kind, said.clone())),
                None => {}
            }
            match self.next_input() {
                Ok(Input::Bytes(bytes)) => {
                    self.held = bytes;
                    self.at = 0;
                }
                Ok(Input::Closed) | Err(RecvTimeoutError::Disconnected) => {
                    self.ended = Some(Ended::Closed);
                    return Ok(0);
                }
                Ok(Input::Failed(error)) => {
                    self.ended = Some(Ended::Failed(error.kind(), error.to_string()));
                    return Err(error);
                }
                Err(RecvTimeoutError::Timeout) if self.closed() => {
                    self.ended = Some(Ended::Closed);
                    return Ok(0);
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "nothing came in the wait",
                    ));
                }
            }
        }
        let count = (self.held.len() - self.at).min(buf.len());
        buf[..count].copy_from_slice(&self.held[self.at..self.at + count]);
        self.at += count;
        Ok(count)
    }
}

impl Write for PolledStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

impl Drop for PolledStream {
    fn drop(&mut self) {
        // the connection is over: the shutdown ends the reading thread's read (on Windows when
        // the other end closes), and its next hand-over finds nobody to take it
        self.closer.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Instant;

    /// A connected pair: the server's end read through a [`PolledStream`], the client's plain.
    fn pair(wait: Option<Duration>) -> (PolledStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let client = TcpStream::connect(listener.local_addr().expect("address")).expect("connect");
        let (served, _) = listener.accept().expect("accept");
        let polled = PolledStream::new(&served, wait, "test-polled").expect("polled");
        (polled, client)
    }

    /// What the socket's read timeout could not promise on Windows: a connection idle for many
    /// waits is still there, and nothing sent after them is lost.
    #[test]
    fn a_quiet_connection_waits_and_loses_nothing() {
        let (mut polled, mut client) = pair(Some(Duration::from_millis(10)));
        let mut buf = [0u8; 64];
        for _ in 0..20 {
            let started = Instant::now();
            let error = polled.read(&mut buf).expect_err("nothing was sent");
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "a read waited too long"
            );
        }
        let message: Vec<u8> = (0..=255u8).cycle().take(100_000).collect();
        client.write_all(&message).expect("send");
        let mut got = Vec::new();
        while got.len() < message.len() {
            match polled.read(&mut buf) {
                Ok(0) => panic!("the connection ended after {} bytes", got.len()),
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
                Err(error) => panic!("a read failed: {error}"),
            }
        }
        assert_eq!(got, message, "the bytes arrived as sent");
    }

    #[test]
    fn writes_reach_the_other_end_and_a_close_reads_as_the_end() {
        let (mut polled, mut client) = pair(Some(Duration::from_millis(10)));
        polled.write_all(b"OK\n").expect("write");
        let mut said = [0u8; 3];
        client.read_exact(&mut said).expect("read");
        assert_eq!(&said, b"OK\n");
        client.write_all(b"BYE").expect("send");
        drop(client);
        let mut buf = [0u8; 8];
        let mut got = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match polled.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
                Err(error) => panic!("a read failed: {error}"),
            }
            assert!(Instant::now() < deadline, "the close was never read");
        }
        assert_eq!(got, b"BYE");
        // and the end again, at once
        assert_eq!(polled.read(&mut buf).expect("ended"), 0);
    }

    /// The other end keeps its socket open, and the serving thread must still learn the
    /// connection was let go from another thread.
    #[test]
    fn a_closer_ends_the_reads_from_another_thread_while_the_other_end_holds_on() {
        let (mut polled, _client) = pair(Some(Duration::from_millis(10)));
        let closer = polled.closer();
        let mut buf = [0u8; 8];
        assert_eq!(
            polled.read(&mut buf).expect_err("quiet").kind(),
            io::ErrorKind::TimedOut
        );
        std::thread::spawn(move || closer.close())
            .join()
            .expect("closed");
        let started = Instant::now();
        assert_eq!(polled.read(&mut buf).expect("ended"), 0);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(polled.read(&mut buf).expect("ended again"), 0);
    }

    #[test]
    fn dropped_it_closes_the_connection() {
        let (polled, mut client) = pair(Some(Duration::from_millis(10)));
        drop(polled);
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let mut buf = [0u8; 8];
        // the shutdown reads as the end at the other station, never as a wait run out
        match client.read(&mut buf) {
            Ok(n) => assert_eq!(n, 0, "nothing was sent"),
            Err(error) => assert!(
                !matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ),
                "the connection was not closed: {error}"
            ),
        }
    }

    /// With no wait a read waits for as long as it takes; a wait set later applies from then on.
    #[test]
    fn no_wait_waits_until_something_comes_and_a_wait_can_be_set() {
        let (mut polled, mut client) = pair(None);
        let sender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            client.write_all(b"late").expect("send");
            client
        });
        let mut buf = [0u8; 8];
        let n = polled.read(&mut buf).expect("waited for it");
        assert_eq!(&buf[..n], b"late");
        let _client = sender.join().expect("sent");
        polled.set_wait(Some(Duration::from_millis(10)));
        assert_eq!(
            polled.read(&mut buf).expect_err("quiet").kind(),
            io::ErrorKind::TimedOut
        );
    }
}
