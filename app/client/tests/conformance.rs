// SPDX-License-Identifier: MIT OR Apache-2.0
//! The conformance tester's client tests (C-01 to C-09, C-12) against this client, through the driver
//! file the tester ships for it. C-11 takes five minutes and runs only by hand:
//! `radiopost-tester client --driver core/tester/drivers/radiopost.json --bin <radiopost> --slow`.

use std::path::Path;

use radiopost_tester::{ClientPlan, run_client};

#[test]
fn the_client_passes_the_conformance_tests() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let driver = std::fs::read_to_string(root.join("core/tester/drivers/radiopost.json"))
        .expect("the driver file");
    let scratch =
        std::env::temp_dir().join(format!("radiopost-conformance-{}", std::process::id()));
    let plan = ClientPlan {
        driver: serde_json::from_str(&driver).expect("the driver is JSON"),
        bin: env!("CARGO_BIN_EXE_radiopost").to_owned(),
        scratch: scratch.clone(),
        only: Vec::new(),
        slow: false,
    };
    let verdicts = run_client(&plan, |_| {}).expect("the tester runs");
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
            "C-01", "C-02", "C-03", "C-04", "C-05", "C-06", "C-07", "C-08", "C-09", "C-12"
        ]
    );
}
