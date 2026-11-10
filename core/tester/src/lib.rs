// SPDX-License-Identifier: MIT OR Apache-2.0
//! RadioPost's conformance tester (`docs/design/01-conformance.md`): it plays the other side of the
//! protocols against a program and says, test by test, whether the program did what the
//! specifications require, and where it did not.
//!
//! It holds the client's tests C-01 to C-08, C-11 and C-12 ([`client`]), run through the
//! tester's own RPMI adapter ([`harness::TestAdapter`]); the gateway's G-01, G-02, G-04, G-07,
//! G-08 and G-09 ([`gateway`]), with the tester's own post office behind ([`office`]); the post
//! office's P-01 to P-04 and P-07 to P-10 ([`postoffice`]); and the modem adapter's A-01 to A-04
//! ([`adapter`]).

pub mod adapter;
pub mod client;
pub mod gateway;
pub mod harness;
pub mod mailserver;
pub mod office;
pub mod postoffice;

use std::path::PathBuf;
use std::time::Instant;

use serde_json::{Value as Json, json};

/// The test post office keys of `vectors/keys.json`, built in: their private halves are published,
/// so they are never real keys.
pub const KEYS_JSON: &str = include_str!("../../../vectors/keys.json");

/// The RPMI lines an adapter reads, and its answers (`vectors/rpmi-lines.json`).
pub const RPMI_LINES_JSON: &str = include_str!("../../../vectors/rpmi-lines.json");
/// The door's cases (`vectors/email-door.json`), for P-08.
pub const EMAIL_DOOR_JSON: &str = include_str!("../../../vectors/email-door.json");
/// Emails and the messages they make (`vectors/email-in.json`), for P-09.
pub const EMAIL_IN_JSON: &str = include_str!("../../../vectors/email-in.json");
/// `session-01`, which A-04 carries between two instances of an adapter.
pub const SESSION_01_JSON: &str = include_str!("../../../vectors/session-01.json");

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
        id: "C-12",
        checks: "A station-to-station session reported to the post office with both proofs (RP-5 §6.6)",
        slow: false,
        run: |r, t| r.c12(t),
    },
    ClientTest {
        id: "C-11",
        checks: "300 seconds of nothing end the session, and the caller hangs up (RP-3 §8)",
        slow: true,
        run: |r, t| r.c11(t),
    },
];

