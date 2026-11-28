// SPDX-License-Identifier: MIT OR Apache-2.0
//! The stations the modem has heard (wishlist W59, the P2P design §3.4, RP-4 §4.4), kept on this
//! computer for **Call**'s list in P2P.
//!
//! * **By callsign and band.** A station heard on 80 m this morning and on 40 m now is two lines,
//!   so a band it was heard on is never mistaken for the band it is reachable on now. Each keeps
//!   its latest sighting and its last [`READINGS`] SNR readings.
//! * **From every connection.** The connection between calls ([`crate::presence`]), listening and
//!   calling ([`crate::session`]) hand each `heard` here, and each `stations` answer, which the
//!   client asks for whenever it connects: a station Aether heard overnight is listed in the
//!   morning. The same sighting from two connections is kept once.
//! * **Seven days** ([`KEEP_S`]), at most [`MOST`] lines; the computer's, in the first home, as
//!   circles are.
//! * **Written a while at a time** ([`SAVE_EVERY`]): a session's every frame is a sighting.
//! * **Sans clock**: every call takes `now`, Unix seconds.
//!
//! [`view`] is what the panel shows: one line a callsign, the sighting that says most about
//! calling it now, grouped by where it was heard against the radio's dial.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use rpmi::host::Sighting;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::store::write;

/// The file, in the first home.
pub const FILE: &str = "stations.json";
/// How long a sighting is kept.
pub const KEEP_S: u64 = 7 * 86_400;
/// The most lines kept, the oldest dropped first.
pub const MOST: usize = 2000;
/// The SNR readings kept a line.
pub const READINGS: usize = 8;
/// How often the file is written while sightings come.
pub const SAVE_EVERY: Duration = Duration::from_secs(5);
/// A sighting this recent is the list's plain, counted one (the design's §5.2).
pub const RECENT_S: u64 = 30 * 60;
/// A station heard within this of the radio's dial is on it: its signal was in the passband.
pub const SAME_DIAL_HZ: u64 = 1000;

/// One callsign on one band.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Seen {
    /// As heard, in capitals.
    pub callsign: String,
    /// The band, as `40m`; none when the dial was not known or is on no band.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub band: Option<String>,
    /// The radio's dial when it was last heard here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dial_hz: Option<u64>,
    /// When it was last heard, Unix seconds.
    pub at: u64,
    /// The last SNR, dB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snr_db: Option<f64>,
    /// The last readings, oldest first: `[at, snr_db]`.
    #[serde(default)]
    pub readings: Vec<(u64, f64)>,
    /// What was last heard: `beacon`, `call`, `answer`, `probe`, `session`, `datagram`, `id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Whom it was calling, answering or probing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Its last beacon said calls to it are answered now, or not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<bool>,
    /// When its beacon last said so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers_at: Option<u64>,
    /// The bandwidth it runs, Hz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<i64>,
    /// Where its last beacon with one said it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Value>,
    /// The modem that heard it, by its kind (`aether`, `mercury`, …): a station is heard only
    /// by a modem of its own kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modem: Option<String>,
    /// How many times it was heard here.
    #[serde(default)]
    pub count: u32,
}

#[derive(Serialize, Deserialize, Default)]
struct File {
    schema: u32,
    seen: Vec<Seen>,
}

struct Inner {
    seen: Vec<Seen>,
    dirty: bool,
    saved: Option<Instant>,
}

/// The stations heard, shared by every connection that holds the modem. Clones share them.
#[derive(Clone)]
pub struct Stations {
    path: Option<PathBuf>,
    inner: Arc<Mutex<Inner>>,
}

impl std::fmt::Debug for Stations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Stations")
    }
}

