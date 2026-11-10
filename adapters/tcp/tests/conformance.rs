// SPDX-License-Identifier: MIT OR Apache-2.0
//! The conformance tester's modem adapter tests (A-01 to A-04) against two TCP adapters joined as
//! RP-4 §9.5 says, plain and over TLS.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener};

use radiopost_tcp::{Adapter, Config, Security, tls};
use radiopost_tester::adapter::{Plan, run};

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

fn passes(near: SocketAddr, far: SocketAddr, reach: String) {
    let plan = Plan {
        at: near.to_string(),
        far: Some(far.to_string()),
        reach,
        only: Vec::new(),
    };
    let verdicts = run(&plan, |_| {}).expect("the tester runs");
    let failed: Vec<String> = verdicts
        .iter()
        .filter(|v| v.passed != Some(true))
        .map(|v| format!("{}: {}\n  {}", v.id, v.why, v.transcript.join("\n  ")))
        .collect();
    assert_eq!(failed, Vec::<String>::new());
    assert_eq!(verdicts.len(), 4);
}

#[test]
fn plain_tcp_adapters_pass_a01_to_a04() {
    let (far, far_calls) = adapter(Security::Plain, HashMap::new());
    let (near, _) = adapter(Security::Plain, HashMap::new());
    passes(near, far, format!("N1CALL-10@{far_calls}"));
}

#[test]
fn tls_adapters_pass_a01_to_a04() {
    let (cert, key, print) = tls::self_signed().expect("a certificate");
    let (server, _) = tls::server(cert.as_bytes(), key.as_bytes()).expect("TLS");
    let client = || tls::client(vec![tls::parse_fingerprint(&print).expect("a fingerprint")]);
    let (far, far_calls) = adapter(
        Security::Tls {
            server: Some(server),
            client: client().expect("TLS"),
        },
        HashMap::new(),
    );
    let peers = HashMap::from([("N1CALL-10".to_owned(), far_calls.to_string())]);
    let (near, _) = adapter(
        Security::Tls {
            server: None,
            client: client().expect("TLS"),
        },
        peers,
    );
    passes(near, far, "N1CALL-10".into());
}
