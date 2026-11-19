// SPDX-License-Identifier: MIT OR Apache-2.0
//! The service's two stations on the air of the Internet: the TCP adapter (RP-4 §9.5) takes calls
//! from other TCP adapters, and one host on it answers as [`GATEWAY`] or as [`STATION`], whichever
//! was called, recording what crosses. One session at a time: a call while one is under way is
//! told `NO busy` by the adapter.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use radiopost::ErrorCode;
use radiopost::credential::{PostOfficeKey, Trust};
use radiopost::crypto;
use radiopost::message::Compression;
use radiopost::session::called::{Called, CalledConfig};
use radiopost::session::gateway::{Gateway, GatewayConfig, Upstreams};
use radiopost::session::{Caps, Event as SessionEvent, Outgoing};
use radiopost_postoffice::serve::Running;
use radiopost_tcp::{Adapter, Config, Security, tls};
use rpmi::host::{Carrier, Direction, Engine, Event, Host, Silence};
use rpmi::tcp::Connection;

use crate::report::{From, Record};
use crate::setup::{Station, Stations};
use crate::{API_PATH, GATEWAY, STATION, Settings, now, random};

/// RP-3 §8: nothing heard for this long ends the session.
const QUIET: Duration = Duration::from_secs(300);

/// How links are carried: TLS with the service's own certificate, made in `<dir>/tls` the first
/// time, or plain TCP for a test on one computer. Returns the fingerprint callers pin.
///
/// # Errors
///
/// The certificate cannot be made, read or written.
pub fn security(settings: &Settings) -> Result<(Security, Option<String>), String> {
    if settings.plain {
        return Ok((Security::Plain, None));
    }
    let dir = settings.dir.join("tls");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
    if !cert.exists() || !key.exists() {
        let (c, k, _) = tls::self_signed()?;
        std::fs::write(&cert, c).map_err(|e| e.to_string())?;
        std::fs::write(&key, k).map_err(|e| e.to_string())?;
    }
    let read = |p: &PathBuf| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let (server, fingerprint) = tls::server(&read(&cert)?, &read(&key)?)?;
    let client = tls::client(Vec::new())?;
    Ok((
        Security::Tls {
            server: Some(server),
            client,
        },
        Some(fingerprint),
    ))
}

/// Starts the TCP adapter, taking calls at `settings.air`, and the host that answers them.
/// Returns where calls come in.
///
/// # Errors
///
/// A port cannot be taken.
pub fn start(
    settings: &Settings,
    po: &Arc<Running>,
    stations: Stations,
    security: Security,
) -> Result<SocketAddr, String> {
    let adapter = Adapter::new(Config {
        security,
        peers: HashMap::new(),
        name: "RadioPost test service".into(),
    });
    let air = TcpListener::bind(&settings.air)
        .map_err(|e| format!("cannot take calls at {}: {e}", settings.air))?;
    let at = air.local_addr().map_err(|e| e.to_string())?;
    adapter.serve_calls(air);
    let hosts = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let rpmi = hosts.local_addr().map_err(|e| e.to_string())?;
    adapter.serve_hosts(hosts);
    let answering = Answering {
        po: po.clone(),
        domain: settings.domain.to_lowercase(),
        runs: crate::setup::runs_dir(&settings.dir),
        stations,
    };
    // calls are taken once the stations listen: until then the adapter would refuse them
    let (listening, ready) = std::sync::mpsc::channel();
    thread::spawn(move || answering.run(rpmi, &listening));
    ready
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| "the service's stations did not start listening")?;
    Ok(at)
}

/// The host on the adapter: what it answers with.
struct Answering {
    po: Arc<Running>,
    domain: String,
    runs: PathBuf,
    stations: Stations,
}

/// The modem's connection, with the host's requests numbered.
struct Modem {
    conn: Connection,
    host: Host,
}

impl Modem {
    fn open(at: SocketAddr) -> Result<Modem, String> {
        let conn = Connection::open(at).map_err(|e| e.to_string())?;
        conn.set_timeout(Some(Duration::from_millis(250)))
            .map_err(|e| e.to_string())?;
        let mut m = Modem {
            conn,
            host: Host::new(),
        };
        let hello = m.host.hello(concat!(
            "radiopost-test-service ",
            env!("CARGO_PKG_VERSION")
        ));
        m.ask(&hello)?;
        let calls = m.host.callsigns(&[GATEWAY, STATION]);
        m.ask(&calls)?;
        let listen = m.host.listen(true);
        m.ask(&listen)?;
        Ok(m)
    }

