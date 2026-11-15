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

  init --callsign CALL --modem ADDR [--more-modem ADDR]... [--country US]
       [--operator-checks-rules]
                     make a gateway: its key and settings. ADDR is an RPMI adapter's
                     host:port, or Aether's control API (ws://127.0.0.1:8515/v1)
  modem --add ADDR | --remove ADDR
                     answer on one more modem, or no longer on it (W26): one session at a
                     time, the other modems not answering while one carries a session
  modems             the modems the gateway answers on, the first first
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
  copies --users N --days D --max-air-bytes B | copies --off
                     hold copies placed ahead for at most N users, D days each, messages up
                     to B bytes with their text (RP-5 §5.8); register again to publish it
  held               what the gateway holds: copies placed ahead, the queue, the last reports
  block --domain D   carry no sessions to a domain's post office (RP-1 §7.3)
  block --caller C [--note TEXT]
                     refuse a caller from its next call on: an address, or a callsign
                     (without an SSID, every SSID of it); the note is for you alone
  unblock --caller C | --domain D
                     carry that caller's, or that post office's, sessions again
  blocked            the callers and post offices refused
  partner --add CALL | --remove CALL
                     name a partner (RP-7 §1): a gateway this one forwards its queue to when
                     it is cut off, and whose forwarding sessions it answers
  partners           the partners, and how the last call to each went
  window --dial MHZ --bandwidth HZ --every S --at S --length S [--mode USB] [--modem aether]
                     a window (RP-7 §5): when this gateway listens for its partners' calls,
                     opening --at seconds into every --every seconds for --length seconds;
                     only inside its country's segments for unattended data (RP-7 §5.1);
                     register again to publish it
  window --remove N  take window N off (`windows` numbers them)
  windows            the windows, and whether each may be published
  forward --to CALL  call a partner now and forward the queue to it (RP-7 §2), when the
                     gateway is not answering calls; the app's panel does it while it is
  key                the gateway's public key
  serve              answer calls and carry each session to the caller's post office
  update             replace radiopost-gateway, and the other RadioPost programs beside it, with
                     the newest release's, their signature checked with the project's update key
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
        more_modems: a.all("--more-modem"),
        country: a.one("--country").unwrap_or_else(|| "US".into()),
        post_offices: BTreeMap::new(),
        resolver: "https://dns.google/resolve".into(),
        blocked: Vec::new(),
        only: None,
        operator_checks_rules: a.has("--operator-checks-rules"),
        answer_at_start: true,
        copies: None,
        partners: Vec::new(),
        windows: Vec::new(),
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
    let panel = Panel::new(runner.clone(), a.one("--ui").map(PathBuf::from));
    panel.updates().watch();
    let at = panel.start(&format!("127.0.0.1:{port}"))?;
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

/// Whether the gateway holds copies placed ahead, and its limits (RP-5 §4.4, §5.8).
fn copies(dir: &Path, a: &Args) -> Result<(), String> {
    let mut s = Settings::load(dir)?;
    if a.has("--off") {
        s.copies = None;
    } else {
        let n = |flag: &str| -> Result<u64, String> {
            a.need(flag)?
                .parse()
                .map_err(|_| format!("{flag} is a number"))
        };
        s.copies = Some(station::CopiesSettings {
            users: n("--users")?,
            days: n("--days")?,
            max_air_bytes: n("--max-air-bytes")?,
        });
    }
    s.save(dir)?;
    println!("register the gateway again to say so in its record (RP-5 §4.4)");
    Ok(())
}

/// What the gateway holds: copies placed ahead and the queue (W27, W28).
fn held(dir: &Path) {
    let s = radiopost_gateway::store::Store::load(dir);
    println!(
        "{} copies held for {} users; {} session records waiting to be reported, with {} \
         messages; {} forward items",
        s.copies.iter().filter(|c| !c.picked).count(),
        s.users().len(),
        s.records.len(),
        s.queued_ids().len(),
        s.forwards.len()
    );
    for r in &s.records {
        let via = if r.path.is_empty() {
            String::new()
        } else {
            format!(", via {}", r.path.join(" > "))
        };
        println!(
            "  {} ({}) at {}{via}: {} messages, {} pickups{}",
            r.operator,
            r.address,
            r.started,
            r.messages.len(),
            r.pickups.len(),
            r.error
                .as_ref()
                .map(|e| format!("; last try: {e}"))
                .unwrap_or_default()
        );
    }
}

