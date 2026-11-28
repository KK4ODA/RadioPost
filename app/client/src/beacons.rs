// SPDX-License-Identifier: MIT OR Apache-2.0
//! The station's beacons (wishlist W59, `docs/design/07-p2p-stations.md` §5.1, §6 and §7).
//!
//! * **What a beacon is.** The modem's own (RP-4 §4.4): one frame with the station's callsign,
//!   addressed to nobody, so that the stations listening list it. RadioPost asks for each one; the
//!   modem decides only when, inside the window it was given, the channel is clear.
//! * **Carried by whoever holds the modem**, as alerts are ([`crate::alerts`]): the listening
//!   connection while the station listens ([`crate::session`]), otherwise a short connection that
//!   claims the callsign, sends the beacon and closes ([`crate::presence`]). A beacon asked for
//!   waits in [`Beacons::due`]'s queue of one, and [`Beacons::went`] says what became of it.
//! * **The position** (design §7): the position in use, as precisely as the operator chose: none,
//!   the 4- or the 6-character square, or the position itself (the default). A position in use
//!   that is a square's centre goes as that square, never as an exact position it is not. With no
//!   position in use, the modem may put its own in (Aether's Setup grid square); with **None**
//!   chosen, it is told to put nothing in.
//! * **Repeating** (design §6): **Beacon**'s latch. The next beacon falls due one interval after
//!   the station's callsign was last on the air for this client (a beacon, a call, a session), plus
//!   0 to 60 s drawn at random, so a station in use beacons less and two started together drift
//!   apart. It waits while the client calls or is in a session, and with *Only while listening*
//!   until **Listen for P2P** is on; one that cannot go within its window is skipped and said, the
//!   next kept to the interval. It stops after four hours, when the rules refuse it, or when the
//!   latch is released; while the modem repeats its own beacon (Aether's **Repeat**) it sends none.
//!   A station whose gateway scans beacons on each scanned dial in turn.
//! * **Kept** in `beacon.json` in the first home: the interval, *Only while listening* and the
//!   position's precision. A beacon asked for and repeating are not kept: a restart starts neither.
//! * **Sans clock.** Every call takes `now`, in Unix seconds.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rpmi::position::{Position, Precision as Sent};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::store::{Error, write};

/// The beacon's settings, in the first home.
pub const FILE: &str = "beacon.json";
/// How long after one went another may be asked for by hand (design §5.1).
pub const AGAIN_AFTER_S: u64 = 60;
/// How long a beacon may wait for a clear channel (RP-4 §4.4's default).
pub const WITHIN_S: u64 = 120;
/// How long past its window a beacon's end is waited for before it is taken as not sent.
pub const GRACE_S: u64 = 30;
/// The intervals a beacon repeats at, in minutes (design §6, as RP-8 §7's reports).
pub const INTERVALS_MIN: [u32; 4] = [10, 15, 30, 60];
/// How long a beacon repeats before it stops by itself (design §6): past four hours a timer is a
/// forgotten one.
pub const REPEAT_FOR_S: u64 = 4 * 3600;
/// The most drawn at random onto each interval, seconds.
pub const JITTER_S: u64 = 60;

/// How much of the position in use a beacon says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Precision {
    /// Nothing.
    None,
    /// The 4-character square: 2° by 1°.
    Grid4,
    /// The 6-character square: 5′ by 2.5′.
    Grid6,
    /// The position itself.
    #[default]
    Exact,
}

/// What the operator chose, kept in [`FILE`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Minutes between beacons while repeating: one of [`INTERVALS_MIN`].
    pub every_min: u32,
    /// Repeating starts and stops with **Listen for P2P**.
    pub only_while_listening: bool,
    /// How much of the position in use goes.
    pub position: Precision,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            every_min: 15,
            only_while_listening: true,
            position: Precision::Exact,
        }
    }
}

/// Where the station is, as the position in use says it: its latitude and longitude, the square
/// it is the centre of when it is one, and where it came from, in words.
#[derive(Clone, Debug, PartialEq)]
pub struct Spot {
    /// Degrees north.
    pub lat: f64,
    /// Degrees east.
    pub lon: f64,
    /// The square whose centre this is, when the place is a grid square.
    pub square: Option<String>,
    /// Where it came from: *GPS, 12 s ago*, *Home*.
    pub from: String,
}

/// The position in use, asked for as each beacon goes.
pub type Here = Arc<dyn Fn() -> Option<Spot> + Send + Sync>;

