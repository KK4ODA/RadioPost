// SPDX-License-Identifier: MIT OR Apache-2.0
//! `radiopost-po`: a RadioPost post office, and its operator's commands.

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use radiopost::base64url;
use radiopost_postoffice::check::{self, Verdict};
use radiopost_postoffice::documents::Info;
use radiopost_postoffice::serve::{self, Files, Running, Settings};
use radiopost_update::{Product, Watch};
use serde_json::{Value as Json, json};

const USAGE: &str = "\
usage: radiopost-po --dir DIR COMMAND
       radiopost-po update

  init --domain D --base-url URL --operator CALL --contact EMAIL [--country US]
       [--listen 127.0.0.1:8770] [--admin 127.0.0.1:8771] [--jmap URL] [--imap HOST:PORT]
       [--hooks 127.0.0.1:8772] [--hook-token T] [--smtp smtp://USER:PASS@127.0.0.1:587]
                     make a post office: its key, settings and state; print its DNS records
  serve              serve the API (plain HTTP: put HTTPS in front), the operator's port, the
                     door for the mail server's MTA hooks, and send email out by SMTP
  dns                the DNS records to publish (RP-1 §7.1)
  know --domain D --public KEY [--api URL] [--not-before T --not-after T]
                     accept credentials of another post office (restart to take effect)
  code --address A --operator CALL [--send] [--receive] [--role] [--name NAME]
                     a one-time enrollment code (RP-5 §6.1), good for a week
  name --address A --name NAME
                     the display name of the account's email (RP-6 §3.1)
  deliver --to ADDR --from ADDR --subject TEXT --air FILE [--licensed]
                     put a message's air form in a mailbox by hand
  revoke --device KEY revoke every credential of a device key (base64url or hex)
  applications [--open | --close]
                     the applications for accounts (RP-5 §6.7); open or close them
  approve ID [--role] | decline ID --text TEXT | postcard ID
                     decide an application: its credential issued, refused with a reason, or a
                     code made to send by post to the address on the licence
  outgoing           the mail waiting to go on by email
  status             accounts, gateways, codes, sessions
  check [--dkim SELECTOR,…] [--resolver URL]
                     the post office checked from outside: its DNS records, reverse DNS, the API
                     over HTTPS, the door, and the mail server's submission port (no mail sent)
  update             replace radiopost-po, and the other RadioPost programs beside it, with the
                     newest release's, their signature checked with the project's update key
                     (run it with sudo); then restart the service";

struct Args {
    words: Vec<String>,
}

impl Args {
    fn one(&self, flag: &str) -> Option<String> {
        let i = self.words.iter().position(|w| w == flag)?;
        self.words.get(i + 1).cloned()
    }

    fn need(&self, flag: &str) -> Result<String, String> {
        self.one(flag).ok_or_else(|| format!("{flag} is needed"))
    }

    fn has(&self, flag: &str) -> bool {
        self.words.iter().any(|w| w == flag)
    }
}

fn key_bytes(text: &str) -> Option<[u8; 32]> {
    let hex: Option<Vec<u8>> = (text.len() == 64).then(|| {
        (0..64)
            .step_by(2)
            .filter_map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
            .collect()
    });
    hex.filter(|b| b.len() == 32)
        .or_else(|| base64url::decode(text))
        .and_then(|b| b.try_into().ok())
}

fn init(files: &Files, a: &Args) -> Result<(), String> {
    if files.settings().is_ok() {
        return Err("this directory already holds a post office".into());
    }
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| e.to_string())?;
    let at = serve::now();
    let domain = a.need("--domain")?;
    let host = a
        .need("--base-url")?
        .split("://")
        .nth(1)
        .and_then(|r| r.split('/').next())
        .unwrap_or("")
        .to_owned();
    let settings = Settings {
        domains: vec![domain],
        base_url: a.need("--base-url")?,
        listen: a.one("--listen").unwrap_or_else(|| "127.0.0.1:8770".into()),
        admin: a.one("--admin").unwrap_or_else(|| "127.0.0.1:8771".into()),
        info: Info {
            operator: a.need("--operator")?,
            contact: a.need("--contact")?,
            country: a.one("--country").unwrap_or_else(|| "US".into()),
            max_air_bytes: 65_536,
            max_email_bytes: 10_485_760,
            gateways: "any".into(),
            mail: json!({
                "jmap": a.one("--jmap").unwrap_or_else(|| format!("https://{host}/jmap/session")),
                "imap": a.one("--imap").unwrap_or_else(|| format!("{host}:993")),
            }),
            updated: at,
            not_before: at - 86_400,
            not_after: at + 5 * 365 * 86_400,
            post_offices: Vec::new(),
            applications: None,
        },
        known: Vec::new(),
        hooks: Some(a.one("--hooks").unwrap_or_else(|| "127.0.0.1:8772".into())),
        hook_token: a.one("--hook-token"),
        smtp: a.one("--smtp"),
        operators: Vec::new(),
    };
    let hex: String = radiopost::cbor::hex(&seed);
    files.write("key.hex", &(hex + "\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(
            PathBuf::from(a.need("--dir")?).join("key.hex"),
            std::fs::Permissions::from_mode(0o600),
        );
    }
    files.write(
        "config.json",
        &serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?,
    )?;
    let running = Running::load(&PathBuf::from(a.need("--dir")?), serve::system_draw())?;
    let office = running.office();
    files.write(
        "state.json",
        &serde_json::to_string_pretty(&office.state).map_err(|e| e.to_string())?,
    )?;
    let public = SigningKey::from_bytes(&seed).verifying_key().to_bytes();
    let public_hex: String = radiopost::cbor::hex(&public);
    println!("Post office made. key.hex is its secret: keep it, and back it up.\n");
    println!("Publish these DNS records:");
    for r in office.dns_records() {
        println!("  {r}");
    }
    println!("\nIts users trust it with:");
    for d in &settings.domains {
        println!(
            "  radiopost trust {d} {public_hex} {} {}",
            settings.info.not_before, settings.info.not_after
        );
    }
    Ok(())
}