    fn next(&mut self) -> Result<Option<Event>, String> {
        match self.conn.read_message() {
            Ok(Some(m)) => Event::read(&m).map(Some),
            Ok(None) => Err("the adapter closed the connection".into()),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(e.to_string()),
        }
    }

    fn ask(&mut self, request: &rpmi::Message) -> Result<(), String> {
        self.conn.send(request).map_err(|e| e.to_string())?;
        let id = request.get("id").cloned();
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            match self.next()? {
                Some(Event::Ok { id: i }) if Some(&i) == id.as_ref() => return Ok(()),
                Some(Event::Hello { id: i, .. }) if i == id => return Ok(()),
                Some(Event::Error { id: i, code, .. }) if i == id => {
                    return Err(format!("the adapter refused: {code}"));
                }
                _ => {}
            }
        }
        Err("the adapter did not answer".into())
    }
}

impl Answering {
    /// Answers calls for as long as the program runs, opening the adapter again if it goes.
    fn run(mut self, at: SocketAddr, listening: &std::sync::mpsc::Sender<()>) {
        loop {
            let mut modem = match Modem::open(at) {
                Ok(m) => {
                    let _ = listening.send(());
                    m
                }
                Err(e) => {
                    eprintln!("radiopost-test-service: the adapter: {e}");
                    thread::sleep(Duration::from_secs(1));
                    continue;
                }
            };
            loop {
                match modem.next() {
                    Ok(Some(
                        e @ Event::Connected {
                            direction: Direction::In,
                            ..
                        },
                    )) => self.session(&mut modem, &e),
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("radiopost-test-service: the adapter: {e}");
                        break;
                    }
                }
            }
        }
    }

    /// One session, recorded, from the link coming up until it goes.
    fn session(&mut self, modem: &mut Modem, connected: &Event) {
        let Event::Connected { local, remote, .. } = connected else {
            return;
        };
        let record = Arc::new(Mutex::new(Record::new(local, remote, now())));
        let engine = if local.eq_ignore_ascii_case(STATION) {
            self.stations.station.fresh(&self.po);
            self.station_engine(&self.stations.station)
        } else {
            self.stations.gateway.fresh(&self.po);
            self.gateway_engine(&self.stations.gateway)
        };
        let mut carrier = Carrier::new(Recorded {
            engine,
            record: record.clone(),
        });
        let mut event = Some(connected.clone());
        // RP-3 §8: from the last bytes, stopped while the modem receives
        let mut silence = Silence::new();
        let result = loop {
            if let Some(e) = event.take() {
                let (requests, events) = carrier.on_event(&mut modem.host, &e);
                lock(&record).events(&events);
                if let Some(err) = requests.iter().find_map(|r| modem.conn.send(r).err()) {
                    carrier.engine_mut().link_lost();
                    break Err(format!("the adapter: {err}"));
                }
            }
            if carrier.is_ended() {
                break Ok(());
            }
            match modem.next() {
                Ok(Some(e)) => {
                    silence.heard(&e);
                    event = Some(e);
                }
                Ok(None) => {}
                Err(e) => {
                    carrier.engine_mut().link_lost();
                    break Err(format!("the adapter: {e}"));
                }
            }
            if silence.over(QUIET) {
                if let Some(link) = carrier.link() {
                    let bye = modem.host.disconnect(link);
                    let _ = modem.conn.send(&bye);
                }
                let events = carrier.engine_mut().link_lost();
                lock(&record).events(&events);
                break Err(format!("nothing heard for {} seconds", QUIET.as_secs()));
            }
        };
        let mut r = lock(&record);
        r.end(now(), result.err());
        if let Err(e) = crate::report::save(&self.runs, &r) {
            eprintln!("radiopost-test-service: cannot keep the report: {e}");
        }
    }

    /// RPTEST-1: the gateway's engine, online to the test post office and to no other.
    fn gateway_engine(&self, me: &Station) -> Answer {
        let config = GatewayConfig {
            callsign: GATEWAY.into(),
            time: now(),
            session_id: random(),
            challenge: random(),
            caps: Caps {
                compression: vec![0, 1],
                max_pdu: 4096,
                renew: true,
            },
            piece: 4096,
            credentials: Vec::new(),
            uploads: HashMap::new(),
            refused: Vec::new(),
        };
        let po = self.po.clone();
        let domain = self.domain.clone();
        let (credential, key) = (me.credential.clone(), me.key.clone());
        let upstreams: Upstreams = Box::new(move |d: &str| {
            if !d.eq_ignore_ascii_case(&domain) {
                return Err(ErrorCode::UnknownPostOffice);
            }
            let token = login(&po, &credential, &key).ok_or(ErrorCode::PostOfficeUnreachable)?;
            Ok(radiopost_gateway::upstream::served(
                po.clone(),
                API_PATH,
                token,
            ))
        });
        Answer::Gateway(Box::new(Gateway::new(
            config,
            upstreams,
            Box::new(|call| radiopost::thirdparty::may_hand(call, "US")),
        )))
    }

    /// RPTEST-2: a client's station, trusting the test post office, with a note for each caller.
    fn station_engine(&self, me: &Station) -> Answer {
        let mut trust = Trust::default();
        trust.keys.insert(
            self.domain.clone(),
            vec![PostOfficeKey {
                public: crate::setup::public_key(&self.po),
                not_before: 0,
                not_after: u64::MAX,
            }],
        );
        let config = CalledConfig {
            credential: me.credential.clone(),
            device: me.key.clone(),
            time: now(),
            session_id: random(),
            challenge: random(),
            trust,
            seen: Vec::new(),
            caps: Caps {
                compression: vec![0, 1],
                max_pdu: 4096,
                renew: false,
            },
            piece: 4096,
        };
        let domain = self.domain.clone();
        let mail_for = Box::new(move |address: &str| {
            let note = crate::setup::message(
                &domain,
                address,
                "From the test station",
                "RPTEST-2 answered your station-to-station call (RP-3 §10) and handed you this \
                 note.\n",
                (0, None),
            );
            vec![(1, Outgoing::new(note, Compression::Deflate))]
        });
        match Called::new(config, mail_for) {
            Ok(c) => Answer::Station(Box::new(c)),
            Err(code) => Answer::Broken(code),
        }
    }
}

