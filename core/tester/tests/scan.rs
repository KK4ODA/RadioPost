// SPDX-License-Identifier: MIT OR Apache-2.0
//! A-06 (RP-4 §4.2) against the simulated channel's adapter with a modem that scans, shared and
//! not: the test that each adapter without `caps.scan` passes over.

use std::time::Duration;

use radiopost_tester::adapter::{Plan, run};
use rpmi::channel::{Channel, StationConfig};
use rpmi::tcp::Server;
use rpmi::wire::Caps;

fn scanning(shared: bool) -> (Server, String) {
    let mut ch = Channel::new();
    let s = ch.add_station(StationConfig {
        name: "N1CALL".into(),
        modem: "the loopback adapter".into(),
        caps: Caps {
            bandwidths: vec![2300],
            max_links: 1,
            delivery: true,
            broadcast: true,
            quality: false,
            busy: true,
            regulatory: false,
            max_queue: 65_536,
            shared,
            dial: true,
            scan: true,
            call_s: true,
            tone: false,
        },
        prefix: "g".into(),
        frame_bytes: 128,
        dial_hz: Some(14_105_000),
    });
    let server = Server::new(ch, Duration::from_secs(10));
    let at = server.listen(s, "127.0.0.1:0").expect("a port");
    (server, at.to_string())
}

fn a06(at: String) -> Vec<String> {
    let plan = Plan {
        at,
        far: None,
        reach: String::new(),
        only: vec!["A-06".into()],
    };
    let verdicts = run(&plan, |_| {}).expect("the tester runs");
    assert_eq!(verdicts.len(), 1);
    verdicts
        .iter()
        .filter(|v| v.passed != Some(true))
        .map(|v| format!("{}: {}\n  {}", v.id, v.why, v.transcript.join("\n  ")))
        .collect()
}

#[test]
fn a_shared_adapter_that_scans_passes_a06() {
    let (_server, at) = scanning(true);
    assert_eq!(a06(at), Vec::<String>::new());
}

#[test]
fn an_adapter_of_one_host_that_scans_passes_a06() {
    let (_server, at) = scanning(false);
    assert_eq!(a06(at), Vec::<String>::new());
}