/// What a beacon says of `spot` at `precision`: a square's centre goes as that square at most.
#[must_use]
pub fn position_sent(spot: &Spot, precision: Precision) -> Option<Position> {
    let finest = match spot.square.as_deref().map(str::len) {
        Some(6) => Precision::Grid6,
        Some(_) => Precision::Grid4,
        None => Precision::Exact,
    };
    let rank = |p: Precision| match p {
        Precision::None => 0,
        Precision::Grid4 => 1,
        Precision::Grid6 => 2,
        Precision::Exact => 3,
    };
    let goes = if rank(precision) <= rank(finest) {
        precision
    } else {
        finest
    };
    let sent = match goes {
        Precision::None => return None,
        Precision::Grid4 => Sent::Grid4,
        Precision::Grid6 => Sent::Grid6,
        Precision::Exact => Sent::Exact,
    };
    Some(Position {
        lat: spot.lat,
        lon: spot.lon,
        precision: sent,
        grid: None,
    })
}

/// What became of the last beacon.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Outcome {
    /// When it ended, Unix seconds.
    pub at: u64,
    /// Whether it went on the air.
    pub sent: bool,
    /// The dial it went on, when the modem says.
    pub dial_hz: Option<u64>,
    /// Why not, when it did not.
    pub text: Option<String>,
}

/// A beacon to send now.
#[derive(Clone, Debug, PartialEq)]
pub struct Due {
    /// The `ref` its `beacon-sent` comes back with.
    pub reference: String,
    /// How long it may wait for a clear channel.
    pub within_s: u64,
    /// The dial it goes on, when one is asked for.
    pub dial_hz: Option<u64>,
    /// Where it says the station is.
    pub position: Option<Position>,
    /// `Some(false)`: the operator chose to say nothing, so the modem adds no position of its own
    /// (RP-4 §4.4). With no position in use and not that, the modem may add its own (Aether's
    /// Setup grid square).
    pub modem_position: Option<bool>,
}

/// What the schedule is told of the station each time it is asked (design §6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each is a fact about the station the schedule waits on"
)]
pub struct Context {
    /// The station listens for P2P.
    pub listening: bool,
    /// The client calls, is in a session, or sends an alert: no beacon now.
    pub busy: bool,
    /// The station's callsign is on the air for this client now: a call or a session.
    pub on_air: bool,
    /// The modem repeats a beacon of its own (RP-4 `status.beaconing`): the client sends none.
    pub modem_repeats: bool,
}

/// Repeating: since when, until when, and when the next falls due.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Repeat {
    since: u64,
    until: u64,
    next: u64,
}

struct State {
    path: Option<PathBuf>,
    settings: Settings,
    made: u64,
    repeat: Option<Repeat>,
    /// Why repeating ended, until it starts again.
    ended: Option<String>,
    /// When the station's callsign was last on the air for this client.
    on_air: Option<u64>,
    /// Whether the beacon asked for is the schedule's: one that cannot go is skipped.
    scheduled: bool,
    /// Which of a scan's dials the next scheduled beacon goes on.
    turn: usize,
    /// The jitter's draws (xorshift).
    rng: u64,
    /// Asked for and not yet taken by a connection: its reference and when it was asked.
    asked: Option<(String, u64)>,
    /// Taken by a connection: its reference and until when its end is waited for.
    out: Option<(String, u64)>,
    last: Option<Outcome>,
    here: Option<Here>,
}

/// The station's beacons: shared by the panel and the connections that hold the modem.
#[derive(Clone)]
pub struct Beacons {
    inner: Arc<Mutex<State>>,
}

impl std::fmt::Debug for Beacons {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Beacons").finish_non_exhaustive()
    }
}

