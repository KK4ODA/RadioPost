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
use rpmi::host::{Carrier, Direction, Engine, Event, Host, Silence};
use rpmi::tcp::Connection;
use rpmi::wire::{Caps as ModemCaps, Message as Line};
use serde_json::{Value, json};

use crate::selection::attempts::{self, Recorder, SessionNotes, Trace};
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
    /// The channel to call on (wishlist W14): with a modem that has a dial (RP-4 §4.1) the radio
    /// is tuned there before the call when it is not there already.
    pub dial_hz: Option<u64>,
    /// Whether a call that had to tune the radio tunes it back after (RP-4 §4.1). The panel tunes
    /// the radio when its operator picks the channel, so that channel is the operator's own
    /// choice, and the radio stays there; the command line's `--dial` goes back.
    pub tune_back: bool,
    /// On a channel a gateway scans (RP-5 §4.4, `scan_s`), how long the call's tries must keep
    /// starting to find it there: asked of a modem with `caps.call_s` (RP-4 §4).
    pub call_s: Option<u64>,
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
    /// Alerts (RP-8): while listening for a call, the station hears broadcasts and sends what
    /// waits, on the connection that holds its callsigns.
    pub alerts: Option<crate::presence::Carrier>,
    /// The client's other homes, one a callsign (wishlist W46): listening, it answers calls to
    /// their callsigns too, each session carried by the home whose callsign was called.
    pub also: Vec<std::path::PathBuf>,
    /// Where the station is, as the call's record keeps it (choosing a gateway, §15.2): the
    /// panel's position in use, its GPS included. None: the places saved, without a GPS, or the
    /// grid square set before places.
    pub place: Option<Value>,
    /// The station profile in use, by its id, as the call's record keeps it (§15.2).
    pub station: Option<String>,
    /// What was predicted for the channel called, as the gateway list showed it, for the call's
    /// record (choosing a gateway, §12.1).
    pub predicted: Option<Value>,
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
            dial_hz: None,
            tune_back: true,
            call_s: None,
            quiet: QUIET,
            stop: None,
            renew_within: crate::renewal::WITHIN,
            live: None,
            alerts: None,
            also: Vec::new(),
            place: None,
            station: None,
            predicted: None,
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
    /// The codes of the other station's `ERROR`s (RP-3 §5.9), in order.
    pub errors: Vec<String>,
    /// `NOTICE`s heard (RP-3 §5.10).
    pub notices: u32,
    /// This side refused the other station: a gateway answered a station call (RP-3 §10), or its
    /// answer did not check.
    pub refused_other: bool,
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

/// How long a `busy` answer to `hello` is tried again: an adapter that serves one program at a
/// time (RP-4 §3) may answer it while it is still closing this program's last connection.
const BUSY_PATIENCE: Duration = Duration::from_secs(2);

/// A connection to the adapter, with the host's requests numbered.
struct Modem {
    conn: Connection,
    host: Host,
    /// The modem as the user named it.
    name: String,
    /// What its `hello` said it can do.
    caps: ModemCaps,
    /// How long a session may be silent (RP-3 §8).
    quiet: Duration,
    /// The modem's name, as its `hello` gave it.
    said_name: String,
    /// The record of the call under way, fed every event and request (choosing a gateway,
    /// §12.1), when there is one.
    trace: Option<Arc<Mutex<Trace>>>,
}

impl Modem {
    fn open(store: &Store, opts: &Options, extra: &[String]) -> Result<(Modem, String), Error> {
        let me = store.me()?;
        let mut modem = Modem::reach(store, opts)?;
        let mut claimed = vec![me.operator.as_str()];
        claimed.extend(extra.iter().map(String::as_str));
        let callsigns = modem.host.callsigns(&claimed);
        modem.ask(&callsigns)?;
        Ok((modem, me.operator))
    }

