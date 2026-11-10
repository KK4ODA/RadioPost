// SPDX-License-Identifier: MIT OR Apache-2.0
//! The adapter against real Aether daemons: two `aetherd`, joined by Aether's simulated channel
//! (`[sim]`), each with this adapter in front, carry session-03 (RP-3 §10) between its two
//! engines, and the transmissions must arrive byte for byte.
//!
//! It needs an `aetherd` of 0.2.0-beta.146 or later, named by `AETHERD`, and takes as long as the
//! session takes on the simulated air (about a minute):
//!
//! ```text
//! AETHERD=/path/to/aetherd cargo test -p radiopost-aether --test aetherd -- --ignored
//! ```

#[path = "../../../core/rpmi/tests/common/mod.rs"]
mod common;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use radiopost::session::Event as SessionEvent;
use rpmi::base64;
use rpmi::host::{Carrier, Engine, Event, Host};
use rpmi::tcp::Connection;
use rpmi::wire::Message;
use serde_json::Value as Json;

use common::{Answering, Calling, transmissions, vectors, world};

struct Daemon {
    child: Child,
    log: PathBuf,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Daemon {
    fn start(binary: &str, dir: &Path, name: &str, callsign: &str, sim: &str) -> Daemon {
        let config = dir.join(format!("{name}.toml"));
        std::fs::write(
            &config,
            format!(
                "schema_version = 13\ncallsign = \"{callsign}\"\n\
                 [radio]\nwait_for_clear = false\n\
                 [control]\nbind = \"127.0.0.1:0\"\n\
                 [record]\nauto = false\nsend_to_project = false\n\
                 [sim]\n{sim}\nsnr_db = 25.0\n"
            ),
        )
        .expect("a configuration");
        let log = dir.join(format!("{name}.log"));
        let child = Command::new(binary)
            .arg("--config")
            .arg(&config)
            .stdout(Stdio::from(std::fs::File::create(&log).expect("a log")))
            .stderr(Stdio::null())
            .spawn()
            .expect("aetherd starts");
        Daemon { child, log }
    }

    /// What the log says between `before` and `after`, once it says it.
    fn logged(&mut self, before: &str, after: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let mut text = String::new();
            if let Ok(mut f) = std::fs::File::open(&self.log) {
                let _ = f.read_to_string(&mut text);
            }
            if let Some((found, _)) = text
                .split_once(before)
                .and_then(|(_, rest)| rest.split_once(after))
            {
                return found.to_owned();
            }
            assert!(
                self.child.try_wait().expect("a status").is_none(),
                "aetherd exited:\n{text}"
            );
            assert!(
                Instant::now() < deadline,
                "aetherd never said {before:?}:\n{text}"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn control(&mut self) -> String {
        format!("ws://{}/v1", self.logged("listening on ws://", "/v1"))
    }
}

/// A RadioPost station's program, as in the channel's tests: hello, its callsign, a call or
/// `listen`, and the session through a carrier.
struct Program<E: Engine<Event = SessionEvent>> {
    host: Host,
    callsign: &'static str,
    calls: Option<&'static str>,
    carrier: Carrier<E>,
    events: Vec<SessionEvent>,
    sent: Vec<(String, Vec<u8>)>,
    failed: Option<String>,
    said: Vec<String>,
}

impl<E: Engine<Event = SessionEvent>> Program<E> {
    fn new(engine: E, callsign: &'static str, calls: Option<&'static str>) -> Self {
        Program {
            host: Host::new(),
            callsign,
            calls,
            carrier: Carrier::new(engine),
            events: Vec::new(),
            sent: Vec::new(),
            failed: None,
            said: Vec::new(),
        }
    }

    fn on_line(&mut self, m: &Message) -> Vec<Message> {
        self.said
            .push(m.get("op").and_then(Json::as_str).unwrap_or("").to_owned());
        let event = Event::read(m).expect("an event");
        let out = match &event {
            Event::Hello { caps, .. } => {
                assert!(caps.shared, "an Aether with claims is shared");
                vec![self.host.callsigns(&[self.callsign])]
            }
            Event::Ok { id } if *id == 2 => match self.calls {
                Some(remote) => vec![self.host.connect(remote, None, None)],
                None => vec![self.host.listen(true)],
            },
            Event::Error { code, text, .. } => panic!("{}: error {code}: {text:?}", self.callsign),
            Event::Failed { code, .. } => {
                self.failed = Some(code.clone());
                vec![]
            }
            _ => {
                let (out, said) = self.carrier.on_event(&mut self.host, &event);
                self.events.extend(said);
                out
            }
        };
        for m in &out {
            if m.get("op") == Some(&"send".into()) {
                let data = base64::decode(m["data"].as_str().expect("data")).expect("base64");
                let reference = m.get("ref").and_then(Json::as_str).expect("a ref");
                self.sent.push((reference.into(), data));
            }
        }
        out
    }
}

fn run<E: Engine<Event = SessionEvent>>(
    mut program: Program<E>,
    adapter: std::net::SocketAddr,
    ready: Option<mpsc::Sender<()>>,
) -> Program<E> {
    let mut conn = Connection::open(adapter).expect("the adapter");
    conn.set_timeout(Some(Duration::from_secs(180)))
        .expect("a timeout");
    conn.send(&program.host.hello("RadioPost test station"))
        .expect("sent");
    let mut ready = ready;
    while !program.carrier.is_ended() && program.failed.is_none() {
        let m = conn
            .read_message()
            .expect("a line in time")
            .expect("the adapter stays");
        let listening = m.get("op") == Some(&"ok".into()) && m.get("id") == Some(&3.into());
        for out in program.on_line(&m) {
            conn.send(&out).expect("sent");
        }
        if listening && let Some(r) = ready.take() {
            r.send(()).expect("told");
        }
    }
    conn.close();
    program
}

#[test]
#[ignore = "needs AETHERD, an aetherd binary of 0.2.0-beta.146 or later"]
fn session_03_through_two_aether_daemons() {
    let binary = std::env::var("AETHERD").expect("AETHERD names an aetherd binary");
    let dir = std::env::temp_dir().join(format!("radiopost-aether-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    // The stations' own callsigns are not the hosts': each host claims its own (ADR-0106).
    let mut ana_modem = Daemon::start(&binary, &dir, "a", "N0CALL-9", "listen = \"127.0.0.1:0\"");
    let channel = ana_modem.logged("simulated channel (listening on ", ")");
    let mut facundo_modem = Daemon::start(
        &binary,
        &dir,
        "b",
        "KK4ODA-9",
        &format!("connect = \"{channel}\""),
    );
    let ana_adapter =
        radiopost_aether::serve::listen("127.0.0.1:0", &ana_modem.control()).expect("a port");
    let facundo_adapter =
        radiopost_aether::serve::listen("127.0.0.1:0", &facundo_modem.control()).expect("a port");

    let w = world();
    let file = vectors("session-03.json");
    let ana = Program::new(Answering(common::session_03_called(&w)), "N0CALL", None);
    let facundo = Program::new(
        Calling(common::session_03_caller(&w)),
        "KK4ODA",
        Some("N0CALL"),
    );
    let (tx, rx) = mpsc::channel();
    let ana_runs = thread::spawn(move || run(ana, ana_adapter, Some(tx)));
    rx.recv_timeout(Duration::from_secs(30))
        .expect("Ana's host listens");
    let facundo_runs = thread::spawn(move || run(facundo, facundo_adapter, None));
    let facundo = facundo_runs.join().expect("the caller's host");
    let ana = ana_runs.join().expect("the station's host");

    assert_eq!(facundo.failed, None, "the call: {:?}", facundo.said);
    let mut c = facundo.sent.iter();
    let mut a = ana.sent.iter();
    let ours: Vec<(String, Vec<u8>)> = transmissions(&file)
        .iter()
        .map(|(from, _)| {
            if from == "caller" { c.next() } else { a.next() }
                .expect("a transmission")
                .clone()
        })
        .collect();
    let theirs: Vec<(String, Vec<u8>)> = transmissions(&file)
        .into_iter()
        .enumerate()
        .map(|(i, t)| (format!("t{}", i + 1), t.1))
        .collect();
    assert_eq!(ours, theirs, "session-03's transmissions");
    assert_eq!(
        common::received(&facundo.events),
        vec![w.messages["ana-to-facundo"].content_id()]
    );
    assert_eq!(
        common::received(&ana.events),
        vec![w.messages["facundo-direct"].content_id()]
    );
    assert_eq!(
        common::delivered(&facundo.events),
        vec![w.messages["facundo-direct"].content_id()]
    );
    for said in ["incoming", "connected", "received", "disconnected"] {
        assert!(
            ana.said.iter().any(|s| s == said),
            "Ana's host heard {said}"
        );
    }
    for said in [
        "connected",
        "queued",
        "delivered",
        "received",
        "disconnected",
    ] {
        assert!(
            facundo.said.iter().any(|s| s == said),
            "Facundo's host heard {said}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Sends a request and returns its answer, past any events before it.
fn ask(conn: &mut Connection, request: &Message) -> Message {
    conn.send(request).expect("sent");
    loop {
        let m = conn
            .read_message()
            .expect("a line in time")
            .expect("the adapter stays");
        if m.get("id") == request.get("id") {
            return m;
        }
    }
}

fn code_of(m: &Message) -> &str {
    m.get("code")
        .or(m.get("op"))
        .and_then(Json::as_str)
        .unwrap_or("")
}

/// RP-4 §1.1 on a real station: each host's callsigns are a claim of its own, the first claim
/// keeps a callsign, and a host that leaves lets its callsigns go (Aether's ADR-0106).
#[test]
#[ignore = "needs AETHERD, an aetherd binary of 0.2.0-beta.146 or later"]
fn two_hosts_share_one_aether_station() {
    let binary = std::env::var("AETHERD").expect("AETHERD names an aetherd binary");
    let dir = std::env::temp_dir().join(format!("radiopost-shared-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    let mut modem = Daemon::start(&binary, &dir, "a", "N1CALL", "listen = \"127.0.0.1:0\"");
    let adapter = radiopost_aether::serve::listen("127.0.0.1:0", &modem.control()).expect("a port");
    let open = || {
        let c = Connection::open(adapter).expect("the adapter");
        c.set_timeout(Some(Duration::from_secs(20)))
            .expect("a timeout");
        c
    };
    let (mut radiopost, mut other) = (open(), open());
    let (mut h1, mut h2) = (Host::new(), Host::new());
    let hello = ask(&mut radiopost, &h1.hello("RadioPost gateway 0.1.0"));
    assert_eq!(hello["caps"]["shared"], true, "{hello:?}");
    assert_eq!(
        code_of(&ask(&mut radiopost, &h1.callsigns(&["N1CALL-10"]))),
        "ok"
    );
    assert_eq!(code_of(&ask(&mut radiopost, &h1.listen(true))), "ok");
    ask(&mut other, &h2.hello("Other mail 2.3"));
    assert_eq!(
        code_of(&ask(&mut other, &h2.callsigns(&["N1CALL-10"]))),
        "claimed"
    );
    assert_eq!(
        code_of(&ask(&mut other, &h2.callsigns(&["N1CALL-1"]))),
        "ok"
    );
    let theirs = ask(&mut other, &h2.status());
    assert_eq!(
        (&theirs["callsigns"], &theirs["listening"]),
        (&serde_json::json!(["N1CALL-1"]), &Json::Bool(false))
    );
    let ours = ask(&mut radiopost, &h1.status());
    assert_eq!(
        (&ours["callsigns"], &ours["listening"]),
        (&serde_json::json!(["N1CALL-10"]), &Json::Bool(true))
    );
    // a host that leaves takes its callsigns with it
    radiopost.close();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let m = ask(&mut other, &h2.callsigns(&["N1CALL-1", "N1CALL-10"]));
        if code_of(&m) == "ok" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "N1CALL-10 was never let go: {m:?}"
        );
        thread::sleep(Duration::from_millis(100));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
