// SPDX-License-Identifier: MIT OR Apache-2.0
//! What every test stands on: the program under test and its driver, the test post office, the
//! test adapter, and a peer that speaks RP-3's PDUs through it and keeps the transcript.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ed25519_dalek::SigningKey;
use radiopost::cbor::{self, Value};
use radiopost::credential::{Body, Credential, PostOfficeKey, Trust, flags};
use radiopost::pdu::{self, Deframer};
use rpmi::channel::{Channel, StationConfig};
use rpmi::host::{Event, Host};
use rpmi::tcp::{Local, Server};
use rpmi::wire::Caps;
use serde_json::Value as Json;

/// A test's verdict, or the first point where the program left what the specifications allow.
pub type Outcome = Result<(), String>;

/// The program under test, run through its driver file: each command a template, with `{bin}`,
/// `{home}` and the test's values put in.
pub struct Driver {
    bin: String,
    commands: HashMap<String, Vec<String>>,
}

impl Driver {
    /// A driver from its JSON, for the program at `bin`.
    ///
    /// # Errors
    ///
    /// The JSON is not a driver.
    pub fn new(json: &Json, bin: &str) -> Result<Driver, String> {
        let commands = json
            .as_object()
            .ok_or("a driver is a JSON object")?
            .iter()
            .filter_map(|(k, v)| {
                let words = v
                    .as_array()?
                    .iter()
                    .map(|w| w.as_str().map(str::to_owned))
                    .collect::<Option<Vec<String>>>();
                Some((k.clone(), words?))
            })
            .collect();
        Ok(Driver {
            bin: bin.to_owned(),
            commands,
        })
    }

    fn words(&self, name: &str, vars: &[(&str, &str)]) -> Result<Vec<String>, String> {
        let words = self
            .commands
            .get(name)
            .ok_or_else(|| format!("the driver has no `{name}` command"))?;
        Ok(words
            .iter()
            .map(|w| {
                vars.iter()
                    .fold(w.replace("{bin}", &self.bin), |w, (k, v)| {
                        w.replace(&format!("{{{k}}}"), v)
                    })
            })
            .collect())
    }

    /// A template's first word with the values put in: a URL the program serves (its door).
    ///
    /// # Errors
    ///
    /// The driver has no such template.
    pub fn template(&self, name: &str, vars: &[(&str, &str)]) -> Result<String, String> {
        self.words(name, vars)?
            .into_iter()
            .next()
            .ok_or_else(|| format!("the driver's `{name}` is empty"))
    }