    /// The adapter, after `hello` and nothing more (RP-4 §3): no callsigns, so the station answers
    /// and calls nothing for it. Enough for the dial and the tuner tone (§4.1, §4.3).
    fn reach(store: &Store, opts: &Options) -> Result<Modem, Error> {
        if let Some(test) = store.conformance()
            && opts.adapter != test
        {
            return Err(format!(
                "in conformance mode this client reaches only the test adapter at {test}; \
                 `radiopost conformance off` ends it"
            ));
        }
        let adapter = adapter_for(&opts.adapter)?;
        // an adapter that serves one program at a time may not yet have seen this program's last
        // connection close, so `busy` is tried again for a moment before it is believed
        let patience = std::time::Instant::now() + BUSY_PATIENCE;
        let modem = loop {
            let conn = Connection::open(adapter.as_str()).map_err(|e| {
                format!("cannot reach the modem's adapter at {}: {e}", opts.adapter)
            })?;
            conn.set_timeout(Some(opts.quiet))
                .map_err(|e| e.to_string())?;
            let mut modem = Modem {
                conn,
                host: Host::new(),
                name: opts.adapter.clone(),
                caps: ModemCaps::from_json(&Value::Null),
                quiet: opts.quiet,
                said_name: String::new(),
                trace: None,
            };
            let hello = modem.host.hello(&opts.app);
            match modem.ask(&hello) {
                Ok(Event::Hello {
                    caps, modem: said, ..
                }) => {
                    modem.caps = caps;
                    modem.said_name = said;
                    break modem;
                }
                Ok(_) => break modem,
                Err(why)
                    if why.starts_with("the modem refused hello: busy")
                        && std::time::Instant::now() < patience =>
                {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(why) => return Err(why),
            }
        };
        Ok(modem)
    }

    /// What the call's record is told, when a call is recorded.
    fn note(&self, f: impl FnOnce(&mut Trace)) {
        if let Some(t) = &self.trace {
            f(&mut t.lock().unwrap_or_else(PoisonError::into_inner));
        }
    }

    fn send(&mut self, m: &Line) -> Result<(), Error> {
        self.note(|t| t.sending(m));
        self.conn.send(m).map_err(|e| {
            self.note(Trace::fault);
            format!("the adapter's connection failed: {e}")
        })
    }

    fn next(&mut self) -> Result<Event, Error> {
        let read = self
            .conn
            .read_message()
            .map_err(|e| format!("the adapter said nothing in time, or failed: {e}"))
            .and_then(|m| m.ok_or_else(|| "the adapter closed the connection".to_owned()));
        let m = read.inspect_err(|_| self.note(Trace::fault))?;
        let e = Event::read(&m)
            .map_err(|why| format!("the adapter wrote what RP-4 does not: {why}"))?;
        self.note(|t| t.saw(&e));
        Ok(e)
    }

    /// The adapter's next event, or `None` when a quarter of a second passes without one.
    fn poll(&mut self) -> Result<Option<Event>, Error> {
        match self.conn.read_message() {
            Ok(Some(m)) => {
                let e = Event::read(&m)
                    .map_err(|why| format!("the adapter wrote what RP-4 does not: {why}"))?;
                self.note(|t| t.saw(&e));
                Ok(Some(e))
            }
            Ok(None) => {
                self.note(Trace::fault);
                Err("the adapter closed the connection".into())
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Ok(None)
            }
            Err(e) => {
                self.note(Trace::fault);
                Err(format!("the adapter's connection failed: {e}"))
            }
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
                    self.note(|t| t.refused(op, code));
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
                Event::Dial { id: Some(i), .. } if Some(i) == id.as_ref() => return Ok(e),
                _ => {}
            }
        }
    }

    /// Tunes the radio to `dial_hz` for a call, when the modem has a dial (RP-4 §4.1), and gives
    /// the dial it left, to go back to. A modem without one leaves the tuning to the user, who is
    /// told where.
    fn tune_for_call(
        &mut self,
        dial_hz: Option<u64>,
        say: &mut dyn FnMut(&str),
    ) -> Result<Option<u64>, Error> {
        let Some(hz) = dial_hz else {
            return Ok(None);
        };
        if !self.caps.dial {
            tell(say, &by_hand(hz));
            return Ok(None);
        }
        let read = self.host.dial();
        let Event::Dial { dial_hz: was, .. } = self.ask(&read)? else {
            return Err("the modem did not say where its dial is".into());
        };
        if was == hz {
            return Ok(None);
        }
        let tune = self.host.tune(hz);
        self.ask(&tune)
            .map_err(|why| format!("cannot tune the radio to {} MHz: {why}", mhz(hz)))?;
        tell(say, &format!("tuned the radio to {} MHz", mhz(hz)));
        Ok(Some(was))
    }

    /// Tunes the radio back to the dial a call moved it from. The adapter may not have put the
    /// link down yet, so `busy` is tried again for a while.
    fn tune_back(&mut self, was: u64, say: &mut dyn FnMut(&str)) {
        let patience = std::time::Instant::now() + TUNE_BACK_PATIENCE;
        loop {
            let tune = self.host.tune(was);
            match self.ask(&tune) {
                Ok(_) => {
                    tell(say, &format!("tuned the radio back to {} MHz", mhz(was)));
                    return;
                }
                Err(why)
                    if why.starts_with("the modem refused tune: busy")
                        && std::time::Instant::now() < patience =>
                {
                    std::thread::sleep(Duration::from_millis(250));
                }
                Err(why) => {
                    let text = format!("could not tune the radio back to {} MHz: {why}", mhz(was));
                    tell(say, &text);
                    return;
                }
            }
        }
    }
}