impl Beacons {
    /// The beacons of the first home `base`, their settings read from [`FILE`].
    #[must_use]
    pub fn open(base: &Path) -> Beacons {
        let path = base.join(FILE);
        let settings = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Settings>(&bytes).ok())
            .map(Settings::checked)
            .unwrap_or_default();
        Beacons::with(Some(path), settings)
    }

    /// Beacons kept nowhere: for tests.
    #[must_use]
    pub fn in_memory() -> Beacons {
        Beacons::with(None, Settings::default())
    }

    fn with(path: Option<PathBuf>, settings: Settings) -> Beacons {
        // the draws differ from one computer and run to the next, so two stations started
        // together drift apart; kept stores (tests) draw the same each time
        let seed = if path.is_some() {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(1, |d| u64::from(d.subsec_nanos()) ^ d.as_secs())
        } else {
            0x9E37_79B9_7F4A_7C15
        };
        Beacons {
            inner: Arc::new(Mutex::new(State {
                path,
                settings,
                made: 0,
                repeat: None,
                ended: None,
                on_air: None,
                scheduled: false,
                turn: 0,
                rng: seed | 1,
                asked: None,
                out: None,
                last: None,
                here: None,
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Where the position in use comes from.
    pub fn set_here(&self, here: Here) {
        self.lock().here = Some(here);
    }

    /// The settings.
    #[must_use]
    pub fn settings(&self) -> Settings {
        self.lock().settings.clone()
    }

    /// Changes the settings and keeps them.
    ///
    /// # Errors
    /// An interval not offered, or a file that cannot be written.
    pub fn set(&self, settings: Settings) -> Result<(), Error> {
        if !INTERVALS_MIN.contains(&settings.every_min) {
            return Err(format!(
                "a beacon repeats every {} minutes",
                words_of(&INTERVALS_MIN)
            ));
        }
        let mut s = self.lock();
        s.settings = settings;
        if let Some(path) = &s.path {
            let bytes = serde_json::to_vec_pretty(&s.settings).map_err(|e| e.to_string())?;
            write(path, &bytes)?;
        }
        Ok(())
    }

    /// One beacon, now: **Send a beacon now**.
    ///
    /// # Errors
    /// One is already waiting, or one went less than [`AGAIN_AFTER_S`] ago.
    pub fn ask_now(&self, now: u64) -> Result<(), &'static str> {
        let mut s = self.lock();
        s.expire(now);
        if s.asked.is_some() || s.out.is_some() {
            return Err("a beacon is already waiting to go out");
        }
        if s.last
            .as_ref()
            .is_some_and(|l| l.sent && now < l.at + AGAIN_AFTER_S)
        {
            return Err("a beacon went less than a minute ago");
        }
        s.made += 1;
        let reference = format!("b{}", s.made);
        s.asked = Some((reference, now));
        s.scheduled = false;
        Ok(())
    }

    /// **Beacon**'s latch pressed in: repeating, the first beacon at once (or once the station
    /// listens, with *Only while listening*), for four hours at most.
    pub fn start_repeating(&self, now: u64) {
        let mut s = self.lock();
        s.repeat = Some(Repeat {
            since: now,
            until: now + REPEAT_FOR_S,
            next: now,
        });
        s.ended = None;
    }

    /// Repeating stops: the latch released, or `why`.
    pub fn stop_repeating(&self, why: Option<&str>) {
        let mut s = self.lock();
        if s.repeat.take().is_some() {
            s.ended = why.map(str::to_owned);
        }
    }

    /// Whether the beacon repeats.
    #[must_use]
    pub fn repeating(&self) -> bool {
        self.lock().repeat.is_some()
    }

    /// The station's callsign is on the air for this client now (a call, a session): the next
    /// beacon falls due an interval after.
    pub fn on_air(&self, now: u64) {
        self.lock().put_on_air(now);
    }

    /// The schedule, asked: a beacon asked for when one falls due and the station may send it.
    /// A line for the log when repeating stops by itself.
    #[allow(
        clippy::must_use_candidate,
        reason = "called for what it does; the line is for the log when there is one"
    )]
    pub fn tick(&self, now: u64, ctx: Context) -> Option<String> {
        let mut s = self.lock();
        s.expire(now);
        if ctx.on_air {
            s.put_on_air(now);
        }
        let r = s.repeat?;
        if now >= r.until {
            s.repeat = None;
            s.ended = Some("after four hours".into());
            return Some(
                "beacon: repeating stopped after four hours; press Beacon to start four more"
                    .into(),
            );
        }
        let held = ctx.modem_repeats
            || ctx.busy
            || (s.settings.only_while_listening && !ctx.listening)
            || s.asked.is_some()
            || s.out.is_some();
        if held || now < r.next {
            return None;
        }
        s.made += 1;
        let reference = format!("b{}", s.made);
        s.asked = Some((reference, now));
        s.scheduled = true;
        let next = now + s.interval_s();
        if let Some(r) = s.repeat.as_mut() {
            r.next = next;
        }
        None
    }

    /// The rules or the modem refused a beacon: waiting will not change it, so repeating stops.
    #[allow(
        clippy::must_use_candidate,
        reason = "called for what it does; the line is for the log when there is one"
    )]
    pub fn refused(&self, why: &str) -> Option<String> {
        let mut s = self.lock();
        s.repeat?;
        s.repeat = None;
        s.ended = Some(why.to_owned());
        Some(format!("beacon: repeating stopped: {why}"))
    }

    /// Whether a beacon waits for a connection to take it.
    #[must_use]
    pub fn wants(&self, now: u64) -> bool {
        let mut s = self.lock();
        s.expire(now);
        s.asked.is_some()
    }

    /// The beacon to send now, taken: the connection that takes it reports on it with
    /// [`Beacons::went`]. `scanning` is the dials a gateway on the station scans: a scheduled
    /// beacon goes on each in turn (design §6).
    #[must_use]
    pub fn due(&self, now: u64, scanning: &[u64]) -> Option<Due> {
        let (reference, here, precision, dial_hz) = {
            let mut s = self.lock();
            s.expire(now);
            let (reference, _) = s.asked.take()?;
            s.out = Some((reference.clone(), now + WITHIN_S + GRACE_S));
            let dial_hz = if s.scheduled && !scanning.is_empty() {
                let dial = scanning[s.turn % scanning.len()];
                s.turn = s.turn.wrapping_add(1);
                Some(dial)
            } else {
                None
            };
            (reference, s.here.clone(), s.settings.position, dial_hz)
        };
        // the position in use is asked for outside the lock: a GPS may take a moment
        let position = here
            .and_then(|here| here())
            .and_then(|spot| position_sent(&spot, precision));
        Some(Due {
            reference,
            within_s: WITHIN_S,
            dial_hz,
            position,
            modem_position: (precision == Precision::None).then_some(false),
        })
    }

    /// What became of the beacon with `reference`.
    pub fn went(
        &self,
        reference: &str,
        sent: bool,
        dial_hz: Option<u64>,
        text: Option<&str>,
        now: u64,
    ) {
        let mut s = self.lock();
        let ours = s.out.as_ref().is_some_and(|(r, _)| r == reference)
            || s.asked.as_ref().is_some_and(|(r, _)| r == reference);
        if !ours {
            return;
        }
        s.out = None;
        s.asked = None;
        if sent {
            s.put_on_air(now);
        }
        s.last = Some(Outcome {
            at: now,
            sent,
            dial_hz,
            text: text.map(str::to_owned),
        });
    }

    /// Whatever waits will not go: no connection could carry it, with why.
    pub fn failed(&self, why: &str, now: u64) {
        let mut s = self.lock();
        if s.asked.take().is_some() || s.out.take().is_some() {
            s.last = Some(Outcome {
                at: now,
                sent: false,
                dial_hz: None,
                text: Some(why.to_owned()),
            });
        }
    }

    /// What the panel shows: the settings, whether one waits, the last one's outcome, and what the
    /// position would say now.
    #[must_use]
    pub fn json(&self, now: u64) -> Value {
        let (settings, waiting, last, here, again, repeating, ended) = {
            let mut s = self.lock();
            s.expire(now);
            let repeating = s
                .repeat
                .map(|r| json!({"since": r.since, "until": r.until, "next": r.next.max(now)}));
            let again = s
                .last
                .as_ref()
                .filter(|l| l.sent)
                .map(|l| (l.at + AGAIN_AFTER_S).saturating_sub(now))
                .filter(|&left| left > 0);
            (
                s.settings.clone(),
                s.asked.is_some() || s.out.is_some(),
                s.last.clone(),
                s.here.clone(),
                again,
                repeating,
                s.ended.clone(),
            )
        };
        let spot = here.and_then(|here| here());
        let goes = spot
            .as_ref()
            .and_then(|spot| position_sent(spot, settings.position));
        json!({
            "settings": settings,
            "intervals_min": INTERVALS_MIN,
            "waiting": waiting,
            "again_in_s": again,
            "last": last,
            "repeating": repeating,
            "ended": ended,
            "position": {
                "goes": goes.as_ref().map(Position::to_json),
                "grid": goes.as_ref().and_then(|p| match p.precision {
                    Sent::Exact => None,
                    Sent::Grid4 => Some(rpmi::position::locator(p.lat, p.lon, 4)),
                    Sent::Grid6 => Some(rpmi::position::locator(p.lat, p.lon, 6)),
                }),
                "from": spot.map(|s| s.from),
                // with no position in use, the modem may put its own in; with None, nothing goes
                "modems_own": goes.is_none() && settings.position != Precision::None,
            },
        })
    }
}

