// SPDX-License-Identifier: MIT OR Apache-2.0
//! One RP-3 session over a modem, through any RPMI adapter (RP-4): calling a gateway or another
//! station, or answering a station that calls (RP-3 §10).
//!
//! The engine is made when the link comes up, on the clock of that moment. What it reports is put
//! in the store before the requests it led to are written, so a message received is on disk before
//! the `RECEIPT` that takes it is sent (RP-3 §5.4).

use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use radiopost::message::{Compression, Message};
use radiopost::session::called::{Called, CalledConfig};
use radiopost::session::client::{Chooser, Client, ClientConfig};
use radiopost::session::{
    Caps, Event as SessionEvent, Outgoing, Progress, Proof, Proofs, WaitingEntry,
};
use radiopost::{ContentId, ErrorCode};
use rpmi::base64;
use rpmi::host::{Carrier, Direction, Engine, Event, Host};
use rpmi::tcp::Connection;
use rpmi::wire::Message as Line;
use serde_json::{Value, json};

use crate::store::{Error, Store};

/// RP-3 §8: nothing heard for this long ends the session.
pub const QUIET: Duration = Duration::from_secs(300);

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
    /// How long before the credential runs out a gateway call asks for a renewed one
    /// ([`crate::renewal::WITHIN`]).
    pub renew_within: u64,
    /// Kept up to date while the session runs: how far its messages have crossed, for a window
    /// to show.
    pub live: Option<Arc<Mutex<Live>>>,
}

/// A session as it runs: how far its messages have crossed both ways, and what the modem says.
#[derive(Clone, Debug, Default)]
pub struct Live {
    /// What the engine has carried, in bytes of the messages' air forms (RP-2 §4).
    pub progress: Progress,
    /// The bytes handed to the modem that it has not yet passed on: what it last reported
    /// (RP-4 `queued`), with what was sent since, and less what it said the other station holds
    /// (`delivered`). Sent messages count as crossed only past this.
    pub queued: u64,
    /// The data rate the modem says it uses (RP-4 `quality`).
    pub rate_bps: Option<f64>,
    /// Whether the modem says it is receiving now.
    pub receiving: Option<bool>,
    /// Each send not yet acknowledged, by its `ref`, with its size.
    sends: VecDeque<(Value, u64)>,
}

impl Live {
    /// A send of `bytes` handed to the modem under `reference`.
    fn sent(&mut self, reference: Option<&Value>, bytes: u64) {
        self.queued += bytes;
        if let Some(r) = reference {
            self.sends.push_back((r.clone(), bytes));
        }
    }

    /// What the modem said of the link.
    fn heard(&mut self, event: &Event) {
        match event {
            Event::Queued { bytes, .. } => self.queued = *bytes,
            Event::Delivered { reference, .. } => {
                if let Some(at) = self.sends.iter().position(|(r, _)| r == reference) {
                    self.sends.drain(..=at);
                    self.queued = self.queued.min(self.sends.iter().map(|(_, n)| n).sum());
                }
            }
            Event::Quality {
                rate_bps,
                receiving,
                ..
            } => {
                if rate_bps.is_some() {
                    self.rate_bps = *rate_bps;
                }
                if receiving.is_some() {
                    self.receiving = *receiving;
                }
            }
            _ => {}
        }
    }

    /// What a window shows: each way the messages (not counting those refused), how many are
    /// whole, their bytes and how many of them have crossed; the modem's rate; whether it is
    /// receiving.
    #[must_use]
    pub fn summary(&self) -> Value {
        let way = |list: &[radiopost::session::Crossing], (bytes, crossed): (u64, u64)| {
            let going: Vec<_> = list.iter().filter(|c| !c.refused).collect();
            json!({
                "messages": going.len(), "whole": going.iter().filter(|c| c.whole).count(),
                "bytes": bytes, "crossed": crossed,
            })
        };
        json!({
            "sending": way(&self.progress.sending, self.progress.sent_bytes(self.queued)),
            "receiving": way(&self.progress.receiving, self.progress.received_bytes()),
            "rate_bps": self.rate_bps,
            "receiving_now": self.receiving,
        })
    }
}