/// How long a call's end waits for the adapter to let the dial go back.
const TUNE_BACK_PATIENCE: Duration = Duration::from_secs(30);

/// What the operator is told when the modem cannot set the radio's dial (RP-4 §4.1): which dial
/// to set, by hand. The mobile of 2026-11-17, keyed by RTS with no CAT, was such a radio.
fn by_hand(hz: u64) -> String {
    format!(
        "set the radio's dial to {} MHz (USB) yourself: the modem cannot set it",
        mhz(hz)
    )
}

/// Where the radio is once its operator picked a channel ([`tune`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tuned {
    /// The radio was there already.
    There,
    /// The modem moved the radio there from this dial.
    Moved(u64),
    /// The modem cannot set the dial: the operator sets it by hand.
    ByHand,
}

/// What a channel picked did: where the radio is, what was said, and whether the modem keys a
/// tuner tone (RP-4 §4.3), so the panel offers one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picked {
    /// Where the radio is.
    pub tuned: Tuned,
    /// What the operator is told.
    pub said: String,
    /// The modem keys a tuner tone.
    pub tone: bool,
}

/// Tunes the radio to `dial_hz` now, as its operator picks the channel (a gateway's, or the dial
/// of a station call), so that the radio and the antenna tuner can be set before the call; the
/// call then finds the radio there (wishlist W14, RP-4 §4.1). The radio stays: the channel is the
/// operator's choice. A modem that cannot set the dial is said so, with the dial to set by hand.
///
/// # Errors
///
/// The adapter cannot be reached, or the modem refused the dial: something is up on the station,
/// or the rules or the radio would not have it.
pub fn tune(
    store: &Store,
    opts: &Options,
    dial_hz: u64,
    say: &mut dyn FnMut(&str),
) -> Result<Picked, Error> {
    let mut modem = Modem::reach(store, opts)?;
    let result = modem.tune_to(dial_hz);
    modem.conn.close();
    let picked = result?;
    tell(say, &picked.said);
    Ok(picked)
}

impl Modem {
    fn tune_to(&mut self, hz: u64) -> Result<Picked, Error> {
        let keys_tone = self.caps.tone;
        if !self.caps.dial {
            return Ok(Picked {
                tuned: Tuned::ByHand,
                said: by_hand(hz),
                tone: keys_tone,
            });
        }
        let read = self.host.dial();
        let Event::Dial { dial_hz: was, .. } = self.ask(&read)? else {
            return Err("the modem did not say where its dial is".into());
        };
        if was == hz {
            return Ok(Picked {
                tuned: Tuned::There,
                said: format!("the radio is on {} MHz", mhz(hz)),
                tone: keys_tone,
            });
        }
        let tune = self.host.tune(hz);
        self.ask(&tune)
            .map_err(|why| format!("cannot tune the radio to {} MHz: {why}", mhz(hz)))?;
        Ok(Picked {
            tuned: Tuned::Moved(was),
            said: format!("tuned the radio to {} MHz", mhz(hz)),
            tone: keys_tone,
        })
    }
}

/// The tuner tone (RP-4 §4.3): a steady carrier for `seconds` (at most ten), for an antenna tuner
/// on the channel picked; 0 stops it. The modem keys it; RadioPost holds no connection while it
/// sounds, so a second press asks again with 0.
///
/// # Errors
///
/// The adapter cannot be reached, the modem keys no tuner tone, or it refused: something is up on
/// the station, the channel is busy, or the rules would not have it.
pub fn tone(
    store: &Store,
    opts: &Options,
    seconds: u64,
    say: &mut dyn FnMut(&str),
) -> Result<(), Error> {
    let mut modem = Modem::reach(store, opts)?;
    let result = if modem.caps.tone {
        let tone = modem.host.tone(seconds.min(rpmi::wire::TONE_MAX_S));
        modem.ask(&tone).map(|_| ())
    } else {
        Err("this modem cannot key a tuner tone: use the radio's own tune button".into())
    };
    modem.conn.close();
    result?;
    tell(
        say,
        &if seconds == 0 {
            "tuner tone stopped".to_owned()
        } else {
            format!("tuner tone on for {seconds} s")
        },
    );
    Ok(())
}