/// One of the operator's commands, sent to the running post office's own port.
fn admin(files: &Files, method: &str, path: &str, body: &Json) -> Result<Json, String> {
    let at = files.settings()?.admin;
    let mut s = TcpStream::connect(&at)
        .map_err(|e| format!("the post office is not running at {at}: {e}"))?;
    let text = body.to_string();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: {at}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    )
    .map_err(|e| e.to_string())?;
    let mut answer = String::new();
    s.read_to_string(&mut answer).map_err(|e| e.to_string())?;
    let body = answer.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    let json: Json =
        serde_json::from_str(body).map_err(|_| format!("the post office said: {answer}"))?;
    if let Some(e) = json.get("error").and_then(Json::as_str) {
        return Err(e.to_owned());
    }
    Ok(json)
}

fn check(files: &Files, a: &Args) -> Result<(), String> {
    let settings = files.settings()?;
    let public = files.key()?.verifying_key().to_bytes();
    let resolver = a
        .one("--resolver")
        .unwrap_or_else(|| radiopost_api::RESOLVER.into());
    let selectors: Vec<String> = a
        .one("--dkim")
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let mut lines = check::run(&settings, &public, &resolver, &selectors);
    lines.push(update_line());
    lines.push(match admin(files, "GET", "/status", &Json::Null) {
        Ok(status) => check::Line {
            verdict: Verdict::Note,
            text: format!(
                "the post office: {} accounts, {} waiting to go out by email",
                status["accounts"], status["outgoing"]
            ),
        },
        Err(e) => check::Line {
            verdict: Verdict::Fail,
            text: format!("the operator's port: {e}"),
        },
    });
    for line in &lines {
        let mark = match line.verdict {
            Verdict::Ok => "ok  ",
            Verdict::Fail => "FAIL",
            Verdict::Note => "note",
        };
        println!("{mark}  {}", line.text);
    }
    let failed = lines.iter().filter(|l| l.verdict == Verdict::Fail).count();
    if failed > 0 {
        return Err(format!("{failed} of {} checks failed", lines.len()));
    }
    println!("ready");
    Ok(())
}

/// Whether a newer release is out, as a line of `check`.
fn update_line() -> check::Line {
    let current = env!("CARGO_PKG_VERSION");
    let (verdict, text) =
        match radiopost_update::check(&radiopost_update::channel(), Product::Tools, current) {
            Ok(None) => (
                Verdict::Ok,
                format!("RadioPost {current} is the newest release"),
            ),
            Ok(Some(offer)) => (
                Verdict::Note,
                format!(
                    "RadioPost {} is out (this is {current}): `sudo radiopost-po update`, then \
                     restart the service",
                    offer.version
                ),
            ),
            Err(e) => (Verdict::Note, format!("the update channel: {e}")),
        };
    check::Line { verdict, text }
}

