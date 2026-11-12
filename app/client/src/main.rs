// SPDX-License-Identifier: MIT OR Apache-2.0
//! `radiopost`: the RadioPost client on the command line. `radiopost help` says how to use it.

use std::fmt::Write as _;
use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use radiopost::credential::PostOfficeKey;
use radiopost::message::{Message, key};
use radiopost_client::compose::{Draft, File, media_type};
use radiopost_client::session::{self, Fetch, Kind, Options, Packing};
use radiopost_client::store::{Error, Store, unhex};
use radiopost_client::{addresses, online};

const HELP: &str = "\
radiopost: the RadioPost client

usage: radiopost [--home DIR] COMMAND ...

  key                      make this device's key (once) and print its public key
  init --credential FILE   keep the credential a post office issued for this device's key
  trust DOMAIN PUBLIC_HEX [NOT_BEFORE NOT_AFTER]
                           trust a post office key (RP-1 §7.3)
  whoami                   the account and callsign the credential is for
  enroll --address ADDR --operator CALL --code CODE [--post-office URL]
                           enroll this device with the code the post office's operator gave
                           (RP-5 §6.1), making its key if there is none, and keep the credential
  post-office URL          the post office's API, when DNS does not say (RP-1 §7.1)
  renew                    a new credential for this device, over the Internet (RP-5 §6.2)
  report                   send the station-to-station sessions waiting to the post office
                           (RP-5 §6.6)
  settings [--set JSON]    the account's settings at the post office (RP-5 §6.3)
  addresses [--add 'NAME <ADDR>']... [--remove ADDR]...
                           the address list: whom you have written to, offered as To and Cc are
                           typed in the panel; this computer's own, not the accept list
  accept [--add ENTRY]... [--remove ENTRY]...
                           the accept list: addresses, or @domain (RP-5 §6.5)
  compose --to ADDR [--to ADDR] [--cc ADDR] [--subject TEXT] [--priority 0|1|2]
          [--attach FILE]... [--body FILE]
                           write a message into the outbox; the text from stdin without --body
  import FILE              put a message, in its canonical encoding (RP-2), in the outbox
  outbox | inbox | sent | trash
                           list a folder
  show ID                  show a message (ID: the start of its content ID)
  delete ID [--forever]    move a message to the trash, from whichever folder holds it; with
                           --forever, delete it for good. Here only: the post office, gateways
                           and other stations keep what they hold
  restore ID               put a message in the trash back in the folder it was deleted from
  empty-trash              delete every message in the trash for good
  attachments [ID [NAME]...]
                           ask on the next call to a gateway for a received message's attachments
                           held at the post office, all or those named (RP-3 §5.6); without ID,
                           what will be asked for
  reports                  the station-to-station sessions waiting to be reported (RP-5 §6.6)
  conformance --adapter HOST:PORT | conformance off
                           conformance mode: reach only the conformance tester's test adapter
  call CALLSIGN [--station] [--fetch all|none|BYTES] [SESSION OPTIONS]
                           call a gateway, or with --station another client's station (RP-3 §10)
  gateways [--refresh]     the gateways your post office lists (RP-5 §4.4), nearest first when
                           your grid is set; --refresh fetches the list over the Internet
  grid [SQUARE | --clear]  your Maidenhead grid square (EM73 or EM73tt), to rank gateways
  waiting                  what waited at your post office at the last call to a gateway
  fetch SERIAL... [--not]  fetch these waiting messages on the next call to a gateway, whatever
                           --fetch says; --not stops asking for them
  listen [SESSION OPTIONS] answer one station's call (RP-3 §10)
  panel [--port N] [--ui DIR]
                           serve the client's panel on this computer (port 8620) and say where
  update                   replace radiopost, and the other RadioPost programs beside it, with the
                           newest release's, their signature checked with the project's key
                           (the desktop application updates itself from its own window)

session options:
  --adapter HOST:PORT      the modem's RPMI adapter (default 127.0.0.1:8600)
  --bandwidth HZ           ask the modem for this bandwidth
  --plain                  send messages uncompressed
  --clock UNIX --nonce HEX --session-id HEX --challenge HEX
                           fix the clock and the random draws, to replay a session exactly