/// A test of the gateway role.
pub struct GatewayTest {
    /// Its ID.
    pub id: &'static str,
    /// What it checks.
    pub checks: &'static str,
    run: fn(&mut gateway::Run<'_>, &mut Vec<String>, bool) -> harness::Outcome,
}

/// The gateway's tests, in order (Phase 2: a gateway online).
pub const GATEWAY_TESTS: &[GatewayTest] = &[
    GatewayTest {
        id: "G-01",
        checks: "An online session: the post office asked while the link turns around, WELCOME, mail both ways, pickups reported, the session closed (RP-3 §9, RP-5 §5.2–§5.7)",
        run: |r, t, _| r.g01(t),
    },
    GatewayTest {
        id: "G-02",
        checks: "Credentials and statements refused in order, each with its code; a skewed clock or a replay answered with a challenge (RP-1 §5, §6.3, RP-3 §5.9)",
        run: |r, t, _| r.g02(t),
    },
    GatewayTest {
        id: "G-04",
        checks: "A download resumed at another gateway, an upload at the same one (RP-3 §7)",
        run: |r, t, _| r.g04(t),
    },
    GatewayTest {
        id: "G-07",
        checks: "A misbehaving caller: a frame over the limit, an unknown type, a message not accepted, and (--slow) a caller that never hangs up (RP-3 §2, §4, §5.4, §8)",
        run: |r, t, slow| r.g07(t, slow),
    },
    GatewayTest {
        id: "G-08",
        checks: "A credential renewed by radio (RP-3 §5.8, RP-5 §6.2)",
        run: |r, t, _| r.g08(t),
    },
    GatewayTest {
        id: "G-09",
        checks: "Post offices found in the DNS it is given, and a blocked one unknown (RP-1 §7.1, §7.3)",
        run: |r, t, _| r.g09(t),
    },
];

/// Runs the gateway's tests, telling `progress` of each verdict as it comes.
///
/// # Errors
///
/// The driver or the test keys cannot be read.
pub fn run_gateway(
    plan: &ClientPlan,
    mut progress: impl FnMut(&Verdict),
) -> Result<Vec<Verdict>, String> {
    let driver = harness::Driver::new(&plan.driver, &plan.bin)?;
    let mut run = gateway::Run {
        driver: &driver,
        offices: harness::PostOffices::from_json(KEYS_JSON)?,
        scratch: plan.scratch.clone(),
    };
    let mut verdicts = Vec::new();
    for t in GATEWAY_TESTS {
        if !wanted(t.id, &plan.only) {
            continue;
        }
        let started = Instant::now();
        let mut transcript = Vec::new();
        let slow = plan.slow || plan.only.iter().any(|o| o.eq_ignore_ascii_case(t.id));
        let (passed, why) = match (t.run)(&mut run, &mut transcript, slow && plan.slow) {
            Ok(()) => (Some(true), String::new()),
            Err(e) => (Some(false), e),
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

/// A test of the post office role.
pub struct PostOfficeTest {
    /// Its ID.
    pub id: &'static str,
    /// What it checks.
    pub checks: &'static str,
    run: fn(&mut postoffice::Run<'_>, &mut Vec<String>) -> harness::Outcome,
}

/// The post office's tests, in order (Phase 2).
pub const POST_OFFICE_TESTS: &[PostOfficeTest] = &[
    PostOfficeTest {
        id: "P-01",
        checks: "Its public documents as JWS under its key, and its DNS records (RP-1 §7, RP-5 §4)",
        run: |r, t| r.p01(t),
    },
    PostOfficeTest {
        id: "P-02",
        checks: "Logging in and tokens; 401 against 403 (RP-5 §2, §3)",
        run: |r, t| r.p02(t),
    },
    PostOfficeTest {
        id: "P-03",
        checks: "Enrollment with a code, and renewal (RP-5 §6.1, §6.2)",
        run: |r, t| r.p03(t),
    },
    PostOfficeTest {
        id: "P-04",
        checks: "Sessions: the checks in order, messages taken and refused, pickups, a challenge taken once (RP-5 §5.2–§5.7)",
        run: |r, t| r.p04(t),
    },
    PostOfficeTest {
        id: "P-07",
        checks: "Station-to-station sessions reported (RP-5 §6.6)",
        run: |r, t| r.p07(t),
    },
    PostOfficeTest {
        id: "P-08",
        checks: "The door: each refusal in order, and the accept list (RP-6 §2.1, §2.2)",
        run: |r, t| r.p08(t),
    },
    PostOfficeTest {
        id: "P-09",
        checks: "Email to message and back, Radio-Path, the part between post offices, a bounce (RP-6 §2.4, §3, §4)",
        run: |r, t| r.p09(t),
    },
    PostOfficeTest {
        id: "P-10",
        checks: "Third-party mail flagged (RP-6 §5.1, RP-5 §5.2)",
        run: |r, t| r.p10(t),
    },
];

/// Runs the post office's tests, telling `progress` of each verdict as it comes.
///
/// # Errors
///
/// The driver or the test keys cannot be read.
pub fn run_post_office(
    plan: &ClientPlan,
    mut progress: impl FnMut(&Verdict),
) -> Result<Vec<Verdict>, String> {
    let driver = harness::Driver::new(&plan.driver, &plan.bin)?;
    let mut run = postoffice::Run {
        driver: &driver,
        offices: harness::PostOffices::from_json(KEYS_JSON)?,
        scratch: plan.scratch.clone(),
    };
    let mut verdicts = Vec::new();
    for t in POST_OFFICE_TESTS {
        if !wanted(t.id, &plan.only) {
            continue;
        }
        let started = Instant::now();
        let mut transcript = Vec::new();
        let (passed, why) = match (t.run)(&mut run, &mut transcript) {
            Ok(()) => (Some(true), String::new()),
            Err(e) => (Some(false), e),
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