fn lock(record: &Mutex<Record>) -> std::sync::MutexGuard<'_, Record> {
    record.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A gateway's login at the test post office (RP-5 §3), through its API in this process.
fn login(po: &Running, credential: &[u8], key: &ed25519_dalek::SigningKey) -> Option<String> {
    let path = |p: &str| format!("{API_PATH}{p}");
    let (_, _, text) = po.api("POST", &path("/auth/challenge"), None, b"");
    let nonce = serde_json::from_str::<serde_json::Value>(&text).ok()?["nonce"]
        .as_str()?
        .to_owned();
    let mut signed = radiopost::base64url::decode(&nonce)?;
    signed.extend_from_slice(po.office().config.base_url.as_bytes());
    let signature = crypto::sign(crypto::context::API_LOGIN, &signed, key);
    let body = serde_json::json!({
        "credential": radiopost::base64url::encode(credential),
        "nonce": nonce,
        "signature": radiopost::base64url::encode(&signature),
    });
    let (_, _, text) = po.api(
        "POST",
        &path("/auth/token"),
        None,
        body.to_string().as_bytes(),
    );
    serde_json::from_str::<serde_json::Value>(&text).ok()?["token"]
        .as_str()
        .map(str::to_owned)
}

/// Whichever engine answers.
enum Answer {
    Gateway(Box<Gateway>),
    Station(Box<Called>),
    /// The station's own credential would not do: nothing answers.
    Broken(ErrorCode),
}

/// An engine whose every transmission, each way, is kept in the session's record.
struct Recorded {
    engine: Answer,
    record: Arc<Mutex<Record>>,
}

impl Engine for Recorded {
    type Event = SessionEvent;
    fn opening(&mut self) -> Option<Vec<u8>> {
        None
    }
    fn receive(&mut self, bytes: &[u8]) -> Vec<SessionEvent> {
        lock(&self.record).transmission(From::Caller, bytes);
        match &mut self.engine {
            Answer::Gateway(g) => g.receive(bytes),
            Answer::Station(s) => s.receive(bytes),
            Answer::Broken(code) => {
                lock(&self.record).note(&format!(
                    "the service's station could not start: {}",
                    code.name()
                ));
                Vec::new()
            }
        }
    }
    fn transmit(&mut self) -> Option<Vec<u8>> {
        let bytes = match &mut self.engine {
            Answer::Gateway(g) => g.transmit(),
            Answer::Station(s) => s.transmit(),
            Answer::Broken(_) => None,
        }?;
        lock(&self.record).transmission(From::Service, &bytes);
        Some(bytes)
    }
    fn is_over(&self) -> bool {
        false
    }
    fn link_lost(&mut self) -> Vec<SessionEvent> {
        match &mut self.engine {
            Answer::Gateway(g) => g.link_closed(),
            Answer::Station(s) => s.link_closed(),
            Answer::Broken(_) => Vec::new(),
        }
    }
}
