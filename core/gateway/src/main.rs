// SPDX-License-Identifier: MIT OR Apache-2.0
//! `radiopost-gateway`: a RadioPost gateway, online (RP-3 §9, RP-5 §5), and its operator's
//! commands.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use radiopost::base64url;
use radiopost_gateway::operator;
use radiopost_gateway::panel::Panel;
use radiopost_gateway::runner::Runner;
use radiopost_gateway::station::{self, Settings};
use serde_json::{Value as Json, json};

const USAGE: &str = "\
usage: radiopost-gateway [--dir DIR] COMMAND

DIR is the gateway's directory: $RADIOPOST_GATEWAY, else .radiopost-gateway in your home
directory, as the RadioPost Gateway app has it.

  init --callsign CALL --modem ADDR [--country US] [--operator-checks-rules]
                     make a gateway: its key and settings. ADDR is an RPMI adapter's
                     host:port, or Aether's control API (ws://127.0.0.1:8515/v1)
  register --home HOME [--grid G] [--place TEXT] [--channel HZ,MODE,MODEM,BW]...
                     register the gateway with your post office (RP-5 §6.4), logged in
                     as you with your client's key and credential in HOME, and keep the
                     gateway's credential
  credential --file FILE
                     keep a credential a post office issued for the gateway's key by other
                     means than `register` (raw bytes or hex)
  post-office --domain D --url URL
                     reach a domain's post office at URL without asking DNS
  conformance --resolver URL
                     conformance mode (the conformance tester's §2.3): ask only this DNS
                     resolver, the tester's
  block --domain D   carry no sessions to a domain's post office (RP-1 §7.3)
  key                the gateway's public key
  serve              answer calls and carry each session to the caller's post office
  panel [--port N] [--ui DIR]
                     serve the gateway's panel on this computer (port 8621) and say where;
                     it answers calls when the gateway is set to answer at start";

struct Args {
    words: Vec<String>,
}

impl Args {
    fn one(&self, flag: &str) -> Option<String> {
        let i = self.words.iter().position(|w| w == flag)?;
        self.words.get(i + 1).cloned()
    }

    fn all(&self, flag: &str) -> Vec<String> {
        self.words
            .windows(2)
            .filter(|w| w[0] == flag)
            .map(|w| w[1].clone())
            .collect()
    }

    fn need(&self, flag: &str) -> Result<String, String> {
        self.one(flag).ok_or_else(|| format!("{flag} is needed"))
    }

    fn has(&self, flag: &str) -> bool {
        self.words.iter().any(|w| w == flag)
    }
}

fn init(dir: &Path, a: &Args) -> Result<(), String> {
    let settings = Settings {
        callsign: a.need("--callsign")?.to_uppercase(),
        modem: a.need("--modem")?,
        country: a.one("--country").unwrap_or_else(|| "US".into()),
        post_offices: BTreeMap::new(),
        resolver: "https://dns.google/resolve".into(),
        blocked: Vec::new(),
        only: None,
        operator_checks_rules: a.has("--operator-checks-rules"),
        answer_at_start: true,
    };
    station::make(dir, &settings)?;
    println!(
        "gateway {} made in {}\nits key: {}",
        settings.callsign,
        dir.display(),
        base64url::encode(station::key(dir)?.verifying_key().as_bytes())
    );
    println!(
        "next: radiopost-gateway --dir {} register --home YOUR_CLIENT_HOME",
        dir.display()
    );
    Ok(())
}

fn channels(a: &Args) -> Result<Vec<Json>, String> {
    a.all("--channel")
        .iter()
        .map(|c| {
            let p: Vec<&str> = c.split(',').collect();
            let (Some(hz), Some(mode), Some(modem), Some(bw)) = (
                p.first().and_then(|x| x.parse::<u64>().ok()),
                p.get(1),
                p.get(2),
                p.get(3).and_then(|x| x.parse::<u64>().ok()),
            ) else {
                return Err(format!(
                    "{c}: a channel is HZ,MODE,MODEM,BW (7101000,USB,aether,2300)"
                ));
            };
            Ok(json!({"dial_hz": hz, "mode": mode, "modem": modem, "bandwidth_hz": bw}))
        })
        .collect()
}

fn register(dir: &Path, a: &Args) -> Result<(), String> {
    let mut record = json!({"channels": channels(a)?, "hours": "always", "internet": true});
    for (flag, key) in [("--grid", "grid"), ("--place", "place")] {
        if let Some(v) = a.one(flag) {
            record[key] = v.into();
        }
    }
    let said = operator::register(dir, &PathBuf::from(a.need("--home")?), record)?;
    println!("{said}");
    Ok(())
}

/// The panel, for a gateway with no screen: on loopback, for a browser on the same machine.
fn panel(dir: &Path, a: &Args) -> Result<(), String> {
    let runner = Arc::new(Runner::new(dir).echoing());
    let port = a.one("--port").unwrap_or_else(|| "8621".into());
    let at = Panel::new(runner.clone(), a.one("--ui").map(PathBuf::from))
        .start(&format!("127.0.0.1:{port}"))?;
    println!("the panel is at http://{at}/");
    if Settings::load(dir).is_ok_and(|s| s.answer_at_start) {
        if let Err(e) = runner.start() {
            println!("not answering: {e}");
        }
    }
    loop {
        std::thread::park();
    }
}

fn run(dir: &Path, a: &Args, command: &str) -> Result<(), String> {
    match command {
        "init" => init(dir, a),
        "register" => register(dir, a),
        "post-office" => {
            let mut s = Settings::load(dir)?;
            s.post_offices
                .insert(a.need("--domain")?.to_lowercase(), a.need("--url")?);
            s.save(dir)
        }
        "credential" => {
            let file = a.need("--file")?;
            let bytes = std::fs::read(&file).map_err(|e| format!("{file}: {e}"))?;
            let body = station::keep_credential(dir, &bytes)?;
            println!(
                "{} for {}, credential {}",
                body.operator, body.address, body.serial
            );
            Ok(())
        }
        "conformance" => {
            let mut s = Settings::load(dir)?;
            s.resolver = a.need("--resolver")?;
            s.save(dir)
        }
        "block" => {
            let mut s = Settings::load(dir)?;
            let d = a.need("--domain")?.to_lowercase();
            if !s.blocked.contains(&d) {
                s.blocked.push(d);
            }
            s.save(dir)
        }
        "key" => {
            println!(
                "{}",
                base64url::encode(station::key(dir)?.verifying_key().as_bytes())
            );
            Ok(())
        }
        "serve" => station::serve(dir, &mut |line| println!("{line}")),
        "panel" => panel(dir, a),
        _ => Err(USAGE.into()),
    }
}

fn main() -> ExitCode {
    let words: Vec<String> = std::env::args().skip(1).collect();
    let a = Args { words };
    let (Some(dir), Some(command)) = (
        a.one("--dir")
            .or_else(|| Some(station::default_dir().display().to_string())),
        a.words
            .iter()
            .enumerate()
            .find(|(i, w)| !w.starts_with("--") && (*i == 0 || a.words[i - 1] != "--dir"))
            .map(|(_, w)| w.clone()),
    ) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match run(&PathBuf::from(dir), &a, &command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("radiopost-gateway: {e}");
            ExitCode::FAILURE
        }
    }
}
