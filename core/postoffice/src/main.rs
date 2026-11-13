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
use radiopost_postoffice::known::{self, Lookup};
use radiopost_postoffice::serve::{self, Files, Running, Settings};
use radiopost_update::{Product, Watch};
use serde_json::{Value as Json, json};

const USAGE: &str = "\
usage: radiopost-po --dir DIR COMMAND
       radiopost-po update
       radiopost-po install --domain D --operator CALL --contact EMAIL [--dry-run] [--yes]
                     set up a post office on a fresh Debian server, as root: the firewall, the
                     server's name, SSH by key, Caddy, Stalwart, the post office and its jobs
                     runner, then DNS and the check; each step skipped when it is done, so it
                     can run again; --dry-run says what it would do

  init --domain D --base-url URL --operator CALL --contact EMAIL [--country US]
       [--listen 127.0.0.1:8770] [--admin 127.0.0.1:8771] [--jmap URL] [--imap HOST:PORT]
       [--hooks 127.0.0.1:8772] [--hook-token T] [--smtp smtp://USER:PASS@127.0.0.1:587]
                     make a post office: its key, settings and state; print its DNS records
  serve              serve the API (plain HTTP: put HTTPS in front), the operator's port, the
                     door for the mail server's MTA hooks, and send email out by SMTP
  dns                the DNS records to publish (RP-1 §7.1)
  know --domain D [--resolver URL]
                     know another post office (RP-1 §7.5): its DNS records and documents are
                     fetched, shown, and refreshed every six hours by the service
  know --domain D --public KEY [--api URL] [--not-before T --not-after T]
                     know one by a key typed here, for one not in DNS yet (restart to take
                     effect); what is fetched of it later serves first
  post-offices [--block D | --unblock D | --forget D | --refresh]
                     the post offices known, seen (met, and not known) and blocked; block one,
                     unblock it, forget one known, or fetch the known ones again
  code --address A --operator CALL [--send] [--receive] [--role] [--name NAME]
                     a one-time enrollment code (RP-5 §6.1), good for a week
  name --address A --name NAME
                     the display name of the account's email (RP-6 §3.1)
  deliver --to ADDR --from ADDR --subject TEXT --air FILE [--licensed]
                     put a message's air form in a mailbox by hand
  revoke --device KEY revoke every credential of a device key (base64url or hex)
  applications [--open | --close]
                     the applications for accounts (RP-5 §6.7); open or close them
  licences [--on | --off | --fetch | --from ZIP]
                     the copy of the FCC's licence database: turn it on or off (fetched each
                     week), fetch it now, or build it from a downloaded l_amat.zip
  qrz --username U --password P
                     look callsigns up in QRZ.com's callbook too, with your account (empty clears)
  mailboxes [--stalwart URL --key KEY | --command PROGRAM | --off] [--check]
                     make each approved account's mailbox in the mail server: by Stalwart's API,
                     or by your program (given the address and name; the password in
                     RADIOPOST_PASSWORD); --check asks without making one
  lookup CALL [--name NAME]
                     what the post office finds of a callsign: the FCC copy, QRZ, its history
  health             check the post office now, as the service does each hour, and say how it went
  jobs [--install]   run the jobs the desk asked for (the root-owned runner does, as root); with
                     --install, put that runner in place once (run with sudo)
  backup --to FILE [--passphrase-file F]
                     the post office's key, settings and state, and the newest backup of the
                     mail server's mailboxes, in one file, sealed with the passphrase in F when
                     given
  restore FILE [--force] [--passphrase-file F] [--without-mail]
                     put a backup's files back (stop the service first), and its mailboxes:
                     Stalwart stopped, its data moved aside and replaced, Stalwart started again
                     (as root); --without-mail leaves the mail server alone
  mail-backup        back up the mail server's mailboxes now: Stalwart stopped, /var/lib/stalwart
                     and /etc/stalwart archived, Stalwart started again; the last three kept
                     (as root: the jobs runner does it each night)
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
        blocked: Vec::new(),
        resolver: None,
        hooks: Some(a.one("--hooks").unwrap_or_else(|| "127.0.0.1:8772".into())),
        hook_token: a.one("--hook-token"),
        smtp: a.one("--smtp"),
        operators: Vec::new(),
        lookups: radiopost_postoffice::lookups::LookupSettings::default(),
        mailboxes: radiopost_postoffice::mailboxes::MailboxSettings::default(),
        health: radiopost_postoffice::serve::HealthSettings::default(),
        compress: true,
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
    if !selectors.is_empty() {
        // the hourly check asks the same from now on
        let _ = admin(files, "PUT", "/settings", &json!({"dkim": selectors}));
    }
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
            Verdict::Warn => "warn",
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

