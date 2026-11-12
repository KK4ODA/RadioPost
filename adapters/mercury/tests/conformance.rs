// SPDX-License-Identifier: MIT OR Apache-2.0
//! The conformance tester's modem adapter tests (A-01 to A-04) against two adapters, each on a
//! stand-in Mercury modem, the two modems on one channel (`tests/common`).

mod common;

use common::Air;
use radiopost_mercury::serve::listen;
use radiopost_tester::adapter::{Plan, run};

#[test]
fn two_adapters_on_stand_in_modems_pass_a01_to_a04() {
    let (air, modems) = Air::new();
    let near = listen("127.0.0.1:0", &modems[0].to_string()).expect("a port");
    let far = listen("127.0.0.1:0", &modems[1].to_string()).expect("a port");
    let plan = Plan {
        at: near.to_string(),
        far: Some(far.to_string()),
        reach: "N1CALL-10".into(),
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
    // nothing was asked of the modems that a real Mercury answers badly
    assert_eq!(air.faults(), Vec::<String>::new());
}
