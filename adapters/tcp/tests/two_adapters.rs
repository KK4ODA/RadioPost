// SPDX-License-Identifier: MIT OR Apache-2.0
//! Two TCP adapters joined as RP-4 §9.5 says, each with its host: links up, bytes both ways,
//! ended in order and at once, plain and over TLS, and each refusal.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener};
use std::time::Duration;

use radiopost_tcp::{Adapter, Config, Security, tls};
use rpmi::host::Host;
use rpmi::tcp::Connection;
use rpmi::wire::Message;
use serde_json::Value;

/// An adapter on free ports: where its host connects, and where it takes calls.
fn adapter(security: Security, peers: HashMap<String, String>) -> (SocketAddr, SocketAddr) {
    let a = Adapter::new(Config {
        security,
        peers,
        name: "radiopost-tcp under test".into(),
    });
    let hosts = TcpListener::bind("127.0.0.1:0").expect("a port");
    let calls = TcpListener::bind("127.0.0.1:0").expect("a port");
    let at = (
        hosts.local_addr().expect("bound"),
        calls.local_addr().expect("bound"),
    );
    a.serve_hosts(hosts);
    a.serve_calls(calls);
    at
}

struct Station {
    conn: Connection,
    host: Host,
}

impl Station {
    fn open(at: SocketAddr, callsigns: &[&str], listen: bool) -> Station {
        let conn = Connection::open(at).expect("the adapter");
        conn.set_timeout(Some(Duration::from_secs(10)))
            .expect("a timeout");
        let mut s = Station {
            conn,
            host: Host::new(),
        };
        let hello = s.host.hello("two_adapters");
        s.ask(&hello, "hello");
        let calls = s.host.callsigns(callsigns);
        s.ask(&calls, "ok");
        if listen {
            let on = s.host.listen(true);
            s.ask(&on, "ok");
        }
        s
    }

    fn ask(&mut self, m: &Message, op: &str) -> Message {
        self.conn.send(m).expect("sent");
        self.until(op)
    }

    /// The next message whose `op` is `op`, skipping `queued`, `incoming` and `ok`.
    fn until(&mut self, op: &str) -> Message {
        loop {
            let m = self
                .conn
                .read_message()
                .expect("the adapter answers")
                .expect("the adapter keeps the connection");
            let got = m["op"].as_str().unwrap_or_default().to_owned();
            if got == op {
                return m;
            }
            assert!(
                matches!(got.as_str(), "queued" | "incoming" | "ok"),
                "{got} where {op} was due: {m:?}"
            );
        }
    }

    fn call(&mut self, remote: &str) -> Message {
        let connect = self.host.connect(remote, None, None);
        self.ask(&connect, "ok");
        loop {
            let m = self.conn.read_message().expect("read").expect("open");
            if matches!(m["op"].as_str(), Some("connected" | "failed")) {
                return m;
            }
        }
    }

    fn send(&mut self, link: &str, data: &[u8]) {
        let m = self.host.send(link, data, None);
        self.conn.send(&m).expect("sent");
    }

    /// Joins `received` until `n` bytes have come.
    fn receive(&mut self, n: usize) -> Vec<u8> {
        let mut got = Vec::new();
        while got.len() < n {
            let m = self.until("received");
            let data = m["data"].as_str().expect("data");
            got.extend(rpmi::base64::decode(data).expect("base64"));
        }
        got
    }
}