/// `know`: another post office accepted (RP-1 §7.5), as DNS gives it, or by a key typed here.
fn know(files: &Files, a: &Args) -> Result<(), String> {
    let domain = a.need("--domain")?.to_lowercase();
    if a.has("--public") {
        return know_typed(files, a, &domain);
    }
    let settings = files.settings()?;
    let resolver = a
        .one("--resolver")
        .or(settings.resolver.clone())
        .unwrap_or_else(|| radiopost_api::RESOLVER.into());
    match known::fetch(&resolver, &domain, serve::now()) {
        Lookup::Failed(e) => return Err(format!("DNS did not answer: {e}")),
        Lookup::NoRecord => {
            return Err(format!(
                "{domain} has no RadioPost API record in DNS (RP-1 §7.1): if it is a post office \
                 not in DNS yet, give its key with --public"
            ));
        }
        Lookup::Found { found, error, .. } => {
            println!("{domain}");
            println!("  API      {}", found.api);
            for k in &found.keys {
                println!("  key      {}", k.kid);
            }
            println!(
                "  DNSSEC   {}",
                if found.dnssec { "signed" } else { "not signed" }
            );
            for (label, k) in [
                ("operator", "operator"),
                ("contact", "contact"),
                ("country", "country"),
            ] {
                if let Some(v) = found.info[k].as_str() {
                    println!("  {label:<8} {v}");
                }
            }
            if let Some(e) = error {
                println!("  not whole: {e}; the service keeps trying");
            }
        }
    }
    match admin(files, "POST", "/post-offices", &json!({"accept": domain})) {
        Ok(_) => println!("{domain} known; the service fetches it now and every six hours"),
        Err(e) if e.starts_with("the post office is not running") => {
            let mut settings = files.settings()?;
            if !known::accepted(&settings).contains(&domain) {
                settings.known.push(json!({"domain": domain}));
            }
            settings.blocked.retain(|b| b != &domain);
            files.write(
                "config.json",
                &serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?,
            )?;
            println!("{domain} known; the service fetches it when it starts");
        }
        Err(e) => return Err(e),
    }
    Ok(())
}

