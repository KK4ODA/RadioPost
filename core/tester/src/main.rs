// SPDX-License-Identifier: MIT OR Apache-2.0
//! `radiopost-tester`: the conformance tester's command line.

use std::path::PathBuf;
use std::process::ExitCode;

use radiopost_tester::{ClientPlan, Verdict, run_client, summary};

const USAGE: &str = "\
usage:
  radiopost-tester client --driver FILE --bin PROGRAM [--only ID,...] [--slow] [--json FILE]
                          [--scratch DIR]
      test a client (C-01 to C-08, C-11) through the tester's own modem adapter. The driver file
      says how to run the program (core/tester/drivers/radiopost.json is the radiopost client's);
      the program is put in conformance mode in a fresh home for each test. C-11 takes five
      minutes: it runs with --slow, or when named in --only.
  radiopost-tester gateway --driver FILE --bin PROGRAM [--only ID,...] [--slow] [--json FILE]
                           [--scratch DIR]
      test a gateway online (G-01, G-02, G-04, G-07 to G-09): the tester calls it through its
      own modem adapter and runs the post office behind it (core/tester/drivers/
      radiopost-gateway.json is radiopost-gateway's driver). G-07's 300-second wait runs with
      --slow.
  radiopost-tester post-office --driver FILE --bin PROGRAM [--only ID,...] [--json FILE]
                               [--scratch DIR]
      test a post office (P-01 to P-04, P-07 to P-10): the tester starts it in a fresh directory
      for each test, and plays its users, a gateway, another post office's user and the mail
      server around it (core/tester/drivers/radiopost-po.json is radiopost-po's driver).
  radiopost-tester adapter --at HOST:PORT [--far HOST:PORT] [--reach REMOTE] [--only ID,...]
                           [--json FILE]
      test a modem adapter (A-01 to A-04): the one whose hosts connect at --at, and for A-02 and
      A-04 a second instance at --far, joined to the first, whose host --reach names in
      `connect` (N1CALL-10 when not given; for the TCP adapter, N1CALL-10@HOST:PORT).

Exit status: 0 when every test run passed, 1 when one failed, 2 when the tester could not run.";

struct Args {
    words: Vec<String>,
}

impl Args {
    fn one(&self, flag: &str) -> Option<String> {
        let i = self.words.iter().position(|w| w == flag)?;
        self.words.get(i + 1).cloned()
    }

    fn has(&self, flag: &str) -> bool {
        self.words.iter().any(|w| w == flag)
    }

    fn list(&self, flag: &str) -> Vec<String> {
        self.one(flag)
            .map(|s| s.split(',').map(|x| x.trim().to_owned()).collect())
            .unwrap_or_default()
    }
}

fn say(v: &Verdict) {
    let result = match v.passed {
        Some(true) => "pass",
        Some(false) => "FAIL",
        None => "skip",
    };
    println!("{} {result} {:6.1} s  {}", v.id, v.seconds, v.checks);
    if v.passed != Some(true) && !v.why.is_empty() {
        for line in v.why.lines() {
            println!("     {line}");
        }
    }
    if v.passed == Some(false) {
        println!("     what crossed the link:");
        for line in &v.transcript {
            println!("       {line}");
        }
    }
}

fn finish(role: &str, program: &str, verdicts: &[Verdict], json: Option<String>) -> ExitCode {
    let failed = verdicts.iter().filter(|v| v.passed == Some(false)).count();
    let passed = verdicts.iter().filter(|v| v.passed == Some(true)).count();
    let skipped = verdicts.len() - failed - passed;
    println!("{passed} passed, {failed} failed, {skipped} skipped");
    if let Some(file) = json {
        let text =
            serde_json::to_string_pretty(&summary(role, program, verdicts)).unwrap_or_default();
        if let Err(e) = std::fs::write(&file, text + "\n") {
            eprintln!("radiopost-tester: {file}: {e}");
            return ExitCode::from(2);
        }
    }
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn client(args: &Args) -> Result<ExitCode, String> {
    let driver_file = args.one("--driver").ok_or("client needs --driver FILE")?;
    let bin = args.one("--bin").ok_or("client needs --bin PROGRAM")?;
    let text = std::fs::read_to_string(&driver_file).map_err(|e| format!("{driver_file}: {e}"))?;
    let driver = serde_json::from_str(&text).map_err(|e| format!("{driver_file}: {e}"))?;
    let (scratch, keep) = match args.one("--scratch") {
        Some(dir) => (PathBuf::from(dir), true),
        None => (
            std::env::temp_dir().join(format!("radiopost-tester-{}", std::process::id())),
            false,
        ),
    };
    let plan = ClientPlan {
        driver,
        bin: bin.clone(),
        scratch: scratch.clone(),
        only: args.list("--only"),
        slow: args.has("--slow"),
    };
    let verdicts = run_client(&plan, say)?;
    if !keep {
        let _ = std::fs::remove_dir_all(&scratch);
    }
    Ok(finish("client", &bin, &verdicts, args.one("--json")))
}

fn plan(args: &Args, role: &str) -> Result<(ClientPlan, bool), String> {
    let driver_file = args
        .one("--driver")
        .ok_or_else(|| format!("{role} needs --driver FILE"))?;
    let bin = args
        .one("--bin")
        .ok_or_else(|| format!("{role} needs --bin PROGRAM"))?;
    let text = std::fs::read_to_string(&driver_file).map_err(|e| format!("{driver_file}: {e}"))?;
    let driver = serde_json::from_str(&text).map_err(|e| format!("{driver_file}: {e}"))?;
    let (scratch, keep) = match args.one("--scratch") {
        Some(dir) => (PathBuf::from(dir), true),
        None => (
            std::env::temp_dir().join(format!("radiopost-tester-{}", std::process::id())),
            false,
        ),
    };
    Ok((
        ClientPlan {
            driver,
            bin,
            scratch,
            only: args.list("--only"),
            slow: args.has("--slow"),
        },
        keep,
    ))
}

fn gateway(args: &Args) -> Result<ExitCode, String> {
    let (plan, keep) = plan(args, "gateway")?;
    let verdicts = radiopost_tester::run_gateway(&plan, say)?;
    if !keep {
        let _ = std::fs::remove_dir_all(&plan.scratch);
    }
    Ok(finish("gateway", &plan.bin, &verdicts, args.one("--json")))
}

fn post_office(args: &Args) -> Result<ExitCode, String> {
    let (plan, keep) = plan(args, "post-office")?;
    let verdicts = radiopost_tester::run_post_office(&plan, say)?;
    if !keep {
        let _ = std::fs::remove_dir_all(&plan.scratch);
    }
    Ok(finish(
        "post office",
        &plan.bin,
        &verdicts,
        args.one("--json"),
    ))
}

fn adapter(args: &Args) -> Result<ExitCode, String> {
    let at = args.one("--at").ok_or("adapter needs --at HOST:PORT")?;
    let plan = radiopost_tester::adapter::Plan {
        at: at.clone(),
        far: args.one("--far"),
        reach: args
            .one("--reach")
            .unwrap_or_else(|| "N1CALL-10".to_owned()),
        only: args.list("--only"),
    };
    let verdicts = radiopost_tester::adapter::run(&plan, say)?;
    Ok(finish("modem adapter", &at, &verdicts, args.one("--json")))
}

fn main() -> ExitCode {
    let words: Vec<String> = std::env::args().skip(1).collect();
    let args = Args {
        words: words.iter().skip(1).cloned().collect(),
    };
    let result = match words.first().map(String::as_str) {
        Some("client") => client(&args),
        Some("adapter") => adapter(&args),
        Some("gateway") => gateway(&args),
        Some("post-office") => post_office(&args),
        _ => {
            println!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    result.unwrap_or_else(|e| {
        eprintln!("radiopost-tester: {e}");
        ExitCode::from(2)
    })
}