fn text(m: &Message, key: &str) -> String {
    m.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn carry(near: &mut Station, far: &mut Station, remote: &str) {
    let up = near.call(remote);
    assert_eq!(text(&up, "op"), "connected", "{up:?}");
    assert_eq!(text(&up, "remote"), "N1CALL-10");
    assert_eq!(text(&up, "local"), "KK4ODA");
    assert_eq!(text(&up, "direction"), "out");
    let theirs = far.until("connected");
    assert_eq!(text(&theirs, "remote"), "KK4ODA");
    assert_eq!(text(&theirs, "local"), "N1CALL-10");
    assert_eq!(text(&theirs, "direction"), "in");
    let (near_link, far_link) = (text(&up, "link"), text(&theirs, "link"));

    let hello: Vec<u8> = (0..=255).cycle().take(5000).collect();
    near.send(&near_link, &hello);
    assert_eq!(far.receive(hello.len()), hello);
    far.send(&far_link, b"WELCOME and DONE");
    assert_eq!(near.receive(16), b"WELCOME and DONE");

    let bye = near.host.disconnect(&near_link);
    near.conn.send(&bye).expect("sent");
    let gone = near.until("disconnected");
    assert_eq!(text(&gone, "reason"), "local");
    let gone = far.until("disconnected");
    assert_eq!(text(&gone, "reason"), "remote");
}

#[test]
fn a_link_over_plain_tcp_carries_bytes_both_ways_and_ends_in_order() {
    let (far_rpmi, far_calls) = adapter(Security::Plain, HashMap::new());
    let peers = HashMap::from([("N1CALL-10".to_owned(), far_calls.to_string())]);
    let (near_rpmi, _) = adapter(Security::Plain, peers);
    let mut far = Station::open(far_rpmi, &["N1CALL-10"], true);
    let mut near = Station::open(near_rpmi, &["KK4ODA"], false);
    // by the operator's name for the station, by callsign and address, and by address alone
    carry(&mut near, &mut far, "N1CALL-10");
    carry(&mut near, &mut far, &format!("N1CALL-10@{far_calls}"));
    carry(&mut near, &mut far, &far_calls.to_string());
}

#[test]
fn a_link_over_tls_needs_the_certificate_trusted() {
    let (cert, key, print) = tls::self_signed().expect("a certificate");
    let (server, server_print) = tls::server(cert.as_bytes(), key.as_bytes()).expect("TLS");
    assert_eq!(server_print, print);
    let pin = tls::parse_fingerprint(&print).expect("a fingerprint");
    let (far_rpmi, far_calls) = adapter(
        Security::Tls {
            server: Some(server),
            client: tls::client(Vec::new()).expect("TLS"),
        },
        HashMap::new(),
    );
    let mut far = Station::open(far_rpmi, &["N1CALL-10"], true);

    let (trusting, _) = adapter(
        Security::Tls {
            server: None,
            client: tls::client(vec![pin]).expect("TLS"),
        },
        HashMap::new(),
    );
    let mut near = Station::open(trusting, &["KK4ODA"], false);
    carry(&mut near, &mut far, &format!("N1CALL-10@{far_calls}"));

    // a certificate of no web authority, and no fingerprint for it: no link
    let (wary, _) = adapter(
        Security::Tls {
            server: None,
            client: tls::client(Vec::new()).expect("TLS"),
        },
        HashMap::new(),
    );
    let mut stranger = Station::open(wary, &["W1AW"], false);
    let failed = stranger.call(&format!("N1CALL-10@{far_calls}"));
    assert_eq!(text(&failed, "op"), "failed", "{failed:?}");
    assert_eq!(text(&failed, "code"), "refused");
}

#[test]
fn calls_are_refused_as_rp4_9_5_says() {
    let (far_rpmi, far_calls) = adapter(Security::Plain, HashMap::new());
    let (near_rpmi, _) = adapter(Security::Plain, HashMap::new());
    let mut far = Station::open(far_rpmi, &["N1CALL-10", "N1CALL-11"], false);
    let mut near = Station::open(near_rpmi, &["KK4ODA"], false);
    let to = |call: &str| format!("{call}@{far_calls}");

    // not listening
    let failed = near.call(&to("N1CALL-10"));
    assert_eq!(
        (text(&failed, "op"), text(&failed, "code")),
        ("failed".into(), "refused".into())
    );
    let on = far.host.listen(true);
    far.ask(&on, "ok");
    // not its callsign
    let failed = near.call(&to("N9ZZZ"));
    assert_eq!(text(&failed, "code"), "refused");
    // a name nobody mapped
    let failed = near.call("N9ZZZ");
    assert_eq!(text(&failed, "code"), "refused");
    // an address alone: the host's first callsign answers
    let up = near.call(&far_calls.to_string());
    assert_eq!(text(&up, "remote"), "N1CALL-10");
    far.until("connected");
    // busy while that link is up
    let (third_rpmi, _) = adapter(Security::Plain, HashMap::new());
    let mut third = Station::open(third_rpmi, &["N2CALL-5"], false);
    let failed = third.call(&to("N1CALL-11"));
    assert_eq!(text(&failed, "code"), "busy");
    // ended at once: the other side hears `remote`
    let abort = near.host.abort(Some(&text(&up, "link")));
    near.conn.send(&abort).expect("sent");
    assert_eq!(text(&near.until("disconnected"), "reason"), "local");
    assert_eq!(text(&far.until("disconnected"), "reason"), "remote");
    // and the second callsign answers once the link is gone
    let up = third.call(&to("N1CALL-11"));
    assert_eq!(text(&up, "remote"), "N1CALL-11");

    // a second program is turned away: the adapter is not shared
    let mut other = Connection::open(near_rpmi).expect("the adapter");
    other
        .set_timeout(Some(Duration::from_secs(10)))
        .expect("a timeout");
    let mut host = Host::new();
    other.send(&host.hello("another program")).expect("sent");
    let busy = other.read_message().expect("read").expect("an answer");
    assert_eq!(text(&busy, "code"), "busy");
    assert!(other.read_message().expect("closed").is_none());
}
