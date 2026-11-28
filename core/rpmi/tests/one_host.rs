// SPDX-License-Identifier: MIT OR Apache-2.0
//! An adapter that is not shared turns a second host away `busy` only while the first is still
//! connected (RP-4 §3). A host that leaves and comes back at once, or a program that hands the
//! modem to the next at once, has closed its first connection before it says `hello` on the
//! second; the adapter must not answer `busy` because it has not yet read that close. Both of
//! rpmi's hosts' sides are checked: the lobby the Mercury, VARA, packet and ARDOP adapters share,
//! and the simulated channel's TCP server (the loopback adapter).

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use rpmi::channel::{Channel, StationConfig};
use rpmi::lobby::{Arrival, FromHost, hosts};
use rpmi::tcp::Server;
use rpmi::wire::Caps;

/// How many times a host hands over to the next. Without the wait for a close not yet read, the
/// lobby lost a handover in the first few dozen, the simulated modem's server one in a few hundred.
const ROUNDS: usize = 500;
const WAIT: Duration = Duration::from_secs(5);

fn hello(at: SocketAddr) -> (TcpStream, BufReader<TcpStream>) {
    let mut s = TcpStream::connect(at).expect("the adapter listens");
    s.set_read_timeout(Some(WAIT)).expect("a timeout");
    s.write_all(b"{\"op\":\"hello\",\"id\":\"h\",\"version\":[1,1],\"app\":\"test\"}\n")
        .expect("hello is written");
    let reader = BufReader::new(s.try_clone().expect("a clone"));
    (s, reader)
}

fn read(reader: &mut BufReader<TcpStream>) -> String {
    let mut line = String::new();
    let _ = reader.read_line(&mut line);
    line
}

/// The station's side of the lobby: the host's `hello`, as it arrives.
fn arrive(arrivals: &Receiver<Arrival>, round: usize, said: impl FnOnce() -> String) -> Arrival {
    let Ok(arrival) = arrivals.recv_timeout(WAIT) else {
        panic!(
            "round {round}: the host did not arrive; it was told {:?}",
            said()
        );
    };
    match arrival.lines.recv_timeout(WAIT) {
        Ok(FromHost::Line(line)) => assert!(line.starts_with(b"{\"op\":\"hello\"")),
        _ => panic!("round {round}: the arrival carried no hello"),
    }
    arrival
}

#[test]
fn the_lobby_takes_a_host_that_comes_as_another_leaves() {
    let (at, arrivals) = hosts("127.0.0.1:0").expect("a port");
    let (mut host, mut reader) = hello(at);
    let mut arrival = arrive(&arrivals, 0, String::new);
    for round in 1..=ROUNDS {
        drop(host);
        drop(reader);
        let (next, mut next_reader) = hello(at);
        let next_arrival = arrive(&arrivals, round, || read(&mut next_reader));
        drop(arrival);
        (host, reader, arrival) = (next, next_reader, next_arrival);
    }
    // a second host while the first is connected is still turned away
    let (_second, mut second_reader) = hello(at);
    let said = read(&mut second_reader);
    assert!(said.contains("\"busy\""), "{said}");
    drop((host, reader, arrival));
}

#[test]
fn the_simulated_modem_takes_a_host_that_comes_as_another_leaves() {
    let mut ch = Channel::new();
    let station = ch.add_station(StationConfig {
        name: "KK4ODA".into(),
        modem: "a simulated modem".into(),
        caps: Caps {
            bandwidths: vec![2300],
            max_links: 1,
            delivery: true,
            broadcast: true,
            quality: false,
            busy: true,
            regulatory: false,
            max_queue: 65536,
            shared: false,
            dial: false,
            scan: false,
            call_s: false,
            tone: false,
            heard: false,
            beacon: false,
            position: false,
        },
        prefix: "c".into(),
        frame_bytes: 128,
        dial_hz: None,
    });
    let server = Server::new(ch, WAIT);
    let at = server.listen(station, "127.0.0.1:0").expect("a port");
    let (mut host, mut reader) = hello(at);
    assert!(read(&mut reader).contains("\"op\":\"hello\""));
    for round in 1..=ROUNDS {
        drop(host);
        drop(reader);
        let (next, mut next_reader) = hello(at);
        let said = read(&mut next_reader);
        assert!(said.contains("\"op\":\"hello\""), "round {round}: {said}");
        (host, reader) = (next, next_reader);
    }
    let (_second, mut second_reader) = hello(at);
    let said = read(&mut second_reader);
    assert!(said.contains("\"busy\""), "{said}");
    drop((host, reader));
}
