// SPDX-License-Identifier: MIT OR Apache-2.0
//! The TCP adapter serves one host (RP-4 §3): a second is turned away `busy` only while the first
//! is still connected. A host that leaves and another that comes at once (the conformance tester
//! between two tests, a client between two calls) must find the modem free, though the adapter may
//! not yet have read the first one's close.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use radiopost_tcp::{Adapter, Config, Security};

/// How many times a host hands over to the next. Without the wait for a close not yet read, the
/// lobby lost a handover in the first few dozen and the TCP adapter about one in three hundred.
const ROUNDS: usize = 500;

fn adapter() -> SocketAddr {
    let a = Adapter::new(Config {
        security: Security::Plain,
        peers: HashMap::new(),
        name: "radiopost-tcp under test".into(),
    });
    let hosts = TcpListener::bind("127.0.0.1:0").expect("a port");
    let at = hosts.local_addr().expect("bound");
    a.serve_hosts(hosts);
    at
}

/// Connects, says `hello`, and reads the answer.
fn hello(at: SocketAddr) -> (TcpStream, String) {
    let mut s = TcpStream::connect(at).expect("the adapter listens");
    s.set_read_timeout(Some(Duration::from_secs(5)))
        .expect("a timeout");
    s.write_all(b"{\"op\":\"hello\",\"id\":\"h\",\"version\":[1,1],\"app\":\"test\"}\n")
        .expect("hello is written");
    let mut line = String::new();
    let _ = BufReader::new(s.try_clone().expect("a clone")).read_line(&mut line);
    (s, line)
}

#[test]
fn a_host_that_comes_as_another_leaves_has_the_modem() {
    let at = adapter();
    let (mut host, said) = hello(at);
    assert!(said.contains("\"op\":\"hello\""), "{said}");
    for round in 1..=ROUNDS {
        drop(host);
        let (next, said) = hello(at);
        assert!(said.contains("\"op\":\"hello\""), "round {round}: {said}");
        host = next;
    }
    // a second host while the first is connected is still turned away
    let (_second, said) = hello(at);
    assert!(said.contains("\"busy\""), "{said}");
    drop(host);
}