/// The amateur band a dial is on, by the bands' edges in hertz: `None` off every band.
#[must_use]
pub fn band_of(dial_hz: u64) -> Option<&'static str> {
    const BANDS: [(&str, u64, u64); 16] = [
        ("160m", 1_800_000, 2_000_000),
        ("80m", 3_500_000, 4_000_000),
        ("60m", 5_330_000, 5_410_000),
        ("40m", 7_000_000, 7_300_000),
        ("30m", 10_100_000, 10_150_000),
        ("20m", 14_000_000, 14_350_000),
        ("17m", 18_068_000, 18_168_000),
        ("15m", 21_000_000, 21_450_000),
        ("12m", 24_890_000, 24_990_000),
        ("10m", 28_000_000, 29_700_000),
        ("6m", 50_000_000, 54_000_000),
        ("2m", 144_000_000, 148_000_000),
        ("1.25m", 219_000_000, 225_000_000),
        ("70cm", 420_000_000, 450_000_000),
        ("33cm", 902_000_000, 928_000_000),
        ("23cm", 1_240_000_000, 1_300_000_000),
    ];
    BANDS
        .iter()
        .find(|(_, low, high)| (*low..=*high).contains(&dial_hz))
        .map(|(name, ..)| *name)
}

impl Stations {
    /// The stations kept under the first home `base`; none when the file is missing or unread.
    #[must_use]
    pub fn open(base: &Path) -> Stations {
        let path = base.join(FILE);
        let seen = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<File>(&t).ok())
            .filter(|f| f.schema == 1)
            .map(|f| f.seen)
            .unwrap_or_default();
        Stations::with(Some(path), seen)
    }

    /// Stations kept in memory only, for tests.
    #[must_use]
    pub fn in_memory() -> Stations {
        Stations::with(None, Vec::new())
    }

    fn with(path: Option<PathBuf>, seen: Vec<Seen>) -> Stations {
        Stations {
            path,
            inner: Arc::new(Mutex::new(Inner {
                seen,
                dirty: false,
                saved: None,
            })),
        }
    }

    /// A station heard, by the modem of kind `modem`; `now` dates a sighting that says no time.
    /// One older than the line it would update changes nothing (a `stations` answer read after a
    /// `heard`), and the same sighting handed twice counts once.
    pub fn heard(&self, s: &Sighting, modem: Option<&str>, now: u64) {
        let callsign = s.callsign.trim().to_ascii_uppercase();
        if callsign.is_empty() {
            return;
        }
        // a time ahead of this computer's clock is taken as now
        let at = s.at.unwrap_or(now).min(now);
        if now.saturating_sub(at) > KEEP_S {
            return;
        }
        let band = s.dial_hz.and_then(band_of).map(str::to_owned);
        let mut inner = self.lock();
        let found = inner
            .seen
            .iter()
            .position(|e| e.callsign == callsign && e.band == band);
        let index = if let Some(i) = found {
            let line = &inner.seen[i];
            let again = at == line.at && line.kind == s.kind && line.snr_db == s.snr_db;
            if at < line.at || again {
                return;
            }
            i
        } else {
            inner.seen.push(Seen {
                callsign: callsign.clone(),
                band: band.clone(),
                dial_hz: None,
                at,
                snr_db: None,
                readings: Vec::new(),
                kind: None,
                to: None,
                answers: None,
                answers_at: None,
                bandwidth: None,
                position: None,
                modem: None,
                count: 0,
            });
            inner.seen.len() - 1
        };
        let line = &mut inner.seen[index];
        line.at = at;
        line.count = line.count.saturating_add(1);
        line.dial_hz = s.dial_hz.or(line.dial_hz);
        line.kind.clone_from(&s.kind);
        line.to.clone_from(&s.to);
        if let Some(snr) = s.snr_db {
            line.snr_db = Some(snr);
            line.readings.push((at, snr));
            if line.readings.len() > READINGS {
                line.readings.remove(0);
            }
        }
        // a session's frames say nothing of the bandwidth or the beacon's word: keep the last
        if let Some(bw) = s.bandwidth {
            line.bandwidth = Some(bw);
        }
        if let Some(answers) = s.answers {
            line.answers = Some(answers);
            line.answers_at = Some(at);
        }
        if let Some(p) = &s.position {
            line.position = Some(p.to_json());
        }
        if let Some(m) = modem {
            line.modem = Some(m.to_owned());
        }
        inner.dirty = true;
        prune(&mut inner.seen, now);
        drop(inner);
        self.save_if_due();
    }

    /// Every line kept, the most recently heard first.
    #[must_use]
    pub fn all(&self) -> Vec<Seen> {
        let mut all = self.lock().seen.clone();
        all.sort_by_key(|e| std::cmp::Reverse(e.at));
        all
    }

    /// Forget them all.
    pub fn clear(&self) {
        let mut inner = self.lock();
        inner.seen.clear();
        inner.dirty = true;
        drop(inner);
        self.save();
    }

    /// Writes the file when something changed and [`SAVE_EVERY`] has passed since the last write.
    pub fn save_if_due(&self) {
        let due = {
            let inner = self.lock();
            inner.dirty && inner.saved.is_none_or(|t| t.elapsed() >= SAVE_EVERY)
        };
        if due {
            self.save();
        }
    }

    /// Writes the file now, when something changed.
    pub fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let mut inner = self.lock();
        if !inner.dirty {
            return;
        }
        let file = File {
            schema: 1,
            seen: inner.seen.clone(),
        };
        if let Ok(bytes) = serde_json::to_vec_pretty(&file)
            && write(path, &bytes).is_ok()
        {
            inner.dirty = false;
            inner.saved = Some(Instant::now());
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Sightings older than [`KEEP_S`] go, and past [`MOST`] the oldest.
fn prune(seen: &mut Vec<Seen>, now: u64) {
    seen.retain(|e| now.saturating_sub(e.at) <= KEEP_S);
    if seen.len() > MOST {
        seen.sort_by_key(|e| std::cmp::Reverse(e.at));
        seen.truncate(MOST);
    }
}

/// What the view is worked out against.
#[derive(Debug, Default)]
pub struct Context {
    /// The radio's dial, as the modem last said it; else the dial typed for P2P.
    pub radio_hz: Option<u64>,
    /// Where this station is, for distance and bearing.
    pub here: Option<(f64, f64)>,
    /// Callsigns left out: the station's own, and the listed gateways' (D47).
    pub leave_out: HashSet<String>,
    /// The last P2P session that came up with each callsign, Unix seconds.
    pub worked: HashMap<String, u64>,
    /// The modem in the Modem box, by its kind.
    pub modem: Option<String>,
}

/// Where a station was heard, against the radio's dial: the groups of **Call**'s list.
fn place(seen: &Seen, radio_hz: Option<u64>) -> &'static str {
    let Some(radio) = radio_hz else {
        return "unknown";
    };
    match seen.dial_hz {
        Some(d) if d.abs_diff(radio) <= SAME_DIAL_HZ => "here",
        Some(_) if seen.band.is_some() && seen.band.as_deref() == band_of(radio) => "band",
        Some(_) => "other",
        None => "unknown",
    }
}

