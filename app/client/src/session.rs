// SPDX-License-Identifier: MIT OR Apache-2.0
//! One RP-3 session over a modem, through any RPMI adapter (RP-4): calling a gateway or another
//! station, or answering a station that calls (RP-3 §10).
//!
//! The engine is made when the link comes up, on the clock of that moment. What it reports is put
//! in the store before the requests it led to are written, so a message received is on disk before
//! the `RECEIPT` that takes it is sent (RP-3 §5.4).

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use radiopost::ContentId;
use radiopost::message::{Compression, Message};
use radiopost::session::called::{Called, CalledConfig};
use radiopost::session::client::{Client, ClientConfig};
use radiopost::session::{Caps, Event as SessionEvent, Outgoing, Proof, Proofs, WaitingEntry};
use rpmi::base64;
use rpmi::host::{Carrier, Direction, Engine, Event, Host};
use rpmi::tcp::Connection;
use rpmi::wire::Message as Line;
use serde_json::{Value, json};

use crate::store::{Error, Store};

/// How compressible messages go on the air (RP-2 §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Packing {
    /// Each message in whichever of its air forms is smaller.
    Smallest,
    /// No compression: the air forms of the session vectors.
    None,
}

/// Which waiting messages to ask a gateway for (RP-3 §5.2): the user's standing choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fetch {
    /// Everything waiting.
    All,
    /// What is at most this many bytes on the air.
    UpTo(u64),
    /// Nothing but what comes unasked.
    Nothing,
}

/// The session's settings. The clock and the random draws are the program's own unless given:
/// given, the session is reproducible, as the conformance tester's replay asks (its §2.4).
#[derive(Clone, Debug)]
pub struct Options {
    /// The modem: an RPMI adapter's address (RP-4 §2), or Aether HF's control API (`ws://…`, RP-4
    /// §9.1), for which this program runs the adapter itself.
    pub adapter: String,
    /// The program's name and version, for `hello`.
    pub app: String,
    /// A fixed clock, Unix seconds.
    pub clock: Option<u64>,
    /// A fixed `HELLO` nonce.
    pub nonce: Option<[u8; 8]>,
    /// A fixed session ID, when answering.
    pub session_id: Option<[u8; 8]>,
    /// A fixed challenge, when answering.
    pub challenge: Option<[u8; 16]>,
    /// How messages go on the air.
    pub packing: Packing,
    /// What to ask a gateway for.
    pub fetch: Fetch,
    /// How many bytes a gateway may send unasked (RP-3 §5.1, `auto`).
    pub auto: u64,
    /// A bandwidth to ask the modem for.
    pub bandwidth: Option<i64>,
    /// The longest the adapter may say nothing before the session is given up (RP-3 §8).
    pub quiet: Duration,
    /// Set, it stops a station listening for a call: how a program's window says "stop".
    pub stop: Option<Arc<AtomicBool>>,
}

impl Options {
    /// Settings for an adapter at `adapter`, with the program's own clock and draws.
    #[must_use]
    pub fn new(adapter: &str) -> Options {
        Options {
            adapter: adapter.to_owned(),
            app: format!("RadioPost client {}", env!("CARGO_PKG_VERSION")),
            clock: None,
            nonce: None,
            session_id: None,
            challenge: None,
            packing: Packing::Smallest,
            fetch: Fetch::All,
            auto: 1000,
            bandwidth: None,
            quiet: Duration::from_secs(600),
            stop: None,
        }
    }

    fn now(&self) -> u64 {
        self.clock.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
        })
    }
}

/// Who is called.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A gateway (RP-3 §5).
    Gateway,
    /// Another client's station (RP-3 §10), which proves who it is.
    Station,
}

/// What a session did.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// The station at the other end.
    pub remote: String,
    /// This side's transmissions, in order.
    pub transmissions: Vec<Vec<u8>>,
    /// Messages received.
    pub received: Vec<ContentId>,
    /// Messages the other station took.
    pub delivered: Vec<ContentId>,
    /// Why the call or the session failed, when it did.
    pub failed: Option<String>,
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("the system's random numbers");
    b
}

/// RP-3's capabilities this client offers: every compression, PDUs up to 4 096 bytes.
fn caps() -> Caps {
    Caps {
        compression: vec![0, 1, 2],
        max_pdu: 4096,
        renew: false,
    }
}