impl State {
    /// One interval and a draw of the jitter, seconds.
    fn interval_s(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        u64::from(self.settings.every_min) * 60 + self.rng % (JITTER_S + 1)
    }

    /// The station's callsign on the air now: the next beacon an interval after.
    fn put_on_air(&mut self, now: u64) {
        self.on_air = Some(now);
        let wait = self.interval_s();
        if let Some(r) = self.repeat.as_mut() {
            r.next = r.next.max(now + wait);
        }
    }

    /// A beacon asked for that no connection took, or whose end never came, is given up.
    fn expire(&mut self, now: u64) {
        if self
            .asked
            .as_ref()
            .is_some_and(|(_, at)| now > at + WITHIN_S + GRACE_S)
        {
            self.asked = None;
            self.last = Some(Outcome {
                at: now,
                sent: false,
                dial_hz: None,
                text: Some("no connection to the modem could send it".into()),
            });
        }
        if self.out.as_ref().is_some_and(|(_, until)| now > *until) {
            self.out = None;
            self.last = Some(Outcome {
                at: now,
                sent: false,
                dial_hz: None,
                text: Some("the modem never said it went out".into()),
            });
        }
    }
}

impl Settings {
    /// The settings with an interval not offered put back to the default's.
    fn checked(self) -> Settings {
        if INTERVALS_MIN.contains(&self.every_min) {
            self
        } else {
            Settings {
                every_min: Settings::default().every_min,
                ..self
            }
        }
    }
}

