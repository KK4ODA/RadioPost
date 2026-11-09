// SPDX-License-Identifier: MIT OR Apache-2.0
//! RadioPost's conformance tester (`docs/design/01-conformance.md`): it plays the other side of the
//! protocols against a program and says, test by test, whether the program did what the
//! specifications require, and where it did not.
//!
//! It holds the client's tests C-01 to C-08 and C-11 ([`client`]), run through the tester's own
//! RPMI adapter ([`harness::TestAdapter`]).

pub mod client;
pub mod harness;

use std::path::PathBuf;
use std::time::Instant;

use serde_json::{Value as Json, json};

/// The test post office keys of `vectors/keys.json`, built in: their private halves are published,
/// so they are never real keys.
pub const KEYS_JSON: &str = include_str!("../../../vectors/keys.json");

/// One test's verdict.
pub struct Verdict {
    /// Its ID (`C-01`).
    pub id: &'static str,
    /// What it checks, with its sections.
    pub checks: &'static str,
    /// `Some(true)` passed, `Some(false)` failed, `None` skipped.
    pub passed: Option<bool>,
    /// Why it failed or was skipped, in a sentence or two.
    pub why: String,
    /// What crossed between the tester and the program.
    pub transcript: Vec<String>,
    /// How long it took.
    pub seconds: f64,
}

/// A test of the client role.
pub struct ClientTest {
    /// Its ID.
    pub id: &'static str,
    /// What it checks.
    pub checks: &'static str,
    /// It takes minutes: run only when asked.
    pub slow: bool,
    run: fn(&mut client::Run<'_>, &mut Vec<String>) -> harness::Outcome,
}

/// The client's tests, in order.
pub const CLIENT_TESTS: &[ClientTest] = &[
    ClientTest {
        id: "C-01",
        checks: "An ordinary session: HELLO signed, offers, MSG pieces, RECEIPT, REQUEST, DONE, the hang-up (RP-3 §3, §5, §8)",
        slow: false,
        run: |r, t| r.c01(t),
    },
    ClientTest {
        id: "C-02",
        checks: "A challenge answered with AUTH (RP-3 §5.3, RP-1 §6.1)",
        slow: false,
        run: |r, t| r.c02(t),
    },
    ClientTest {
        id: "C-03",
        checks: "The credential by hash, then in full when asked (RP-3 §5.1, §5.8)",
        slow: false,
        run: |r, t| r.c03(t),
    },
    ClientTest {
        id: "C-04",
        checks: "An upload and a download broken and resumed, through the same gateway and another (RP-3 §7)",
        slow: false,
        run: |r, t| r.c04(t),
    },
    ClientTest {
        id: "C-05",
        checks: "Offers declined and messages failed: nothing sent that was not accepted (RP-3 §5.2, §5.4, §5.9)",
        slow: false,
        run: |r, t| r.c05(t),
    },
    ClientTest {
        id: "C-06",
        checks: "Bad frames answered ERROR malformed or too-large, and the session ended (RP-3 §2, §4)",
        slow: false,
        run: |r, t| r.c06(t),
    },
    ClientTest {
        id: "C-07",
        checks: "Calling a station: its proof checked first, a bad one refused (RP-3 §10, RP-1 §6.5)",
        slow: false,
        run: |r, t| r.c07(t),
    },
    ClientTest {
        id: "C-08",
        checks: "Called by a station: its own proof given, only mail for its account taken (RP-3 §10)",
        slow: false,
        run: |r, t| r.c08(t),
    },
    ClientTest {
        id: "C-11",
        checks: "300 seconds of nothing end the session, and the caller hangs up (RP-3 §8)",
        slow: true,
        run: |r, t| r.c11(t),
    },
];

/// What to run against a client.
pub struct ClientPlan {
    /// The program's driver file, read.
    pub driver: Json,
    /// The program.
    pub bin: String,
    /// Where each test's home goes.
    pub scratch: PathBuf,
    /// Only these tests, when not empty.
    pub only: Vec<String>,
    /// Run the slow tests too.
    pub slow: bool,
}

fn wanted(id: &str, only: &[String]) -> bool {
    only.is_empty() || only.iter().any(|o| o.eq_ignore_ascii_case(id))
}

/// Runs the client's tests, telling `progress` of each verdict as it comes.
///
/// # Errors
///
/// The driver or the test keys cannot be read.
pub fn run_client(
    plan: &ClientPlan,
    mut progress: impl FnMut(&Verdict),
) -> Result<Vec<Verdict>, String> {
    let driver = harness::Driver::new(&plan.driver, &plan.bin)?;
    let mut run = client::Run {
        driver: &driver,
        offices: harness::PostOffices::from_json(KEYS_JSON)?,
        scratch: plan.scratch.clone(),
    };
    let mut verdicts = Vec::new();
    for t in CLIENT_TESTS {
        if !wanted(t.id, &plan.only) {
            continue;
        }
        let started = Instant::now();
        let mut transcript = Vec::new();
        let (passed, why) = if t.slow && !plan.slow && plan.only.is_empty() {
            (None, "slow: run with --slow".to_owned())
        } else {
            match (t.run)(&mut run, &mut transcript) {
                Ok(()) => (Some(true), String::new()),
                Err(e) => (Some(false), e),
            }
        };
        let v = Verdict {
            id: t.id,
            checks: t.checks,
            passed,
            why,
            transcript,
            seconds: started.elapsed().as_secs_f64(),
        };
        progress(&v);
        verdicts.push(v);
    }
    Ok(verdicts)
}

/// The summary in JSON (the outline's §4).
#[must_use]
pub fn summary(role: &str, program: &str, verdicts: &[Verdict]) -> Json {
    let tests: Vec<Json> = verdicts
        .iter()
        .map(|v| {
            json!({
                "id": v.id,
                "checks": v.checks,
                "result": match v.passed {
                    Some(true) => "pass",
                    Some(false) => "fail",
                    None => "skipped",
                },
                "why": v.why,
                "seconds": (v.seconds * 10.0).round() / 10.0,
                "transcript": v.transcript,
            })
        })
        .collect();
    json!({
        "tester": format!("radiopost-tester {}", env!("CARGO_PKG_VERSION")),
        "role": role,
        "program": program,
        "passed": verdicts.iter().all(|v| v.passed != Some(false)),
        "tests": tests,
    })
}