fn outgoing(message: Message, packing: Packing) -> Outgoing {
    match packing {
        Packing::None => Outgoing::new(message, Compression::None),
        Packing::Smallest => {
            let plain = message.air_form(Compression::None).len();
            let packed = message.air_form(Compression::Deflate).len();
            let c = if packed < plain {
                Compression::Deflate
            } else {
                Compression::None
            };
            Outgoing::new(message, c)
        }
    }
}

/// The caller's engine on a link: it speaks first.
struct Calling(Client);

impl Engine for Calling {
    type Event = SessionEvent;
    fn opening(&mut self) -> Option<Vec<u8>> {
        Some(self.0.hello())
    }
    fn receive(&mut self, bytes: &[u8]) -> Vec<SessionEvent> {
        self.0.receive(bytes)
    }
    fn transmit(&mut self) -> Option<Vec<u8>> {
        self.0.transmit()
    }
    fn is_over(&self) -> bool {
        self.0.is_over()
    }
    fn link_lost(&mut self) -> Vec<SessionEvent> {
        self.0.link_broken()
    }
}

/// The station called: it waits for `HELLO`, and the caller hangs up.
struct Answering(Called);

impl Engine for Answering {
    type Event = SessionEvent;
    fn opening(&mut self) -> Option<Vec<u8>> {
        None
    }
    fn receive(&mut self, bytes: &[u8]) -> Vec<SessionEvent> {
        self.0.receive(bytes)
    }
    fn transmit(&mut self) -> Option<Vec<u8>> {
        self.0.transmit()
    }
    fn is_over(&self) -> bool {
        false
    }
    fn link_lost(&mut self) -> Vec<SessionEvent> {
        self.0.link_closed()
    }
}

/// The address to reach a modem at: an RPMI adapter's `host:port` as given, or for Aether HF's
/// control API (`ws://…`) the Aether adapter (RP-4 §9.1) this program runs for it, started on first
/// use and kept, one for each address.
fn adapter_for(modem: &str) -> Result<String, Error> {
    static RUNNING: OnceLock<Mutex<HashMap<String, SocketAddr>>> = OnceLock::new();
    if !modem.starts_with("ws://") {
        return Ok(modem.to_owned());
    }
    let mut running = RUNNING
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(at) = running.get(modem) {
        return Ok(at.to_string());
    }
    let at = radiopost_aether::serve::listen("127.0.0.1:0", modem)
        .map_err(|e| format!("cannot start the Aether adapter: {e}"))?;
    running.insert(modem.to_owned(), at);
    Ok(at.to_string())
}

/// A connection to the adapter, with the host's requests numbered.
struct Modem {
    conn: Connection,
    host: Host,
    /// The modem as the user named it.
    name: String,
}

impl Modem {
    fn open(store: &Store, opts: &Options) -> Result<(Modem, String), Error> {
        let me = store.me()?;
        let adapter = adapter_for(&opts.adapter)?;
        let conn = Connection::open(adapter.as_str())
            .map_err(|e| format!("cannot reach the modem's adapter at {}: {e}", opts.adapter))?;
        conn.set_timeout(Some(opts.quiet))
            .map_err(|e| e.to_string())?;
        let mut modem = Modem {
            conn,
            host: Host::new(),
            name: opts.adapter.clone(),
        };
        let hello = modem.host.hello(&opts.app);
        modem.ask(&hello)?;
        let callsigns = modem.host.callsigns(&[me.operator.as_str()]);
        modem.ask(&callsigns)?;
        Ok((modem, me.operator))
    }

    fn send(&mut self, m: &Line) -> Result<(), Error> {
        self.conn
            .send(m)
            .map_err(|e| format!("the adapter's connection failed: {e}"))
    }

    fn next(&mut self) -> Result<Event, Error> {
        let m = self
            .conn
            .read_message()
            .map_err(|e| format!("the adapter said nothing in time, or failed: {e}"))?
            .ok_or("the adapter closed the connection")?;
        Event::read(&m).map_err(|why| format!("the adapter wrote what RP-4 does not: {why}"))
    }

    /// The adapter's next event, or `None` when a quarter of a second passes without one.
    fn poll(&mut self) -> Result<Option<Event>, Error> {
        match self.conn.read_message() {
            Ok(Some(m)) => Event::read(&m)
                .map(Some)
                .map_err(|why| format!("the adapter wrote what RP-4 does not: {why}")),
            Ok(None) => Err("the adapter closed the connection".into()),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(format!("the adapter's connection failed: {e}")),
        }
    }