/// The log's line for a beacon's end.
#[must_use]
pub fn said(sent: bool, dial_hz: Option<u64>, text: Option<&str>) -> String {
    match (sent, dial_hz) {
        (true, Some(hz)) => format!("beacon: sent on {} MHz", crate::session::mhz(hz)),
        (true, None) => "beacon: sent".to_owned(),
        (false, _) => format!("beacon: not sent: {}", text.unwrap_or("no reason given")),
    }
}

/// `10, 15, 30 or 60`.
fn words_of(minutes: &[u32]) -> String {
    let shown: Vec<String> = minutes.iter().map(u32::to_string).collect();
    match shown.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
        _ => shown.join(""),
    }
}

#[cfg(test)]
mod tests {
    use super::{Beacons, Precision, Settings, Spot, position_sent};
    use rpmi::position::Precision as Sent;
    use std::sync::Arc;

    fn spot(square: Option<&str>) -> Spot {
        Spot {
            lat: 34.4621,
            lon: -83.7514,
            square: square.map(str::to_owned),
            from: "Home".into(),
        }
    }

    #[test]
    fn a_beacon_says_no_more_of_the_position_than_chosen_or_known() {
        let exact = spot(None);
        let goes = |s: &Spot, p| position_sent(s, p).map(|p| p.precision);
        assert_eq!(goes(&exact, Precision::Exact), Some(Sent::Exact));
        assert_eq!(goes(&exact, Precision::Grid6), Some(Sent::Grid6));
        assert_eq!(goes(&exact, Precision::Grid4), Some(Sent::Grid4));
        assert_eq!(goes(&exact, Precision::None), None);
        // a place that is a square's centre goes as that square, never as a position it is not
        assert_eq!(
            goes(&spot(Some("EM84cl")), Precision::Exact),
            Some(Sent::Grid6)
        );
        assert_eq!(
            goes(&spot(Some("EM84")), Precision::Exact),
            Some(Sent::Grid4)
        );
        assert_eq!(
            goes(&spot(Some("EM84")), Precision::Grid6),
            Some(Sent::Grid4)
        );
    }

    #[test]
    fn one_beacon_at_a_time_and_not_again_within_a_minute() {
        let b = Beacons::in_memory();
        b.set_here(Arc::new(|| Some(spot(None))));
        b.ask_now(1000).expect("idle");
        assert_eq!(
            b.ask_now(1001),
            Err("a beacon is already waiting to go out")
        );
        assert!(b.wants(1001));
        let due = b.due(1002, &[]).expect("one waits");
        assert_eq!(due.within_s, 120);
        assert_eq!(due.position.map(|p| p.precision), Some(Sent::Exact));
        assert!(!b.wants(1002), "taken");
        assert_eq!(
            b.ask_now(1003),
            Err("a beacon is already waiting to go out")
        );
        b.went(&due.reference, true, Some(7_101_000), None, 1010);
        assert_eq!(b.ask_now(1030), Err("a beacon went less than a minute ago"));
        assert_eq!(b.json(1030)["again_in_s"], 40);
        b.ask_now(1071).expect("a minute on");
        // one no connection takes is given up, and said
        assert!(!b.wants(1071 + 151));
        let view = b.json(1071 + 151);
        assert_eq!(view["last"]["sent"], false);
        assert_eq!(view["waiting"], false);
        // and one taken whose end never comes
        b.ask_now(2000).expect("idle");
        let _ = b.due(2000, &[]);
        assert_eq!(
            b.json(2000 + 151)["last"]["text"],
            "the modem never said it went out"
        );
    }