    /// Runs a command to its end; its standard output.
    ///
    /// # Errors
    ///
    /// It could not run, or ended with a failure.
    pub fn run(&self, name: &str, vars: &[(&str, &str)], stdin: &str) -> Result<String, String> {
        let words = self.words(name, vars)?;
        let mut child = Command::new(&words[0])
            .args(&words[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run {}: {e}", words[0]))?;
        if let Some(mut input) = child.stdin.take() {
            let _ = input.write_all(stdin.as_bytes());
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(format!(
                "`{name}` failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    /// Starts a command and leaves it running (a call, or listening).
    ///
    /// # Errors
    ///
    /// It could not start.
    pub fn start(&self, name: &str, vars: &[(&str, &str)]) -> Result<Running, String> {
        let words = self.words(name, vars)?;
        let child = Command::new(&words[0])
            .args(&words[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run {}: {e}", words[0]))?;
        Ok(Running { child })
    }
}

/// A command of the program's, running.
pub struct Running {
    child: Child,
}

impl Running {
    /// Waits at most `timeout` for it to end: whether it ended, and what it wrote.
    #[must_use]
    pub fn finish(mut self, timeout: Duration) -> (Option<bool>, String) {
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                break Some(status.success());
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let mut said = String::new();
        if let Some(mut out) = self.child.stdout.take() {
            let _ = out.read_to_string(&mut said);
        }
        if let Some(mut err) = self.child.stderr.take() {
            let _ = err.read_to_string(&mut said);
        }
        (status, said)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The clock, Unix seconds.
#[must_use]
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Hexadecimal text as bytes.
#[must_use]
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    (s.len() % 2 == 0)
        .then(|| {
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
                .collect()
        })
        .flatten()
}

/// A new device key.
///
/// # Panics
///
/// When the system has no random numbers to give.
#[must_use]
pub fn new_key() -> SigningKey {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).expect("the system's random numbers");
    SigningKey::from_bytes(&seed)
}

/// Random bytes.
///
/// # Panics
///
/// When the system has no random numbers to give.
#[must_use]
pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("the system's random numbers");
    b
}

/// The test post offices of `vectors/keys.json`: their private halves are published, so they are
/// never real keys, and a program in conformance mode never takes them on the air (§2.3).
pub struct PostOffices {
    keys: Vec<(String, SigningKey, u64, u64)>,
    serial: u64,
}

impl PostOffices {
    /// The keys valid now and for the next 90 days, from the text of `keys.json`.
    ///
    /// # Errors
    ///
    /// The text is not the keys file, or no key is valid.
    pub fn from_json(text: &str) -> Result<PostOffices, String> {
        let v: Json = serde_json::from_str(text).map_err(|e| format!("keys.json: {e}"))?;
        let t = now();
        let keys: Vec<_> = v["keys"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|k| {
                let domain = k["label"]
                    .as_str()?
                    .strip_prefix("po:")?
                    .split(':')
                    .next()?;
                let seed: [u8; 32] = unhex(k["seed_hex"].as_str()?)?.try_into().ok()?;
                let (nb, na) = (k["not_before"].as_u64()?, k["not_after"].as_u64()?);
                (nb <= t && t + 90 * 86_400 < na)
                    .then(|| (domain.to_owned(), SigningKey::from_bytes(&seed), nb, na))
            })
            .collect();
        if keys.is_empty() {
            return Err("no test post office key in keys.json is valid now".into());
        }
        Ok(PostOffices { keys, serial: 7000 })
    }

    /// Every test key, as `trust` takes them: domain, public key, validity.
    #[must_use]
    pub fn published(&self) -> Vec<(String, [u8; 32], u64, u64)> {
        self.keys
            .iter()
            .map(|(d, k, nb, na)| (d.clone(), k.verifying_key().to_bytes(), *nb, *na))
            .collect()
    }

    /// The trust a verifier with every test key has.
    #[must_use]
    pub fn trust(&self) -> Trust {
        let mut trust = Trust::default();
        for (d, public, nb, na) in self.published() {
            trust.keys.entry(d).or_default().push(PostOfficeKey {
                public,
                not_before: nb,
                not_after: na,
            });
        }
        trust
    }

    /// The test key of a domain.
    #[must_use]
    pub fn key_of(&self, domain: &str) -> Option<&SigningKey> {
        self.keys
            .iter()
            .find(|(d, ..)| d == domain)
            .map(|(_, k, ..)| k)
    }

    /// A credential with the body given, signed by its address's domain's test key: for the
    /// credentials a test needs wrong (expired, of an unknown domain).
    ///
    /// # Panics
    ///
    /// When no test key of the address's domain is valid now.
    #[must_use]
    pub fn issue_body(&mut self, body: &Body) -> Vec<u8> {
        let domain = body.address.rsplit_once('@').map_or("", |(_, d)| d);
        let key = self
            .key_of(domain)
            .unwrap_or_else(|| panic!("no test key for {domain}"));
        Credential::issue(body, key).encode()
    }

    /// A credential for `address`'s account, valid from now for 30 days, may send and receive.
    ///
    /// # Panics
    ///
    /// When no test key of the address's domain is valid now.
    pub fn issue(&mut self, address: &str, operator: &str, device: &[u8; 32]) -> Vec<u8> {
        let domain = address.rsplit_once('@').map_or("", |(_, d)| d);
        let (_, key, _, _) = self
            .keys
            .iter()
            .find(|(d, ..)| d == domain)
            .unwrap_or_else(|| panic!("no test key for {domain}"));
        self.serial += 1;
        let t = now();
        let body = Body {
            v: 1,
            serial: self.serial,
            address: address.into(),
            operator: operator.into(),
            device: *device,
            issued: t - 60,
            expires: t + 30 * 86_400,
            flags: flags::SEND | flags::RECEIVE,
        };
        Credential::issue(&body, key).encode()
    }
}

/// The program under test, set up in a fresh home: its own key, a credential the test post office
/// issued for it, the test keys trusted, conformance mode on.
pub struct Program<'a> {
    /// Its driver.
    pub driver: &'a Driver,
    /// Its home.
    pub home: PathBuf,
    /// Its account.
    pub address: String,
    /// Its callsign.
    pub operator: String,
    /// Its credential.
    pub credential: Vec<u8>,
    /// The test adapter's address, where it is to call from.
    pub adapter: String,
}

impl<'a> Program<'a> {
    /// Sets the program up for one test.
    ///
    /// # Errors
    ///
    /// A command failed, or `key` printed no key.
    pub fn setup(
        driver: &'a Driver,
        scratch: &Path,
        test: &str,
        offices: &mut PostOffices,
        adapter: SocketAddr,
    ) -> Result<Program<'a>, String> {
        let home = scratch.join(test);
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).map_err(|e| e.to_string())?;
        let h = home.to_string_lossy().into_owned();
        let at = adapter.to_string();
        driver.run("conformance", &[("home", &h), ("adapter", &at)], "")?;
        let out = driver.run("key", &[("home", &h)], "")?;
        let public: [u8; 32] = out
            .split_whitespace()
            .find_map(|w| unhex(w).and_then(|b| b.try_into().ok()))
            .ok_or_else(|| format!("`key` printed no 32-byte key in hex: {out:?}"))?;
        let address = "program@radiopost.org".to_owned();
        let operator = "KK4ODA".to_owned();
        let credential = offices.issue(&address, &operator, &public);
        let file = home.join("credential.hex");
        std::fs::write(&file, cbor::hex(&credential)).map_err(|e| e.to_string())?;
        driver.run(
            "init",
            &[("home", &h), ("credential", &file.to_string_lossy())],
            "",
        )?;
        for (domain, key, nb, na) in offices.published() {
            driver.run(
                "trust",
                &[
                    ("home", &h),
                    ("domain", &domain),
                    ("public", &cbor::hex(&key)),
                    ("not_before", &nb.to_string()),
                    ("not_after", &na.to_string()),
                ],
                "",
            )?;
        }
        Ok(Program {
            driver,
            home,
            address,
            operator,
            credential,
            adapter: at,
        })
    }

    fn home(&self) -> String {
        self.home.to_string_lossy().into_owned()
    }

    /// Writes a message to `to` with the program, for its outbox.
    ///
    /// # Errors
    ///
    /// The command failed.
    pub fn compose(&self, to: &str, subject: &str, body: &str) -> Result<(), String> {
        self.driver
            .run(
                "compose",
                &[("home", &self.home()), ("to", to), ("subject", subject)],
                body,
            )
            .map(|_| ())
    }

    /// Starts the program calling `callsign`: a gateway, or with `station` another client.
    ///
    /// # Errors
    ///
    /// It could not start.
    pub fn call(&self, callsign: &str, station: bool) -> Result<Running, String> {
        let name = if station { "call_station" } else { "call" };
        self.driver.start(
            name,
            &[
                ("home", &self.home()),
                ("callsign", callsign),
                ("adapter", &self.adapter),
            ],
        )
    }

    /// Starts the program listening for a call.
    ///
    /// # Errors
    ///
    /// It could not start.
    pub fn listen(&self) -> Result<Running, String> {
        self.driver.start(
            "listen",
            &[("home", &self.home()), ("adapter", &self.adapter)],
        )
    }

    /// What a folder lists (`inbox` or `outbox`).
    ///
    /// # Errors
    ///
    /// The command failed.
    pub fn folder(&self, name: &str) -> Result<String, String> {
        self.driver.run(name, &[("home", &self.home())], "")
    }
}

/// The test adapter (§2.2): the loopback channel with the program's station, which it reaches over
/// TCP, and the tester's, inside this process.
pub struct TestAdapter {
    server: Server,
    /// Where the program connects.
    pub program_at: SocketAddr,
    /// Where each program connects, when the test runs several (two gateways).
    pub programs_at: Vec<SocketAddr>,
    tester_station: usize,
}

impl TestAdapter {
    /// A test adapter whose `received` cut what the program hears into `frame_bytes`.
    ///
    /// # Errors
    ///
    /// No port could be taken.
    pub fn new(frame_bytes: usize) -> Result<TestAdapter, String> {
        TestAdapter::with_programs(frame_bytes, 1)
    }

    /// A test adapter with `programs` stations for programs under test, each with its own port.
    ///
    /// # Errors
    ///
    /// No port could be taken.
    pub fn with_programs(frame_bytes: usize, programs: usize) -> Result<TestAdapter, String> {
        let caps = Caps {
            bandwidths: vec![500, 2300],
            max_links: 1,
            delivery: true,
            broadcast: false,
            quality: false,
            busy: true,
            regulatory: false,
            max_queue: 1 << 20,
            shared: false,
        };
        let station = |name: &str, prefix: &str, frame| StationConfig {
            name: name.into(),
            modem: "radiopost-tester's test adapter".into(),
            caps: caps.clone(),
            prefix: prefix.into(),
            frame_bytes: frame,
        };
        let mut ch = Channel::new();
        let stations: Vec<usize> = (0..programs.max(1))
            .map(|i| {
                ch.add_station(station(
                    &format!("program{i}"),
                    &format!("p{i}"),
                    frame_bytes,
                ))
            })
            .collect();
        let tester_station = ch.add_station(station("tester", "t", 4096));
        // a call nobody answers fails in 3 s: the tester calls a program until it listens
        let server = Server::new(ch, Duration::from_secs(3));
        let programs_at = stations
            .iter()
            .map(|s| server.listen(*s, "127.0.0.1:0").map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TestAdapter {
            server,
            program_at: programs_at[0],
            programs_at,
            tester_station,
        })
    }

    /// The tester's side of the air, answering to `callsigns`, listening or not.
    ///
    /// # Errors
    ///
    /// The adapter refused what the tester asked.
    pub fn peer(&self, callsigns: &[&str], listen: bool) -> Result<Peer, String> {
        let local = self.server.attach(self.tester_station, "tester");
        let mut peer = Peer {
            local,
            host: Host::new(),
            link: None,
            stream: Deframer::new(),
            pending: Vec::new(),
            transcript: Vec::new(),
        };
        let hello = peer.host.hello("radiopost-tester");
        peer.ask(&hello)?;
        let calls = peer.host.callsigns(callsigns);
        peer.ask(&calls)?;
        if listen {
            let on = peer.host.listen(true);
            peer.ask(&on)?;
        }
        Ok(peer)
    }
}

/// The tester's station: RP-3's PDUs over one link of the test adapter.
pub struct Peer {
    local: Local,
    host: Host,
    link: Option<String>,
    stream: Deframer,
    pending: Vec<Event>,
    /// What crossed the link, both ways, as the vectors write PDUs.
    pub transcript: Vec<String>,
}

/// What the program sent that is not a PDU in good order.
#[derive(Debug)]
pub enum Heard {
    /// A PDU.
    Pdu(Value),
    /// The link is gone; `remote` when the program hung up.
    Gone {
        /// How the adapter said it ended, from this side.
        reason: String,
    },
    /// Nothing within the time.
    Silence,
}

impl Peer {
    fn ask(&mut self, request: &rpmi::Message) -> Outcome {
        self.local.send(request);
        let id = request.get("id").cloned();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let Some(m) = self.local.next(Duration::from_millis(200)) else {
                continue;
            };
            let e = Event::read(&m).map_err(|e| format!("the test adapter: {e}"))?;
            match &e {
                Event::Ok { id: i } if Some(i) == id.as_ref() => return Ok(()),
                Event::Hello { id: Some(i), .. } if Some(i) == id.as_ref() => return Ok(()),
                Event::Error {
                    id: Some(i), code, ..
                } if Some(i) == id.as_ref() => {
                    return Err(format!("the test adapter refused the tester: {code}"));
                }
                _ => self.pending.push(e),
            }
        }
        Err("the test adapter did not answer the tester".into())
    }

    fn event(&mut self, timeout: Duration) -> Option<Event> {
        if !self.pending.is_empty() {
            return Some(self.pending.remove(0));
        }
        self.local.next(timeout).and_then(|m| Event::read(&m).ok())
    }

    /// Waits for a link: the program's call answered, or the tester's answered.
    ///
    /// # Errors
    ///
    /// None in time, or the call failed.
    pub fn wait_link(&mut self, timeout: Duration) -> Outcome {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match self.event(Duration::from_millis(100)) {
                Some(Event::Connected { link, .. }) => {
                    self.link = Some(link);
                    // a new link starts a new stream: nothing of a broken one carries over
                    self.stream = Deframer::new();
                    return Ok(());
                }
                Some(Event::Failed { code, .. }) => {
                    return Err(format!("the tester's call failed: {code}"));
                }
                _ => {}
            }
        }
        Err("the program never connected".into())
    }

    /// Calls the program's callsign.
    ///
    /// # Errors
    ///
    /// The call failed.
    pub fn call(&mut self, callsign: &str) -> Outcome {
        let connect = self.host.connect(callsign, None, None);
        self.ask(&connect)?;
        self.wait_link(Duration::from_secs(20))
    }

    /// Calls the program's callsign until it answers: it may not be listening yet.
    ///
    /// # Errors
    ///
    /// No call was answered within the time.
    pub fn call_within(&mut self, callsign: &str, within: Duration) -> Outcome {
        let deadline = Instant::now() + within;
        loop {
            match self.call(callsign) {
                Ok(()) => return Ok(()),
                Err(e) if Instant::now() >= deadline => return Err(e),
                Err(_) => std::thread::sleep(Duration::from_millis(200)),
            }
        }
    }

    /// One transmission of PDUs.
    ///
    /// # Panics
    ///
    /// When a PDU does not encode: the tester's own scripts are at fault.
    pub fn send(&mut self, pdus: &[Value]) {
        let mut bytes = Vec::new();
        for p in pdus {
            self.transcript
                .push(format!("tester  → {}", short(&p.diagnostic())));
            bytes.extend(pdu::frame(p).expect("the tester's PDUs encode"));
        }
        self.send_raw(&bytes);
    }

    /// Bytes on the link as they are, framed or not.
    pub fn send_raw(&mut self, bytes: &[u8]) {
        if let Some(link) = self.link.clone() {
            let m = self.host.send(&link, bytes, None);
            self.local.send(&m);
        }
    }

    /// Ends the link now, as a link that breaks (RP-3 §7).
    pub fn abort(&mut self) {
        if let Some(link) = self.link.take() {
            let m = self.host.abort(Some(&link));
            self.local.send(&m);
            self.transcript.push("tester  ✕ the link broken".into());
        }
    }

    /// Hangs up in order, as a caller does (RP-3 §8).
    pub fn hang_up(&mut self) {
        if let Some(link) = self.link.take() {
            let m = self.host.disconnect(&link);
            self.local.send(&m);
            self.transcript.push("tester  ✕ hung up".into());
        }
    }

    /// The program's next PDU, at most `max_pdu` bytes, or what came instead.
    pub fn next(&mut self, timeout: Duration, max_pdu: usize) -> Heard {
        let deadline = Instant::now() + timeout;
        loop {
            match self.stream.next(max_pdu) {
                Some(Ok(p)) => {
                    self.transcript
                        .push(format!("program → {}", short(&p.diagnostic())));
                    return Heard::Pdu(p);
                }
                Some(Err(code)) => {
                    self.transcript
                        .push(format!("program → a frame that does not decode ({code})"));
                    return Heard::Gone {
                        reason: format!("a bad frame: {code}"),
                    };
                }
                None => {}
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Heard::Silence;
            }
            match self.event(left.min(Duration::from_millis(200))) {
                Some(Event::Received { link, data }) if Some(&link) == self.link.as_ref() => {
                    self.stream.push(&data);
                }
                Some(Event::Disconnected { link, reason, .. })
                    if Some(&link) == self.link.as_ref() =>
                {
                    self.link = None;
                    self.transcript
                        .push(format!("program ✕ the link ended ({reason})"));
                    return Heard::Gone { reason };
                }
                _ => {}
            }
        }
    }

    /// The program's next PDU, which must come.
    ///
    /// # Errors
    ///
    /// The link ended or went quiet first.
    pub fn expect(&mut self, what: &str) -> Result<Value, String> {
        match self.next(Duration::from_secs(20), 1 << 16) {
            Heard::Pdu(p) => Ok(p),
            Heard::Gone { reason } => Err(format!("the link ended ({reason}) before {what}")),
            Heard::Silence => Err(format!("nothing for 20 s where {what} was due")),
        }
    }

    /// Waits for the program to hang up (RP-3 §8), with no more PDUs but `DONE`, which a station
    /// may say more than once (§5.7).
    ///
    /// # Errors
    ///
    /// It sent more, or kept the link.
    pub fn expect_hang_up(&mut self, within: Duration) -> Outcome {
        let deadline = Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.next(left, 1 << 16) {
                Heard::Gone { reason } if reason == "remote" => return Ok(()),
                Heard::Gone { reason } => {
                    return Err(format!("the link ended otherwise: {reason}"));
                }
                Heard::Pdu(p) if p.get(0).and_then(Value::as_u64) == Some(pdu::kind::DONE) => {}
                Heard::Pdu(p) => {
                    return Err(format!(
                        "a PDU where the caller should hang up: {}",
                        short(&p.diagnostic())
                    ));
                }
                Heard::Silence => {
                    return Err(format!("the program did not hang up within {within:?}"));
                }
            }
        }
    }
}

fn short(text: &str) -> String {
    if text.len() > 240 {
        let cut = (0..=240)
            .rev()
            .find(|i| text.is_char_boundary(*i))
            .unwrap_or(0);
        format!("{}…", &text[..cut])
    } else {
        text.to_owned()
    }
}
