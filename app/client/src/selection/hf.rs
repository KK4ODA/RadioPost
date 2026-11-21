// SPDX-License-Identifier: MIT OR Apache-2.0
//! HF channels (choosing a gateway, § 10): VOACAP both ways for each gateway's HF channels, run in
//! one batch and kept, and read through the modem's profile into the chance of a link at the hour
//! and the rate each way once linked.
//!
//! Nothing is learned yet (§ 12): the SNR offset δ is 0, the modem's efficiency η is 1, and the
//! chance is VOACAP's own spread of the month's days. Phase 4 corrects both from this station's
//! sessions; until then the panel says so.
//!
//! Each way of a path is its own engine run, with its own transmitter's power and antenna and its
//! own receiver's antenna and noise (§ 10.2): VOACAP's loss is not quite reciprocal either (up to
//! 7 dB at the edge of a band's opening, measured on hf-predict's engine). A run covers the
//! month's 24 hours, so the hour is chosen when it is read.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use radioprop_geo::LatLon;
use radioprop_hf::predictor::{self, BothWays, PathRequest};
use radioprop_hf::propagation::{FrequencyPrediction, PropagationEngine};
use radioprop_hf::station::{self as hf_station, Mode, StationProfile};
use radioprop_hf::timeutil;
use radioprop_hf::voacap::VoacaplEngine;
use radioprop_place::station::{AntennaKind, DIPOLE_DBI, HF_FEEDLINE_DB, Noise, Resolved, Said};
use radioprop_terrain::budget::phi;

use super::evidence::{self, Evidence};
use super::learning::{ForLine, Learning, Offset};
use serde::Deserialize;
use serde_json::{Value, json};

/// From VOACAP's SNR in 1 Hz to the modem profile's, in 3 kHz: 10·log10(3000).
pub const IN_3_KHZ_DB: f64 = 34.771_212_547_196_626;
/// A decile lies this many standard deviations from the median of a normal distribution.
const DECILE_SIGMAS: f64 = 1.281_551_565_544_6;
/// Under this, ground wave may reach a gateway too, which VOACAP does not model.
pub const GROUND_WAVE_KM: f64 = 30.0;
/// Near-vertical incidence (§ 10.3): a path under 500 km whose take-off angle is above 60°.
const NVIS_KM: f64 = 500.0;
const NVIS_ANGLE_DEG: f64 = 60.0;
/// A path that reaches this latitude, north or south, is a high-latitude one (§ 10.3).
const HIGH_LATITUDE_DEG: f64 = 60.0;
/// VOACAP's probability of multipath at or above which multipath is taken as likely.
const MULTIPATH_LIKELY: f64 = 0.5;
/// Where the modem's audio is centred above the dial, Hz: the frequency the engine is asked about.
const AUDIO_CENTRE_HZ: u64 = 1500;
/// The most frequencies one engine run takes.
const RUN_FREQUENCIES: usize = 11;
/// A gateway's station where its record says nothing: hf-predict's "Fixed gateway (RMS)".
const GATEWAY_POWER_W: f64 = 100.0;
const GATEWAY_AGL_M: f64 = 10.0;
/// hf-predict's antennas: a half-wave dipole 10 m and 5 m up, a quarter-wave vertical, a mobile
/// whip. A dipole lower than `LOW_DIPOLE_M` is taken as the 5 m one.
const DIPOLE_10M: &str = "hfp/dipole10.voa";
const DIPOLE_5M: &str = "hfp/dipole05.voa";
const VERTICAL: &str = "hfp/vertical.voa";
const WHIP: &str = "hfp/whip.voa";
const LOW_DIPOLE_M: f64 = 7.5;
/// Modelled antennas include the ground, so VOACAP may use any take-off angle (hf-predict's own).
const MIN_ANGLE_DEG: f64 = 0.1;
/// The required SNR VOACAP is told, dB-Hz: Aether's call on a clean channel. Nothing read here
/// depends on it (VOACAP's `REL` and `SNRxx` do; the median and its deciles do not).
const TOLD_REQUIRED_DB_HZ: f64 = 15.3;
/// Predictions kept; past this the store starts again (a month's places and gateways need far
/// fewer).
const KEEP: usize = 2000;

/// A modem's profile (§ 10.3): for each of its airs, each rung's net rate and the SNR its frames
/// need on each channel class, and what a call and its answer need. Aether's is `aether.json`,
/// written by `tools/modems/aether_profile.py` from Aether's own measured curves.
#[derive(Debug, Deserialize)]
pub struct Profile {
    /// The modem's name in RP-0 §9.4's registry.
    pub modem: String,
    control_db: Thresholds,
    airs: BTreeMap<String, Vec<Rung>>,
}

/// SNR in 3 kHz, dB, on ITU-R F.1487's Good, Moderate and Poor channels.
#[derive(Clone, Copy, Debug, Deserialize)]
struct Thresholds {
    good: f64,
    moderate: f64,
    poor: f64,
}

impl Thresholds {
    fn of(self, class: Class) -> f64 {
        match class {
            Class::Good => self.good,
            Class::Moderate => self.moderate,
            Class::Poor => self.poor,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Rung {
    name: String,
    net_bps: f64,
    threshold_db: Thresholds,
    /// The classes whose curve never fell to one frame in ten in the range measured: there the
    /// threshold is only a lower bound.
    #[serde(default)]
    beyond: Vec<String>,
}

/// A step of g(SNR): from `threshold` up, the modem carries `bps` with the rung `name`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Step<'a> {
    /// SNR in 3 kHz, dB, where the rung's frames fail one time in ten.
    pub threshold: f64,
    /// The rung's net rate, bit/s.
    pub bps: f64,
    /// The rung's name, as the modem's specification gives it.
    pub name: &'a str,
}

impl Profile {
    /// Aether's profile.
    ///
    /// # Panics
    ///
    /// Never: the file is compiled in and a test reads it.
    #[must_use]
    pub fn aether() -> &'static Profile {
        static PROFILE: OnceLock<Profile> = OnceLock::new();
        PROFILE.get_or_init(|| {
            serde_json::from_str(include_str!("aether.json")).expect("aether.json reads")
        })
    }

    /// The profile of a channel's `modem` (RP-0 §9.4), if one is held.
    #[must_use]
    pub fn of_modem(modem: &str) -> Option<&'static Profile> {
        (modem == "aether").then(Profile::aether)
    }

