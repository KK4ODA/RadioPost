// SPDX-License-Identifier: MIT OR Apache-2.0
//! The conformance tester's gateway tests against this gateway, through the driver file the tester
//! ships for it. G-07's 300-second wait runs only by hand: `radiopost-tester gateway --driver core/tester/drivers/radiopost-gateway.json --bin <radiopost-gateway> --slow`.

use std::path::Path;

use radiopost_tester::{ClientPlan, run_gateway};

#[test]
fn the_gateway_passes_the_conformance_tests() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let driver = std::fs::read_to_string(root.join("core/tester/drivers/radiopost-gateway.json"))
        .expect("the driver file");
    let scratch = std::env::temp_dir().join(format!(
        "radiopost-conformance-gateway-{}",
        std::process::id()
    ));
    let plan = ClientPlan {
        driver: serde_json::from_str(&driver).expect("the driver is JSON"),
        bin: env!("CARGO_BIN_EXE_radiopost-gateway").to_owned(),
        scratch: scratch.clone(),
        only: Vec::new(),
        slow: false,
    };
    let verdicts = run_gateway(&plan, |_| {}).expect("the tester runs");
    let _ = std::fs::remove_dir_all(&scratch);
    let failed: Vec<String> = verdicts
        .iter()
        .filter(|v| v.passed == Some(false))
        .map(|v| format!("{}: {}\n  {}", v.id, v.why, v.transcript.join("\n  ")))
        .collect();
    assert_eq!(failed, Vec::<String>::new());
    let ran: Vec<&str> = verdicts
        .iter()
        .filter(|v| v.passed == Some(true))
        .map(|v| v.id)
        .collect();
    assert_eq!(
        ran,
        [
            "G-01", "G-02", "G-03", "G-04", "G-05", "G-07", "G-08", "G-09"
        ]
    );
}
