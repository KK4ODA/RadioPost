// SPDX-License-Identifier: MIT OR Apache-2.0
//! `radiopost-issue`: a post office's key and the credentials it issues, for on-air tests before
//! the post office service exists. `radiopost-issue help` says how to use it.

use std::path::Path;
use std::process::ExitCode;

use radiopost_issue::{Error, MAX_DAYS, PostOffice, flags_of};

const HELP: &str = "\
radiopost-issue: a post office's key and the credentials it issues (RP-1 §4)

  radiopost-issue key --domain DOMAIN --out FILE [--years N]
      make a post office key for DOMAIN (valid N years, 1 by default), kept in FILE, which is
      the post office's secret; print the command every tester runs to trust it
  radiopost-issue trust FILE
      print that command again
  radiopost-issue issue --key FILE --address ADDR --operator CALL --device HEX
                        [--days N] [--flags send,receive]
      print a credential, in hex, for ADDR at DOMAIN, the operator's callsign and the device
      key `radiopost key` printed on the tester's computer; the tester keeps it with
      `radiopost init --credential FILE` (90 days at most; send and receive by default)

The test keys in vectors/ are published: never use them on the air.
";

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn need(args: &[String], name: &str) -> Result<String, Error> {
    flag(args, name).ok_or_else(|| format!("{name} is needed: `radiopost-issue help`"))
}

fn number(args: &[String], name: &str, default: u64) -> Result<u64, Error> {
    flag(args, name).map_or(Ok(default), |v| {
        v.parse().map_err(|_| format!("{name} is a number"))
    })
}

fn run(args: &[String]) -> Result<(), Error> {
    match args.first().map(String::as_str) {
        None | Some("help" | "--help" | "-h") => print!("{HELP}"),
        Some("key") => {
            let out = need(args, "--out")?;
            let path = Path::new(&out);
            if path.exists() {
                return Err(format!(
                    "{out} exists: a post office key is never replaced in place"
                ));
            }
            let po = PostOffice::new(
                &need(args, "--domain")?,
                now(),
                365 * number(args, "--years", 1)?,
            )?;
            po.save(path)?;
            println!("{}", po.trust_command());
        }
        Some("trust") => {
            let file = args.get(1).ok_or("trust FILE")?;
            println!("{}", PostOffice::load(Path::new(file))?.trust_command());
        }
        Some("issue") => {
            let file = need(args, "--key")?;
            let path = Path::new(&file);
            let mut po = PostOffice::load(path)?;
            let credential = po.issue(
                &need(args, "--address")?,
                &need(args, "--operator")?,
                &need(args, "--device")?,
                now(),
                number(args, "--days", MAX_DAYS)?,
                flags_of(&flag(args, "--flags").unwrap_or_else(|| "send,receive".into()))?,
            )?;
            po.save(path)?;
            println!("{}", radiopost::cbor::hex(&credential));
        }
        Some(other) => return Err(format!("no command {other}: `radiopost-issue help`")),
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("radiopost-issue: {e}");
            ExitCode::FAILURE
        }
    }
}