/// The order of the groups.
fn rank(place: &str) -> u8 {
    match place {
        "here" => 0,
        "band" => 1,
        "other" => 2,
        _ => 3,
    }
}

/// **Call**'s list in P2P: one line a callsign, the sighting that says most about calling it
/// now (the one on the radio's dial, else its band, else the latest), grouped by where it was
/// heard and the most recently heard first in each group. `recent` counts the stations heard in
/// the last [`RECENT_S`].
#[must_use]
pub fn view(all: &[Seen], ctx: &Context, now: u64) -> Value {
    let mut best: HashMap<&str, &Seen> = HashMap::new();
    for seen in all {
        if ctx.leave_out.contains(&seen.callsign) {
            continue;
        }
        let better = match best.get(seen.callsign.as_str()) {
            None => true,
            Some(kept) => {
                let (a, b) = (
                    rank(place(seen, ctx.radio_hz)),
                    rank(place(kept, ctx.radio_hz)),
                );
                a < b || (a == b && seen.at > kept.at)
            }
        };
        if better {
            best.insert(&seen.callsign, seen);
        }
    }
    let mut lines: Vec<(&str, &Seen)> = best
        .into_values()
        .map(|seen| (place(seen, ctx.radio_hz), seen))
        .collect();
    lines.sort_by(|(pa, a), (pb, b)| rank(pa).cmp(&rank(pb)).then(b.at.cmp(&a.at)));
    let recent = lines
        .iter()
        .filter(|(_, s)| now.saturating_sub(s.at) <= RECENT_S)
        .count();
    let stations: Vec<Value> = lines
        .iter()
        .map(|(place, seen)| line_of(seen, place, ctx, all, now))
        .collect();
    json!({
        "radio_hz": ctx.radio_hz,
        "band": ctx.radio_hz.and_then(band_of),
        "recent": recent,
        "stations": stations,
    })
}