    /// Sends a request and waits for its answer, past events before it.
    fn ask(&mut self, request: &Line) -> Result<Event, Error> {
        self.send(request)?;
        let id = request.get("id").cloned();
        loop {
            let e = self.next()?;
            match &e {
                Event::Error {
                    id: Some(i),
                    code,
                    text,
                } if Some(i) == id.as_ref() => {
                    let op = request.get("op").and_then(Value::as_str).unwrap_or("");
                    if code == "no-modem" {
                        return Err(format!(
                            "the modem at {} cannot be reached: is it running?",
                            self.name
                        ));
                    }
                    return Err(format!(
                        "the modem refused {op}: {code}{}",
                        text.as_deref()
                            .map(|t| format!(" ({t})"))
                            .unwrap_or_default()
                    ));
                }
                Event::Ok { id: i } if Some(i) == id.as_ref() => return Ok(e),
                Event::Hello { id: Some(i), .. } if Some(i) == id.as_ref() => return Ok(e),
                _ => {}
            }
        }
    }
}

/// Carries a session on a link that has come up, until it ends, putting what happens in the
/// store and telling the user.
fn carry<E: Engine<Event = SessionEvent>>(
    modem: &mut Modem,
    carrier: &mut Carrier<E>,
    connected: &Event,
    store: &mut Store,
    report: &mut Report,
    stats: &mut SessionStats,
    say: &mut dyn FnMut(&str),
) -> Result<(), Error> {
    let mut event = connected.clone();
    loop {
        let (requests, said) = carrier.on_event(&mut modem.host, &event);
        for e in said {
            apply(e, store, report, stats, say)?;
        }
        for r in &requests {
            if r.get("op").and_then(Value::as_str) == Some("send")
                && let Some(bytes) = r
                    .get("data")
                    .and_then(Value::as_str)
                    .and_then(base64::decode)
            {
                report.transmissions.push(bytes);
            }
            modem.send(r)?;
        }
        if carrier.is_ended() {
            return Ok(());
        }
        event = match modem.next() {
            Ok(e) => e,
            Err(why) => {
                // RP-3 §8: nothing heard for too long; drop the link and keep what was held
                if let Some(link) = carrier.link() {
                    let abort = modem.host.abort(Some(link));
                    let _ = modem.send(&abort);
                }
                for e in carrier.engine_mut().link_lost() {
                    apply(e, store, report, stats, say)?;
                }
                report.failed = Some(why.clone());
                store.save()?;
                return Err(why);
            }
        };
    }
}

/// What the session said that outlives it.
#[derive(Default)]
struct SessionStats {
    /// The session's clock.
    now: u64,
    welcomed: bool,
    top_serial: Option<u64>,
    /// The air forms this side offered, by content ID.
    airs: HashMap<ContentId, Vec<u8>>,
    /// What the other station took, and what came in, each with its air form, in order.
    sent: Vec<(ContentId, Vec<u8>)>,
    received: Vec<(ContentId, Vec<u8>)>,
}

/// Base64url without padding (RP-0 §4.2), as RP-5's bodies write bytes.
fn base64url(bytes: &[u8]) -> String {
    base64::encode(bytes)
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}

/// RP-5 §6.6: the `sessions` entry a station-to-station session is reported with, when it
/// carried anything and both stations proved themselves.
fn report_of(proofs: Option<Proofs>, role: &str, stats: &SessionStats) -> Option<Value> {
    let proofs = proofs?;
    if stats.sent.is_empty() && stats.received.is_empty() {
        return None;
    }
    let proof = |p: &Proof| {
        json!({"statement": base64url(&p.statement), "signature": base64url(&p.signature),
               "credential": base64url(&p.credential)})
    };
    let list = |l: &[(ContentId, Vec<u8>)]| {
        l.iter()
            .map(|(id, air)| json!({"id": id.text(), "air": base64url(air)}))
            .collect::<Vec<_>>()
    };
    Some(json!({
        "started": proofs.started,
        "role": role,
        "mine": proof(&proofs.mine),
        "theirs": proof(&proofs.theirs),
        "sent": list(&stats.sent),
        "received": list(&stats.received),
    }))
}