    /// What a call and its answer need on `class`, dB in 3 kHz.
    #[must_use]
    pub fn call_db(&self, class: Class) -> f64 {
        self.control_db.of(class)
    }

    /// The steps of g(SNR) on a channel `bandwidth_hz` wide, on `class` (§ 10.3): each the
    /// fastest rung from its threshold up, thresholds rising, each rate above every one before.
    /// Aether's 500 Hz air serves 500 Hz channels; its 2300 Hz air those 2300 Hz and wider (a
    /// 2750 Hz channel runs the 2300 Hz ladder). A rung whose threshold on `class` is only a lower
    /// bound is left out: its frames were never seen to get through there.
    #[must_use]
    pub fn steps(&self, bandwidth_hz: u64, class: Class) -> Vec<Step<'_>> {
        let air = match bandwidth_hz {
            2300.. => "2300",
            500..2300 => "500",
            _ => return Vec::new(),
        };
        let mut usable: Vec<&Rung> = self
            .airs
            .get(air)
            .into_iter()
            .flatten()
            .filter(|r| !r.beyond.iter().any(|b| b == class.name()))
            .collect();
        usable.sort_by(|a, b| {
            let (ta, tb) = (a.threshold_db.of(class), b.threshold_db.of(class));
            ta.total_cmp(&tb).then(b.net_bps.total_cmp(&a.net_bps))
        });
        let mut steps: Vec<Step<'_>> = Vec::new();
        for r in usable {
            if steps.last().is_none_or(|s| r.net_bps > s.bps) {
                steps.push(Step {
                    threshold: r.threshold_db.of(class),
                    bps: r.net_bps,
                    name: &r.name,
                });
            }
        }
        steps
    }
}

/// A channel class (§ 10.3): which of the modem's thresholds apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// ITU-R F.1487's Good: little spread in delay or Doppler (ground wave).
    Good,
    /// Moderate: one sky-wave hop at mid latitude.
    Moderate,
    /// Poor: near-vertical, several hops, high latitude or multipath likely.
    Poor,
}

impl Class {
    /// The class a record names (`good`, `moderate`, `poor`).
    #[must_use]
    pub fn from_name(name: &str) -> Option<Class> {
        [Class::Good, Class::Moderate, Class::Poor]
            .into_iter()
            .find(|c| c.name() == name)
    }

    /// Its name, as the profile and the panel write it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Class::Good => "good",
            Class::Moderate => "moderate",
            Class::Poor => "poor",
        }
    }
}

/// The SNR's spread over the month's days at an hour, dB in 3 kHz: VOACAP's median and the
/// distances to its lower and upper deciles, taken as a split normal (§ 10.3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spread {
    /// The median day's SNR, dB in 3 kHz.
    pub median: f64,
    /// How far below the median one day in ten falls, dB.
    pub lower: f64,
    /// How far above the median one day in ten rises, dB.
    pub upper: f64,
}

impl Spread {
    /// VOACAP's figures for one frequency, moved by `adjust_db` (gains it did not model, less
    /// the transmitter's feed line).
    #[must_use]
    pub fn of(f: &FrequencyPrediction, adjust_db: f64) -> Spread {
        Spread {
            median: f.snr_db - IN_3_KHZ_DB + adjust_db,
            lower: f.snr_lower_decile_db,
            upper: f.snr_upper_decile_db,
        }
    }

    /// The share of days the SNR reaches `x`: one normal below the median with the lower decile's
    /// σ, another above with the upper's, each holding half the days.
    #[must_use]
    pub fn above(self, x: f64) -> f64 {
        let decile = if x < self.median {
            self.lower
        } else {
            self.upper
        };
        let sigma = (decile / DECILE_SIGMAS).max(0.1);
        1.0 - phi((x - self.median) / sigma)
    }
}

/// The mean rate on the days the SNR reaches `call` (the days a link is made): g(SNR) averaged
/// over them. `None` when no such day is to be expected.
#[must_use]
pub fn linked_rate(s: Spread, steps: &[Step<'_>], call: f64) -> Option<f64> {
    let linked = s.above(call);
    if linked < 1e-6 {
        return None;
    }
    let mut sum = 0.0;
    for (i, step) in steps.iter().enumerate() {
        let from = step.threshold.max(call);
        let to = steps.get(i + 1).map_or(f64::INFINITY, |n| n.threshold);
        if to > from {
            let beyond_to = if to.is_finite() { s.above(to) } else { 0.0 };
            sum += step.bps * (s.above(from) - beyond_to);
        }
    }
    Some(sum / linked)
}

/// The rung a median day carries.
#[must_use]
pub fn median_rung<'a>(s: Spread, steps: &[Step<'a>]) -> Option<Step<'a>> {
    steps
        .iter()
        .rev()
        .find(|st| st.threshold <= s.median)
        .copied()
}

/// The channel class of a path (§ 10.3), from VOACAP's mode, take-off angle and multipath
/// probability at the hour, the path's length, and the latitude it reaches nearest a pole; with
/// the reason in words. Ground wave is not modelled, so no sky-wave path is Good.
#[must_use]
pub fn class_of(distance_km: f64, f: &FrequencyPrediction, reach_deg: f64) -> (Class, String) {
    let mode: String = f.mode.split_whitespace().collect();
    if distance_km < NVIS_KM && f.takeoff_angle_deg > NVIS_ANGLE_DEG {
        return (Class::Poor, format!("near-vertical ({mode})"));
    }
    match mode.chars().next().and_then(|c| c.to_digit(10)) {
        Some(hops) if hops >= 2 => return (Class::Poor, format!("{hops} hops ({mode})")),
        None => return (Class::Poor, format!("long, several hops ({mode})")),
        Some(_) => {}
    }
    if reach_deg >= HIGH_LATITUDE_DEG {
        return (Class::Poor, format!("one hop, high latitude ({mode})"));
    }
    if f.multipath_probability >= MULTIPATH_LIKELY {
        return (Class::Poor, format!("one hop, multipath likely ({mode})"));
    }
    (Class::Moderate, format!("one hop ({mode})"))
}