The home is --home, else $RADIOPOST_HOME, else .radiopost in the user's home directory.
";

/// A date as people read it, UTC (RP-0 §4.4).
fn utc(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    // civil from days (H. Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60
    )
}

/// The arguments after the command: flags with values, and the rest in order.
struct Args {
    flags: Vec<(String, String)>,
    switches: Vec<String>,
    rest: Vec<String>,
}

const SWITCHES: &[&str] = &[
    "--station",
    "--plain",
    "--refresh",
    "--clear",
    "--not",
    "--forever",
];

impl Args {
    fn parse(args: &[String]) -> Result<Args, Error> {
        let mut out = Args {
            flags: Vec::new(),
            switches: Vec::new(),
            rest: Vec::new(),
        };
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if SWITCHES.contains(&a.as_str()) {
                out.switches.push(a.clone());
            } else if a.starts_with("--") {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{a} needs a value"))?;
                out.flags.push((a.clone(), v.clone()));
                i += 1;
            } else {
                out.rest.push(a.clone());
            }
            i += 1;
        }
        Ok(out)
    }

    fn all(&self, name: &str) -> Vec<String> {
        self.flags
            .iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .collect()
    }

    fn one(&self, name: &str) -> Option<String> {
        self.all(name).pop()
    }

    fn has(&self, name: &str) -> bool {
        self.switches.iter().any(|s| s == name)
    }
}

fn fixed<const N: usize>(args: &Args, name: &str) -> Result<Option<[u8; N]>, Error> {
    args.one(name)
        .map(|h| {
            unhex(&h)
                .and_then(|b| b.try_into().ok())
                .ok_or_else(|| format!("{name} is {N} bytes in hex"))
        })
        .transpose()
}

fn options(args: &Args) -> Result<Options, Error> {
    let mut o = Options::new(
        &args
            .one("--adapter")
            .unwrap_or_else(|| format!("127.0.0.1:{}", rpmi::DEFAULT_PORT)),
    );
    o.bandwidth = args
        .one("--bandwidth")
        .map(|b| b.parse().map_err(|_| "--bandwidth is a number of Hz"))
        .transpose()?;
    o.clock = args
        .one("--clock")
        .map(|c| c.parse().map_err(|_| "--clock is Unix seconds"))
        .transpose()?;
    o.nonce = fixed(args, "--nonce")?;
    o.session_id = fixed(args, "--session-id")?;
    o.challenge = fixed(args, "--challenge")?;
    if args.has("--plain") {
        o.packing = Packing::None;
    }
    o.fetch = match args.one("--fetch").as_deref() {
        None | Some("all") => Fetch::All,
        Some("none") => Fetch::Nothing,
        Some(n) => Fetch::UpTo(n.parse().map_err(|_| "--fetch is all, none or a size")?),
    };
    Ok(o)
}