fn line_of(seen: &Seen, place: &str, ctx: &Context, all: &[Seen], now: u64) -> Value {
    let mut v = serde_json::to_value(seen).unwrap_or(Value::Null);
    v["where"] = place.into();
    v["age_s"] = now.saturating_sub(seen.at).into();
    // where else it was heard, the latest first
    let mut elsewhere: Vec<&Seen> = all
        .iter()
        .filter(|e| e.callsign == seen.callsign && e.band != seen.band)
        .collect();
    elsewhere.sort_by_key(|e| std::cmp::Reverse(e.at));
    v["elsewhere"] = elsewhere
        .iter()
        .map(|e| json!({"band": e.band, "dial_hz": e.dial_hz, "at": e.at, "snr_db": e.snr_db}))
        .collect::<Vec<_>>()
        .into();
    if let Some(at) = ctx.worked.get(&seen.callsign) {
        v["worked_at"] = (*at).into();
    }
    // a station heard by another modem than the one in the Modem box answers none of its calls
    if let (Some(heard_by), Some(mine)) = (&seen.modem, &ctx.modem)
        && heard_by != mine
    {
        v["other_modem"] = true.into();
    }
    if let (Some(here), Some(p)) = (
        ctx.here,
        seen.position
            .as_ref()
            .and_then(rpmi::position::Position::from_json),
    ) {
        let there = (p.lat, p.lon);
        v["km"] = crate::grid::km_between(here, there).round().into();
        v["bearing"] = crate::grid::bearing(here, there).round().into();
    }
    v
}

#[cfg(test)]
mod tests {
    use super::{Context, Stations, band_of, view};
    use rpmi::host::Sighting;
    use serde_json::json;

    const NOW: u64 = 1_795_213_320;

    fn heard(callsign: &str, dial_hz: u64, at: u64, snr: f64) -> Sighting {
        Sighting {
            snr_db: Some(snr),
            dial_hz: Some(dial_hz),
            at: Some(at),
            kind: Some("beacon".into()),
            ..Sighting::of(callsign)
        }
    }

    #[test]
    fn bands_are_found_by_their_edges() {
        assert_eq!(band_of(7_101_000), Some("40m"));
        assert_eq!(band_of(3_591_500), Some("80m"));
        assert_eq!(band_of(14_107_000), Some("20m"));
        assert_eq!(band_of(146_520_000), Some("2m"));
        assert_eq!(band_of(9_000_000), None);
    }

    #[test]
    fn a_station_is_a_line_a_band_and_the_same_sighting_counts_once() {
        let s = Stations::in_memory();
        s.heard(
            &heard("n2call", 3_590_000, NOW - 7200, -2.0),
            Some("aether"),
            NOW,
        );
        s.heard(
            &heard("N2CALL", 7_101_000, NOW - 60, 8.0),
            Some("aether"),
            NOW,
        );
        s.heard(
            &heard("N2CALL", 7_101_000, NOW - 60, 8.0),
            Some("aether"),
            NOW,
        );
        let all = s.all();
        assert_eq!(all.len(), 2, "two bands");
        assert_eq!(all[0].band.as_deref(), Some("40m"));
        assert_eq!(all[0].count, 1, "handed twice, counted once");
        // an older sighting changes nothing
        s.heard(
            &heard("N2CALL", 7_101_000, NOW - 600, 1.0),
            Some("aether"),
            NOW,
        );
        assert_eq!(s.all()[0].snr_db, Some(8.0));
        // readings keep the last eight
        for i in 0..10 {
            s.heard(
                &heard(
                    "N2CALL",
                    7_101_000,
                    NOW - 50 + i,
                    f64::from(u32::try_from(i).unwrap_or(0)),
                ),
                None,
                NOW,
            );
        }
        assert_eq!(s.all()[0].readings.len(), 8);
        // a week and more is forgotten
        s.heard(
            &heard("N9CALL", 7_101_000, NOW - 8 * 86_400, 0.0),
            None,
            NOW,
        );
        assert!(s.all().iter().all(|e| e.callsign != "N9CALL"));
    }