/// A dial in MHz, to the kilohertz or finer when it needs it: 7.101, 3.5915.
#[must_use]
pub fn mhz(hz: u64) -> String {
    let fraction = format!("{:06}", hz % 1_000_000);
    let fraction = fraction.trim_end_matches('0');
    format!("{}.{fraction:0<3}", hz / 1_000_000)
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
    // RP-3 §8: the silence counts from the last bytes, stopped while the modem receives
    let mut silence = Silence::new();
    loop {
        silence.heard(&event);
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
        let next = if silence.over(modem.quiet) {
            modem.note(Trace::silence);
            Err(format!(
                "nothing arrived for {} seconds",
                modem.quiet.as_secs()
            ))
        } else {
            modem.next()
        };
        event = match next {
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
    /// Called for station-to-station mail, a gateway answered (RP-3 §10): said once, in plain
    /// words, in place of the refusal that follows.
    not_a_station: bool,
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
    let mut report = json!({
        "started": proofs.started,
        "role": role,
        "mine": proof(&proofs.mine),
        "theirs": proof(&proofs.theirs),
        "sent": list(&stats.sent),
        "received": list(&stats.received),
    });
    // the caller's HELLO nonce, when its statement is in challenge form and holds none
    if let Some(nonce) = proofs.nonce {
        report["nonce"] = json!(base64url(&nonce));
    }
    Some(report)
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
        SessionEvent::Notice(text) => {
            report.notices += 1;
            tell(say, &format!("notice: {text}"));
        }
        SessionEvent::Error { code, text } => {
            report.errors.push(code.to_string());
            let why = format!(
                "the other station sent ERROR {code}{}",
                text.map(|t| format!(": {t}")).unwrap_or_default()
            );
            tell(say, &why);
            report.failed = Some(why);
        }
        SessionEvent::NotAStation | SessionEvent::Refused(_) => refused(&e, report, stats, say),
        SessionEvent::NewCredential(bytes) => {
            store.set_credential(&bytes)?;
            tell(say, "a renewed credential, kept for the next session");
        }
        // only a gateway calling a partner hears `Placed` and `Records` (RP-7 §2), never a client
        SessionEvent::Placed { .. } | SessionEvent::Records(_) | SessionEvent::End => {}
    }
    Ok(())
}

/// This side ended the session: the other station's answer was refused. A station called for P2P
/// that turned out a gateway (RP-3 §10) is said once, in plain words, in place of the refusal.
fn refused(
    e: &SessionEvent,
    report: &mut Report,
    stats: &mut SessionStats,
    say: &mut dyn FnMut(&str),
) {
    let why = match e {
        SessionEvent::NotAStation => {
            stats.not_a_station = true;
            format!(
                "{} is a gateway: for P2P, call the other operator's own callsign while their \
                 client listens",
                report.remote
            )
        }
        SessionEvent::Refused(_) if stats.not_a_station => return,
        SessionEvent::Refused(code) => format!("refused the other station: {code}"),
        _ => return,
    };
    tell(say, &why);
    report.refused_other = true;
    report.failed = Some(why);
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
    let came: Vec<ContentId> = stats.received.iter().map(|(id, _)| *id).collect();
    store.listed(remote, stats.now, &stats.listed, &came);
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
    let base = crate::callsigns::base_of(store.home());
    let record = Recorder::begin(&base, opts.now(), attempt(store, remote, kind, opts));
    let trace = record.trace();
    let result = call_traced(store, remote, kind, opts, say, &trace);
    record.finish(result.as_ref().err().map(String::as_str));
    result
}

/// [`call`], with what happens told to `trace`, the call's record (choosing a gateway, §12.1).
fn call_traced(
    store: &mut Store,
    remote: &str,
    kind: Kind,
    opts: &Options,
    say: &mut dyn FnMut(&str),
    trace: &Arc<Mutex<Trace>>,
) -> Result<Report, Error> {
    let (mut modem, me) = Modem::open(store, opts, &[])?;
    modem.trace = Some(Arc::clone(trace));
    let said = modem.said_name.clone();
    modem.note(|t| t.opened(&said, &me));
    let back = modem
        .tune_for_call(opts.dial_hz, say)?
        .filter(|_| opts.tune_back);
    let result = call_on(&mut modem, store, remote, kind, opts, say);
    // tuning back is after the call, not part of it
    modem.trace = None;
    if let Some(was) = back {
        modem.tune_back(was, say);
    }
    modem.conn.close();
    result
}

/// What a call sets out to do, as known before it is placed: its channel, the operator's grid
/// square, and the messages it will offer and ask for (choosing a gateway, §13.1).
fn attempt(store: &Store, remote: &str, kind: Kind, opts: &Options) -> attempts::Call {
    let now = opts.now();
    let offered = if kind == Kind::Station {
        store.outbox.clone()
    } else {
        store.outbox_for(remote, now)
    };
    let mut plan = attempts::Plan {
        messages: offered.len(),
        ..attempts::Plan::default()
    };
    for w in &offered {
        if let Ok(m) = store.message(&w.id) {
            let priority = m
                .map()
                .get(radiopost::message::key::PRIORITY)
                .and_then(radiopost::cbor::Value::as_u64)
                .unwrap_or(0);
            plan.priority = plan.priority.max(priority);
            plan.out_bytes += outgoing(m, opts.packing).air.len() as u64;
        }
    }
    if kind == Kind::Gateway {
        plan.in_listed_bytes = store
            .waiting_entries()
            .iter()
            .filter(|e| fetched(opts.fetch, e))
            .filter_map(|e| e["size"].as_u64())
            .sum();
    }
    attempts::Call {
        remote: remote.to_owned(),
        gateway: kind == Kind::Gateway,
        dial_hz: opts.dial_hz,
        bandwidth_hz: opts.bandwidth,
        call_s: opts.call_s.or_else(|| {
            (kind == Kind::Gateway)
                .then(|| store.scan_s_of(remote, opts.dial_hz))
                .flatten()
        }),
        place: opts.place.clone().or_else(|| saved_place(store)),
        station: opts.station.clone(),
        plan,
        predicted: opts.predicted.clone(),
    }
}

/// Where the station is with no GPS at hand (the command line): the position the places saved
/// give, or the grid square set before there were places.
fn saved_place(store: &Store) -> Option<Value> {
    let base = crate::callsigns::base_of(store.home());
    match crate::selection::position::saved(&base) {
        Some(h) => Some(json!({"lat": h.lat, "lon": h.lon, "grid": h.grid, "source": "place"})),
        None => store
            .grid
            .as_ref()
            .map(|g| json!({"grid": g, "source": "grid"})),
    }
}

/// Whether a message that waited at the last call will be asked for: marked, or taken by the
/// fetch rule, as [`chooser`] decides.
fn fetched(fetch: Fetch, entry: &Value) -> bool {
    entry["fetch"] == true || entry["size"].as_u64().is_some_and(|s| takes(fetch, s))
}

/// How long the call's tries must keep starting (RP-4 §4, `call_s`): a gateway that scans its
/// channels is found by a call that keeps trying as long as its record says (RP-5 §4.4), asked for
/// or read from the kept list. A modem without `caps.call_s` cannot be asked, and the user is told
/// the call may end too soon.
fn call_lasting(
    modem: &Modem,
    store: &Store,
    remote: &str,
    kind: Kind,
    opts: &Options,
    say: &mut dyn FnMut(&str),
) -> Option<u64> {
    let wanted = opts.call_s.or_else(|| {
        (kind == Kind::Gateway)
            .then(|| store.scan_s_of(remote, opts.dial_hz))
            .flatten()
    });
    let lasting = wanted.filter(|_| modem.caps.call_s);
    if let Some(s) = wanted
        && lasting.is_none()
    {
        tell(
            say,
            &format!(
                "{remote} scans its channels and is found by a call that keeps trying {s} s; this \
                 modem cannot be asked to (RP-4 §4), so the call may end before the gateway comes \
                 round to this channel"
            ),
        );
    }
    lasting
}

/// Places the call (RP-4 §4, `connect`) at `bandwidth` with tries lasting `call_s`, and waits for
/// its link: `None` when it failed, said and put in `report`.
fn place(
    modem: &mut Modem,
    remote: &str,
    (bandwidth, call_s): (Option<i64>, Option<u64>),
    report: &mut Report,
    say: &mut dyn FnMut(&str),
) -> Result<Option<Event>, Error> {
    let connect = modem.host.connect_lasting(remote, None, bandwidth, call_s);
    let id = connect.get("id").cloned();
    modem.ask(&connect)?;
    say_at(say, &format!("calling {remote}"), bandwidth);
    loop {
        let e = modem.next()?;
        match &e {
            Event::Connected {
                id: i, bandwidth, ..
            } if *i == id => {
                // the bandwidth the modem says the link runs at: what was asked, or its own
                say_at(say, &format!("connected to {remote}"), *bandwidth);
                return Ok(Some(e));
            }
            Event::Failed {
                id: i, code, text, ..
            } if *i == id => {
                let why = format!(
                    "the call failed: {code}{}",
                    text.as_deref()
                        .map(|t| format!(" ({t})"))
                        .unwrap_or_default()
                );
                report.failed = Some(why.clone());
                tell(say, &why);
                return Ok(None);
            }
            _ => {}
        }
    }
}

/// Says `line`, and at what bandwidth when one is known.
fn say_at(say: &mut dyn FnMut(&str), line: &str, hz: Option<i64>) {
    match hz {
        Some(hz) => tell(say, &format!("{line} at {hz} Hz")),
        None => tell(say, line),
    }
}

/// [`call`] on a modem opened and tuned.
fn call_on(
    modem: &mut Modem,
    store: &mut Store,
    remote: &str,
    kind: Kind,
    opts: &Options,
    say: &mut dyn FnMut(&str),
) -> Result<Report, Error> {
    let lasting = call_lasting(modem, store, remote, kind, opts, say);
    let mut report = Report {
        remote: remote.to_owned(),
        ..Report::default()
    };
    let Some(connected) = place(modem, remote, (opts.bandwidth, lasting), &mut report, say)? else {
        return Ok(report);
    };
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
        modem,
        &mut carrier,
        &connected,
        store,
        &mut report,
        &mut stats,
        opts.live.as_deref(),
        say,
    );
    let over = carrier.engine().0.is_over();
    modem.note(|t| {
        t.session(SessionNotes {
            online: stats.online,
            errors: report.errors.clone(),
            notices: report.notices,
            over,
            refused_other: report.refused_other,
            taken: report.delivered.len(),
            queued: report.queued.len(),
            received: report.received.len(),
        });
    });
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
    Ok(report)
}