/// `partner` and `partners`: the gateways it forwards to and answers forwarding sessions from
/// (RP-7 §1, §2).
fn partners(dir: &Path, a: &Args, command: &str) -> Result<(), String> {
    let mut s = Settings::load(dir)?;
    if command == "partners" {
        let store = radiopost_gateway::store::Store::load(dir);
        for p in &s.partners {
            let state = store.partners.get(&p.callsign).cloned().unwrap_or_default();
            let last = if state.called == 0 {
                "never called".to_owned()
            } else {
                format!("called at {}: {}", state.called, state.said)
            };
            println!("{}: {last}", p.callsign);
        }
        return Ok(());
    }
    if let Some(call) = a.one("--add") {
        let call = call.trim().to_ascii_uppercase();
        if !s.is_partner(&call) {
            s.partners.push(station::PartnerSettings { callsign: call });
        }
    } else {
        let call = a.need("--remove")?;
        if !s.is_partner(&call) {
            return Err(format!("{call} is not a partner"));
        }
        s.partners
            .retain(|p| !p.callsign.eq_ignore_ascii_case(&call));
    }
    s.save(dir)
}

/// `modem` and `modems`: the modems the gateway answers on (W26).
fn modems(dir: &Path, a: &Args, command: &str) -> Result<(), String> {
    let mut s = Settings::load(dir)?;
    if command == "modems" {
        println!("{} (calls it places go on this one)", s.modem);
        for m in &s.more_modems {
            println!("{m}");
        }
        return Ok(());
    }
    if let Some(m) = a.one("--add") {
        if m == s.modem || s.more_modems.contains(&m) {
            return Err(format!("the gateway answers on {m} already"));
        }
        s.more_modems.push(m);
    } else {
        let m = a.need("--remove")?;
        if m == s.modem {
            return Err(format!(
                "{m} is the gateway's first modem: `init` names it, and the gateway's settings"
            ));
        }
        if !s.more_modems.contains(&m) {
            return Err(format!("the gateway does not answer on {m}"));
        }
        s.more_modems.retain(|x| *x != m);
    }
    s.save(dir)
}

/// `window` and `windows`: when the gateway listens for its partners' calls (RP-7 §5).
fn windows(dir: &Path, a: &Args, command: &str) -> Result<(), String> {
    let mut s = Settings::load(dir)?;
    if command == "window" {
        if let Some(n) = a.one("--remove") {
            let i: usize = n.parse().map_err(|_| "--remove takes a window's number")?;
            if i == 0 || i > s.windows.len() {
                return Err(format!("there is no window {i}"));
            }
            s.windows.remove(i - 1);
        } else {
            let n = |flag: &str| -> Result<u64, String> {
                a.need(flag)?
                    .parse()
                    .map_err(|_| format!("{flag} is a number"))
            };
            let mhz: f64 = a
                .need("--dial")?
                .parse()
                .map_err(|_| "--dial is a frequency in MHz, such as 7.101")?;
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a dial frequency in hertz"
            )]
            let window = station::WindowSettings {
                dial_hz: (mhz * 1e6).round() as u64,
                mode: a.one("--mode").unwrap_or_else(|| "USB".into()),
                modem: a.one("--modem").unwrap_or_else(|| "aether".into()),
                bandwidth_hz: n("--bandwidth")?,
                every_s: n("--every")?,
                at_s: n("--at")?,
                length_s: n("--length")?,
            };
            if let Some(why) = window.refused(&s.country) {
                return Err(format!("this window cannot be published: {why}"));
            }
            s.windows.push(window);
        }
        s.save(dir)?;
        println!("register again to publish the windows in the gateway's record (RP-5 §4.4)");
    }
    for (i, w) in s.windows.iter().enumerate() {
        let said = w.refused(&s.country).map_or_else(
            || "published".to_owned(),
            |why| format!("not published: {why}"),
        );
        #[allow(clippy::cast_precision_loss, reason = "a dial frequency shown in MHz")]
        let mhz = w.dial_hz as f64 / 1e6;
        println!(
            "{}: {mhz:.4} MHz {} {} {} Hz, {} s into every {} s for {} s; {said}",
            i + 1,
            w.mode,
            w.modem,
            w.bandwidth_hz,
            w.at_s,
            w.every_s,
            w.length_s
        );
    }
    Ok(())
}