    #[test]
    fn the_view_groups_by_where_and_leaves_out_gateways_and_its_own() {
        let s = Stations::in_memory();
        s.heard(
            &heard("N2CALL", 7_101_000, NOW - 120, 8.0),
            Some("aether"),
            NOW,
        );
        s.heard(
            &heard("N4CALL", 7_083_000, NOW - 1500, 6.0),
            Some("aether"),
            NOW,
        );
        s.heard(
            &heard("N7CALL", 3_591_500, NOW - 3600, -2.0),
            Some("aether"),
            NOW,
        );
        s.heard(
            &heard("N1CALL-10", 7_101_000, NOW - 30, 3.0),
            Some("aether"),
            NOW,
        );
        s.heard(
            &heard("N0CALL", 7_101_000, NOW - 30, 3.0),
            Some("aether"),
            NOW,
        );
        // N2CALL heard on 80 m later: the 40 m line still says most about calling on 40 m
        s.heard(
            &heard("N2CALL", 3_591_500, NOW - 60, 1.0),
            Some("mercury"),
            NOW,
        );
        let mut sighting = heard("N6CALL", 7_101_500, NOW - 360, 14.0);
        sighting.position = rpmi::position::Position::from_json(
            &json!({"lat": 34.4621, "lon": -83.7514, "precision": "exact"}),
        );
        sighting.answers = Some(true);
        s.heard(&sighting, Some("aether"), NOW);
        let ctx = Context {
            radio_hz: Some(7_101_000),
            here: Some((33.78123, -84.38765)),
            leave_out: ["N1CALL-10".to_owned(), "N0CALL".to_owned()].into(),
            worked: [("N6CALL".to_owned(), NOW - 86_400)].into(),
            modem: Some("aether".into()),
        };
        let v = view(&s.all(), &ctx, NOW);
        let order: Vec<(&str, &str)> = v["stations"]
            .as_array()
            .expect("stations")
            .iter()
            .map(|l| {
                (
                    l["callsign"].as_str().unwrap_or(""),
                    l["where"].as_str().unwrap_or(""),
                )
            })
            .collect();
        assert_eq!(
            order,
            [
                ("N2CALL", "here"),
                ("N6CALL", "here"),
                ("N4CALL", "band"),
                ("N7CALL", "other")
            ]
        );
        assert_eq!(v["recent"], 3, "N7CALL an hour ago is not recent");
        assert_eq!(v["band"], "40m");
        let n2 = &v["stations"][0];
        assert_eq!(n2["elsewhere"][0]["band"], "80m");
        assert!(n2.get("other_modem").is_none());
        let n6 = &v["stations"][1];
        assert_eq!(n6["worked_at"], NOW - 86_400);
        assert_eq!(n6["answers"], true);
        assert!(
            n6["km"]
                .as_f64()
                .is_some_and(|k| (90.0..105.0).contains(&k)),
            "{n6}"
        );
        assert!(
            n6["bearing"]
                .as_f64()
                .is_some_and(|b| (30.0..50.0).contains(&b)),
            "{n6}"
        );
        // with no dial, one group
        let v = view(&s.all(), &Context::default(), NOW);
        assert!(
            v["stations"]
                .as_array()
                .expect("stations")
                .iter()
                .all(|l| l["where"] == "unknown")
        );
    }

    #[test]
    fn the_file_is_written_and_read_back() {
        let dir = std::env::temp_dir().join(format!("radiopost-stations-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        let s = Stations::open(&dir);
        s.heard(
            &heard("N2CALL", 7_101_000, NOW - 60, 8.0),
            Some("aether"),
            NOW,
        );
        s.save();
        let again = Stations::open(&dir);
        assert_eq!(again.all(), s.all());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