/// The addresses in each `--to` or `--cc` given, as the panel reads its fields: commas,
/// semicolons or spaces between them, and `Name <address>` as well as a bare address.
fn typed_addresses(given: &[String]) -> Result<Vec<(Option<String>, String)>, Error> {
    let mut found: Vec<(Option<String>, String)> = Vec::new();
    for text in given {
        for (name, a) in radiopost_client::compose::parse_named(text)? {
            if !found.iter().any(|(_, x)| *x == a) {
                found.push((name, a));
            }
        }
    }
    Ok(found)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A gateway from the post office's list, on one line: callsign, place, distance, channels,
/// hours.
fn gateway_line(g: &serde_json::Value) -> String {
    let channels: Vec<String> = g["channels"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            format!(
                "{:.4} MHz {} Hz",
                c["dial_hz"].as_f64().unwrap_or(0.0) / 1e6,
                c["bandwidth_hz"]
            )
        })
        .collect();
    let mut place = [g["place"].as_str(), g["grid"].as_str()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
    if let Some(km) = g["km"].as_f64() {
        let _ = write!(place, ", {km} km");
    }
    format!(
        "{:<10} {place}\n           {}; {}",
        g["callsign"].as_str().unwrap_or("?"),
        channels.join(", "),
        g["hours"].as_str().unwrap_or("hours not said")
    )
}

fn line_of(store: &Store, id: &radiopost::ContentId) -> String {
    match store.message(id) {
        Ok(m) => {
            let text = |k| {
                m.map()
                    .get(k)
                    .and_then(radiopost::cbor::Value::as_text)
                    .unwrap_or("")
                    .to_owned()
            };
            let date = m
                .map()
                .get(key::DATE)
                .and_then(radiopost::cbor::Value::as_u64)
                .unwrap_or(0);
            format!(
                "{}  {}  {}  {}",
                &id.text()[..6],
                utc(date),
                text(key::FROM),
                text(key::SUBJECT)
            )
        }
        Err(e) => format!("{}  ({e})", &id.text()[..6]),
    }
}

fn show(m: &Message) -> String {
    use radiopost::cbor::Value;
    let map = m.map();
    let list = |k| {
        map.get(k)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_text)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    };
    let mut out = format!("From: {}\nTo: {}\n", m.from().unwrap_or(""), list(key::TO));
    if map.get(key::CC).is_some() {
        let _ = writeln!(out, "Cc: {}", list(key::CC));
    }
    let date = map.get(key::DATE).and_then(Value::as_u64).unwrap_or(0);
    let _ = writeln!(out, "Date: {}", utc(date));
    match map.get(key::PRIORITY).and_then(Value::as_u64) {
        Some(1) => out.push_str("Priority: priority\n"),
        Some(2) => out.push_str("Priority: EMERGENCY\n"),
        _ => {}
    }
    let _ = writeln!(
        out,
        "Subject: {}\nID: {}",
        map.get(key::SUBJECT).and_then(Value::as_text).unwrap_or(""),
        m.content_id().text()
    );
    for f in map
        .get(key::FILES)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let _ = writeln!(
            out,
            "Attached: {} ({}, {} bytes)",
            f.get(1).and_then(Value::as_text).unwrap_or("?"),
            f.get(2).and_then(Value::as_text).unwrap_or("?"),
            f.get(3).and_then(Value::as_bytes).map_or(0, <[u8]>::len)
        );
    }
    for (name, kind, size, _) in Store::held(m) {
        let _ = writeln!(
            out,
            "Held at the post office: {name} ({kind}, {size} bytes)"
        );
    }
    out.push('\n');
    out.push_str(map.get(key::BODY).and_then(Value::as_text).unwrap_or(""));
    out
}

/// `delete` and `restore`: a message moved to the trash from every folder that holds it, deleted
/// for good with `--forever`, or put back from the trash.
fn trash_command(store: &mut Store, command: &str, args: &Args) -> Result<(), Error> {
    use radiopost_client::store::Folder;
    let prefix = args.rest.first().ok_or(format!("{command} ID"))?;
    let id = store.find(prefix)?;
    let in_trash = |store: &Store| -> Vec<Folder> {
        store
            .trash
            .iter()
            .filter(|t| t.was.id() == id)
            .map(|t| t.was.folder())
            .collect()
    };
    if command == "restore" {
        let from = in_trash(store);
        if from.is_empty() {
            return Err("that message is not in the trash".into());
        }
        for folder in from {
            store.restore(&id, folder)?;
            println!("back in the {}", folder.name());
        }
        return Ok(());
    }
    let holding: Vec<Folder> = [Folder::Inbox, Folder::Outbox, Folder::Sent]
        .into_iter()
        .filter(|f| match f {
            Folder::Inbox => store.inbox.iter().any(|a| a.id == id),
            Folder::Outbox => store.outbox.iter().any(|w| w.id == id),
            Folder::Sent => store.sent.contains(&id),
        })
        .collect();
    let forever = args.has("--forever");
    if holding.is_empty() && !forever {
        return Err("that message is in no folder: `radiopost trash` lists the trash".into());
    }
    for folder in &holding {
        store.delete(*folder, &id, now())?;
        if !forever {
            println!("moved from the {} to the trash", folder.name());
        }
    }
    if forever {
        let from = in_trash(store);
        if from.is_empty() {
            return Err("that message is in no folder and not in the trash".into());
        }
        for folder in from {
            store.purge(&id, folder)?;
        }
        println!("deleted for good");
    }
    Ok(())
}

