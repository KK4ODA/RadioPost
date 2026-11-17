// SPDX-License-Identifier: MIT OR Apache-2.0
//! The conformance tester's modem adapter tests (A-01 to A-04) against two packet adapters, each
//! on a stand-in TNC's AGWPE port, the two TNCs on one channel (`tests/common`).

mod common;

use std::thread;
use std::time::{Duration, Instant};

use common::Air;
use radiopost_packet::serve::{listen, quiet};
use radiopost_tester::adapter::{Plan, run};

#[test]
fn two_adapters_on_stand_in_tncs_pass_a01_to_a04() {
    let (air, tncs) = Air::new();
    let near = listen("127.0.0.1:0", &tncs[0].to_string(), 0, quiet()).expect("a port");
    let far = listen("127.0.0.1:0", &tncs[1].to_string(), 0, quiet()).expect("a port");
    let plan = Plan {
        at: near.to_string(),
        far: Some(far.to_string()),
        reach: "N1CALL-10".into(),
        only: Vec::new(),
    };
    let verdicts = run(&plan, |_| {}).expect("the tester runs");
    let failed: Vec<String> = verdicts
        .iter()
        // A-06 is for an adapter that scans (RP-4 §4.2): this one does not
        .filter(|v| v.passed != Some(true) && !(v.id == "A-06" && v.passed.is_none()))
        .map(|v| format!("{}: {}\n  {}", v.id, v.why, v.transcript.join("\n  ")))
        .collect();
    assert_eq!(failed, Vec::<String>::new());
    assert_eq!(verdicts.len(), 5);
    // the hosts gone, the TNCs hold none of their callsigns and none of their connections
    let until = Instant::now() + Duration::from_secs(10);
    while (air.programs(0) + air.programs(1) > 0) && Instant::now() < until {
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!((air.programs(0), air.programs(1)), (0, 0));
    assert_eq!(air.faults(), Vec::<String>::new());
}