/// `update`: the programs beside this one replaced with the newest release's.
fn update() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("where is this program: {e}"))?;
    let dir = exe.parent().ok_or("this program is in no directory")?;
    let said = radiopost_update::install_tools(
        &radiopost_update::channel(),
        env!("CARGO_PKG_VERSION"),
        dir,
        radiopost_update::public_key(),
    )?;
    println!("{said}");
    if said.starts_with("updated") {
        println!("restart the post office to run it: sudo systemctl restart radiopost-po");
    }
    Ok(())
}

/// Says in the log when a newer release is out: at start, and when the channel offers another
/// (asked every six hours).
fn watch_updates(running: &Running) {
    let watch = Watch::new(
        Product::Tools,
        env!("CARGO_PKG_VERSION"),
        &radiopost_update::channel(),
    );
    running.watch_updates(Arc::clone(&watch));
    std::thread::spawn(move || {
        let mut said = None;
        loop {
            watch.check();
            if let Some(offer) = watch.offer()
                && said.as_ref() != Some(&offer.version)
            {
                eprintln!(
                    "radiopost-po: RadioPost {} is out (this is {}): `sudo radiopost-po update`, \
                     then `sudo systemctl restart radiopost-po`",
                    offer.version,
                    env!("CARGO_PKG_VERSION")
                );
                said = Some(offer.version);
            }
            std::thread::sleep(radiopost_update::EVERY);
        }
    });
}

fn know(files: &Files, a: &Args) -> Result<(), String> {
    let mut settings = files.settings()?;
    let public = key_bytes(&a.need("--public")?).ok_or("--public is not a 32-byte key")?;
    let domain = a.need("--domain")?;
    let key = json!({
        "kid": radiopost::cbor::hex(&radiopost::crypto::kid(&public)),
        "public": base64url::encode(&public),
        "not_before": a.one("--not-before").and_then(|t| t.parse::<u64>().ok()).unwrap_or(0),
        "not_after": a.one("--not-after").and_then(|t| t.parse::<u64>().ok()).unwrap_or(u64::MAX),
    });
    let entry = json!({"domain": domain, "api": a.one("--api").unwrap_or_default(), "keys": [key]});
    settings
        .known
        .retain(|k| k.get("domain").and_then(Json::as_str) != Some(domain.as_str()));
    settings.known.push(entry.clone());
    settings
        .info
        .post_offices
        .retain(|k| k.get("domain").and_then(Json::as_str) != Some(domain.as_str()));
    settings.info.post_offices.push(entry);
    files.write(
        "config.json",
        &serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?,
    )?;
    println!("{domain} known. Restart the post office for it to take effect.");
    Ok(())
}

/// `applications`: list them, or open or close them.
fn applications(files: &Files, a: &Args) -> Result<(), String> {
    if a.has("--open") || a.has("--close") {
        let take = if a.has("--open") { "open" } else { "closed" };
        admin(
            files,
            "POST",
            "/applications",
            &json!({"applications": take}),
        )?;
        println!("applications {take}");
        return Ok(());
    }
    let list = admin(files, "GET", "/applications", &Json::Null)?;
    let entries = list["applications"].as_array().cloned().unwrap_or_default();
    if entries.is_empty() {
        println!("no applications");
    }
    for e in entries {
        let s = |k: &str| e[k].as_str().unwrap_or("").to_owned();
        println!(
            "{}  {:<9} {} {} ({}), {}",
            s("application"),
            s("status"),
            s("address"),
            s("operator"),
            s("name"),
            s("contact")
        );
        if let Some(code) = e["code"].as_str() {
            println!("    postcard code {code}");
        }
    }
    Ok(())
}

/// `approve`, `decline`, `postcard`: an application decided (RP-5 §7.3).
fn decide(files: &Files, a: &Args, command: &str) -> Result<(), String> {
    let id = a
        .words
        .iter()
        .skip_while(|w| w.as_str() != command)
        .nth(1)
        .filter(|w| !w.starts_with("--"))
        .cloned()
        .ok_or("which application? Its ID, `ap_…`")?;
    let mut body = json!({"application": id, "decision": command});
    if command == "approve" {
        body["decision"] = json!("approve");
        if a.has("--role") {
            body["flags"] = json!(["send", "receive", "role"]);
        }
    }
    if command == "decline" {
        body["text"] = json!(a.need("--text")?);
    }
    let entry = admin(files, "POST", "/decide", &body)?;
    match command {
        "postcard" => println!(
            "postcard code {} for {} ({}): good for 30 days",
            entry["code"].as_str().unwrap_or(""),
            entry["address"].as_str().unwrap_or(""),
            entry["operator"].as_str().unwrap_or("")
        ),
        _ => println!(
            "{} {}",
            entry["address"].as_str().unwrap_or(""),
            entry["status"].as_str().unwrap_or("")
        ),
    }
    Ok(())
}