    #[test]
    fn a_repeating_beacon_falls_due_an_interval_after_the_callsign_was_last_on_the_air() {
        use super::{Context, JITTER_S, REPEAT_FOR_S};
        let b = Beacons::in_memory();
        let listening = Context {
            listening: true,
            ..Context::default()
        };
        // only while listening, the default: nothing until the station listens
        b.start_repeating(1000);
        assert_eq!(b.tick(1000, Context::default()), None);
        assert!(!b.wants(1000));
        b.tick(1001, listening);
        let first = b.due(1001, &[]).expect("the first at once");
        b.went(&first.reference, true, None, None, 1010);
        // the next an interval (15 min) after it went, and no more than a minute later
        let mut next = 1010;
        while !b.wants(next) {
            next += 1;
            b.tick(next, listening);
            assert!(next < 1010 + 15 * 60 + JITTER_S + 2, "never fell due");
        }
        assert!(next >= 1010 + 15 * 60, "{next}");
        // a session puts the callsign on the air: the beacon after waits an interval from it
        let due = b.due(next, &[]).expect("taken");
        b.went(&due.reference, true, None, None, next);
        b.tick(
            next + 300,
            Context {
                on_air: true,
                busy: true,
                ..listening
            },
        );
        let mut later = next + 300;
        while !b.wants(later) {
            later += 1;
            b.tick(later, listening);
        }
        assert!(later >= next + 300 + 15 * 60, "{later}");
        // one that does not go is said, and the next keeps to the interval
        let due = b.due(later, &[]).expect("taken");
        b.went(
            &due.reference,
            false,
            None,
            Some("the channel stayed busy"),
            later + 120,
        );
        assert!(!b.wants(later + 121));
        // four hours, then it stops and says so
        assert!(b.repeating());
        let said = b.tick(1000 + REPEAT_FOR_S, listening);
        assert_eq!(
            said.as_deref(),
            Some("beacon: repeating stopped after four hours; press Beacon to start four more")
        );
        assert!(!b.repeating());
        assert_eq!(b.json(1000 + REPEAT_FOR_S)["ended"], "after four hours");
    }

    #[test]
    fn a_scanning_station_beacons_on_each_dial_in_turn_and_none_beside_the_modems_own() {
        use super::Context;
        let b = Beacons::in_memory();
        b.set(Settings {
            every_min: 10,
            only_while_listening: false,
            position: Precision::None,
        })
        .expect("kept");
        b.start_repeating(0);
        let dials = [3_590_000, 7_101_000];
        let mut seen = Vec::new();
        let mut t = 0;
        while seen.len() < 3 {
            b.tick(t, Context::default());
            if let Some(due) = b.due(t, &dials) {
                assert_eq!(due.modem_position, Some(false), "None says nothing");
                seen.push(due.dial_hz);
                b.went(&due.reference, true, due.dial_hz, None, t);
            }
            t += 1;
        }
        assert_eq!(seen, [Some(3_590_000), Some(7_101_000), Some(3_590_000)]);
        // the modem repeats its own: the client sends none
        let quiet = Context {
            modem_repeats: true,
            ..Context::default()
        };
        for t in t..t + 2000 {
            b.tick(t, quiet);
            assert!(!b.wants(t));
        }
        // the rules refuse it: repeating stops
        assert_eq!(
            b.refused("under automatic control").as_deref(),
            Some("beacon: repeating stopped: under automatic control")
        );
        assert!(!b.repeating());
    }

    #[test]
    fn the_settings_are_kept_and_an_interval_not_offered_is_refused() {
        let dir = std::env::temp_dir().join(format!("rp-beacons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let b = Beacons::open(&dir);
        assert_eq!(b.settings(), Settings::default());
        let wanted = Settings {
            every_min: 30,
            only_while_listening: false,
            position: Precision::Grid6,
        };
        b.set(wanted.clone()).expect("kept");
        assert_eq!(Beacons::open(&dir).settings(), wanted);
        let refused = b.set(Settings {
            every_min: 5,
            ..wanted
        });
        assert_eq!(
            refused,
            Err("a beacon repeats every 10, 15, 30 or 60 minutes".into())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