/// How far from the equator the short great circle between two points goes, degrees.
#[must_use]
pub fn reach_deg(a: LatLon, b: LatLon) -> f64 {
    (0..=16)
        .map(|i| {
            radioprop_geo::intermediate(a, b, f64::from(i) / 16.0)
                .lat
                .abs()
        })
        .fold(0.0, f64::max)
}

/// The VOACAP antenna for a kind (RP-0 §9.6) at a height, and whether it is the kind's own: a
/// kind with no file of its own (a Yagi, a loop, …) runs as a dipole at its height, its gain over
/// a dipole's added (`gain_db` in [`End`]).
fn antenna_file(kind: &str, agl_m: f64) -> (&'static str, bool) {
    let dipole = if agl_m < LOW_DIPOLE_M {
        DIPOLE_5M
    } else {
        DIPOLE_10M
    };
    match kind {
        "vertical" => (VERTICAL, true),
        "whip" => (WHIP, true),
        "nvis" => (DIPOLE_5M, true),
        "dipole" => (dipole, true),
        _ => (dipole, false),
    }
}

fn kind_name(kind: AntennaKind) -> &'static str {
    match kind {
        AntennaKind::Vertical => "vertical",
        AntennaKind::Dipole => "dipole",
        AntennaKind::Yagi => "yagi",
        AntennaKind::JPole => "j-pole",
        AntennaKind::Collinear => "collinear",
        AntennaKind::Loop => "loop",
        AntennaKind::Nvis => "nvis",
        AntennaKind::Whip => "whip",
        AntennaKind::Other => "other",
    }
}

/// One end of a path as the engine takes it, and what the engine leaves out: the antenna's gain
/// over the file it runs as, and the feed line, which costs on transmit (on receive HF's noise
/// comes in with the signal, so a few dB of feed line hardly moves the SNR).
#[derive(Clone, Debug)]
struct End {
    station: StationProfile,
    gain_db: f64,
    feed_db: f64,
    assumed: Vec<String>,
}

/// VOACAP's man-made noise figure for an environment: dB below 1 W in 1 Hz at 3 MHz (ITU-R P.372).
fn noise_db(noise: Noise) -> f64 {
    -noise.dbw_per_hz(3.0)
}

fn noise_words(noise: Noise) -> &'static str {
    match noise {
        Noise::Industrial => "industrial",
        Noise::Residential => "residential",
        Noise::Rural => "rural",
        Noise::Remote => "remote",
    }
}

/// The operator's end, from the station profile in use.
fn yours(st: &Resolved) -> End {
    let hf = st.hf;
    let kind = kind_name(hf.antenna.value);
    let (file, own) = antenna_file(kind, hf.agl_m.value);
    let mut assumed = Vec::new();
    if hf.power_w.said == Said::Assumed {
        assumed.push(format!("your power {:.0} W", hf.power_w.value));
    }
    if hf.antenna.said == Said::Assumed || hf.agl_m.said == Said::Assumed {
        assumed.push(format!("your antenna a {kind} {:.0} m up", hf.agl_m.value));
    }
    if !own && hf.gain_dbi.said == Said::Assumed {
        assumed.push(format!("your antenna's gain {:.1} dBi", hf.gain_dbi.value));
    }
    if hf.feedline_db.said == Said::Assumed {
        assumed.push(format!("your feed line {:.0} dB", hf.feedline_db.value));
    }
    if st.noise.said == Said::Assumed {
        assumed.push(format!("your noise {}", noise_words(st.noise.value)));
    }
    End {
        station: StationProfile {
            name: "you".into(),
            power_watts: hf.power_w.value,
            antenna: file.into(),
            noise_db: noise_db(st.noise.value),
            min_angle_deg: MIN_ANGLE_DEG,
        },
        gain_db: if own {
            0.0
        } else {
            hf.gain_dbi.value - DIPOLE_DBI
        },
        feed_db: hf.feedline_db.value,
        assumed,
    }
}

/// The gateway's end on a channel, from its record (RP-5 §4.4: the channel's `station`, the
/// site's height) and what it does not say assumed.
fn theirs(channel: &Value, site_agl: Option<f64>) -> End {
    let st = radiopost::gateway_list::station(channel);
    let mut assumed = Vec::new();
    let power = st.power_w.unwrap_or_else(|| {
        assumed.push(format!("its power {GATEWAY_POWER_W:.0} W"));
        GATEWAY_POWER_W
    });
    let kind = st.antenna.as_deref().unwrap_or("dipole");
    let agl = st.agl_m.or(site_agl);
    if st.antenna.is_none() || agl.is_none() {
        assumed.push(format!(
            "its antenna a {kind} {:.0} m up",
            agl.unwrap_or(GATEWAY_AGL_M)
        ));
    }
    let (file, own) = antenna_file(kind, agl.unwrap_or(GATEWAY_AGL_M));
    let gain_db = if own {
        0.0
    } else {
        st.gain_dbi.map_or(0.0, |g| g - DIPOLE_DBI)
    };
    if !own && st.gain_dbi.is_none() {
        assumed.push(format!("its antenna's gain {DIPOLE_DBI} dBi"));
    }
    assumed.push(format!("its feed line {HF_FEEDLINE_DB:.0} dB"));
    assumed.push("its noise residential".into());
    End {
        station: StationProfile {
            name: "gateway".into(),
            power_watts: power,
            antenna: file.into(),
            noise_db: noise_db(Noise::Residential),
            min_angle_deg: MIN_ANGLE_DEG,
        },
        gain_db,
        feed_db: HF_FEEDLINE_DB,
        assumed,
    }
}