/// `serve`: the API, the operator's port, the door and email out, until the process ends.
fn serve_all(dir: &std::path::Path, files: &Files) -> Result<(), String> {
    let settings = files.settings()?;
    let running = Arc::new(Running::load(dir, serve::system_draw())?);
    eprintln!("radiopost-po: RadioPost {}", env!("CARGO_PKG_VERSION"));
    watch_updates(&running);
    let (api, ops) = serve::serve(running.clone(), &settings.listen, &settings.admin)?;
    eprintln!(
        "radiopost-po: {} on http://{api} (for {}), operator's port {ops}",
        settings.domains.join(", "),
        settings.base_url
    );
    if let Some(hooks) = &settings.hooks {
        let at = serve::serve_hooks(running.clone(), hooks)?;
        eprintln!("radiopost-po: the door answers the mail server's MTA hooks on http://{at}");
    }
    match &settings.smtp {
        Some(url) => {
            serve::send_mail(running, url, |line| {
                eprintln!("radiopost-po: email out: {line}");
            })?;
            eprintln!("radiopost-po: email leaves through the mail server's submission port");
        }
        None => {
            eprintln!("radiopost-po: no `smtp` in config.json: email waits in `outgoing`");
        }
    }
    loop {
        std::thread::park();
    }
}

fn run(a: &Args, command: &str) -> Result<(), String> {
    if command == "update" {
        return update();
    }
    let dir = PathBuf::from(a.need("--dir")?);
    let files = Files::new(&dir);
    match command {
        "init" => init(&files, a),
        "serve" => serve_all(&dir, &files),
        "dns" => {
            let running = Running::load(&dir, serve::system_draw())?;
            for r in running.office().dns_records() {
                println!("{r}");
            }
            Ok(())
        }
        "know" => know(&files, a),
        "code" => {
            let mut flags = Vec::new();
            for f in ["send", "receive", "role"] {
                if a.has(&format!("--{f}")) {
                    flags.push(f);
                }
            }
            if flags.is_empty() {
                flags = vec!["send", "receive"];
            }
            let body = json!({"address": a.need("--address")?, "operator": a.need("--operator")?,
                              "flags": flags, "name": a.one("--name").unwrap_or_default()});
            let answer = admin(&files, "POST", "/code", &body)?;
            println!(
                "{}  for {} ({}), good for a week",
                answer["code"].as_str().unwrap_or(""),
                answer["address"].as_str().unwrap_or(""),
                answer["operator"].as_str().unwrap_or("")
            );
            Ok(())
        }
        "name" => {
            let body = json!({"address": a.need("--address")?, "name": a.need("--name")?});
            admin(&files, "POST", "/name", &body)?;
            Ok(())
        }
        "deliver" => {
            let air = std::fs::read(a.need("--air")?).map_err(|e| e.to_string())?;
            let body = json!({"to": a.need("--to")?, "from": a.need("--from")?,
                "subject": a.one("--subject").unwrap_or_default(), "air": base64url::encode(&air),
                "third_party": !a.has("--licensed")});
            let answer = admin(&files, "POST", "/deliver", &body)?;
            println!("delivered as #{}", answer["serial"]);
            Ok(())
        }
        "revoke" => {
            let device = key_bytes(&a.need("--device")?).ok_or("--device is not a 32-byte key")?;
            let answer = admin(
                &files,
                "POST",
                "/revoke",
                &json!({"device": base64url::encode(&device)}),
            )?;
            println!("revoked serials {}", answer["revoked"]);
            Ok(())
        }
        "applications" => applications(&files, a),
        "approve" | "decline" | "postcard" => decide(&files, a, command),
        "outgoing" | "status" => {
            let answer = admin(&files, "GET", &format!("/{command}"), &Json::Null)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&answer).unwrap_or_default()
            );
            Ok(())
        }
        "check" => check(&files, a),
        _ => Err(format!("unknown command {command}\n{USAGE}")),
    }
}

fn main() -> ExitCode {
    let words: Vec<String> = std::env::args().skip(1).collect();
    let command = words
        .iter()
        .enumerate()
        .find(|(i, w)| !w.starts_with("--") && (*i == 0 || !words[i - 1].starts_with("--")))
        .map(|(_, w)| w.clone());
    let Some(command) = command else {
        println!("{USAGE}");
        return ExitCode::from(2);
    };
    match run(&Args { words }, &command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("radiopost-po: {e}");
            ExitCode::FAILURE
        }
    }
}