/// `post-offices`: what the service knows, has seen and has blocked; or a decision on one.
fn post_offices(files: &Files, a: &Args) -> Result<(), String> {
    let mut decision = None;
    for action in ["block", "unblock", "forget"] {
        if let Some(d) = a.one(&format!("--{action}")) {
            decision = Some(json!({action: d}));
        }
    }
    if a.has("--refresh") {
        decision = Some(json!({"refresh": true}));
    }
    let view = match decision {
        Some(body) => admin(files, "POST", "/post-offices", &body)?,
        None => admin(files, "GET", "/post-offices", &Json::Null)?,
    };
    let day = |v: &Json| {
        v.as_u64()
            .filter(|t| *t > 0)
            .map_or_else(|| "never".to_owned(), radiopost_postoffice::lookups::day)
    };
    println!("Known:");
    for k in view["known"].as_array().into_iter().flatten() {
        let error = k["error"]
            .as_str()
            .map_or_else(String::new, |e| format!("  last try failed: {e}"));
        println!(
            "  {}  {}  fetched {}{error}",
            k["domain"].as_str().unwrap_or(""),
            k["api"].as_str().unwrap_or("(nothing fetched yet)"),
            day(&k["found"])
        );
    }
    println!("Seen (met, not known):");
    for s in view["seen"].as_array().into_iter().flatten() {
        let how: Vec<&str> = s["how"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
            .collect();
        println!(
            "  {}  run by {}  met {} ({})",
            s["domain"].as_str().unwrap_or(""),
            s["operator"].as_str().unwrap_or("?"),
            day(&s["last"]),
            how.join(", ")
        );
    }
    let blocked: Vec<&str> = view["blocked"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .collect();
    println!(
        "Blocked: {}",
        if blocked.is_empty() {
            "none".to_owned()
        } else {
            blocked.join(", ")
        }
    );
    Ok(())
}

/// `know --public`: another post office known by a key typed here, before it is in DNS.
fn know_typed(files: &Files, a: &Args, domain: &str) -> Result<(), String> {
    let mut settings = files.settings()?;
    let public = key_bytes(&a.need("--public")?).ok_or("--public is not a 32-byte key")?;
    let key = json!({
        "kid": radiopost::cbor::hex(&radiopost::crypto::kid(&public)),
        "public": base64url::encode(&public),
        "not_before": a.one("--not-before").and_then(|t| t.parse::<u64>().ok()).unwrap_or(0),
        "not_after": a.one("--not-after").and_then(|t| t.parse::<u64>().ok()).unwrap_or(u64::MAX),
    });
    let entry = json!({"domain": domain, "api": a.one("--api").unwrap_or_default(), "keys": [key]});
    settings
        .known
        .retain(|k| k.get("domain").and_then(Json::as_str) != Some(domain));
    settings.known.push(entry);
    settings.blocked.retain(|b| b != domain);
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

/// `lookup CALL [--name NAME]`: the evidence the operator's desk shows (D35).
fn lookup(files: &Files, a: &Args) -> Result<(), String> {
    let call = a
        .words
        .iter()
        .skip_while(|w| w.as_str() != "lookup")
        .nth(1)
        .filter(|w| !w.starts_with("--"))
        .cloned()
        .ok_or("which callsign?")?;
    let name = a.one("--name").unwrap_or_default();
    let enc = |s: &str| s.replace('%', "%25").replace(' ', "+").replace('&', "%26");
    let e = admin(
        files,
        "GET",
        &format!("/lookup?call={}&name={}", enc(&call), enc(&name)),
        &Json::Null,
    )?;
    println!("{}", e["verdict"].as_str().unwrap_or(""));
    for line in e["lines"].as_array().into_iter().flatten() {
        println!("  {}", line.as_str().unwrap_or(""));
    }
    for flag in e["flags"].as_array().into_iter().flatten() {
        println!("  ! {}", flag.as_str().unwrap_or(""));
    }
    Ok(())
}

/// `health`, `jobs`, `backup`, `restore`, `mail-backup`: the server's own care.
fn care(dir: &std::path::Path, files: &Files, a: &Args, command: &str) -> Result<(), String> {
    match command {
        "health" => {
            let h = admin(files, "POST", "/health", &Json::Null)?;
            for l in h["lines"].as_array().into_iter().flatten() {
                let mark = match l["verdict"].as_str() {
                    Some("ok") => "ok  ",
                    Some("fail") => "FAIL",
                    Some("warn") => "warn",
                    _ => "note",
                };
                println!("{mark}  {}", l["text"].as_str().unwrap_or(""));
            }
            Ok(())
        }
        "jobs" => {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            if a.has("--install") {
                println!("{}", radiopost_postoffice::jobs::install(dir, &exe)?);
                return Ok(());
            }
            let table = radiopost_postoffice::jobs::table(dir, &exe);
            for line in radiopost_postoffice::jobs::run_pending(dir, &table, serve::now) {
                eprintln!("radiopost-po jobs: {line}");
            }
            Ok(())
        }
        "backup" => {
            let zip = radiopost_postoffice::backups::archive(dir, true)?;
            let data = match passphrase(a)? {
                Some(p) => radiopost_postoffice::backups::seal(&zip, &p)?,
                None => zip,
            };
            let to = a.need("--to")?;
            std::fs::write(&to, data).map_err(|e| format!("{to}: {e}"))?;
            println!("written {to}");
            Ok(())
        }
        "restore" => {
            let from = a
                .words
                .iter()
                .skip_while(|w| w.as_str() != "restore")
                .nth(1)
                .filter(|w| !w.starts_with("--"))
                .cloned()
                .ok_or("which backup?")?;
            let file = std::fs::read(&from).map_err(|e| format!("{from}: {e}"))?;
            let zip = radiopost_postoffice::backups::open(&file, passphrase(a)?.as_deref())?;
            let written = radiopost_postoffice::backups::restore(&zip, dir, a.has("--force"))?;
            println!("restored {}", written.join(", "));
            if !a.has("--without-mail") {
                let server = radiopost_postoffice::backups::MailServer::stalwart();
                let restored = radiopost_postoffice::backups::restore_mail(
                    &zip,
                    &server,
                    serve::now(),
                )
                .map_err(|e| {
                    format!(
                        "{e}\nthe post office's files are back; for the mailboxes, run restore \
                         again as root with --force, or give --without-mail"
                    )
                })?;
                match restored {
                    Some(said) => println!("{said}"),
                    None => println!("the backup holds no mailboxes: the mail server is untouched"),
                }
            }
            println!("start the post office again");
            Ok(())
        }
        "mail-backup" => {
            let (path, bytes) = radiopost_postoffice::backups::mail_backup(
                dir,
                &radiopost_postoffice::backups::MailServer::stalwart(),
                &radiopost_postoffice::lookups::day(serve::now()),
            )?;
            println!("mailboxes backed up: {} ({bytes} bytes)", path.display());
            Ok(())
        }
        _ => Err(format!("unknown command {command}")),
    }
}

/// `install`: the guide's set-up done step by step (`radiopost_postoffice::install`).
fn install(a: &Args) -> Result<(), String> {
    use radiopost_postoffice::install::{self, Options, Secrets, System};
    let options = Options {
        domain: a.need("--domain")?,
        operator: a.need("--operator")?.to_uppercase(),
        contact: a.need("--contact")?,
        dir: PathBuf::from(
            a.one("--dir")
                .unwrap_or_else(|| "/var/lib/radiopost".into()),
        ),
        exe: PathBuf::from("/usr/local/bin/radiopost-po"),
        from: std::env::current_exe().map_err(|e| e.to_string())?,
    }
    .checked()?;
    let dry_run = a.has("--dry-run");
    // the secrets made once, and kept for a second run
    let kept = std::path::Path::new("/root/radiopost-install.json");
    let made: Json = std::fs::read_to_string(kept)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| {
            let fresh = |n: usize| {
                let mut b = vec![0u8; n];
                getrandom::fill(&mut b).expect("the system's random numbers");
                radiopost::cbor::hex(&b)
            };
            json!({"hook_token": fresh(24), "smtp_password": fresh(24)})
        });
    if !dry_run {
        std::fs::write(kept, made.to_string())
            .map_err(|e| format!("{}: {e} (run it as root)", kept.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(kept, std::fs::Permissions::from_mode(0o600));
        }
    }
    let secrets = Secrets {
        hook_token: made["hook_token"].as_str().unwrap_or_default().to_owned(),
        smtp_password: made["smtp_password"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    };
    let steps = install::plan(&options, &secrets, &System);
    let yes = a.has("--yes");
    let wait = || {
        if yes {
            return true;
        }
        print!("\n    Done? Type y and Enter to go on (anything else stops here): ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).is_ok() && line.trim().eq_ignore_ascii_case("y")
    };
    install::execute(&steps, dry_run, &mut std::io::stdout(), wait)?;
    if !dry_run {
        println!(
            "\nThe post office is set up. Its secrets are in {}. Enroll your own client with \
             a code: {} --dir {} code --address you@{} --operator {}",
            kept.display(),
            options.exe.display(),
            options.dir.display(),
            options.domain,
            options.operator
        );
    }
    Ok(())
}

/// The passphrase in `--passphrase-file`, its first line, when given.
fn passphrase(a: &Args) -> Result<Option<String>, String> {
    a.one("--passphrase-file")
        .map(|f| {
            std::fs::read_to_string(&f)
                .map(|t| t.lines().next().unwrap_or("").to_owned())
                .map_err(|e| format!("{f}: {e}"))
        })
        .transpose()
}

/// `mailboxes`: how approved accounts' mailboxes are made, set and checked.
fn mailboxes(files: &Files, a: &Args) -> Result<(), String> {
    let set = if let Some(url) = a.one("--stalwart") {
        Some(json!({"kind": "stalwart", "url": url, "api_key": a.one("--key").unwrap_or_default()}))
    } else if let Some(cmd) = a.one("--command") {
        Some(json!({"kind": "command", "command": cmd}))
    } else if a.has("--off") {
        Some(json!({"kind": ""}))
    } else {
        None
    };
    if let Some(m) = set {
        admin(files, "PUT", "/settings", &json!({"mailboxes": m}))?;
    }
    let s = admin(files, "GET", "/settings", &Json::Null)?;
    let kind = s["mailboxes"]["kind"].as_str().unwrap_or("");
    println!(
        "mailboxes: {}",
        if kind.is_empty() { "by hand" } else { kind }
    );
    if a.has("--check") {
        let c = admin(files, "POST", "/mailboxes/check", &Json::Null)?;
        println!("{}", c["said"].as_str().unwrap_or(""));
    }
    Ok(())
}

/// `licences`: the FCC copy's state; turned on or off, fetched, or built from a file.
fn licences(dir: &std::path::Path, files: &Files, a: &Args) -> Result<(), String> {
    if let Some(zip) = a.one("--from") {
        let out = dir.join("licences");
        std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        let n = radiopost_postoffice::licences::build_from_zip(
            std::path::Path::new(&zip),
            &out.join("fcc.tsv"),
        )?;
        println!("{n} callsigns in {}", out.join("fcc.tsv").display());
        return Ok(());
    }
    if a.has("--on") || a.has("--off") {
        admin(files, "PUT", "/settings", &json!({"fcc": a.has("--on")}))?;
    }
    if a.has("--fetch") {
        admin(files, "POST", "/licences", &Json::Null)?;
        println!("fetching the FCC's file: `radiopost-po licences` says when it is done");
    }
    let s = admin(files, "GET", "/settings", &Json::Null)?;
    let f = &s["fcc"];
    println!(
        "FCC copy {}: {} callsigns{}{}",
        if f["on"].as_bool() == Some(true) {
            "on"
        } else {
            "off"
        },
        f["count"],
        if f["fetching"].as_bool() == Some(true) {
            ", fetching now"
        } else {
            ""
        },
        f["last"]
            .as_str()
            .map(|l| format!(", last fetch: {l}"))
            .unwrap_or_default()
    );
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
    if command == "install" {
        return install(a);
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
        "post-offices" => post_offices(&files, a),
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
            if answer["operates"] == json!(true) {
                println!(
                    "{} operates this post office: whoever enrolls with this code can use the desk",
                    answer["operator"].as_str().unwrap_or("")
                );
            }
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
        "licences" => licences(&dir, &files, a),
        "qrz" => {
            let body = json!({"qrz_username": a.need("--username")?,
                              "qrz_password": a.one("--password").unwrap_or_default()});
            admin(&files, "PUT", "/settings", &body)?;
            println!("QRZ lookups set");
            Ok(())
        }
        "lookup" => lookup(&files, a),
        "mailboxes" => mailboxes(&files, a),
        "health" | "jobs" | "backup" | "restore" | "mail-backup" => care(&dir, &files, a, command),
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