/// The VOACAP engine's folder (`bin/voacapl` and `itshfbc`, as hf-predict's
/// `engines/voacapl/build.sh` makes it): `RADIOPOST_VOACAP` when set; else `bundled`, the folder
/// the desktop application's installer put it in, when it holds one; else `engine` beside the
/// program, or in its bundle's `Resources` on macOS.
///
/// # Errors
///
/// No folder holds the engine: said in words.
pub fn engine_dir(bundled: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("RADIOPOST_VOACAP") {
        let dir = PathBuf::from(dir);
        radioprop_hf::engine::Engine::at(&dir)?;
        return Ok(dir);
    }
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
    let beside = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let candidates = [
        bundled.map(Path::to_path_buf),
        Some(beside.join("engine")),
        Some(beside.join("..").join("Resources").join("engine")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|dir| radioprop_hf::engine::Engine::at(dir).is_ok())
        .ok_or_else(|| "the VOACAP engine is not installed with this program".into())
}

/// One engine run planned: a gateway's HF channels (up to eleven) both ways.
struct Planned {
    gateway: usize,
    channels: Vec<usize>,
    key: String,
    request: PathRequest,
    you: End,
    them: End,
    to: LatLon,
}

/// A gateway's channels with one station at its end, gathered for one run.
struct Run {
    key: String,
    them: End,
    channels: Vec<(usize, f64)>,
}

/// A kept prediction: both ways, or why the engine could not give them.
type Kept = Arc<Result<BothWays, String>>;

/// HF predictions for the gateway list: the engine, and the runs kept (by place, month, gateway,
/// frequencies and both stations, § 10.2).
pub struct Hf {
    engine: Result<Box<dyn PropagationEngine + Send + Sync>, String>,
    kept: Mutex<HashMap<String, Kept>>,
    workers: usize,
    /// hf-predict's evidence API (§ 11.4).
    evidence: Evidence,
}

impl Default for Hf {
    fn default() -> Self {
        Self::new()
    }
}

impl Hf {
    /// With the engine found by [`engine_dir`], its runs in the computer's temporary folder.
    #[must_use]
    pub fn new() -> Self {
        Self::bundled(None)
    }

    /// As [`Hf::new`], the desktop application's own copy of the engine, `bundled`, preferred to
    /// one beside the program ([`engine_dir`]).
    #[must_use]
    pub fn bundled(bundled: Option<&Path>) -> Self {
        let runs = std::env::temp_dir().join("radiopost-voacap");
        let engine = engine_dir(bundled).and_then(|dir| {
            VoacaplEngine::new(&dir, &runs)
                .map(|e| Box::new(e) as Box<dyn PropagationEngine + Send + Sync>)
        });
        Self::with(engine).with_evidence(Evidence::new())
    }

    /// The same, asking `evidence` what was heard: tests give a stand-in for hf-predict.
    #[must_use]
    pub fn with_evidence(mut self, evidence: Evidence) -> Self {
        self.evidence = evidence;
        self
    }

    /// With this engine: tests give one that makes its predictions up.
    #[must_use]
    pub fn with_engine(engine: Box<dyn PropagationEngine + Send + Sync>) -> Self {
        Self::with(Ok(engine))
    }

    /// With no engine: every HF channel says why it is not predicted.
    #[must_use]
    pub fn without(why: &str) -> Self {
        Self::with(Err(why.to_owned()))
    }

    fn with(engine: Result<Box<dyn PropagationEngine + Send + Sync>, String>) -> Self {
        Self {
            engine,
            kept: Mutex::new(HashMap::new()),
            workers: std::thread::available_parallelism().map_or(2, usize::from),
            evidence: Evidence::off(),
        }
    }

    /// Whether the engine is there, how many predictions are kept, and whether hf-predict
    /// answered when last asked what was heard (`null`, not asked).
    #[must_use]
    pub fn status(&self) -> Value {
        let kept = self.kept.lock().map_or(0, |k| k.len());
        let heard = self.evidence.answered();
        match &self.engine {
            Ok(e) => json!({"engine": e.name(), "kept": kept, "heard": heard}),
            Err(why) => json!({"engine": Value::Null, "why": why, "kept": kept, "heard": heard}),
        }
    }

    /// Puts an `hf` member on each HF channel of `gateways` (USB, under 30 MHz, not blocked): its
    /// prediction at the whole hour nearest `now` (seconds since 1970), from `here` with the
    /// operator's `station`, or why there is none. `bands`, the station profile's (empty, all):
    /// a channel on another band is not predicted. Runs the engine once for every prediction
    /// not kept, all in one batch. `today` when `now` is the present: what this station heard in
    /// the last hour counts then (§ 10.5), never at another time.
    pub fn judge(
        &self,
        here: Option<LatLon>,
        station: &Resolved,
        bands: &[String],
        gateways: &mut [Value],
        (now, today): (i64, bool),
        learning: Option<&Learning>,
    ) {
        let (year, month, _, hour) = timeutil::civil(now + 1800);
        let at = u64::try_from(now).unwrap_or(0);
        let plans = self.plan(here, station, bands, gateways, year, month);
        self.run_missing(&plans);
        let heard = if today {
            self.heard(here, &plans, now)
        } else {
            None
        };
        let kept = self.kept.lock().map(|k| k.clone()).unwrap_or_default();
        for (p, plan) in plans.iter().enumerate() {
            let Some(result) = kept.get(&plan.key) else {
                continue;
            };
            let toward = heard.as_ref().and_then(|h| h.get(p));
            let callsign = gateways[plan.gateway]["callsign"]
                .as_str()
                .unwrap_or("")
                .to_owned();
            let learned = |band: &str| learning.and_then(|l| l.for_line(&callsign, band, at));
            for (i, &c) in plan.channels.iter().enumerate() {
                let channel = &gateways[plan.gateway]["channels"][c];
                let hf = match result.as_ref() {
                    Ok(both) => read(plan, both, channel, (i, hour), toward, &learned),
                    Err(why) => json!({"state": "failed", "why": why,
                                       "words": "HF not predicted: the engine failed"}),
                };
                gateways[plan.gateway]["channels"][c]["hf"] = hf;
            }
        }
    }

    /// What this station heard in the last hour toward each planned run's gateway, from
    /// hf-predict when it runs (§ 11.4), by the run's index.
    fn heard(&self, here: Option<LatLon>, plans: &[Planned], now: i64) -> Option<evidence::Heard> {
        let places: Vec<LatLon> = plans.iter().map(|p| p.to).collect();
        let mut bands: Vec<String> = plans
            .iter()
            .filter_map(|p| p.request.frequencies_mhz.as_ref())
            .flatten()
            .filter_map(|&f| hf_station::hf_band_at(f).map(|b| b.name.to_owned()))
            .collect();
        bands.sort();
        bands.dedup();
        self.evidence.ask(here?, &places, &bands, now)
    }

    /// The runs `gateways` need, each channel that cannot have one given its reason now.
    fn plan(
        &self,
        here: Option<LatLon>,
        station: &Resolved,
        bands: &[String],
        gateways: &mut [Value],
        year: i32,
        month: u32,
    ) -> Vec<Planned> {
        let you = yours(station);
        let from = here.map(radioprop_geo::to_maidenhead);
        let mut plans = Vec::new();
        for (g, gw) in gateways.iter_mut().enumerate() {
            let site = radiopost::gateway_list::site(gw);
            let to = site
                .as_ref()
                .map(|s| LatLon {
                    lat: s.lat,
                    lon: s.lon,
                })
                .or_else(|| {
                    gw["grid"]
                        .as_str()
                        .and_then(|grid| radioprop_geo::from_maidenhead(grid).ok())
                });
            // the channels to run, by the gateway's end on each (a channel's own station)
            let mut runs: Vec<Run> = Vec::new();
            let Some(channels) = gw["channels"].as_array_mut() else {
                continue;
            };
            for (c, channel) in channels.iter_mut().enumerate() {
                let Some(hz) = hf_dial(channel) else { continue };
                if channel["blocked"] == true {
                    continue;
                }
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "a dial in hertz, well inside 2^52"
                )]
                let mhz = (hz + AUDIO_CENTRE_HZ) as f64 / 1e6;
                if let Some(why) = self.unpredicted(from.is_some(), to.is_some(), mhz, bands) {
                    channel["hf"] = why;
                    continue;
                }
                let them = theirs(channel, site.as_ref().and_then(|s| s.agl_m));
                let key = serde_json::to_string(&them.station).unwrap_or_default();
                match runs
                    .iter_mut()
                    .find(|r| r.key == key && r.channels.len() < RUN_FREQUENCIES)
                {
                    Some(run) => run.channels.push((c, mhz)),
                    None => runs.push(Run {
                        key,
                        them,
                        channels: vec![(c, mhz)],
                    }),
                }
            }
            if let (Some(from), Some(to)) = (&from, to) {
                let gw_at = format!("{:.4}, {:.4}", to.lat, to.lon);
                plans.extend(runs.into_iter().map(|run| {
                    let frequencies: Vec<f64> = run.channels.iter().map(|c| c.1).collect();
                    let key = format!(
                        "{from}|{gw_at}|{year}-{month:02}|{frequencies:?}|{}|{}",
                        serde_json::to_string(&you.station).unwrap_or_default(),
                        run.key
                    );
                    let request = PathRequest {
                        tx_position: from.clone(),
                        rx_position: gw_at.clone(),
                        year,
                        month,
                        ssn: None,
                        tx_station: you.station.clone(),
                        rx_station: run.them.station.clone(),
                        mode: Mode::Ft8,
                        required_reliability_pct: 90.0,
                        long_path: false,
                        required_snr_db_hz: Some(TOLD_REQUIRED_DB_HZ),
                        frequencies_mhz: Some(frequencies),
                    };
                    Planned {
                        gateway: g,
                        channels: run.channels.iter().map(|c| c.0).collect(),
                        key,
                        request,
                        you: you.clone(),
                        them: run.them,
                        to,
                    }
                }));
            }
        }
        plans
    }

    /// Why an HF channel at `mhz` cannot be predicted, if it cannot: no engine, no position for
    /// either end, a band outside the engine's, or one the station profile does not cover.
    fn unpredicted(&self, from: bool, to: bool, mhz: f64, bands: &[String]) -> Option<Value> {
        let not = |state: &str, words: String| Some(json!({"state": state, "words": words}));
        if let Err(why) = &self.engine {
            return not("no-engine", format!("HF not predicted: {why}"));
        }
        if !from {
            return not(
                "no-position",
                "HF not predicted: where you are is not known".into(),
            );
        }
        if !to {
            return not(
                "no-site",
                "HF not predicted: the gateway's record gives no position".into(),
            );
        }
        match hf_station::hf_band_at(mhz) {
            None => not("outside", "HF not predicted: only 80 m to 10 m are".into()),
            Some(band) if !covers(bands, band.name) => not(
                "not-your-bands",
                format!("Your station profile does not cover {}", band.name),
            ),
            Some(_) => None,
        }
    }

    /// Runs, in one batch, every planned prediction not kept, and keeps them.
    fn run_missing(&self, plans: &[Planned]) {
        let Ok(engine) = &self.engine else { return };
        let missing: Vec<&Planned> = {
            let Ok(kept) = self.kept.lock() else { return };
            let mut seen = std::collections::HashSet::new();
            plans
                .iter()
                .filter(|p| !kept.contains_key(&p.key) && seen.insert(p.key.clone()))
                .collect()
        };
        if missing.is_empty() {
            return;
        }
        let requests: Vec<PathRequest> = missing.iter().map(|p| p.request.clone()).collect();
        let results = predictor::predict_both_ways_many(&**engine, &requests, self.workers);
        if let Ok(mut kept) = self.kept.lock() {
            if kept.len() + missing.len() > KEEP {
                kept.clear();
            }
            for (p, r) in missing.iter().zip(results) {
                kept.insert(p.key.clone(), Arc::new(r));
            }
        }
    }
}