fn apply(
    e: SessionEvent,
    store: &mut Store,
    report: &mut Report,
    stats: &mut SessionStats,
    say: &mut dyn FnMut(&str),
) -> Result<(), Error> {
    match e {
        SessionEvent::Listed(entries) => {
            stats.welcomed = true;
            for WaitingEntry {
                serial,
                id,
                from,
                subject,
                size,
                ..
            } in entries
            {
                say(&format!(
                    "waiting: #{serial} {} from {from}, {size} bytes: {subject}",
                    &id.text()[..6]
                ));
            }
        }
        SessionEvent::Received {
            serial,
            message,
            air,
        } => {
            store.received(&message, &report.remote, stats.now)?;
            let id = message.content_id();
            stats.received.push((id, air));
            store.drop_partial(&id);
            if let Some(s) = serial {
                stats.top_serial = Some(stats.top_serial.map_or(s, |t| t.max(s)));
            }
            say(&format!(
                "received {} from {}",
                &id.text()[..6],
                message.from().unwrap_or("?")
            ));
            report.received.push(id);
        }
        SessionEvent::Failed { id, code } => {
            say(&format!(
                "{} arrived damaged and was dropped: {code}",
                &id.text()[..6]
            ));
        }
        SessionEvent::Delivered(id) => {
            store.delivered(&id);
            if let Some(air) = stats.airs.get(&id) {
                stats.sent.push((id, air.clone()));
            }
            store.save()?;
            say(&format!("delivered {}", &id.text()[..6]));
            report.delivered.push(id);
        }
        SessionEvent::NotTaken { id, code } => {
            say(&format!(
                "{} not taken: {code}; it stays in the outbox",
                &id.text()[..6]
            ));
        }
        SessionEvent::Held(partial) => {
            say(&format!(
                "{} held in part ({} of {} bytes), to finish next time",
                &partial.id.text()[..6],
                partial.held.len(),
                partial.total
            ));
            store.hold(&partial)?;
        }
        SessionEvent::StationProved(body) => {
            say(&format!(
                "the other station is {} ({}), its credential and statement checked",
                body.operator, body.address
            ));
        }
        SessionEvent::Notice(text) => say(&format!("notice: {text}")),
        SessionEvent::Error { code, text } => {
            let why = format!(
                "the other station sent ERROR {code}{}",
                text.map(|t| format!(": {t}")).unwrap_or_default()
            );
            say(&why);
            report.failed = Some(why);
        }
        SessionEvent::Refused(code) => {
            let why = format!("refused the other station: {code}");
            say(&why);
            report.failed = Some(why);
        }
        SessionEvent::NewCredential(bytes) => {
            store.set_credential(&bytes)?;
            say("a renewed credential, kept for the next session");
        }
        SessionEvent::End => {}
    }
    Ok(())
}

/// Calls `remote` and carries a session: a gateway's (RP-3 §5), or a station's (RP-3 §10), which
/// must prove who it is.
///
/// # Errors
///
/// No identity yet, the adapter cannot be reached or refuses, or the session breaks.
pub fn call(
    store: &mut Store,
    remote: &str,
    kind: Kind,
    opts: &Options,
    say: &mut dyn FnMut(&str),
) -> Result<Report, Error> {
    let (mut modem, _) = Modem::open(store, opts)?;
    let connect = modem.host.connect(remote, None, opts.bandwidth);
    let id = connect.get("id").cloned();
    modem.ask(&connect)?;
    say(&format!("calling {remote}"));
    let mut report = Report {
        remote: remote.to_owned(),
        ..Report::default()
    };
    let connected = loop {
        let e = modem.next()?;
        match &e {
            Event::Connected { id: i, .. } if *i == id => break e,
            Event::Failed { id: i, code, text } if *i == id => {
                let why = format!(
                    "the call failed: {code}{}",
                    text.as_deref()
                        .map(|t| format!(" ({t})"))
                        .unwrap_or_default()
                );
                report.failed = Some(why.clone());
                say(&why);
                return Ok(report);
            }
            _ => {}
        }
    };
    say(&format!("connected to {remote}"));
    let now = opts.now();
    let (cursor, cached) = match kind {
        Kind::Gateway => store.gateway(remote),
        Kind::Station => (0, false),
    };
    let mut outbox = Vec::new();
    let mut stats = SessionStats {
        now,
        ..SessionStats::default()
    };
    for w in &store.outbox {
        let o = outgoing(store.message(&w.id)?, opts.packing);
        stats.airs.insert(w.id, o.air.clone());
        outbox.push(o);
    }
    let fetch = opts.fetch;
    let cfg = ClientConfig {
        credential: store.credential()?,
        device: store.device()?,
        called: remote.to_owned(),
        time: now,
        nonce: opts.nonce.unwrap_or_else(random),
        outbox,
        cursor,
        auto: opts.auto,
        partials: store.partials(),
        caps: caps(),
        credential_cached: cached,
        piece: 4096,
        station: (kind == Kind::Station).then(|| store.trust()).transpose()?,
    };
    let chooser = Box::new(move |entries: &[WaitingEntry]| {
        entries
            .iter()
            .filter(|e| match fetch {
                Fetch::All => true,
                Fetch::UpTo(n) => e.size <= n,
                Fetch::Nothing => false,
            })
            .map(|e| e.serial)
            .collect()
    });
    let client = Client::new(cfg, chooser).map_err(|e| format!("cannot start a session: {e}"))?;
    let mut carrier = Carrier::new(Calling(client));
    let result = carry(
        &mut modem,
        &mut carrier,
        &connected,
        store,
        &mut report,
        &mut stats,
        say,
    );
    if kind == Kind::Station
        && let Some(r) = report_of(carrier.engine().0.proofs(), "caller", &stats)
    {
        store.reports.push(r);
        say("this session is kept, to report to the post office when there is Internet");
    }
    store.save()?;
    result?;
    if kind == Kind::Gateway && stats.welcomed {
        let top = stats.top_serial.map_or(cursor, |t| t.max(cursor));
        store.set_gateway(remote, top, true);
    }
    store.save()?;
    say(&format!("the session with {remote} is over"));
    modem.conn.close();
    Ok(report)
}

