// SPDX-License-Identifier: MIT OR Apache-2.0
//! The conformance tester's post office tests against this post office, through the driver file the tester
//! ships for it. P-08 and P-09 wait for the post office's mail to go out, up to 20 seconds each.

use std::path::Path;

use radiopost_tester::{ClientPlan, run_post_office};

#[test]
fn the_postoffice_passes_the_conformance_tests() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let driver = std::fs::read_to_string(root.join("core/tester/drivers/radiopost-po.json"))
        .expect("the driver file");
    let scratch = std::env::temp_dir().join(format!(
        "radiopost-conformance-postoffice-{}",
        std::process::id()
    ));
    let plan = ClientPlan {
        driver: serde_json::from_str(&driver).expect("the driver is JSON"),
        bin: env!("CARGO_BIN_EXE_radiopost-po").to_owned(),
        scratch: scratch.clone(),
        only: Vec::new(),
        slow: false,
    };
    let verdicts = run_post_office(&plan, |_| {}).expect("the tester runs");
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
            "P-01", "P-02", "P-03", "P-04", "P-05", "P-06", "P-07", "P-08", "P-09", "P-10"
        ]
    );
}