#[allow(clippy::too_many_lines, reason = "one arm a command")]
fn run(words: &[String]) -> Result<(), Error> {
    let (home_flag, words) = match words {
        [flag, dir, rest @ ..] if flag == "--home" => (Some(dir.clone()), rest),
        _ => (None, words),
    };
    let Some((command, rest)) = words.split_first() else {
        print!("{HELP}");
        return Ok(());
    };
    let args = Args::parse(rest)?;
    if command == "update" {
        // the programs beside this one, not the home: it opens none
        let exe = std::env::current_exe().map_err(|e| format!("where is this program: {e}"))?;
        let at = exe.parent().ok_or("this program is in no directory")?;
        let said = radiopost_update::install_tools(
            &radiopost_update::channel(),
            env!("CARGO_PKG_VERSION"),
            at,
            radiopost_update::public_key(),
        )?;
        println!("{said}");
        return Ok(());
    }
    let mut store = Store::open(&radiopost_client::store::home(home_flag.as_deref()))?;
    // what the program says, and any panic, in a file of the home's: a window's log is gone
    // once it closes (2026-11-12)
    radiopost::logfile::keep_in(&store.home().join(radiopost_client::LOG_FILE));
    radiopost::logfile::record_panics();
    match command.as_str() {
        "help" | "--help" | "-h" => print!("{HELP}"),
        "key" => {
            let key = if let Ok(k) = store.device() {
                k
            } else {
                let mut seed = [0u8; 32];
                getrandom::fill(&mut seed).map_err(|e| e.to_string())?;
                let k = ed25519_dalek::SigningKey::from_bytes(&seed);
                store.set_device(&k)?;
                k
            };
            let public = radiopost::cbor::hex(&key.verifying_key().to_bytes());
            println!("{public}");
        }
        "init" => {
            if let Some(path) = args.one("--device-key") {
                let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
                let seed: [u8; 32] = unhex(&text)
                    .and_then(|b| b.try_into().ok())
                    .ok_or_else(|| format!("{path} is not a 32-byte seed in hex"))?;
                store.set_device(&ed25519_dalek::SigningKey::from_bytes(&seed))?;
            }
            let path = args
                .one("--credential")
                .ok_or("init needs --credential FILE")?;
            let bytes = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
            let bytes = std::str::from_utf8(&bytes)
                .ok()
                .and_then(unhex)
                .unwrap_or(bytes);
            store.device()?;
            let body = store.set_credential(&bytes)?;
            println!(
                "{} ({}), credential {} until {}",
                body.address,
                body.operator,
                body.serial,
                utc(body.expires)
            );
        }
        "trust" => {
            let [domain, public, ..] = args.rest.as_slice() else {
                return Err("trust DOMAIN PUBLIC_HEX [NOT_BEFORE NOT_AFTER]".into());
            };
            let public: [u8; 32] = unhex(public)
                .and_then(|b| b.try_into().ok())
                .ok_or("a post office key is 32 bytes in hex")?;
            let bound = |i: usize, default: u64| {
                args.rest.get(i).map_or(Ok(default), |s| {
                    s.parse().map_err(|_| "a time is Unix seconds")
                })
            };
            store.add_trust(
                domain,
                &PostOfficeKey {
                    public,
                    not_before: bound(2, 0)?,
                    not_after: bound(3, u64::MAX)?,
                },
            )?;
        }
        "whoami" => {
            let me = store.me()?;
            println!(
                "{} ({}), credential {} until {}",
                me.address,
                me.operator,
                me.serial,
                utc(me.expires)
            );
        }
        "compose" => {
            let me = store.me()?;
            let body = if let Some(path) = args.one("--body") {
                std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?
            } else {
                let mut text = String::new();
                std::io::stdin()
                    .read_to_string(&mut text)
                    .map_err(|e| e.to_string())?;
                text
            };
            let mut files = Vec::new();
            for path in args.all("--attach") {
                let data = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
                let name = std::path::Path::new(&path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("file")
                    .to_owned();
                files.push(File {
                    media_type: media_type(&name).into(),
                    name,
                    data,
                });
            }
            let to = typed_addresses(&args.all("--to"))?;
            let cc = typed_addresses(&args.all("--cc"))?;
            let plain = |n: &[(Option<String>, String)]| -> Vec<String> {
                n.iter().map(|(_, a)| a.clone()).collect()
            };
            let draft = Draft {
                from: me.address,
                to: plain(&to),
                cc: plain(&cc),
                date: now(),
                subject: args.one("--subject").unwrap_or_default(),
                body,
                files,
                priority: args
                    .one("--priority")
                    .map_or(Ok(0), |p| p.parse().map_err(|_| "--priority is 0, 1 or 2"))?,
                reply_to: None,
            };
            let id = store.queue(&draft.message()?)?;
            addresses::remember(store.home(), &[to, cc].concat(), now())?;
            println!("{}", id.text());
        }
        "import" => {
            let path = args.rest.first().ok_or("import FILE")?;
            let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
            let m = Message::from_canonical(&bytes).map_err(|e| {
                format!("{path} is not a message in its canonical encoding: {}", e.0)
            })?;
            println!("{}", store.queue(&m)?.text());
        }
        "outbox" => {
            for w in &store.outbox {
                println!("{}", line_of(&store, &w.id));
            }
        }
        "sent" => {
            for id in &store.sent {
                println!("{}", line_of(&store, id));
            }
        }
        "inbox" => {
            for a in &store.inbox {
                println!("{}  via {}", line_of(&store, &a.id), a.station);
            }
        }
        "trash" => {
            for t in &store.trash {
                println!(
                    "{}  from the {}",
                    line_of(&store, &t.was.id()),
                    t.was.folder().name()
                );
            }
        }
        "delete" | "restore" => trash_command(&mut store, command, &args)?,
        "empty-trash" => {
            let n = store.empty_trash()?;
            println!(
                "{n} message{} deleted for good",
                if n == 1 { "" } else { "s" }
            );
        }
        "conformance" => {
            if args.rest.first().map(String::as_str) == Some("off") {
                store.set_conformance(None)?;
            } else {
                let at = args
                    .one("--adapter")
                    .ok_or("conformance --adapter HOST:PORT, or conformance off")?;
                store.set_conformance(Some(&at))?;
            }
        }
        "enroll" => {
            let said = online::enroll(
                &mut store,
                args.one("--post-office").as_deref(),
                &args.one("--address").ok_or("enroll needs --address")?,
                &args.one("--operator").ok_or("enroll needs --operator")?,
                &args.one("--code").ok_or("enroll needs --code")?,
            )?;
            println!("{said}");
        }
        "post-office" => {
            let url = args.rest.first().ok_or("post-office URL")?;
            store.post_office = Some(url.trim_end_matches('/').to_owned());
            store.save()?;
        }
        "renew" => println!("{}", online::renew(&store)?),
        "report" => {
            let said = online::send_reports(&mut store)?;
            if said.is_empty() {
                println!("nothing to report");
            }
            for line in said {
                println!("{line}");
            }
        }
        "settings" => {
            let change = args
                .one("--set")
                .map(|t| serde_json::from_str::<serde_json::Value>(&t))
                .transpose()
                .map_err(|e| format!("--set: {e}"))?;
            let now = online::settings(&store, change.as_ref())?;
            println!("{}", serde_json::to_string_pretty(&now).unwrap_or_default());
        }
        "addresses" => {
            for typed in args.all("--add") {
                addresses::add(store.home(), &typed)?;
            }
            for address in args.all("--remove") {
                addresses::remove(store.home(), &address)?;
            }
            for e in addresses::list(store.home()) {
                let used = if e.uses == 0 {
                    "added by hand".to_owned()
                } else {
                    format!(
                        "written to {} time{}, last {}",
                        e.uses,
                        if e.uses == 1 { "" } else { "s" },
                        utc(e.used)
                    )
                };
                println!("{:<40} {used}", e.written());
            }
        }
        "accept" => {
            for entry in online::accept_list(&store, &args.all("--add"), &args.all("--remove"))? {
                println!("{entry}");
            }
        }
        "reports" => {
            for r in &store.reports {
                println!(
                    "{}  {} with {} sent, {} received",
                    utc(r["started"].as_u64().unwrap_or(0)),
                    r["role"].as_str().unwrap_or("?"),
                    r["sent"].as_array().map_or(0, Vec::len),
                    r["received"].as_array().map_or(0, Vec::len)
                );
            }
        }
        "show" => {
            let prefix = args.rest.first().ok_or("show ID")?;
            let id = store.find(prefix)?;
            println!("{}", show(&store.message(&id)?));
        }
        "attachments" => {
            if let Some((prefix, names)) = args.rest.split_first() {
                let id = store.find(prefix)?;
                let asked = store.ask_for_held(&id, names)?;
                println!("asked on the next call to a gateway: {}", asked.join(", "));
            } else {
                for line in store.files_asked_for() {
                    println!("{line}");
                }
            }
        }
        "call" => {
            let remote = args
                .rest
                .first()
                .ok_or("call CALLSIGN")?
                .to_ascii_uppercase();
            let kind = if args.has("--station") {
                Kind::Station
            } else {
                Kind::Gateway
            };
            let report = session::call(&mut store, &remote, kind, &options(&args)?, &mut |s| {
                println!("{s}");
            })?;
            if let Some(why) = report.failed {
                return Err(why);
            }
        }
        "gateways" => {
            if args.has("--refresh") {
                let n = online::gateways(&mut store)?;
                println!("{n} gateways listed");
            }
            let Some(list) = &store.gateway_list else {
                return Err("no gateway list kept yet: `radiopost gateways --refresh`".into());
            };
            println!(
                "{}'s post office, fetched {}",
                list["domain"].as_str().unwrap_or("?"),
                utc(list["fetched"].as_u64().unwrap_or(0))
            );
            for g in store.gateways_ranked() {
                println!("{}", gateway_line(&g));
            }
        }
        "grid" => {
            if args.has("--clear") {
                store.set_grid(None)?;
            } else if let Some(g) = args.rest.first() {
                store.set_grid(Some(g))?;
            }
            println!("{}", store.grid.as_deref().unwrap_or("no grid square set"));
        }
        "waiting" => {
            let Some(w) = &store.waiting else {
                return Err("no call to a gateway yet".into());
            };
            println!(
                "at the call to {}, {}:",
                w["via"].as_str().unwrap_or("?"),
                utc(w["at"].as_u64().unwrap_or(0))
            );
            for e in store.waiting_entries() {
                println!(
                    "#{:<5} {:>7} bytes  {}  {}{}{}",
                    e["serial"],
                    e["size"],
                    e["from"].as_str().unwrap_or(""),
                    e["subject"].as_str().unwrap_or(""),
                    if e["held"] == true {
                        "  (attachments held)"
                    } else {
                        ""
                    },
                    if e["fetch"] == true {
                        "  [fetch on the next call]"
                    } else {
                        ""
                    },
                );
            }
        }
        "fetch" => {
            if args.rest.is_empty() {
                return Err("fetch SERIAL... [--not]".into());
            }
            for serial in &args.rest {
                let n = serial
                    .trim_start_matches('#')
                    .parse()
                    .map_err(|_| format!("{serial} is not a serial"))?;
                store.fetch_on_next_call(n, !args.has("--not"))?;
            }
        }
        "panel" => {
            let port = args.one("--port").unwrap_or_else(|| "8620".into());
            let panel = radiopost_client::panel::Panel::new(
                store.home(),
                args.one("--ui").map(PathBuf::from),
            );
            panel.updates().watch();
            let at = panel.start(&format!("127.0.0.1:{port}"))?;
            println!("the panel is at http://{at}/");
            loop {
                std::thread::park();
            }
        }
        "listen" => {
            let report = session::answer(&mut store, &options(&args)?, &mut |s| {
                println!("{s}");
            })?;
            if let Some(why) = report.failed {
                return Err(why);
            }
        }
        other => return Err(format!("no command {other}: `radiopost help` lists them")),
    }
    Ok(())
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match run(&argv) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("radiopost: {e}");
            ExitCode::FAILURE
        }
    }
}