/// `block`, `unblock` and `blocked`: callers refused (wishlist W24), read at each call, and post
/// offices blocked (RP-1 §7.3), a setting.
fn blocking(dir: &Path, a: &Args, command: &str) -> Result<(), String> {
    match command {
        "block" => {
            if let Some(caller) = a.one("--caller") {
                let note = a.one("--note").unwrap_or_default();
                println!(
                    "{}",
                    radiopost_gateway::refused::add(dir, &caller, &note, station::now())?
                );
                return Ok(());
            }
            let mut s = Settings::load(dir)?;
            let d = a.need("--domain")?.to_lowercase();
            if !s.blocked.contains(&d) {
                s.blocked.push(d);
            }
            s.save(dir)
        }
        "unblock" => {
            if let Some(caller) = a.one("--caller") {
                if !radiopost_gateway::refused::remove(dir, &caller)? {
                    return Err(format!("{caller} was not refused"));
                }
                println!("carrying {caller}'s sessions again from the next call on");
                return Ok(());
            }
            let mut s = Settings::load(dir)?;
            let d = a.need("--domain")?.to_lowercase();
            if !s.blocked.contains(&d) {
                return Err(format!("{d} was not blocked"));
            }
            s.blocked.retain(|b| *b != d);
            s.save(dir)
        }
        "blocked" => {
            let s = Settings::load(dir)?;
            for d in &s.blocked {
                println!("post office {d}");
            }
            for r in radiopost_gateway::refused::list(dir) {
                let note = if r.note.is_empty() {
                    String::new()
                } else {
                    format!(": {}", r.note)
                };
                println!("caller {}{note}", r.caller);
            }
            Ok(())
        }
        _ => Err(format!("unknown command {command}")),
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
        "copies" => copies(dir, a),
        "held" => {
            held(dir);
            Ok(())
        }
        "block" | "unblock" | "blocked" => blocking(dir, a, command),
        "partner" | "partners" => partners(dir, a, command),
        "modem" | "modems" => modems(dir, a, command),
        "window" | "windows" => windows(dir, a, command),
        "forward" => station::forward_once(dir, &a.need("--to")?, &mut |line| println!("{line}")),
        "key" => {
            println!(
                "{}",
                base64url::encode(station::key(dir)?.verifying_key().as_bytes())
            );
            Ok(())
        }
        "serve" => station::serve(dir, &mut |line| println!("{line}")),
        "panel" => panel(dir, a),
        "update" => {
            let exe = std::env::current_exe().map_err(|e| format!("where is this program: {e}"))?;
            let at = exe.parent().ok_or("this program is in no directory")?;
            let said = radiopost_update::install_tools(
                &radiopost_update::channel(),
                env!("CARGO_PKG_VERSION"),
                at,
                radiopost_update::public_key(),
            )?;
            println!("{said}");
            if said.starts_with("updated") {
                println!("restart the gateway to run it");
            }
            Ok(())
        }
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
    let dir = PathBuf::from(dir);
    // what the gateway says, and any panic, in its own folder's log file
    radiopost::logfile::keep_in(&dir.join(radiopost_gateway::LOG_FILE));
    radiopost::logfile::record_panics();
    match run(&dir, &a, &command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("radiopost-gateway: {e}");
            ExitCode::FAILURE
        }
    }
}
