// SPDX-License-Identifier: MIT OR Apache-2.0
//! A host's connection to an adapter (`rpmi::tcp::Connection`), against a plain socket standing in
//! for the adapter: a wait that runs out is only a wait, and a line that comes in pieces across one
//! is read whole.
//!
//! The connection used to wait with a read timeout on its socket. On Windows such a read can fail
//! with os error 997 and lose what it had, and on every platform a wait that ran out mid-line
//! dropped the line's first part: the next read found the rest and called it not JSON.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use rpmi::tcp::Connection;
use serde_json::json;

/// A host's connection and the adapter's end of it.
fn pair() -> (Connection, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let at = listener.local_addr().expect("bound");
    let conn = Connection::open(at).expect("connected");
    let (adapter, _) = listener.accept().expect("accepted");
    (conn, adapter)
}

#[test]
fn a_line_that_comes_in_pieces_across_a_wait_is_read_whole() {
    let (mut conn, mut adapter) = pair();
    conn.set_timeout(Some(Duration::from_millis(50)))
        .expect("a wait");
    let line = json!({"op": "received", "link": "a1", "data": "QUJD".repeat(2000)}).to_string();
    let (first, rest) = line.split_at(line.len() / 2);
    adapter.write_all(first.as_bytes()).expect("half");
    adapter.flush().expect("sent");
    // the wait runs out with half a line in: a wait, not a fault
    let error = conn.read_message().expect_err("nothing whole yet");
    assert_eq!(error.kind(), ErrorKind::TimedOut, "{error}");
    adapter.write_all(rest.as_bytes()).expect("the rest");
    adapter.write_all(b"\n").expect("its LF");
    let deadline = Instant::now() + Duration::from_secs(5);
    let m = loop {
        match conn.read_message() {
            Ok(Some(m)) => break m,
            Ok(None) => panic!("the connection closed"),
            Err(e) if e.kind() == ErrorKind::TimedOut => {}
            Err(e) => panic!("the line was lost: {e}"),
        }
        assert!(Instant::now() < deadline, "the line never came");
    };
    assert_eq!(serde_json::Value::Object(m).to_string(), line);
}

#[test]
fn a_quiet_connection_is_waited_on_and_still_carries_lines() {
    let (mut conn, mut adapter) = pair();
    conn.set_timeout(Some(Duration::from_millis(10)))
        .expect("a wait");
    for _ in 0..30 {
        let error = conn.read_message().expect_err("quiet");
        assert_eq!(error.kind(), ErrorKind::TimedOut, "{error}");
    }
    adapter
        .write_all(b"{\"op\":\"busy\",\"busy\":true}\n")
        .expect("a line");
    conn.set_timeout(Some(Duration::from_secs(5)))
        .expect("a wait");
    let m = conn.read_message().expect("read").expect("a line");
    assert_eq!(m["op"], "busy");
    // with no wait, a read waits for as long as it takes
    conn.set_timeout(None).expect("no wait");
    let late = thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        adapter
            .write_all(b"{\"op\":\"busy\",\"busy\":false}\n")
            .expect("a line");
        adapter
    });
    let m = conn.read_message().expect("read").expect("a line");
    assert_eq!(m["busy"], false);
    let _adapter = late.join().expect("written");
}

#[test]
fn the_adapter_closing_reads_as_the_end_and_closing_reaches_the_adapter() {
    let (mut conn, adapter) = pair();
    conn.set_timeout(Some(Duration::from_millis(10)))
        .expect("a wait");
    drop(adapter);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match conn.read_message() {
            Ok(None) => break,
            Ok(Some(m)) => panic!("nothing was sent: {m:?}"),
            Err(e) if e.kind() == ErrorKind::TimedOut => {}
            Err(e) => panic!("a close is not a fault: {e}"),
        }
        assert!(Instant::now() < deadline, "the close was never read");
    }

    // the host closes: the adapter's end reads the close at once
    let (conn, mut adapter) = pair();
    conn.close();
    adapter
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("a timeout");
    let mut buf = [0u8; 8];
    assert_eq!(adapter.read(&mut buf).expect("the close"), 0);
}