/// A channel's dial, if it is an HF one: USB, under 30 MHz.
fn hf_dial(channel: &Value) -> Option<u64> {
    let hz = channel["dial_hz"].as_u64()?;
    (hz < 30_000_000 && channel["mode"] != "FM").then_some(hz)
}

/// Whether a station profile's bands (`40m`, empty for all) cover a band named as hf-predict
/// names it (`40 m`).
fn covers(bands: &[String], band: &str) -> bool {
    let plain = |s: &str| s.replace(' ', "").to_ascii_lowercase();
    bands.is_empty() || bands.iter().any(|b| plain(b) == plain(band))
}

/// A chance in words, as the terrain says it (§ 8.6).
fn likely(p: f64) -> &'static str {
    if p >= 0.8 {
        "likely to work"
    } else if p >= 0.4 {
        "may work"
    } else {
        "unlikely to work"
    }
}

/// A rate in words, to two figures.
#[must_use]
pub fn rate_words(bps: Option<f64>) -> String {
    match bps {
        None => "nothing".into(),
        Some(b) if b >= 1000.0 => format!("{:.1} kbit/s", b / 1000.0),
        Some(b) => {
            let step = if b >= 100.0 { 10.0 } else { 1.0 };
            format!("{:.0} bit/s", (b / step).round() * step)
        }
    }
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// One HF channel's prediction at `hour` (UTC, 0–23), read from both ways of a run: its class,
/// the SNR each way, and with a modem profile the chance of a link and the rate each way. `heard`,
/// what this station heard lately toward the gateway, band by band: moderate or strong evidence on
/// the channel's band raises the share of days it is open (§ 10.5).
fn read(
    plan: &Planned,
    both: &BothWays,
    channel: &Value,
    (index, hour): (usize, u32),
    heard: Option<&HashMap<String, Value>>,
    learned: &dyn Fn(&str) -> Option<ForLine>,
) -> Value {
    let label = if hour == 0 { 24 } else { hour };
    let at = |p: &predictor::PathPrediction| {
        p.run
            .prediction
            .hours
            .iter()
            .find(|h| h.utc_hour == label)
            .and_then(|h| h.frequencies.get(index))
            .cloned()
    };
    let (Some(there), Some(back)) = (at(&both.there), at(&both.back)) else {
        return json!({"state": "failed", "why": format!("no hour {label} in the engine's output"),
                      "words": "HF not predicted: the engine failed"});
    };
    let band = both.there.bands.get(index).map_or("", |b| b.name);
    let up = Spread::of(
        &there,
        plan.you.gain_db - plan.you.feed_db + plan.them.gain_db,
    );
    let down = Spread::of(
        &back,
        plan.them.gain_db - plan.them.feed_db + plan.you.gain_db,
    );
    let distance = both.there.distance_km;
    let (class, class_words) = class_of(distance, &there, reach_deg(both.there.tx, plan.to));
    let modelled = there.muf_day.min(back.muf_day);
    let evidence = heard.and_then(|h| h.get(band));
    let open = evidence.map_or(modelled, |b| evidence::open_with(modelled, b));
    let notes = notes_of(distance, (class, &class_words));
    let mut assumed = plan.you.assumed.clone();
    assumed.extend(plan.them.assumed.iter().cloned());
    let mut out = json!({
        "band": band, "hour": hour,
        "month": format!("{}-{:02}", plan.request.year, plan.request.month),
        "distance_km": distance.round(), "class": class.name(), "class_words": class_words,
        "open": round2(open), "open_model": round2(modelled), "ssn": both.there.ssn.value.round(),
        "engine": both.there.engine, "assumed": assumed, "notes": notes,
    });
    if let Some(b) = evidence {
        out["heard"] = json!({
            "tier": b["tier"], "stations": b["stations"], "best_snr_db": b["best_snr_db"],
            "examples": b["examples"], "hearing_stations": b["hearing_stations"],
            "hearing_you": b["hearing_you"], "words": evidence::words(b),
        });
    }
    let modem = channel["modem"].as_str().unwrap_or("");
    let bandwidth = channel["bandwidth_hz"].as_u64().unwrap_or(0);
    let steps = Profile::of_modem(modem)
        .map(|p| (p, p.steps(bandwidth, class)))
        .filter(|(_, steps)| !steps.is_empty());
    if let Some((profile, steps)) = steps {
        through_profile(
            &mut out,
            (up, down),
            profile.call_db(class),
            &steps,
            (open, hour),
        );
        if let Some(l) = learned(band) {
            through_learning(
                &mut out,
                (up, down),
                profile.call_db(class),
                &steps,
                (open, hour),
                l,
            );
        }
    } else {
        out["state"] = json!("no-modem");
        out["up"] = way(up, None, None, None);
        out["down"] = way(down, None, None, None);
        out["words"] = json!(format!(
            "No model of {}: open on {:.0} % of days at {hour:02}Z, SNR {:.0} dB sending, {:.0} dB \
             receiving",
            if modem.is_empty() { "its modem" } else { modem },
            open * 100.0,
            up.median,
            down.median
        ));
    }
    out
}

/// One way of a path as the panel gets it: the SNR's median and deciles, and with a profile the
/// rate once linked, the rung of a median day and the share of days the call gets through.
fn way(s: Spread, bps: Option<f64>, rung: Option<Step<'_>>, p_call: Option<f64>) -> Value {
    json!({
        "snr_db": round1(s.median), "lower_db": round1(s.lower), "upper_db": round1(s.upper),
        "bps": bps.map(f64::round), "rung": rung.map(|r| r.name), "p_call": p_call.map(round2),
    })
}

/// The chance of a link and the rate each way through a modem's profile (§ 10.3), into `out`:
/// both ways' calls get through (`call`, dB in 3 kHz) on a day the band is `open`.
fn through_profile(
    out: &mut Value,
    (up, down): (Spread, Spread),
    call: f64,
    steps: &[Step<'_>],
    (open, hour): (f64, u32),
) {
    let (p_up, p_down) = (up.above(call), down.above(call));
    let chance = p_up * p_down * open;
    let (up_bps, down_bps) = (linked_rate(up, steps, call), linked_rate(down, steps, call));
    out["state"] = json!("predicted");
    out["chance"] = json!(round2(chance));
    out["chance_words"] = json!(likely(chance));
    out["up"] = way(up, up_bps, median_rung(up, steps), Some(p_up));
    out["down"] = way(down, down_bps, median_rung(down, steps), Some(p_down));
    out["words"] = json!(link_words(chance, hour, (up_bps, down_bps)));
}

/// The same through what was learned (§12.5, §12.6), into `out["learned"]`, the model's own
/// figures left as they are: they are what the call's record keeps and what learning compares
/// with. Each way moves by its offset and widens by the offset's spread; the chance takes the
/// gateway's availability at the hour, the rate the modem's efficiency.
fn through_learning(
    out: &mut Value,
    (up, down): (Spread, Spread),
    call: f64,
    steps: &[Step<'_>],
    (open, hour): (f64, u32),
    l: ForLine,
) {
    let (up, down) = (ForLine::moved(up, l.up), ForLine::moved(down, l.down));
    let (p_up, p_down) = (up.above(call), down.above(call));
    let chance = l.there * p_up * p_down * open;
    let rate = |s: Spread| linked_rate(s, steps, call).map(|b| b * l.eta);
    let (up_bps, down_bps) = (rate(up), rate(down));
    let offset = |o: Offset| json!({"mean": round1(o.mean), "sd": round1(o.sd)});
    out["learned"] = json!({
        "chance": round2(chance), "chance_words": likely(chance),
        "up": way(up, up_bps, median_rung(up, steps), Some(p_up)),
        "down": way(down, down_bps, median_rung(down, steps), Some(p_down)),
        "up_offset": offset(l.up), "down_offset": offset(l.down),
        "there": round2(l.there), "eta": round2(l.eta), "calls": round1(l.calls),
        "words": format!("{} · learned from your calls", link_words(chance, hour, (up_bps, down_bps))),
    });
}

/// A link's chance and rate in words: "Likely: links on 74 % of days at 21Z, about 900 bit/s
/// each way".
fn link_words(chance: f64, hour: u32, (up_bps, down_bps): (Option<f64>, Option<f64>)) -> String {
    let mut words = format!(
        "{}: links on {:.0} % of days at {hour:02}Z",
        capital(likely(chance)),
        chance * 100.0
    );
    if chance >= 0.01 {
        let (sending, receiving) = (rate_words(up_bps), rate_words(down_bps));
        if sending == receiving {
            let _ = write!(words, ", about {sending} each way");
        } else {
            let _ = write!(words, ", about {sending} sending, {receiving} receiving");
        }
    }
    words
}

/// What the prediction leaves out on a path: ground wave on a short one, and on a near-vertical
/// one the echo a disturbed day brings.
fn notes_of(distance_km: f64, class: (Class, &str)) -> Vec<String> {
    let mut notes = Vec::new();
    if distance_km < GROUND_WAVE_KM {
        notes.push(format!(
            "Under {GROUND_WAVE_KM:.0} km ground wave may carry it too, which VOACAP does not model"
        ));
    }
    if class.1.starts_with("near-vertical") {
        notes.push(
            "A disturbed near-vertical path, two rays 7 ms apart, can hold Aether to its tone \
             rungs (its ADR-0087)"
                .into(),
        );
    }
    notes
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(mode: &str, angle: f64, multipath: f64) -> FrequencyPrediction {
        FrequencyPrediction {
            mode: mode.into(),
            takeoff_angle_deg: angle,
            multipath_probability: multipath,
            ..FrequencyPrediction::default()
        }
    }

    #[test]
    fn the_profile_reads_and_holds_both_airs() {
        let p = Profile::aether();
        assert_eq!(p.modem, "aether");
        assert!((p.call_db(Class::Moderate) + 14.5).abs() < 1e-9);
        // 2300 Hz: the tone rungs, then OFDM to 64-QAM on Good
        let good = p.steps(2300, Class::Good);
        assert_eq!(good.first().map(|s| s.name), Some("tone-24"));
        assert_eq!(good.last().map(|s| s.name), Some("QAM64-5/6"));
        // a 2750 Hz channel runs the same ladder; 500 Hz its own
        assert_eq!(p.steps(2750, Class::Good), good);
        assert_eq!(
            p.steps(500, Class::Good).last().map(|s| s.name),
            Some("QAM64-5/6")
        );
        assert_eq!(p.steps(200, Class::Good), Vec::<Step<'_>>::new());
    }

    #[test]
    fn steps_rise_and_leave_out_thresholds_that_are_only_bounds() {
        let p = Profile::aether();
        for bw in [500, 2300] {
            for class in [Class::Good, Class::Moderate, Class::Poor] {
                let steps = p.steps(bw, class);
                for w in steps.windows(2) {
                    assert!(
                        w[1].threshold >= w[0].threshold && w[1].bps > w[0].bps,
                        "{w:?}"
                    );
                }
            }
        }
        // on Poor, 2300 Hz QAM16-3/4 never fell to 1 in 10 (and 64-QAM 5/6 on Moderate)
        let names =
            |bw, c| -> Vec<String> { p.steps(bw, c).iter().map(|s| s.name.to_owned()).collect() };
        assert!(!names(2300, Class::Poor).contains(&"QAM16-3/4".to_owned()));
        assert!(!names(2300, Class::Moderate).contains(&"QAM64-5/6".to_owned()));
        assert!(names(2300, Class::Good).contains(&"QAM64-5/6".to_owned()));
        // on 500 Hz Moderate nothing past QAM16-2/3 was seen to get through
        assert_eq!(
            names(500, Class::Moderate).last().map(String::as_str),
            Some("QAM16-2/3")
        );
        // on Good tone50-75 (112 bit/s) needs more than tone100-105 (157): it is no step there
        assert!(!names(2300, Class::Good).contains(&"tone50-75".to_owned()));
        assert!(names(2300, Class::Good).contains(&"tone100-105".to_owned()));
    }

    #[test]
    fn the_spread_meets_its_deciles() {
        let s = Spread {
            median: 5.0,
            lower: 12.0,
            upper: 8.0,
        };
        assert!((s.above(5.0) - 0.5).abs() < 1e-6);
        assert!((s.above(5.0 - 12.0) - 0.9).abs() < 1e-4);
        assert!((s.above(5.0 + 8.0) - 0.1).abs() < 1e-4);
        assert!(s.above(-100.0) > 0.999_999 && s.above(100.0) < 1e-6);
        // VOACAP's SNR in 1 Hz moved into 3 kHz, and by what it did not model
        let v = FrequencyPrediction {
            snr_db: 40.0,
            snr_lower_decile_db: 12.0,
            snr_upper_decile_db: 8.0,
            ..FrequencyPrediction::default()
        };
        assert!((Spread::of(&v, 2.0).median - (40.0 - 34.771 + 2.0)).abs() < 1e-3);
    }

    #[test]
    fn the_rate_once_linked_is_the_ladder_over_the_linked_days() {
        let steps = [
            Step {
                threshold: -10.0,
                bps: 100.0,
                name: "a",
            },
            Step {
                threshold: 0.0,
                bps: 1000.0,
                name: "b",
            },
        ];
        // nearly every day between the two: the slow rung
        let narrow = Spread {
            median: -5.0,
            lower: 0.1,
            upper: 0.1,
        };
        assert!((linked_rate(narrow, &steps, -12.0).unwrap() - 100.0).abs() < 1e-3);
        assert_eq!(median_rung(narrow, &steps).map(|s| s.name), Some("a"));
        // half the days above 0 dB: half each, of the linked days (all of them)
        let even = Spread {
            median: 0.0,
            lower: 0.1,
            upper: 0.1,
        };
        assert!((linked_rate(even, &steps, -12.0).unwrap() - 550.0).abs() < 1.0);
        // a call that needs more than the slow rung: only the fast rung's days link
        assert!((linked_rate(even, &steps, 0.0).unwrap() - 1000.0).abs() < 1e-3);
        // no linked day: no rate
        let dead = Spread {
            median: -60.0,
            lower: 5.0,
            upper: 5.0,
        };
        assert_eq!(linked_rate(dead, &steps, -12.0), None);
        assert_eq!(median_rung(dead, &steps), None);
    }

    #[test]
    fn a_path_has_the_class_of_its_mode_and_geometry() {
        let class = |km, fp: FrequencyPrediction, reach| class_of(km, &fp, reach);
        assert_eq!(class(80.0, f("1F2", 80.0, 0.0), 34.0).0, Class::Poor);
        assert!(
            class(80.0, f("1F2", 80.0, 0.0), 34.0)
                .1
                .starts_with("near-vertical")
        );
        // short but low: one hop
        assert_eq!(
            class(140.0, f("1 E", 57.6, 0.0), 34.0),
            (Class::Moderate, "one hop (1E)".into())
        );
        assert_eq!(class(1310.0, f("1F2", 19.0, 0.0), 40.0).0, Class::Moderate);
        assert_eq!(class(1310.0, f("1F2", 19.0, 0.97), 40.0).0, Class::Poor);
        assert_eq!(
            class(3567.0, f("2F2", 11.0, 0.0), 47.0),
            (Class::Poor, "2 hops (2F2)".into())
        );
        assert_eq!(class(7739.0, f("F2F2", 13.0, 0.0), 50.0).0, Class::Poor);
        assert_eq!(class(1500.0, f("1F2", 15.0, 0.0), 63.0).0, Class::Poor);
    }

    #[test]
    fn the_reach_of_a_path_is_its_highest_latitude() {
        let atlanta = LatLon {
            lat: 33.7,
            lon: -84.4,
        };
        let london = LatLon {
            lat: 51.5,
            lon: -0.1,
        };
        // the great circle goes well north of both ends
        assert!(reach_deg(atlanta, london) > 53.0);
        let near = LatLon {
            lat: 34.0,
            lon: -84.0,
        };
        assert!((reach_deg(atlanta, near) - 34.0).abs() < 1e-6);
    }

    #[test]
    fn antennas_are_hf_predicts_and_gains_are_over_a_dipole() {
        let theirs_files: Vec<&str> = hf_station::antennas().iter().map(|c| c.value).collect();
        for kind in radiopost::gateway_list::ANTENNAS {
            for agl in [3.0, 15.0] {
                let (file, own) = antenna_file(kind, agl);
                assert!(theirs_files.contains(&file), "{kind}: {file}");
                assert_eq!(own, ["vertical", "whip", "nvis", "dipole"].contains(&kind));
            }
        }
        assert_eq!(antenna_file("dipole", 5.0).0, DIPOLE_5M);
        assert_eq!(antenna_file("dipole", 12.0).0, DIPOLE_10M);
        // a Yagi runs as a dipole at its height, its gain over a dipole added
        let channel = json!({"station": {"antenna": "yagi", "gain_dbi": 9.15, "agl_m": 15.0}});
        let end = theirs(&channel, None);
        assert_eq!(end.station.antenna, DIPOLE_10M);
        assert!((end.gain_db - 7.0).abs() < 1e-9);
        // the noise VOACAP is told: P.372 residential at 3 MHz, 145 dB below 1 W/Hz
        assert!((end.station.noise_db - 144.7).abs() < 0.1);
    }

    #[test]
    fn bands_match_however_they_are_written() {
        assert!(covers(&[], "40 m"));
        assert!(covers(&["40m".into(), "80m".into()], "40 m"));
        assert!(!covers(&["40m".into()], "20 m"));
    }

    #[test]
    fn rates_read_to_two_figures() {
        assert_eq!(rate_words(Some(1093.0)), "1.1 kbit/s");
        assert_eq!(rate_words(Some(531.4)), "530 bit/s");
        assert_eq!(rate_words(Some(54.2)), "54 bit/s");
        assert_eq!(rate_words(None), "nothing");
    }
}