/// The program's name and version, as its `hello` gives them (RP-4 §3). Its calls claim callsigns
/// under it, and its connection between calls ([`crate::presence`]) says it too, so Aether shows
/// the two as one program.
#[must_use]
pub fn app_name() -> String {
    format!("RadioPost client {}", env!("CARGO_PKG_VERSION"))
}

impl Options {
    /// Settings for an adapter at `adapter`, with the program's own clock and draws.
    #[must_use]
    pub fn new(adapter: &str) -> Options {
        Options {
            adapter: adapter.to_owned(),
            app: app_name(),
            clock: None,
            nonce: None,
            session_id: None,
            challenge: None,
            packing: Packing::Smallest,
            fetch: Fetch::All,
            auto: 1000,
            bandwidth: None,
            quiet: QUIET,
            stop: None,
            renew_within: crate::renewal::WITHIN,
            live: None,
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
    /// Messages the other station took: sent, or queued at a cut-off gateway (`queued`).
    pub delivered: Vec<ContentId>,
    /// Of those, the messages a cut-off gateway took into its queue (RP-7 §3.6): not sent yet,
    /// and offered again.
    pub queued: Vec<ContentId>,
    /// Why the call or the session failed, when it did.
    pub failed: Option<String>,
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("the system's random numbers");
    b
}

/// RP-3's capabilities this client offers: every compression, PDUs up to 4 096 bytes; `renew` asks
/// a gateway for a renewed credential (RP-3 §5.8).
fn caps(renew: bool) -> Caps {
    Caps {
        compression: vec![0, 1, 2],
        max_pdu: 4096,
        renew,
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

/// An engine's count of what has crossed.
trait Counted {
    fn progress(&self) -> &Progress;
}

impl Counted for Calling {
    fn progress(&self) -> &Progress {
        self.0.progress()
    }
}

impl Counted for Answering {
    fn progress(&self) -> &Progress {
        self.0.progress()
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
pub(crate) fn adapter_for(modem: &str) -> Result<String, Error> {
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
    let note: radiopost_aether::serve::Note = std::sync::Arc::new(|line: &str| {
        radiopost::logfile::line(&format!("Aether adapter: {line}"));
    });
    let at = radiopost_aether::serve::listen_noting(
        "127.0.0.1:0",
        modem,
        note,
        radiopost::logfile::path(),
    )
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
        if let Some(test) = store.conformance()
            && opts.adapter != test
        {
            return Err(format!(
                "in conformance mode this client reaches only the test adapter at {test}; \
                 `radiopost conformance off` ends it"
            ));
        }
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
#[allow(
    clippy::too_many_arguments,
    reason = "the session's parts, each its own"
)]
fn carry<E: Engine<Event = SessionEvent> + Counted>(
    modem: &mut Modem,
    carrier: &mut Carrier<E>,
    connected: &Event,
    store: &mut Store,
    report: &mut Report,
    stats: &mut SessionStats,
    live: Option<&Mutex<Live>>,
    say: &mut dyn FnMut(&str),
) -> Result<(), Error> {
    let mut event = connected.clone();
    loop {
        let (requests, said) = carrier.on_event(&mut modem.host, &event);
        for e in said {
            apply(e, store, report, stats, say)?;
        }
        let mut handed = Vec::new();
        for r in &requests {
            if r.get("op").and_then(Value::as_str) == Some("send")
                && let Some(bytes) = r
                    .get("data")
                    .and_then(Value::as_str)
                    .and_then(base64::decode)
            {
                handed.push((r.get("ref").cloned(), bytes.len() as u64));
                report.transmissions.push(bytes);
            }
            modem.send(r)?;
        }
        if let Some(live) = live {
            let mut l = live.lock().unwrap_or_else(PoisonError::into_inner);
            l.heard(&event);
            for (reference, n) in handed {
                l.sent(reference.as_ref(), n);
            }
            l.progress = carrier.engine().progress().clone();
        }
        if carrier.is_ended() {
            return Ok(());
        }
        event = match modem.next() {
            Ok(e) => e,
            Err(why) => {
                // RP-3 §8: nothing heard for too long; drop the link and keep what was held
                if let Some(link) = carrier.link() {
                    let hang_up = modem.host.disconnect(link);
                    let _ = modem.send(&hang_up);
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
    /// The station called is a gateway (not a station, RP-3 §10).
    gateway: bool,
    /// What its `WELCOME` said of the post office (RP-3 §5.2 `online`).
    online: Option<bool>,
    top_serial: Option<u64>,
    /// The waiting list the gateway gave, and the serials that came.
    listed: Vec<WaitingEntry>,
    came: Vec<u64>,
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
        SessionEvent::Welcome { online } => welcomed(online, stats, say),
        SessionEvent::Listed(entries) => {
            stats.welcomed = true;
            stats.listed.clone_from(&entries);
            for entry in &entries {
                say_waiting(entry, say);
            }
        }
        SessionEvent::Received {
            serial,
            message,
            air,
        } => {
            store.received(&message, &report.remote, stats.now, serial)?;
            let id = message.content_id();
            stats.received.push((id, air));
            store.drop_partial(&id);
            if let Some(s) = serial {
                stats.top_serial = Some(stats.top_serial.map_or(s, |t| t.max(s)));
                stats.came.push(s);
            }
            say_received(&message, say);
            report.received.push(id);
        }
        SessionEvent::Failed { id, code } => {
            tell(
                say,
                &format!(
                    "{} arrived damaged and was dropped: {code}",
                    &id.text()[..6]
                ),
            );
        }
        SessionEvent::Delivered(id) => handed(id, store, report, stats, say)?,
        SessionEvent::NotTaken {
            id,
            code: ErrorCode::AlreadyHave,
        } if stats.gateway => already_had(id, store, report, stats, say)?,
        SessionEvent::NotTaken { id, code } => {
            tell(
                say,
                &format!(
                    "{} not taken: {code}; it stays in the outbox",
                    &id.text()[..6]
                ),
            );
        }
        SessionEvent::Held(partial) => {
            tell(
                say,
                &format!(
                    "{} held in part ({} of {} bytes), to finish next time",
                    &partial.id.text()[..6],
                    partial.held.len(),
                    partial.total
                ),
            );
            store.hold(&partial)?;
        }
        SessionEvent::StationProved(body) => {
            let checked = "its credential and statement checked";
            tell_apart(
                say,
                &format!(
                    "the other station is {} ({}), {checked}",
                    body.operator, body.address
                ),
                &format!("the other station is {}, {checked}", body.operator),
            );
        }
        SessionEvent::Notice(text) => tell(say, &format!("notice: {text}")),
        SessionEvent::Error { code, text } => {
            let why = format!(
                "the other station sent ERROR {code}{}",
                text.map(|t| format!(": {t}")).unwrap_or_default()
            );
            tell(say, &why);
            report.failed = Some(why);
        }
        SessionEvent::Refused(code) => {
            let why = format!("refused the other station: {code}");
            tell(say, &why);
            report.failed = Some(why);
        }
        SessionEvent::NewCredential(bytes) => {
            store.set_credential(&bytes)?;
            tell(say, "a renewed credential, kept for the next session");
        }
        SessionEvent::End => {}
    }
    Ok(())
}

/// The station called is ready: whether it is a gateway that reaches the post office now.
fn welcomed(online: bool, stats: &mut SessionStats, say: &mut dyn FnMut(&str)) {
    stats.online = Some(online);
    if stats.gateway && !online {
        tell(
            say,
            "the gateway cannot reach your post office now: what it takes waits in its queue, \
             and is offered again until a gateway that can confirms it",
        );
    }
}

/// What a gateway now holds of this client, and what it listed (RP-3 §6.2, RP-7 §3.6).
fn remember_gateway(store: &mut Store, remote: &str, cursor: u64, stats: &SessionStats) {
    let top = stats.top_serial.map_or(cursor, |t| t.max(cursor));
    store.set_gateway(remote, top, true);
    if let Some(online) = stats.online {
        store.set_gateway_online(remote, online);
    }
    store.listed(remote, stats.now, &stats.listed, &stats.came);
}

/// A message the other station took: sent, or queued when a cut-off gateway took it (RP-7
/// §3.6).
fn handed(
    id: ContentId,
    store: &mut Store,
    report: &mut Report,
    stats: &mut SessionStats,
    say: &mut dyn FnMut(&str),
) -> Result<(), Error> {
    if let Some(air) = stats.airs.get(&id) {
        stats.sent.push((id, air.clone()));
    }
    if stats.gateway && stats.online == Some(false) {
        store.queued_at(&id, &report.remote, stats.now);
        store.save()?;
        tell(
            say,
            &format!(
                "{} queued at {}: it stays in the outbox until a gateway that reaches the post \
                 office confirms it",
                &id.text()[..6],
                report.remote
            ),
        );
        report.queued.push(id);
    } else {
        store.delivered(&id);
        store.save()?;
        tell(say, &format!("delivered {}", &id.text()[..6]));
    }
    report.delivered.push(id);
    Ok(())
}

/// A gateway declined a message `already-have` (RP-7 §3.6): an online one's post office holds
/// it, so it is sent; a cut-off one holds it in its queue, or passed it on.
fn already_had(
    id: ContentId,
    store: &mut Store,
    report: &mut Report,
    stats: &SessionStats,
    say: &mut dyn FnMut(&str),
) -> Result<(), Error> {
    if stats.online == Some(true) {
        store.delivered(&id);
        store.save()?;
        tell(
            say,
            &format!("{} is already at the post office: sent", &id.text()[..6]),
        );
        report.delivered.push(id);
    } else {
        if store.queued_for(&id).is_none() {
            store.queued_at(&id, &report.remote, stats.now);
            store.save()?;
        }
        tell(
            say,
            &format!(
                "{} is already in {}'s queue: it stays in the outbox",
                &id.text()[..6],
                report.remote
            ),
        );
    }
    Ok(())
}

/// What a call to a gateway offers (RP-7 §3.6): the outbox, less what a gateway cut off when
/// last called queued less than 30 days ago; what may have been dropped unsent is said, and
/// offered all the same. A station is offered the whole outbox.
fn offered(
    store: &Store,
    remote: &str,
    kind: Kind,
    now: u64,
    say: &mut dyn FnMut(&str),
) -> Vec<crate::store::Waiting> {
    if kind == Kind::Station {
        return store.outbox.clone();
    }
    for q in store.doubtful(now) {
        tell(
            say,
            &format!(
                "{} has waited in {}'s queue for {} days with no word that it reached the post \
                 office: it may have been dropped unsent; offered again",
                &q.id.text()[..6],
                q.gateway,
                now.saturating_sub(q.at) / 86_400
            ),
        );
    }
    store.outbox_for(remote, now)
}

/// A message received: its sender in the window, its ID alone in the log file.
fn say_received(message: &Message, say: &mut dyn FnMut(&str)) {
    let id = &message.content_id().text()[..6];
    let from = message.from().unwrap_or("?");
    tell_apart(
        say,
        &format!("received {id} from {from}"),
        &format!("received {id}"),
    );
}

/// A message waiting at the post office: its sender and subject in the window, its serial, ID
/// and size in the log file.
fn say_waiting(entry: &WaitingEntry, say: &mut dyn FnMut(&str)) {
    let WaitingEntry {
        serial,
        id,
        from,
        subject,
        size,
        ..
    } = entry;
    let id = &id.text()[..6];
    tell_apart(
        say,
        &format!("waiting: #{serial} {id} from {from}, {size} bytes: {subject}"),
        &format!("waiting: #{serial} {id}, {size} bytes"),
    );
}

/// Says `line` on `say`, and writes it to the program's log file ([`radiopost::logfile`]).
fn tell(say: &mut dyn FnMut(&str), line: &str) {
    radiopost::logfile::line(line);
    say(line);
}

/// Says `line` on `say`, and writes `filed` to the log file in its place. The window may name a
/// correspondent or a subject; the file never does, because it goes with the files a station
/// shares when something goes wrong (Aether's `share.include`), and a log says what happened,
/// never what was said.
fn tell_apart(say: &mut dyn FnMut(&str), line: &str, filed: &str) {
    radiopost::logfile::line(filed);
    say(line);
}

/// Whether this call asks for a renewed credential in `HELLO` (RP-3 §5.8): a gateway's, in the
/// credential's last month; a gateway passes the request to the post office, and a station has
/// none to ask.
fn asks_renewal(
    store: &Store,
    kind: Kind,
    now: u64,
    opts: &Options,
    say: &mut dyn FnMut(&str),
) -> Result<bool, Error> {
    let expires = store.me()?.expires;
    let renew = kind == Kind::Gateway && crate::renewal::due(expires, now, opts.renew_within);
    if renew {
        tell(
            say,
            &format!(
                "asking for a renewed credential: this one runs out in {} days",
                crate::renewal::days_left(expires, now)
            ),
        );
    }
    Ok(renew)
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
    tell(say, &format!("calling {remote}"));
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
                tell(say, &why);
                return Ok(report);
            }
            _ => {}
        }
    };
    tell(say, &format!("connected to {remote}"));
    let now = opts.now();
    let (cursor, cached) = match kind {
        Kind::Gateway => store.gateway(remote),
        Kind::Station => (0, false),
    };
    let mut outbox = Vec::new();
    let mut stats = SessionStats {
        now,
        gateway: kind == Kind::Gateway,
        ..SessionStats::default()
    };
    let offered = offered(store, remote, kind, now, say);
    for w in &offered {
        let o = outgoing(store.message(&w.id)?, opts.packing);
        stats.airs.insert(w.id, o.air.clone());
        outbox.push(o);
    }
    let fetch = opts.fetch;
    let renew = asks_renewal(store, kind, now, opts, say)?;
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
        caps: caps(renew),
        credential_cached: cached,
        piece: 4096,
        station: (kind == Kind::Station).then(|| store.trust()).transpose()?,
    };
    let mut client = Client::new(cfg, chooser(fetch, store.fetch_next.clone()))
        .map_err(|e| format!("cannot start a session: {e}"))?;
    if kind == Kind::Gateway {
        ask_for_files(&mut client, store, say);
    }
    let mut carrier = Carrier::new(Calling(client));
    let result = carry(
        &mut modem,
        &mut carrier,
        &connected,
        store,
        &mut report,
        &mut stats,
        opts.live.as_deref(),
        say,
    );
    if carrier.engine().0.files_answered() {
        store.files.clear();
    }
    if kind == Kind::Station
        && let Some(r) = report_of(carrier.engine().0.proofs(), "caller", &stats)
    {
        store.reports.push(r);
        tell(
            say,
            "this session is kept, to report to the post office when there is Internet",
        );
    }
    store.save()?;
    result?;
    if kind == Kind::Gateway && stats.welcomed {
        remember_gateway(store, remote, cursor, &stats);
    }
    store.save()?;
    tell(say, &format!("the session with {remote} is over"));
    modem.conn.close();
    Ok(report)
}

/// What the user fetches of what waits: all, what is no larger than a size, or nothing, and
/// whatever they marked to fetch from the last call's waiting list.
fn chooser(fetch: Fetch, marked: Vec<u64>) -> Chooser {
    Box::new(move |entries: &[WaitingEntry]| {
        entries
            .iter()
            .filter(|e| {
                marked.contains(&e.serial)
                    || match fetch {
                        Fetch::All => true,
                        Fetch::UpTo(n) => e.size <= n,
                        Fetch::Nothing => false,
                    }
            })
            .map(|e| e.serial)
            .collect()
    })
}

/// Hands the engine the held attachments the user asked for (RP-3 §5.6), on a call to a gateway.
fn ask_for_files(client: &mut Client, store: &Store, say: &mut dyn FnMut(&str)) {
    let files = store.files.clone();
    if !files.is_empty() {
        tell(
            say,
            &format!(
                "asking for {} held attachment{}",
                files.len(),
                if files.len() == 1 { "" } else { "s" }
            ),
        );
        client.ask_for_files(files);
    }
}

/// Waits for a station to call: its link's `connected` and its callsign, or `None` when the
/// program said stop first.
fn wait_for_call(modem: &mut Modem, opts: &Options) -> Result<Option<(Event, String)>, Error> {
    modem
        .conn
        .set_timeout(Some(Duration::from_millis(250)))
        .map_err(|e| e.to_string())?;
    let called = loop {
        if opts
            .stop
            .as_ref()
            .is_some_and(|s| s.load(Ordering::Relaxed))
        {
            return Ok(None);
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
    Ok(Some(called))
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
    tell(say, &format!("listening as {me}"));
    let Some((connected, remote)) = wait_for_call(&mut modem, opts)? else {
        tell(say, "stopped listening");
        modem.conn.close();
        return Ok(Report {
            failed: Some("stopped before a call came".into()),
            ..Report::default()
        });
    };
    tell(say, &format!("{remote} called"));
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
        caps: caps(false),
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
        opts.live.as_deref(),
        say,
    );
    if let Some((address, nonce)) = carrier.engine().0.seen() {
        store.saw(address, *nonce, now);
    }
    if let Some(r) = report_of(carrier.engine().0.proofs(), "called", &stats) {
        store.reports.push(r);
        tell(
            say,
            "this session is kept, to report to the post office when there is Internet",
        );
    }
    store.save()?;
    result?;
    tell(say, &format!("the session with {remote} is over"));
    modem.conn.close();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use rpmi::host::Event;
    use serde_json::json;

    use super::Live;

    /// What the modem still holds: the sends handed to it, then what it reports, then what it
    /// says the other station holds.
    #[test]
    fn what_the_modem_still_holds() {
        let mut l = Live::default();
        l.sent(Some(&json!("t1")), 1000);
        l.sent(Some(&json!("t2")), 500);
        assert_eq!(l.queued, 1500, "handed and not yet reported");
        l.heard(&Event::Queued {
            link: "a1".into(),
            bytes: 800,
        });
        assert_eq!(l.queued, 800, "as the modem says");
        l.heard(&Event::Delivered {
            link: "a1".into(),
            reference: json!("t1"),
        });
        assert_eq!(l.queued, 500, "only the second send is left");
        l.sent(Some(&json!("t3")), 200);
        l.heard(&Event::Delivered {
            link: "a1".into(),
            reference: json!("t3"),
        });
        assert_eq!(l.queued, 0, "everything up to t3 is held");
        l.heard(&Event::Quality {
            link: None,
            snr_db: Some(3.0),
            rate_bps: Some(310.0),
            receiving: Some(true),
        });
        let s = l.summary();
        assert_eq!(
            (&s["rate_bps"], &s["receiving_now"]),
            (&json!(310.0), &json!(true))
        );
        assert_eq!(s["sending"]["messages"], 0);
    }
}
