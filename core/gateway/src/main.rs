// SPDX-License-Identifier: MIT OR Apache-2.0
//! `radiopost-gateway`: a RadioPost gateway, online (RP-3 §9, RP-5 §5), and its operator's
//! commands.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ed25519_dalek::SigningKey;
use radiopost::base64url;
use radiopost_gateway::operator;
use radiopost_gateway::station::{self, Settings};
use serde_json::{Value as Json, json};

const USAGE: &str = "\
usage: radiopost-gateway --dir DIR COMMAND

  init --callsign CALL --modem ADDR [--country US] [--operator-checks-rules]
                     make a gateway: its key and settings. ADDR is an RPMI adapter's
                     host:port, or Aether's control API (ws://127.0.0.1:8515/v1/ws)
  register --home HOME [--grid G] [--place TEXT] [--channel HZ,MODE,MODEM,BW]...
                     register the gateway with your post office (RP-5 §6.4), logged in
                     as you with your client's key and credential in HOME, and keep the
                     gateway's credential
  post-office --domain D --url URL
                     reach a domain's post office at URL without asking DNS
  block --domain D   carry no sessions to a domain's post office (RP-1 §7.3)
  key                the gateway's public key
  serve              answer calls and carry each session to the caller's post office";

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

fn hex(bytes: &[u8]) -> String {
    radiopost::cbor::hex(bytes)
}

fn init(dir: &Path, a: &Args) -> Result<(), String> {
    if Settings::load(dir).is_ok() {
        return Err("this directory already holds a gateway".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| e.to_string())?;
    let settings = Settings {
        callsign: a.need("--callsign")?.to_uppercase(),
        modem: a.need("--modem")?,
        country: a.one("--country").unwrap_or_else(|| "US".into()),
        post_offices: BTreeMap::new(),
        resolver: "https://dns.google/resolve".into(),
        blocked: Vec::new(),
        only: None,
        operator_checks_rules: a.has("--operator-checks-rules"),
    };
    station::write(dir, "key.hex", &hex(&seed))?;
    settings.save(dir)?;
    println!(
        "gateway {} made in {}\nits key: {}",
        settings.callsign,
        dir.display(),
        base64url::encode(SigningKey::from_bytes(&seed).verifying_key().as_bytes())
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
        _ => Err(USAGE.into()),
    }
}

fn main() -> ExitCode {
    let words: Vec<String> = std::env::args().skip(1).collect();
    let a = Args { words };
    let (Some(dir), Some(command)) = (
        a.one("--dir"),
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