/// What the user fetches of what waits: all, what is no larger than a size, or nothing, and
/// whatever they marked to fetch from the last call's waiting list (a local message, which has no
/// serial, follows the choice of what to fetch: RP-7 §4.4).
fn chooser(fetch: Fetch, marked: Vec<u64>) -> Chooser {
    Box::new(move |entries: &[WaitingEntry]| {
        entries
            .iter()
            .filter(|e| (e.serial != 0 && marked.contains(&e.serial)) || takes(fetch, e.size))
            .map(|e| e.id)
            .collect()
    })
}

/// Whether the fetch rule takes a waiting message of `size` bytes.
fn takes(fetch: Fetch, size: u64) -> bool {
    match fetch {
        Fetch::All => true,
        Fetch::UpTo(n) => size <= n,
        Fetch::Nothing => false,
    }
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

/// Waits for a station to call: its link's `connected`, its callsign and the callsign it
/// called, or `None` when the program said stop first.
fn wait_for_call(
    modem: &mut Modem,
    opts: &Options,
) -> Result<Option<(Event, String, String)>, Error> {
    modem
        .conn
        .set_timeout(Some(Duration::from_millis(250)))
        .map_err(|e| e.to_string())?;
    // alerts sent on this connection, by their requests' `id`, to their references (RP-8 §6)
    let mut alerting: HashMap<String, String> = HashMap::new();
    let called = loop {
        if stopped(opts) {
            return Ok(None);
        }
        if let Some(c) = opts.alerts.as_ref().filter(|_| modem.caps.broadcast) {
            let now = crate::presence::unix_now();
            c.alerts.tick(now);
            while let Some((reference, frame)) = c.alerts.due(now) {
                let request = modem.host.broadcast(&frame, Some(&reference));
                if let Some(id) = request.get("id") {
                    alerting.insert(id.to_string(), reference);
                }
                modem.send(&request)?;
            }
        }
        let Some(e) = modem.poll()? else {
            continue;
        };
        if let Some(c) = &opts.alerts {
            let now = crate::presence::unix_now();
            match &e {
                Event::Broadcast { data, .. } => c.heard(data),
                Event::BroadcastSent {
                    reference: Some(r),
                    sent,
                    text,
                } => {
                    if let Some(r) = r.as_str() {
                        c.alerts.went(r, *sent, text.as_deref(), now);
                    }
                }
                Event::Error {
                    id: Some(i),
                    code,
                    text,
                } => {
                    if let Some(r) = alerting.remove(&i.to_string()) {
                        let why = text.clone().unwrap_or_else(|| code.clone());
                        c.alerts.went(&r, false, Some(&why), now);
                    }
                }
                _ => {}
            }
        }
        if let Event::Connected {
            direction: Direction::In,
            remote,
            local,
            ..
        } = &e
        {
            let (remote, local) = (remote.clone(), local.clone());
            break (e, remote, local);
        }
    };
    modem
        .conn
        .set_timeout(Some(opts.quiet))
        .map_err(|e| e.to_string())?;
    Ok(Some(called))
}

/// Whether the program said stop.
fn stopped(opts: &Options) -> bool {
    opts.stop
        .as_ref()
        .is_some_and(|s| s.load(Ordering::Relaxed))
}

/// A station listening: the modem's connection, the client's callsigns claimed on it and `listen`
/// on (RP-4 §4), so that the modem answers calls to them. The client's other callsigns come with
/// their homes (wishlist W46).
struct Listener {
    modem: Modem,
    /// The callsign of the home that opened it.
    me: String,
    /// The client's other callsigns, each with its home.
    others: Vec<(String, std::path::PathBuf)>,
}

impl Listener {
    /// Claims the client's callsigns and turns `listen` on.
    fn open(store: &Store, opts: &Options, say: &mut dyn FnMut(&str)) -> Result<Listener, Error> {
        let mine = store.me()?.operator;
        let others: Vec<(String, std::path::PathBuf)> = opts
            .also
            .iter()
            .filter_map(|home| {
                let me = Store::open(home).ok()?.me().ok()?;
                Some((me.operator, home.clone()))
            })
            .filter(|(c, _)| !c.eq_ignore_ascii_case(&mine))
            .collect();
        let extra: Vec<String> = others.iter().map(|(c, _)| c.clone()).collect();
        let (mut modem, me) = Modem::open(store, opts, &extra)?;
        let listen = modem.host.listen(true);
        if let Err(why) = modem.ask(&listen) {
            modem.conn.close();
            return Err(why);
        }
        let all = std::iter::once(me.clone())
            .chain(extra.iter().cloned())
            .collect::<Vec<_>>()
            .join(", ");
        tell(say, &format!("listening as {all}"));
        Ok(Listener { modem, me, others })
    }

    /// Carries the session of a call that came (RP-3 §10), in the home whose callsign was called
    /// (`local`): `store` when it is this one's. The connection stays open, the callsigns claimed.
    fn answer(
        &mut self,
        store: &mut Store,
        (connected, remote, local): (Event, String, String),
        opts: &Options,
        say: &mut dyn FnMut(&str),
    ) -> Result<Report, Error> {
        tell(say, &format!("{remote} called"));
        // the home called; at an SSID or portable form of a callsign, that callsign's (RP-1 §6.3)
        let called = self
            .others
            .iter()
            .find(|(c, _)| c.eq_ignore_ascii_case(&local))
            .or_else(|| {
                if radiopost::callsign::same_base(&local, &self.me) {
                    None
                } else {
                    self.others
                        .iter()
                        .find(|(c, _)| radiopost::callsign::same_base(c, &local))
                }
            })
            .cloned();
        if let Some((call, home)) = called {
            tell(
                say,
                &format!("answering as {call}: the session's mail is in {call}'s folders"),
            );
            let mut other = Store::open(&home)?;
            return answer_on(
                &mut other,
                &mut self.modem,
                (&connected, &remote),
                opts,
                say,
            );
        }
        answer_on(store, &mut self.modem, (&connected, &remote), opts, say)
    }
}

/// Listens for a station's call and carries its session (RP-3 §10), then returns: one call, as
/// the command line's `listen` answers. The panel's Listen is [`keep_listening`].
///
/// # Errors
///
/// No identity yet, the adapter cannot be reached or refuses, or the session breaks.
pub fn answer(
    store: &mut Store,
    opts: &Options,
    say: &mut dyn FnMut(&str),
) -> Result<Report, Error> {
    let mut listener = Listener::open(store, opts, say)?;
    let called = match wait_for_call(&mut listener.modem, opts) {
        Ok(Some(called)) => called,
        Ok(None) => {
            tell(say, "stopped listening");
            listener.modem.conn.close();
            return Ok(Report {
                failed: Some("stopped before a call came".into()),
                ..Report::default()
            });
        }
        Err(why) => {
            listener.modem.conn.close();
            return Err(why);
        }
    };
    let result = listener.answer(store, called, opts, say);
    listener.modem.conn.close();
    result
}

/// How long a station that keeps listening waits before it tries again a modem that went away or
/// could not be reached: as the app's connection between calls does ([`crate::presence`]).
pub const LISTEN_RETRY: Duration = Duration::from_secs(5);

/// What a station that keeps listening ([`keep_listening`]) tells its window as it goes, besides
/// its lines.
#[derive(Debug)]
pub enum Listening {
    /// The client's callsigns are claimed and the modem answers calls to them: waiting for one.
    Waiting,
    /// This station called: a session begins.
    Called(String),
    /// A session is over, however it ended: what it did, or why it broke.
    Ended(Result<Report, Error>),
    /// The station cannot listen now (the modem's adapter cannot be reached, went away or
    /// refused): why. It is tried again every [`LISTEN_RETRY`].
    Held(Error),
}

/// Listens for calls and answers each (RP-3 §10) until `opts.stop` is set: the panel's Listen,
/// a switch that stays on until its operator turns it off (the 40 m sessions of 2026-11-18, when
/// a second call found the station no longer listening).
///
/// After each session, however it ended (done, failed, broken off, the other station gone), the
/// station goes back to listening on the same connection, its callsigns still claimed, so the
/// modem never stops answering them (Aether ends a claim with its connection, its ADR-0106). A
/// connection that failed is opened again; while the adapter cannot be reached or refuses (the
/// modem away, Aether restarting), it is tried again every [`LISTEN_RETRY`], each reason said
/// once. The home at `home` is opened afresh for each call, so a session offers what was written
/// since the last: `told` hears [`Listening::Called`] before the home is opened, and
/// [`Listening::Ended`] once the session's last save is done, so a window that changes the home
/// between sessions can keep its changes and the session's from meeting (the panel's lock).
/// `stop` set during a session ends listening once the session is over.
pub fn keep_listening(
    home: &std::path::Path,
    opts: &Options,
    say: &mut dyn FnMut(&str),
    told: &mut dyn FnMut(Listening),
) {
    let mut listener: Option<Listener> = None;
    // what was last said of why the station cannot listen, so that it is said once
    let mut held: Option<String> = None;
    loop {
        if stopped(opts) {
            break;
        }
        if listener.is_none() {
            match Store::open(home).and_then(|store| Listener::open(&store, opts, say)) {
                Ok(l) => {
                    held = None;
                    listener = Some(l);
                    told(Listening::Waiting);
                }
                Err(why) => {
                    if held.as_deref() != Some(why.as_str()) {
                        tell(
                            say,
                            &format!(
                                "cannot listen: {why}; trying again every {} s",
                                LISTEN_RETRY.as_secs()
                            ),
                        );
                        held = Some(why.clone());
                    }
                    told(Listening::Held(why));
                    pause(opts, LISTEN_RETRY);
                    continue;
                }
            }
        }
        let Some(l) = listener.as_mut() else {
            continue;
        };
        let called = match wait_for_call(&mut l.modem, opts) {
            Ok(Some(called)) => called,
            Ok(None) => break,
            Err(why) => {
                // the connection failed while the station waited (Aether restarting, say): it is
                // opened again after a pause, so an adapter that fails at once is not hammered
                if let Some(gone) = listener.take() {
                    gone.modem.conn.close();
                }
                tell(say, &format!("the modem's connection ended: {why}"));
                // and if it cannot be opened again, that is said too, with its tries
                held = None;
                told(Listening::Held(why));
                pause(opts, LISTEN_RETRY);
                continue;
            }
        };
        if let Event::Connected { remote, .. } = &called.0 {
            told(Listening::Called(remote.clone()));
        }
        let result =
            Store::open(home).and_then(|mut store| l.answer(&mut store, called, opts, say));
        let broke = result.is_err();
        told(Listening::Ended(result));
        if broke {
            // what the connection holds after a session that broke is not known: it is opened
            // again, its callsigns claimed afresh
            if let Some(gone) = listener.take() {
                gone.modem.conn.close();
            }
        } else if !stopped(opts) {
            tell(say, "listening again");
            told(Listening::Waiting);
        }
    }
    if let Some(l) = listener {
        l.modem.conn.close();
    }
    tell(say, "stopped listening");
}

/// Waits `how_long`, or less when the program says stop.
fn pause(opts: &Options, how_long: Duration) {
    let until = std::time::Instant::now() + how_long;
    while std::time::Instant::now() < until && !stopped(opts) {
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The session of a call answered, carried by the home called (RP-3 §10).
fn answer_on(
    store: &mut Store,
    modem: &mut Modem,
    (connected, remote): (&Event, &str),
    opts: &Options,
    say: &mut dyn FnMut(&str),
) -> Result<Report, Error> {
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
        remote: remote.to_owned(),
        ..Report::default()
    };
    let mut carrier = Carrier::new(Answering(called));
    let result = carry(
        modem,
        &mut carrier,
        connected,
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
            peer_snr_db: None,
        });
        let s = l.summary();
        assert_eq!(
            (&s["rate_bps"], &s["receiving_now"]),
            (&json!(310.0), &json!(true))
        );
        assert_eq!(s["sending"]["messages"], 0);
    }
}