/// Listens for a station's call and carries its session (RP-3 §10), then returns.
///
/// # Errors
///
/// No identity yet, the adapter cannot be reached or refuses, or the session breaks.
pub fn answer(
    store: &mut Store,
    opts: &Options,
    say: &mut dyn FnMut(&str),
) -> Result<Report, Error> {
    let (mut modem, me) = Modem::open(store, opts)?;
    let listen = modem.host.listen(true);
    modem.ask(&listen)?;
    say(&format!("listening as {me}"));
    let waiting = Duration::from_millis(250);
    modem
        .conn
        .set_timeout(Some(waiting))
        .map_err(|e| e.to_string())?;
    let (connected, remote) = loop {
        if opts
            .stop
            .as_ref()
            .is_some_and(|s| s.load(Ordering::Relaxed))
        {
            say("stopped listening");
            modem.conn.close();
            return Ok(Report {
                failed: Some("stopped before a call came".into()),
                ..Report::default()
            });
        }
        let Some(e) = modem.poll()? else {
            continue;
        };
        if let Event::Connected {
            direction: Direction::In,
            remote,
            ..
        } = &e
        {
            let remote = remote.clone();
            break (e, remote);
        }
    };
    modem
        .conn
        .set_timeout(Some(opts.quiet))
        .map_err(|e| e.to_string())?;
    say(&format!("{remote} called"));
    let now = opts.now();
    let mut mail = Vec::new();
    let mut stats = SessionStats {
        now,
        ..SessionStats::default()
    };
    for w in &store.outbox {
        let m = store.message(&w.id)?;
        let to: Vec<String> = m.recipients().into_iter().map(str::to_owned).collect();
        let o = outgoing(m, opts.packing);
        stats.airs.insert(w.id, o.air.clone());
        mail.push((to, w.serial, o));
    }
    let mail_for = Box::new(move |address: &str| {
        mail.iter()
            .filter(|(to, _, _)| to.iter().any(|t| t == address))
            .map(|(_, serial, o)| (*serial, o.clone()))
            .collect()
    });
    let cfg = CalledConfig {
        credential: store.credential()?,
        device: store.device()?,
        time: now,
        session_id: opts.session_id.unwrap_or_else(random),
        challenge: opts.challenge.unwrap_or_else(random),
        trust: store.trust()?,
        seen: store.replay_cache(now),
        caps: caps(),
        piece: 4096,
    };
    let called = Called::new(cfg, mail_for).map_err(|e| format!("cannot answer: {e}"))?;
    let mut report = Report {
        remote: remote.clone(),
        ..Report::default()
    };
    let mut carrier = Carrier::new(Answering(called));
    let result = carry(
        &mut modem,
        &mut carrier,
        &connected,
        store,
        &mut report,
        &mut stats,
        say,
    );
    if let Some((address, nonce)) = carrier.engine().0.seen() {
        store.saw(address, *nonce, now);
    }
    if let Some(r) = report_of(carrier.engine().0.proofs(), "called", &stats) {
        store.reports.push(r);
        say("this session is kept, to report to the post office when there is Internet");
    }
    store.save()?;
    result?;
    say(&format!("the session with {remote} is over"));
    modem.conn.close();
    Ok(report)
}
