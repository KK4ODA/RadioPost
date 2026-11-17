// SPDX-License-Identifier: MIT OR Apache-2.0
//! The conformance tester's modem adapter tests (A-01 to A-04) against two VARA adapters, each on
//! a stand-in modem that speaks as VARA's published TNC commands say (Mercury's stand-in, in its
//! VARA mode: `../mercury/tests/common`).

#[path = "../../mercury/tests/common/mod.rs"]
mod common;

use std::sync::Arc;

use common::Air;
use radiopost_mercury::bridge::Dialect;
use radiopost_mercury::serve::listen_speaking;
use radiopost_tester::adapter::{Plan, run};

#[test]
fn two_adapters_on_stand_in_vara_modems_pass_a01_to_a04() {
    let (air, modems) = Air::vara();
    let quiet: radiopost_mercury::serve::Note = Arc::new(|_: &str| {});
    let at = |m: usize| {
        listen_speaking(
            "127.0.0.1:0",
            &modems[m].to_string(),
            Dialect::VaraHf,
            Arc::clone(&quiet),
        )
        .expect("a port")
    };
    let (near, far) = (at(0), at(1));
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
    // VARA identifies the station itself: the adapters sent no CQFRAME, and asked nothing of the
    // modems that VARA's published commands do not have
    assert_eq!(air.idents(), Vec::<(usize, String)>::new());
    assert_eq!(air.faults(), Vec::<String>::new());
}
