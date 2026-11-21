// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the client learns from its call records (choosing a gateway, §12.4–12.7), on this computer
//! with no service anywhere. Everything here is computed from the records: they are never
//! rewritten (§12.1), a better method applies to all past evidence at once (§12.9), and the same
//! records always give the same estimates (records are taken in the order of their start and ID,
//! and every map is ordered). `learned.json` keeps the last result only as a cache of the same
//! inputs, under the learning's [`VERSION`] (§15.1).
//!
//! * **Availability** A, a Beta for each gateway and three-hour block of the day, pooled toward the
//!   gateway and all gateways; a silent call is shared between the gateway being away and the path
//!   being too weak, by their odds just then (§12.3).
//! * **The SNR offset** δ, each way, for each gateway and band (HF) or channel (VHF): a posterior on
//!   a grid of the downlink's δ, the direction offset D (the uplink's is δ + D), A and the heard
//!   share h, each record multiplying it by its likelihood raised to its weight (§12.5).
//! * **Service** S, a Beta for each gateway pooled toward all gateways.
//! * **The modem's efficiency** η, the measured goodput over what the modem's profile carries at the
//!   SNR measured, log-normal, for each modem and bandwidth, pooled toward it for each gateway.
//! * **Link losses** λ, a Gamma–Poisson rate of losses per second of link, for each gateway and
//!   band, pooled toward all sessions.
//! * **Occupancy** O, a Beta for each dial and hour of the day.
//! * **Station health** H: whether this station, not the gateways, is at fault (§12.3).
//! * **Overheads**: the weighted medians of the time to connect and of a failed call.
//!
//! Each record weighs 2^(−age / half-life) × (1 − the chance the station was at fault) × 1 / the
//! calls in its episode (calls to one channel within ten minutes, which fail together).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use radioprop_terrain::budget::phi;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::classify::{self, Class};
use super::hf::{self, Profile, Spread};

const DAY: f64 = 86_400.0;
/// Half-lives, days (§12.4).
const HALF_AVAILABILITY: f64 = 14.0;
const HALF_OFFSET_HF: f64 = 60.0;
const HALF_OFFSET_VHF: f64 = 365.0;
const HALF_SERVICE: f64 = 7.0;
const HALF_EFFICIENCY: f64 = 90.0;
const HALF_LOSSES: f64 = 60.0;
const HALF_OCCUPANCY: f64 = 30.0;
const HALF_OVERHEADS: f64 = 90.0;
/// Calls to one channel this close together are one episode (§12.4).
const EPISODE_S: u64 = 600;
/// Station health looks back a day (§12.4).
const HEALTH_S: u64 = 86_400;
/// The prior chance the station is at fault, and how likely a call fails with a fault (§12.3).
const FAULT_PRIOR: f64 = 0.05;
const FAIL_WITH_FAULT: f64 = 0.95;
/// A failure counts toward station health when it was predicted to connect at least this often.
const HEALTH_LIKELY: f64 = 0.7;
/// The offset grid (§12.5): the downlink's offset from −30 to +30 dB in 1 dB steps, A in 19
/// steps, and the uplink's offset over the downlink's from −12 to +12 dB: learned here with the
/// rest, since the sessions that measure both ways are the ones where both cleared the call, and
/// their difference alone is biased toward nought.
const DELTA_MIN: f64 = -30.0;
const DELTA_STEPS: usize = 61;
const DIR_MIN: i32 = -12;
const DIR_STEPS: usize = 25;
const A_STEPS: usize = 19;
/// The offset's prior spread, dB: VOACAP's own day-to-day deciles at HF, a terrain path's
/// location spread at VHF (§12.5).
const OFFSET_SD_HF: f64 = 8.0;
const OFFSET_SD_VHF: f64 = 10.0;
/// A measured SNR's own error, dB, and the direction offset's prior spread (§12.5).
const MEASURE_SD: f64 = 2.0;
const DIRECTION_SD: f64 = 6.0;
/// Pooling strengths (§12.4: n0 = 2 to 4).
const N0_BLOCK: f64 = 3.0;
const N0_GATEWAY: f64 = 2.0;
/// Service's prior: 0.95, worth two attempts (§12.4).
const SERVICE_PRIOR: f64 = 0.95;
/// η's starting values (§12.4): Aether's, and any other modem's; worth two sessions.
const ETA_AETHER: f64 = 0.6;
const ETA_OTHER: f64 = 0.5;
/// A session counts toward η with this much link and data.
const ETA_MIN_S: f64 = 10.0;
const ETA_MIN_BYTES: f64 = 1000.0;
/// Link losses' prior: one an hour, worth an hour of link.
const LOSS_PRIOR_PER_S: f64 = 1.0 / 3600.0;
const LOSS_PRIOR_S: f64 = 3600.0;
/// A loss with the SNR this far above the call's threshold just before it is no fade (§12.6).
const NOT_A_FADE_DB: f64 = 6.0;
/// Occupancy's prior: 0.1, worth two calls (§12.4).
const OCCUPANCY_PRIOR: f64 = 0.1;
/// Drift: three episodes in a row this unlikely together (§12.7).
const DRIFT_P: f64 = 0.05;

/// A record's weight for a half-life of `days`, `age_s` seconds old.
fn decay(age_s: f64, days: f64) -> f64 {
    (-(age_s.max(0.0)) / (days * DAY)).exp2()
}

/// What a channel was predicted to do when it was called (§12.1).
#[derive(Clone, Debug, PartialEq)]
enum Predicted {
    /// VOACAP both ways, through a modem's profile when one is held: each way's spread (dB in
    /// 3 kHz), the share of days the band is open, the call's threshold (none without a profile).
    Hf {
        band: String,
        up: Spread,
        down: Spread,
        open: f64,
        call_db: Option<f64>,
        chance: Option<f64>,
    },
    /// A VHF path over the terrain: each way's margin and the spread they are read against, and
    /// the chance shown when it was through what was learned.
    Vhf {
        up: f64,
        down: f64,
        spread: f64,
        shown: Option<f64>,
    },
}

impl Predicted {
    fn read(v: &Value) -> Option<Predicted> {
        if let Some(h) = v.get("hf") {
            let state = h["state"].as_str()?;
            if state != "predicted" && state != "no-modem" {
                return None;
            }
            let way = |w: &Value| {
                Some(Spread {
                    median: w["snr_db"].as_f64()?,
                    lower: w["lower_db"].as_f64()?,
                    upper: w["upper_db"].as_f64()?,
                })
            };
            let class = hf::Class::from_name(h["class"].as_str().unwrap_or(""));
            return Some(Predicted::Hf {
                band: h["band"].as_str()?.to_owned(),
                up: way(&h["up"])?,
                down: way(&h["down"])?,
                open: h["open_model"].as_f64().or_else(|| h["open"].as_f64())?,
                call_db: class
                    .filter(|_| state == "predicted")
                    .map(|c| Profile::aether().call_db(c)),
                // the chance shown: through what was learned, when it was
                chance: h["learned"]["chance"]
                    .as_f64()
                    .or_else(|| h["chance"].as_f64()),
            });
        }
        let t = v.get("terrain")?;
        if t["state"] != "judged" {
            return None;
        }
        Some(Predicted::Vhf {
            up: t["up"]["margin_db"].as_f64()?,
            down: t["down"]["margin_db"].as_f64()?,
            spread: t["spread_db"].as_f64()?,
            shown: t["learned"]["chance"].as_f64(),
        })
    }

    /// The chance a call links with the offsets `d_down` and `d_up`, the gateway there; `None`
    /// without a threshold to judge it by.
    fn p_link(&self, d_down: f64, d_up: f64) -> Option<f64> {
        match self {
            Predicted::Hf {
                up,
                down,
                open,
                call_db,
                ..
            } => {
                let call = (*call_db)?;
                Some(shifted(*up, d_up).above(call) * shifted(*down, d_down).above(call) * open)
            }
            Predicted::Vhf {
                up, down, spread, ..
            } => Some(phi((up + d_up) / spread) * phi((down + d_down) / spread)),
        }
    }

    /// The chance the uplink alone fails, with the offset `d_up`.
    fn p_up_fails(&self, d_up: f64) -> Option<f64> {
        match self {
            Predicted::Hf { up, call_db, .. } => Some(1.0 - shifted(*up, d_up).above((*call_db)?)),
            Predicted::Vhf { up, spread, .. } => Some(1.0 - phi((up + d_up) / spread)),
        }
    }

    /// The log density of a downlink SNR `m` with the offset `d`, the day-to-day spread widened by
    /// the measurement's own error (§12.5); `None` where SNRs are not predicted (VHF).
    fn ln_density_down(&self, m: f64, d: f64) -> Option<f64> {
        match self {
            Predicted::Hf { down, .. } => Some(ln_split_density(*down, m - d)),
            Predicted::Vhf { .. } => None,
        }
    }

    fn ln_density_up(&self, m: f64, d: f64) -> Option<f64> {
        match self {
            Predicted::Hf { up, .. } => Some(ln_split_density(*up, m - d)),
            Predicted::Vhf { .. } => None,
        }
    }

    /// The chance shown when the call was placed, for station health.
    fn chance(&self) -> Option<f64> {
        match self {
            Predicted::Hf { chance, .. } => *chance,
            Predicted::Vhf {
                up,
                down,
                spread,
                shown,
            } => Some(shown.unwrap_or_else(|| phi(up / spread) * phi(down / spread))),
        }
    }

    fn residual_down(&self, m: f64) -> Option<f64> {
        match self {
            Predicted::Hf { down, .. } => Some(m - down.median),
            Predicted::Vhf { .. } => None,
        }
    }

    fn residual_up(&self, m: f64) -> Option<f64> {
        match self {
            Predicted::Hf { up, .. } => Some(m - up.median),
            Predicted::Vhf { .. } => None,
        }
    }
}

fn shifted(s: Spread, d: f64) -> Spread {
    Spread {
        median: s.median + d,
        ..s
    }
}

/// ln of the split normal's density at `m` (its median moved by nothing here: the caller moves
/// `m`), each half widened by the measurement's error. The two halves share one normalisation,
/// 2 / (√(2π) (`σ_lower` + `σ_upper`)): each half's own σ there would make the density jump at the
/// median, and an offset either side of it would be favoured for no reason.
fn ln_split_density(s: Spread, m: f64) -> f64 {
    let widened = |decile: f64| ((decile / DECILE_SIGMAS).powi(2) + MEASURE_SD.powi(2)).sqrt();
    let (lower, upper) = (widened(s.lower), widened(s.upper));
    let sigma = if m < s.median { lower } else { upper };
    let x = (m - s.median) / sigma;
    -lower.midpoint(upper).ln() - 0.5 * x * x
}

/// Which channel a record is about: a gateway's band at HF, its dial at VHF.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Candidate {
    /// The gateway called.
    pub callsign: String,
    /// `40 m` at HF; the dial in hertz at VHF.
    pub channel: String,
}

/// The channel a dial is learned under: its band at HF (`40 m`), the dial in hertz from 30 MHz
/// up; `None` outside the amateur bands.
#[must_use]
pub fn channel_of(dial_hz: u64) -> Option<String> {
    if dial_hz >= 30_000_000 {
        return Some(dial_hz.to_string());
    }
    #[allow(clippy::cast_precision_loss, reason = "a dial in hertz")]
    let mhz = dial_hz as f64 / 1e6;
    radioprop_hf::station::hf_band_at(mhz).map(|b| b.name.to_owned())
}

/// One call, as learning reads its record.
#[derive(Clone, Debug)]
struct Rec {
    id: String,
    start: u64,
    callsign: String,
    dial: Option<u64>,
    bandwidth: u64,
    modem: String,
    class: Class,
    predicted: Option<Predicted>,
    heard_snr: Option<f64>,
    snr_down: Option<f64>,
    snr_up: Option<f64>,
    link_s: f64,
    bytes: f64,
    busy: bool,
    call_s: Option<f64>,
    scanned: bool,
    last_snr: Option<f64>,
}

impl Rec {
    fn read(v: &Value) -> Option<Rec> {
        if v["kind"] != "gateway" {
            return None;
        }
        let class = classify::class_of(v);
        let link = &v["link"];
        let handed = link["bytes_handed"].as_f64().unwrap_or(0.0);
        let unacked = link["bytes_unacknowledged"].as_f64().unwrap_or(0.0);
        let last_snr = link["quality"]
            .as_array()
            .into_iter()
            .flatten()
            .rev()
            .find_map(|r| r[1].as_f64());
        Some(Rec {
            id: v["id"].as_str()?.to_owned(),
            start: v["start"].as_u64()?,
            callsign: v["remote"].as_str()?.to_ascii_uppercase(),
            dial: v["channel"]["dial_hz"].as_u64(),
            bandwidth: v["channel"]["bandwidth_hz"]
                .as_u64()
                .or_else(|| link["bandwidth_hz"].as_u64())
                .unwrap_or(0),
            modem: v["modem"].as_str().unwrap_or("").to_owned(),
            class,
            predicted: Predicted::read(&v["predicted"]),
            heard_snr: v["call"]["heard_snr_db"].as_f64(),
            snr_down: link["snr_db"]["median"].as_f64(),
            snr_up: link["peer_snr_db"]["median"].as_f64(),
            link_s: link["duration_s"].as_f64().unwrap_or(0.0),
            bytes: (handed - unacked).max(0.0) + link["bytes_in"].as_f64().unwrap_or(0.0),
            busy: class == Class::ChannelBusy || v["call"]["busy_before"] == true,
            call_s: v["call"]["duration_s"].as_f64(),
            scanned: v["channel"]["call_s"].is_u64(),
            last_snr,
        })
    }

    fn candidate(&self) -> Option<Candidate> {
        let channel = if let Some(Predicted::Hf { band, .. }) = &self.predicted {
            band.clone()
        } else {
            channel_of(self.dial?)?
        };
        Some(Candidate {
            callsign: self.callsign.clone(),
            channel,
        })
    }

    fn hf(&self) -> bool {
        !matches!(self.predicted, Some(Predicted::Vhf { .. }))
            && self.dial.is_none_or(|d| d < 30_000_000)
    }

    /// A link came up: the gateway was there and the path carried, whatever happened after.
    fn linked(&self) -> bool {
        matches!(
            self.class,
            Class::Completed
                | Class::CompletedInPart
                | Class::LinkLost
                | Class::ServiceFailed
                | Class::ProtocolError
        ) || self.link_s > 0.0
    }
}

/// What the operator asked learning to forget (§12.9): records are marked, never deleted, and
/// "show forgotten records" brings them back.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Forgotten {
    /// Each a rule, and when it was made.
    pub rules: Vec<Forget>,
}

/// One thing forgotten.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "what", rename_all = "kebab-case")]
pub enum Forget {
    /// A gateway's history.
    Gateway {
        /// Its callsign.
        callsign: String,
        /// When it was forgotten: records after it count again.
        at: u64,
    },
    /// What was recorded within `km` of a place.
    Place {
        /// Latitude.
        lat: f64,
        /// Longitude.
        lon: f64,
        /// How far around it.
        km: f64,
        /// When.
        at: u64,
    },
    /// Everything before a moment (after a station change).
    Before {
        /// The moment.
        before: u64,
    },
    /// Everything up to when it was forgotten.
    Everything {
        /// When.
        at: u64,
    },
}

/// Where the rules are kept, under the first home.
pub const FORGOTTEN: &str = "selection/forgotten.json";

impl Forgotten {
    /// The rules kept under `base`; none when there is no file.
    #[must_use]
    pub fn load(base: &Path) -> Forgotten {
        std::fs::read_to_string(base.join(FORGOTTEN))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Keeps the rules under `base`.
    ///
    /// # Errors
    ///
    /// The file could not be written.
    pub fn save(&self, base: &Path) -> Result<(), String> {
        let path: PathBuf = base.join(FORGOTTEN);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| e.to_string())
    }

    /// Whether a record is forgotten by any rule.
    #[must_use]
    pub fn forgets(&self, record: &Value) -> bool {
        let start = record["start"].as_u64().unwrap_or(0);
        let remote = record["remote"].as_str().unwrap_or("");
        self.rules.iter().any(|r| match r {
            Forget::Gateway { callsign, at } => {
                start <= *at && remote.eq_ignore_ascii_case(callsign)
            }
            Forget::Place { lat, lon, km, at } => {
                let place = &record["place"];
                start <= *at
                    && place["lat"]
                        .as_f64()
                        .zip(place["lon"].as_f64())
                        .is_some_and(|(a, b)| crate::grid::km_between((a, b), (*lat, *lon)) <= *km)
            }
            Forget::Before { before } => start < *before,
            Forget::Everything { at } => start <= *at,
        })
    }
}

/// Whether this station, not the gateways, looks to be at fault (§12.3).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Health {
    /// The chance the station is at fault now.
    pub p_fault: f64,
    /// The failures in a row behind it: calls predicted to connect that heard nothing.
    pub failures: usize,
    /// The gateways and bands they were on.
    pub gateways: usize,
    /// What to tell the operator, when it is time to.
    pub warning: Option<String>,
}

/// A Beta's estimate: its mean and the weight of evidence behind it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Estimate {
    /// The mean.
    pub mean: f64,
    /// The weighted attempts it rests on, its prior's left out.
    pub n: f64,
}

/// The offset posterior's summary, dB.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Offset {
    /// Its mean.
    pub mean: f64,
    /// Its standard deviation.
    pub sd: f64,
}

/// What was learned of one channel of one gateway.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Learned {
    /// The downlink's offset from the model (the gateway heard here).
    pub down: Offset,
    /// The uplink's (this station heard there).
    pub up: Offset,
    /// The uplink's over the downlink's: D, learned with them.
    pub direction: Offset,
    /// Whether the gateway answers, as the offset grid has it.
    pub there: f64,
    /// Sessions with a measured SNR behind the offsets, weighted.
    pub measured: f64,
    /// Calls behind it, weighted.
    pub calls: f64,
    /// Link losses an hour.
    pub losses_per_hour: f64,
    /// The modem's efficiency here.
    pub efficiency: Estimate,
    /// Outcomes lately that the model found very unlikely (§12.7).
    pub drift: bool,
}

/// Everything learned from the records.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Learning {
    /// Each channel's.
    pub channels: BTreeMap<String, Learned>,
    /// Each gateway's availability, overall and by three-hour block (UTC, block 0 is 00–03).
    pub availability: BTreeMap<String, (Estimate, [f64; 8])>,
    /// Each gateway's service.
    pub service: BTreeMap<String, Estimate>,
    /// Each dial's occupancy by hour (UTC).
    pub occupancy: BTreeMap<u64, [f64; 24]>,
    /// The modem's efficiency over all gateways, by modem and bandwidth.
    pub efficiency: BTreeMap<String, Estimate>,
    /// The uplink's offset over the downlink's, over all gateways, dB: each channel's D, weighted
    /// by the calls behind it.
    pub direction: Offset,
    /// Medians: time to connect, and a failed call's length, by modem, bandwidth and scanning.
    pub overheads: BTreeMap<String, (Option<f64>, Option<f64>)>,
    /// This station's health.
    pub health: Health,
    /// Records taken, and forgotten.
    pub records: usize,
    /// Records the operator forgot.
    pub forgotten: usize,
}

/// The key a candidate is kept under in [`Learning::channels`]: `CALLSIGN channel`.
#[must_use]
pub fn key(callsign: &str, channel: &str) -> String {
    format!("{} {channel}", callsign.to_ascii_uppercase())
}

/// What the post office last saw of each gateway, for availability's prior (§7.4): seconds since
/// 1970, by callsign.
pub type Seen = BTreeMap<String, u64>;

/// The learning's version: a change to its method changes it, and every cache is worked out again
/// (§12.9, §15.1).
pub const VERSION: u64 = 1;

/// The cache of the last result, under the first home.
pub const LEARNED: &str = "selection/learned.json";

/// What a line under a channel takes from what was learned (§10.3): each way's offset, the
/// gateway's availability at the hour, the modem's efficiency there, and the calls behind them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ForLine {
    /// The downlink's offset.
    pub down: Offset,
    /// The uplink's.
    pub up: Offset,
    /// The chance the gateway answers at the hour.
    pub there: f64,
    /// The modem's efficiency.
    pub eta: f64,
    /// Calls behind it, weighted.
    pub calls: f64,
}

impl ForLine {
    /// A prediction's spread for one way moved by `offset`, its deciles widened by the offset's
    /// own uncertainty: what the model says, corrected by what this station saw.
    #[must_use]
    pub fn moved(s: Spread, offset: Offset) -> Spread {
        let widen = |decile: f64| (decile.powi(2) + (DECILE_SIGMAS * offset.sd).powi(2)).sqrt();
        Spread {
            median: s.median + offset.mean,
            lower: widen(s.lower),
            upper: widen(s.upper),
        }
    }
}

/// A normal's tenth percentile, in standard deviations: VOACAP's deciles.
const DECILE_SIGMAS: f64 = 1.281_551_565_544_6;

/// FNV-1a over `bytes`, for the cache's key: the same everywhere, unlike the standard library's
/// hasher.
fn fnv(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

impl Learning {
    /// What was learned at `now` from the records under `base`, less what the operator forgot,
    /// availability's prior from `seen`: from `learned.json` when it was worked out from the same
    /// records, rules and sightings, by this version of the learning, in the same hour (the
    /// records' weights change with their age); otherwise learned again and kept there.
    #[must_use]
    pub fn cached(base: &Path, seen: &Seen, now: u64) -> Learning {
        let records = super::attempts::read(base);
        let forgotten = Forgotten::load(base);
        let mut key = fnv(0xcbf2_9ce4_8422_2325, &VERSION.to_le_bytes());
        key = fnv(key, &(now / 3600).to_le_bytes());
        for r in &records {
            key = fnv(key, r.to_string().as_bytes());
        }
        key = fnv(
            key,
            serde_json::to_string(&forgotten)
                .unwrap_or_default()
                .as_bytes(),
        );
        key = fnv(
            key,
            serde_json::to_string(seen).unwrap_or_default().as_bytes(),
        );
        let key = format!("{key:016x}");
        let path = base.join(LEARNED);
        let kept: Option<Value> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok());
        if let Some(k) = kept.filter(|k| k["version"] == VERSION && k["key"] == key.as_str())
            && let Ok(l) = serde_json::from_value::<Learning>(k["learned"].clone())
        {
            return l;
        }
        let learned = Learning::learn(&records, &forgotten, seen, now);
        let out = serde_json::json!({"version": VERSION, "key": key, "as_of": now,
                                     "learned": learned});
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // a cache: when it cannot be written, the next request learns again
        let _ = std::fs::write(&path, out.to_string());
        learned
    }

    /// What a line under `callsign`'s `channel` takes from what was learned, for the hour `t`;
    /// `None` before any call to it was learned from.
    #[must_use]
    pub fn for_line(&self, callsign: &str, channel: &str, t: u64) -> Option<ForLine> {
        let l = self.channel(callsign, channel)?;
        if l.calls <= 0.0 {
            return None;
        }
        let block = usize::try_from(t % 86_400 / 10_800).unwrap_or(0);
        let there = self
            .availability
            .get(&callsign.to_ascii_uppercase())
            .map_or(l.there, |a| a.1[block]);
        Some(ForLine {
            down: l.down,
            up: l.up,
            there,
            eta: l.efficiency.mean,
            calls: l.calls,
        })
    }

    /// Learns from `records` (as [`super::attempts::read`] gives them) at `now`, leaving out what
    /// `forgotten` forgets, availability's prior from `seen`.
    #[must_use]
    pub fn learn(records: &[Value], forgotten: &Forgotten, seen: &Seen, now: u64) -> Learning {
        let mut all: Vec<Rec> = Vec::new();
        let mut dropped = 0;
        for v in records {
            if forgotten.forgets(v) {
                dropped += 1;
            } else if let Some(r) = Rec::read(v) {
                all.push(r);
            }
        }
        all.sort_by(|a, b| (a.start, &a.id).cmp(&(b.start, &b.id)));
        let episodes = episode_sizes(&all);
        let (health, fault) = health(&all, now);
        let weight = |i: usize, days: f64| {
            let r = &all[i];
            #[allow(clippy::cast_precision_loss, reason = "seconds and small counts")]
            let w = decay((now - r.start.min(now)) as f64, days) / episodes[i] as f64
                * (1.0 - fault.get(&r.id).copied().unwrap_or(0.0));
            w
        };
        let mut out = Learning {
            health,
            records: all.len(),
            forgotten: dropped,
            ..Learning::default()
        };
        // availability first from the model's chances, as each grid's prior; then again with each
        // silent call shared by what the grids learned of its path
        out.availability = availability(&all, &weight, seen, now, &BTreeMap::new());
        out.service = service(&all, &weight);
        out.occupancy = occupancy(&all, now);
        out.efficiency = efficiency(&all, &weight, None);
        out.direction = direction_prior(&all, &weight);
        out.overheads = overheads(&all, now);
        let (channels, away) = channels(&all, &weight, &out);
        out.direction = direction_learned(&channels).unwrap_or(out.direction);
        out.channels = channels;
        out.availability = availability(&all, &weight, seen, now, &away);
        out
    }

    /// What was learned of a gateway's channel: `40 m` at HF, the dial in hertz at VHF.
    #[must_use]
    pub fn channel(&self, callsign: &str, channel: &str) -> Option<&Learned> {
        self.channels.get(&key(callsign, channel))
    }

    /// The whole of it as JSON: for the panel, and to check that the same records always give the
    /// same estimates.
    #[must_use]
    pub fn json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// What was measured on a gateway's channel in the 30 days before `now`, the forgotten left out
/// (§16.4's third column): the calls and how many it answered, each way's SNR over the sessions
/// that read it, the rate of the sessions long enough to judge, and the links lost.
#[must_use]
pub fn measured(
    records: &[Value],
    forgotten: &Forgotten,
    (callsign, channel): (&str, &str),
    now: u64,
) -> Value {
    let since = now.saturating_sub(30 * 86_400);
    let (mut down, mut up, mut rates) = (Vec::new(), Vec::new(), Vec::new());
    let (mut calls, mut answered, mut sessions, mut lost) = (0, 0, 0, 0);
    for v in records.iter().filter(|v| !forgotten.forgets(v)) {
        let Some(r) = Rec::read(v) else { continue };
        if r.start < since
            || !r.callsign.eq_ignore_ascii_case(callsign)
            || r.candidate().is_none_or(|c| c.channel != channel)
            || matches!(
                r.class,
                Class::Cancelled | Class::LocalStation | Class::ChannelBusy | Class::Unknown
            )
        {
            continue;
        }
        calls += 1;
        if r.linked() || r.class == Class::Refused {
            answered += 1;
        }
        if r.linked() {
            sessions += 1;
            if r.class == Class::LinkLost {
                lost += 1;
            }
            if r.link_s >= ETA_MIN_S && r.bytes >= ETA_MIN_BYTES {
                rates.push(r.bytes * 8.0 / r.link_s);
            }
        }
        down.extend(r.snr_down);
        up.extend(r.snr_up);
    }
    let range = |v: &[f64]| {
        if v.is_empty() {
            return Value::Null;
        }
        let min = v.iter().copied().fold(f64::INFINITY, f64::min);
        let max = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        serde_json::json!({"min": (min * 10.0).round() / 10.0, "max": (max * 10.0).round() / 10.0,
                           "n": v.len()})
    };
    serde_json::json!({"calls": calls, "answered": answered, "sessions": sessions, "lost": lost,
                       "down": range(&down), "up": range(&up), "bps": range(&rates)})
}

/// How many calls are in each record's episode: calls to one channel within ten minutes of the one
/// before.
fn episode_sizes(all: &[Rec]) -> Vec<usize> {
    let mut group = vec![0usize; all.len()];
    let mut last: BTreeMap<Option<Candidate>, (u64, usize)> = BTreeMap::new();
    let mut sizes: Vec<usize> = Vec::new();
    for (i, r) in all.iter().enumerate() {
        let c = r.candidate();
        let id = match last.get(&c) {
            Some(&(t, g)) if r.start.saturating_sub(t) <= EPISODE_S => g,
            _ => {
                sizes.push(0);
                sizes.len() - 1
            }
        };
        sizes[id] += 1;
        group[i] = id;
        last.insert(c, (r.start, id));
    }
    group.into_iter().map(|g| sizes[g]).collect()
}

/// Station health over the last day (§12.3): the run of failures since the last call that worked,
/// each one predicted to connect and nothing heard; the chance the station is at fault from a
/// prior of 0.05 and each failure's likelihood ratio, 0.95 against 1 − p̂. Returns the health and
/// that chance for each failure in the run, by record ID.
fn health(all: &[Rec], now: u64) -> (Health, BTreeMap<String, f64>) {
    let mut odds = FAULT_PRIOR / (1.0 - FAULT_PRIOR);
    let mut run: Vec<&Rec> = Vec::new();
    for r in all
        .iter()
        .filter(|r| now.saturating_sub(r.start) <= HEALTH_S)
    {
        let p = r.predicted.as_ref().and_then(Predicted::chance);
        match r.class {
            Class::NoAnswer => {
                if let Some(p) = p.filter(|&p| p >= HEALTH_LIKELY) {
                    odds *= FAIL_WITH_FAULT / (1.0 - p).max(0.01);
                    run.push(r);
                }
            }
            Class::LocalStation => odds *= FAIL_WITH_FAULT / (1.0 - FAIL_WITH_FAULT),
            c if c.completed() || r.linked() || c == Class::NoAnswerHeard => {
                // the station transmitted and was heard: no fault, and the run is over
                odds = FAULT_PRIOR / (1.0 - FAULT_PRIOR);
                run.clear();
            }
            _ => {}
        }
    }
    let p_fault = odds / (1.0 + odds);
    let gateways: BTreeSet<&str> = run.iter().map(|r| r.callsign.as_str()).collect();
    let bands: BTreeSet<String> = run
        .iter()
        .filter_map(|r| r.candidate().map(|c| c.channel))
        .collect();
    let warning = (run.len() >= 3 && (gateways.len() >= 2 || bands.len() >= 2)).then(|| {
        format!(
            "Your last {} calls failed on {} gateway{} that usually answer. Check the radio, \
             antenna and audio before blaming the gateways.",
            run.len(),
            gateways.len(),
            if gateways.len() == 1 { "" } else { "s" }
        )
    });
    let fault = run.iter().map(|r| (r.id.clone(), p_fault)).collect();
    (
        Health {
            p_fault,
            failures: run.len(),
            gateways: gateways.len(),
            warning,
        },
        fault,
    )
}

/// Pools `successes` of `attempts` toward `parent` with `n0` (§12.4).
fn pooled(successes: f64, attempts: f64, parent: f64, n0: f64) -> f64 {
    (successes + n0 * parent) / (attempts + n0)
}

type Weight<'a> = &'a dyn Fn(usize, f64) -> f64;

/// Availability: each gateway's Beta, overall and by three-hour block, pooled block → gateway →
/// all gateways; its prior from when its post office last saw it. A silent call counts against it
/// only by the share the gateway's absence explains (§12.3): `away` has that share for the calls
/// a grid has learned, by record; the others take it from the path's predicted chance.
fn availability(
    all: &[Rec],
    weight: Weight<'_>,
    seen: &Seen,
    now: u64,
    away: &BTreeMap<usize, f64>,
) -> BTreeMap<String, (Estimate, [f64; 8])> {
    // every gateway's counts, overall and by block
    let mut counts: BTreeMap<&str, ([f64; 2], [[f64; 2]; 8])> = BTreeMap::new();
    let mut total = [0.0, 0.0];
    for (i, r) in all.iter().enumerate() {
        let w = weight(i, HALF_AVAILABILITY);
        let there = r.linked() || matches!(r.class, Class::NoAnswerHeard | Class::Refused);
        let away = if let Some(&share) = away.get(&i) {
            share
        } else if r.class == Class::NoAnswer {
            let c = counts.get(r.callsign.as_str());
            let a = c.map_or(0.5, |c| pooled(c.0[0], c.0[0] + c.0[1], 0.5, N0_GATEWAY));
            let p = r
                .predicted
                .as_ref()
                .and_then(Predicted::chance)
                .unwrap_or(0.5);
            (1.0 - a) / ((1.0 - a) + a * (1.0 - p)).max(1e-9)
        } else {
            0.0
        };
        if !there && away == 0.0 {
            continue;
        }
        let (s, f) = if there { (w, 0.0) } else { (0.0, w * away) };
        let block = usize::try_from(r.start % 86_400 / 10_800).unwrap_or(0);
        let c = counts.entry(r.callsign.as_str()).or_default();
        c.0[0] += s;
        c.0[1] += f;
        c.1[block][0] += s;
        c.1[block][1] += f;
        total[0] += s;
        total[1] += f;
    }
    let all_gateways = pooled(total[0], total[0] + total[1], 0.5, N0_GATEWAY);
    counts
        .into_iter()
        .map(|(callsign, (gw, blocks))| {
            // seen by its post office within two days: likely there; not for longer: less so
            let prior = seen.get(callsign).map_or(all_gateways, |&s| {
                if now.saturating_sub(s) <= 2 * 86_400 {
                    0.8
                } else {
                    0.4
                }
            });
            let a = pooled(gw[0], gw[0] + gw[1], prior, N0_GATEWAY);
            let by_block = blocks.map(|b| pooled(b[0], b[0] + b[1], a, N0_BLOCK));
            (
                callsign.to_owned(),
                (
                    Estimate {
                        mean: a,
                        n: gw[0] + gw[1],
                    },
                    by_block,
                ),
            )
        })
        .collect()
}

/// Service: each gateway's Beta from its links (a refusal or a service failure counts against it,
/// as does a link lost with the signal strong just before), pooled toward all gateways, under a
/// prior of 0.95 worth two (§12.4, §12.6).
fn service(all: &[Rec], weight: Weight<'_>) -> BTreeMap<String, Estimate> {
    let mut counts: BTreeMap<&str, [f64; 2]> = BTreeMap::new();
    let mut total = [0.0, 0.0];
    for (i, r) in all.iter().enumerate() {
        let failed = match r.class {
            Class::ServiceFailed | Class::Refused => true,
            Class::LinkLost => not_a_fade(r),
            _ if r.linked() => false,
            _ => continue,
        };
        let w = weight(i, HALF_SERVICE);
        let c = counts.entry(r.callsign.as_str()).or_default();
        c[usize::from(failed)] += w;
        total[usize::from(failed)] += w;
    }
    let all_gateways = pooled(total[0], total[0] + total[1], SERVICE_PRIOR, N0_GATEWAY);
    counts
        .into_iter()
        .map(|(c, n)| {
            (
                c.to_owned(),
                Estimate {
                    mean: pooled(n[0], n[0] + n[1], all_gateways, N0_GATEWAY),
                    n: n[0] + n[1],
                },
            )
        })
        .collect()
}

/// A loss with the SNR well above the call's threshold just before it: no fade (§12.6).
fn not_a_fade(r: &Rec) -> bool {
    let threshold = match &r.predicted {
        Some(Predicted::Hf { call_db, .. }) => *call_db,
        _ => None,
    };
    r.last_snr
        .zip(threshold)
        .is_some_and(|(s, t)| s > t + NOT_A_FADE_DB)
}

/// Occupancy: each dial's Beta by hour of the day, pooled toward the dial's own over all hours,
/// under a prior of 0.1 worth two (§12.4).
fn occupancy(all: &[Rec], now: u64) -> BTreeMap<u64, [f64; 24]> {
    let mut counts: BTreeMap<u64, [[f64; 2]; 24]> = BTreeMap::new();
    for r in all {
        let Some(dial) = r.dial else { continue };
        if r.class == Class::LocalStation || r.class == Class::Unknown {
            continue;
        }
        #[allow(clippy::cast_precision_loss, reason = "seconds")]
        let w = decay((now - r.start.min(now)) as f64, HALF_OCCUPANCY);
        let hour = usize::try_from(r.start % 86_400 / 3600).unwrap_or(0);
        counts.entry(dial).or_insert([[0.0; 2]; 24])[hour][usize::from(!r.busy)] += w;
    }
    counts
        .into_iter()
        .map(|(dial, hours)| {
            let busy: f64 = hours.iter().map(|h| h[0]).sum();
            let n: f64 = hours.iter().map(|h| h[0] + h[1]).sum();
            let dial_all = pooled(busy, n, OCCUPANCY_PRIOR, N0_GATEWAY);
            (
                dial,
                hours.map(|h| pooled(h[0], h[0] + h[1], dial_all, N0_BLOCK)),
            )
        })
        .collect()
}

/// The fastest rate the modem's profile carries at `snr` on the record's channel, bit/s.
fn modelled_rate(r: &Rec, snr: f64) -> Option<f64> {
    let Some(Predicted::Hf {
        call_db: Some(_), ..
    }) = &r.predicted
    else {
        return None;
    };
    // the class the prediction read the path as: the threshold it was made with names it
    let class = [hf::Class::Good, hf::Class::Moderate, hf::Class::Poor]
        .into_iter()
        .find(|c| match &r.predicted {
            Some(Predicted::Hf { call_db, .. }) => *call_db == Some(Profile::aether().call_db(*c)),
            _ => false,
        })?;
    Profile::aether()
        .steps(r.bandwidth, class)
        .iter()
        .rev()
        .find(|s| s.threshold <= snr)
        .map(|s| s.bps)
}

/// The modem's efficiency: the log of measured goodput over the profile's rate at the SNR
/// measured, averaged by weight, for each modem and bandwidth, under a prior (§12.4); for one
/// gateway when `only` names it, pooled toward the modem's.
fn efficiency(
    all: &[Rec],
    weight: Weight<'_>,
    only: Option<(&str, &BTreeMap<String, Estimate>)>,
) -> BTreeMap<String, Estimate> {
    let mut sums: BTreeMap<String, [f64; 2]> = BTreeMap::new();
    for (i, r) in all.iter().enumerate() {
        if only.is_some_and(|(c, _)| c != r.callsign) || !r.linked() {
            continue;
        }
        if r.link_s < ETA_MIN_S || r.bytes < ETA_MIN_BYTES {
            continue;
        }
        let Some(model) = r.snr_down.and_then(|s| modelled_rate(r, s)) else {
            continue;
        };
        let ratio = r.bytes * 8.0 / r.link_s / model;
        let w = weight(i, HALF_EFFICIENCY);
        let e = sums.entry(efficiency_key(r)).or_default();
        e[0] += w * ratio.ln();
        e[1] += w;
    }
    sums.into_iter()
        .map(|(k, [sum, n])| {
            let parent = match only {
                Some((_, modems)) => modems.get(&k).map_or_else(|| start_eta(&k), |e| e.mean),
                None => start_eta(&k),
            };
            let n0 = if only.is_some() { N0_BLOCK } else { N0_GATEWAY };
            let mean = ((sum + n0 * parent.ln()) / (n + n0)).exp();
            (k, Estimate { mean, n })
        })
        .collect()
}

fn efficiency_key(r: &Rec) -> String {
    let modem = r
        .modem
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    format!("{modem} {}", r.bandwidth)
}

fn start_eta(key: &str) -> f64 {
    if key.starts_with("aether") {
        ETA_AETHER
    } else {
        ETA_OTHER
    }
}

/// The direction offset's prior: the uplink's residual less the downlink's, over the sessions that
/// measured both ways, pooled toward nought. Those sessions cleared the call both ways, so this is
/// biased toward nought where one way is often too weak: the grids learn D from every call, with
/// this only as their prior's centre.
fn direction_prior(all: &[Rec], weight: Weight<'_>) -> Offset {
    let (mut sum, mut n) = (0.0, 0.0);
    for (i, r) in all.iter().enumerate() {
        let (Some(p), Some(m_down), Some(m_up)) = (&r.predicted, r.snr_down, r.snr_up) else {
            continue;
        };
        let (Some(rd), Some(ru)) = (p.residual_down(m_down), p.residual_up(m_up)) else {
            continue;
        };
        let w = weight(i, HALF_OFFSET_HF);
        sum += w * (ru - rd);
        n += w;
    }
    Offset {
        mean: sum / (n + N0_GATEWAY),
        sd: DIRECTION_SD / (1.0 + n).sqrt(),
    }
}

/// The direction offset over all gateways, from the grids: the mixture of each channel's posterior
/// D, weighted by the calls behind it; `None` before any.
fn direction_learned(channels: &BTreeMap<String, Learned>) -> Option<Offset> {
    let n: f64 = channels.values().map(|l| l.calls).sum();
    if n <= 0.0 {
        return None;
    }
    let mean = channels
        .values()
        .map(|l| l.calls * l.direction.mean)
        .sum::<f64>()
        / n;
    let var = channels
        .values()
        .map(|l| l.calls * (l.direction.sd.powi(2) + (l.direction.mean - mean).powi(2)))
        .sum::<f64>()
        / n;
    Some(Offset {
        mean,
        sd: var.sqrt(),
    })
}

/// The offsets learned over all gateways on each channel (a band, or a VHF dial): the weighted mean
/// residual of measured downlinks, shrunk toward 0 (§12.5: each gateway's starts there).
fn channel_offsets(all: &[Rec], weight: Weight<'_>) -> BTreeMap<String, f64> {
    let mut sums: BTreeMap<String, [f64; 2]> = BTreeMap::new();
    for (i, r) in all.iter().enumerate() {
        let (Some(p), Some(snr), Some(candidate)) = (&r.predicted, r.snr_down, r.candidate())
        else {
            continue;
        };
        let Some(res) = p.residual_down(snr) else {
            continue;
        };
        let days = if r.hf() {
            HALF_OFFSET_HF
        } else {
            HALF_OFFSET_VHF
        };
        let w = weight(i, days);
        let e = sums.entry(candidate.channel).or_default();
        e[0] += w * res;
        e[1] += w;
    }
    sums.into_iter()
        .map(|(k, [s, n])| (k, s / (n + N0_GATEWAY)))
        .collect()
}

/// One candidate's posterior on the grid (§12.5): its log density over the downlink's offset δ,
/// the direction offset D (the uplink's is δ + D), availability A, and the share *h* of the calls a
/// gateway that is there never heard on which it is heard all the same, on the air for another
/// station. Without *h* the two rows for an unanswered call count the uplink's failure twice: a
/// call heard says the uplink failed, and a silent call's likelihood never says that, had the
/// uplink failed, more of the failures would have been heard (3 dB too low on a simulated stream
/// where a third of them were).
struct Grid {
    ln: Vec<f64>,
}

/// A grid's posterior, summed up: each offset's mean and spread, and A's mean.
struct Summary {
    down: Offset,
    up: Offset,
    direction: Offset,
    there: f64,
}

/// The heard share's steps, under a uniform prior: most gateways are seldom on the air for someone
/// else during a call, a busy one often.
const H_SHARES: [f64; 5] = [0.02, 0.1, 0.25, 0.5, 0.8];
const H_STEPS: usize = H_SHARES.len();
/// One (δ, D) pair's block of cells: A by *h*.
const BLOCK: usize = A_STEPS * H_STEPS;

fn delta_at(i: usize) -> f64 {
    #[allow(clippy::cast_precision_loss, reason = "a grid index")]
    let d = DELTA_MIN + i as f64;
    d
}

fn dir_at(k: usize) -> f64 {
    #[allow(clippy::cast_precision_loss, reason = "a grid index")]
    let d = f64::from(DIR_MIN) + k as f64;
    d
}

fn a_at(j: usize) -> f64 {
    #[allow(clippy::cast_precision_loss, reason = "a grid index")]
    let a = (j as f64 + 0.5) / A_STEPS as f64;
    a
}

/// The first cell of the (δ, D) pair `(i, k)`; A then *h* follow.
fn block(i: usize, k: usize) -> usize {
    (i * DIR_STEPS + k) * BLOCK
}

/// A record's terms on the grid, worked out once: each over the downlink's offset, or over the
/// uplink's (δ + D, from `DELTA_MIN + DIR_MIN` up, one per step).
struct Terms {
    kind: Kind,
    down: Vec<f64>,
    up: Vec<f64>,
    open: f64,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// Linked, each way's log density of what it measured (or of linking, without a reading).
    Linked,
    /// No answer, nothing heard: `down` and `up` the chance each way carries.
    Silent,
    /// No answer, the station heard: `down` its log density (or the log chance the downlink
    /// carries, without a reading), `up` the log chance the uplink fails.
    Heard,
}

const UP_STEPS: usize = DELTA_STEPS + DIR_STEPS - 1;

fn up_at(u: usize) -> f64 {
    #[allow(clippy::cast_precision_loss, reason = "a grid index")]
    let d = DELTA_MIN + f64::from(DIR_MIN) + u as f64;
    d
}

impl Terms {
    /// A record's terms, `None` when it says nothing of the path (§12.5's table).
    fn of(r: &Rec, p: &Predicted) -> Option<Terms> {
        let open = match p {
            Predicted::Hf { open, .. } => *open,
            Predicted::Vhf { .. } => 1.0,
        };
        let ln = |x: f64| x.max(1e-12).ln();
        // the log chance each way carries, the other way at 99 dB, where it carries every day
        let carries_down = || {
            (0..DELTA_STEPS)
                .map(|i| p.p_link(delta_at(i), 99.0).map(ln))
                .collect::<Option<Vec<f64>>>()
        };
        if r.linked() || r.class == Class::Refused {
            let down: Option<Vec<f64>> = (0..DELTA_STEPS)
                .map(|i| r.snr_down.and_then(|m| p.ln_density_down(m, delta_at(i))))
                .collect();
            let up: Option<Vec<f64>> = (0..UP_STEPS)
                .map(|u| r.snr_up.and_then(|m| p.ln_density_up(m, up_at(u))))
                .collect();
            let (down, up) = match (down, up) {
                (Some(d), Some(u)) => (d, u),
                (Some(d), None) => (d, vec![0.0; UP_STEPS]),
                (None, Some(u)) => (vec![0.0; DELTA_STEPS], u),
                // no reading: the chance of linking at all
                (None, None) => (
                    carries_down()?,
                    (0..UP_STEPS)
                        .map(|u| p.p_link(99.0, up_at(u)).map(ln))
                        .collect::<Option<_>>()?,
                ),
            };
            return Some(Terms {
                kind: Kind::Linked,
                down,
                up,
                open,
            });
        }
        match r.class {
            Class::NoAnswer => Some(Terms {
                kind: Kind::Silent,
                down: (0..DELTA_STEPS)
                    .map(|i| p.p_link(delta_at(i), 99.0))
                    .collect::<Option<_>>()?,
                up: (0..UP_STEPS)
                    .map(|u| p.p_link(99.0, up_at(u)))
                    .collect::<Option<_>>()?,
                open,
            }),
            Class::NoAnswerHeard => {
                let measured: Option<Vec<f64>> = (0..DELTA_STEPS)
                    .map(|i| r.heard_snr.and_then(|m| p.ln_density_down(m, delta_at(i))))
                    .collect();
                Some(Terms {
                    kind: Kind::Heard,
                    down: match measured {
                        Some(d) => d,
                        None => carries_down()?,
                    },
                    up: (0..UP_STEPS)
                        .map(|u| p.p_up_fails(up_at(u)).map(ln))
                        .collect::<Option<_>>()?,
                    open,
                })
            }
            _ => None,
        }
    }

    /// Adds `weight` times the log likelihood to the (δ, D) pair `(i, k)`'s block of A by *h*.
    fn add(&self, i: usize, k: usize, weight: f64, cells: &mut [f64], logs: &Logs) {
        let up = i + k;
        match self.kind {
            Kind::Linked => {
                let both = self.down[i] + self.up[up];
                for (j, row) in cells.chunks_exact_mut(H_STEPS).enumerate() {
                    let v = weight * (logs.a[j] + both);
                    for cell in row {
                        *cell += v;
                    }
                }
            }
            Kind::Heard => {
                let both = self.down[i] + self.up[up];
                for (j, row) in cells.chunks_exact_mut(H_STEPS).enumerate() {
                    for (h, cell) in row.iter_mut().enumerate() {
                        *cell += weight * (logs.a[j] + logs.h[h] + both);
                    }
                }
            }
            Kind::Silent => {
                // `down` and `up` each include the band's open days once; with the gateway there,
                // the call links with chance `link` and fails heard with chance `heardable` · h
                let link = self.down[i] * self.up[up] / self.open.max(1e-9);
                let heardable = (self.down[i] - link).max(0.0);
                for (j, row) in cells.chunks_exact_mut(H_STEPS).enumerate() {
                    let a = a_at(j);
                    for (h, cell) in row.iter_mut().enumerate() {
                        let silent = 1.0 - a * (link + H_SHARES[h] * heardable);
                        *cell += weight * silent.max(1e-12).ln();
                    }
                }
            }
        }
    }
}

impl Terms {
    /// For a silent call, the chance the gateway was away, under `posterior` (every cell, this call
    /// among the evidence): Σ posterior · (1 − A) / P(silent), the call's own likelihood divided out
    /// cell by cell.
    fn away(&self, posterior: &[f64]) -> f64 {
        let mut sum = 0.0;
        for i in 0..DELTA_STEPS {
            for k in 0..DIR_STEPS {
                let link = self.down[i] * self.up[i + k] / self.open.max(1e-9);
                let heardable = (self.down[i] - link).max(0.0);
                let start = block(i, k);
                let cells = &posterior[start..start + BLOCK];
                for (j, row) in cells.chunks_exact(H_STEPS).enumerate() {
                    let a = a_at(j);
                    for (h, q) in row.iter().enumerate() {
                        let silent = 1.0 - a * (link + H_SHARES[h] * heardable);
                        sum += q * (1.0 - a) / silent.max(1e-12);
                    }
                }
            }
        }
        sum.clamp(0.0, 1.0)
    }
}

/// The logs of A's and *h*'s steps, worked out once.
struct Logs {
    a: [f64; A_STEPS],
    h: [f64; H_STEPS],
}

impl Logs {
    fn new() -> Logs {
        Logs {
            a: std::array::from_fn(|j| a_at(j).ln()),
            h: H_SHARES.map(f64::ln),
        }
    }
}

impl Grid {
    /// The prior: δ about `centre` with spread `sd`; D about `dir` with §12.5's 6 dB; A a Beta of
    /// mean `a_prior` worth two calls; *h* uniform over its steps.
    fn new(centre: f64, sd: f64, a_prior: f64, dir: f64) -> Grid {
        let (alpha, beta) = (1.0 + 2.0 * a_prior, 1.0 + 2.0 * (1.0 - a_prior));
        let mut ln = vec![0.0; DELTA_STEPS * DIR_STEPS * BLOCK];
        for i in 0..DELTA_STEPS {
            let x = (delta_at(i) - centre) / sd;
            for k in 0..DIR_STEPS {
                let y = (dir_at(k) - dir) / DIRECTION_SD;
                let start = block(i, k);
                for (j, row) in ln[start..start + BLOCK]
                    .chunks_exact_mut(H_STEPS)
                    .enumerate()
                {
                    let a = a_at(j);
                    let v = -0.5 * x * x - 0.5 * y * y
                        + (alpha - 1.0) * a.ln()
                        + (beta - 1.0) * (1.0 - a).ln();
                    row.fill(v);
                }
            }
        }
        Grid { ln }
    }

    fn update(&mut self, t: &Terms, w: f64, logs: &Logs) {
        for i in 0..DELTA_STEPS {
            for k in 0..DIR_STEPS {
                let start = block(i, k);
                t.add(i, k, w, &mut self.ln[start..start + BLOCK], logs);
            }
        }
    }

    /// The posterior over every cell.
    fn normalised(&self) -> Vec<f64> {
        let top = self.ln.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let p: Vec<f64> = self.ln.iter().map(|l| (l - top).exp()).collect();
        let sum: f64 = p.iter().sum();
        p.into_iter().map(|x| x / sum).collect()
    }

    /// The posterior over (δ, D, A), *h* summed out.
    fn marginal(&self) -> Vec<f64> {
        let top = self.ln.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let p: Vec<f64> = self
            .ln
            .chunks_exact(H_STEPS)
            .map(|row| row.iter().map(|l| (l - top).exp()).sum())
            .collect();
        let sum: f64 = p.iter().sum();
        p.into_iter().map(|x| x / sum).collect()
    }

    /// What the grid holds, *h* summed out.
    fn summary(&self) -> Summary {
        let p = self.marginal();
        let mut m = [[0.0; 2]; 3];
        let mut a = 0.0;
        for i in 0..DELTA_STEPS {
            for k in 0..DIR_STEPS {
                let values = [delta_at(i), delta_at(i) + dir_at(k), dir_at(k)];
                let start = block(i, k) / H_STEPS;
                for (j, q) in p[start..start + A_STEPS].iter().enumerate() {
                    for (moments, v) in m.iter_mut().zip(values) {
                        moments[0] += q * v;
                        moments[1] += q * v * v;
                    }
                    a += q * a_at(j);
                }
            }
        }
        let offset = |[mean, m2]: [f64; 2]| Offset {
            mean,
            sd: (m2 - mean * mean).max(0.0).sqrt(),
        };
        Summary {
            down: offset(m[0]),
            up: offset(m[1]),
            direction: offset(m[2]),
            there: a,
        }
    }

    /// The chance the posterior gives an outcome: a link, or none.
    fn predictive(&self, p: &Predicted, linked: bool) -> Option<f64> {
        let post = self.marginal();
        let down: Vec<f64> = (0..DELTA_STEPS)
            .map(|i| p.p_link(delta_at(i), 99.0))
            .collect::<Option<_>>()?;
        let up: Vec<f64> = (0..UP_STEPS)
            .map(|u| p.p_link(99.0, up_at(u)))
            .collect::<Option<_>>()?;
        let open = match p {
            Predicted::Hf { open, .. } => *open,
            Predicted::Vhf { .. } => 1.0,
        };
        let mut sum = 0.0;
        for i in 0..DELTA_STEPS {
            for k in 0..DIR_STEPS {
                let both = down[i] * up[i + k] / open.max(1e-9);
                let start = block(i, k) / H_STEPS;
                for (j, q) in post[start..start + A_STEPS].iter().enumerate() {
                    let link = a_at(j) * both;
                    sum += q * if linked { link } else { 1.0 - link };
                }
            }
        }
        Some(sum)
    }
}

/// Each candidate's learning: the offset grid, its losses and its efficiency, and drift (§12.7).
/// Each silent call's share blamed on the gateway's absence, by record, comes back with them.
fn channels(
    all: &[Rec],
    weight: Weight<'_>,
    learning: &Learning,
) -> (BTreeMap<String, Learned>, BTreeMap<usize, f64>) {
    let offsets = channel_offsets(all, weight);
    let mut by: BTreeMap<Candidate, Vec<usize>> = BTreeMap::new();
    for (i, r) in all.iter().enumerate() {
        if let Some(c) = r.candidate() {
            by.entry(c).or_default().push(i);
        }
    }
    let healthy = learning.health.p_fault < 0.5;
    let context = Context {
        all,
        learning,
        offsets: &offsets,
    };
    let mut away = BTreeMap::new();
    let learned = by
        .into_iter()
        .map(|(c, idx)| {
            let (mut l, drift, mut shares) = context.learn(&idx, weight, &c);
            if healthy && drift {
                // forget faster until it agrees with its evidence again: a quarter of the half-lives
                let quick = |i: usize, days: f64| weight(i, days / 4.0);
                (l, _, shares) = context.learn(&idx, &quick, &c);
                l.drift = true;
            }
            away.extend(shares);
            (key(&c.callsign, &c.channel), l)
        })
        .collect();
    (learned, away)
}

/// What learning one candidate needs of the rest.
struct Context<'a> {
    all: &'a [Rec],
    learning: &'a Learning,
    offsets: &'a BTreeMap<String, f64>,
}

impl Context<'_> {
    /// One candidate's offsets, availability on the grid, losses and efficiency, from the records
    /// `idx` weighed by `weight`; whether its last three episodes were jointly very unlikely,
    /// each under what had been learned before it (§12.7); and each silent call's share blamed on
    /// the gateway's absence, under what was learned.
    fn learn(
        &self,
        idx: &[usize],
        weight: Weight<'_>,
        c: &Candidate,
    ) -> (Learned, bool, Vec<(usize, f64)>) {
        let all = self.all;
        let hf = idx.first().is_none_or(|&i| all[i].hf());
        let days = if hf { HALF_OFFSET_HF } else { HALF_OFFSET_VHF };
        let a_prior = self
            .learning
            .availability
            .get(&c.callsign)
            .map_or(0.5, |a| a.0.mean);
        let centre = self.offsets.get(&c.channel).copied().unwrap_or(0.0);
        let sd = if hf { OFFSET_SD_HF } else { OFFSET_SD_VHF };
        let mut grid = Grid::new(centre, sd, a_prior, self.learning.direction.mean);
        let logs = Logs::new();
        let watched = last_episodes(all, idx);
        let mut seen: Vec<f64> = Vec::new();
        let (mut measured, mut calls) = (0.0, 0.0);
        let (mut losses, mut seconds) = (0.0, 0.0);
        for &i in idx {
            let r = &all[i];
            if watched.contains(&i)
                && let Some(p) = &r.predicted
                && let Some(q) = grid.predictive(p, episode_linked(all, idx, i))
            {
                seen.push(q);
            }
            let w = weight(i, days);
            if let Some(t) = r.predicted.as_ref().and_then(|p| Terms::of(r, p)) {
                grid.update(&t, w, &logs);
                calls += w;
                if r.snr_down.is_some() || r.snr_up.is_some() {
                    measured += w;
                }
            }
            if r.linked() {
                let wl = weight(i, HALF_LOSSES);
                seconds += wl * r.link_s;
                if r.class == Class::LinkLost && !not_a_fade(r) {
                    losses += wl;
                }
            }
        }
        let drift = seen.len() == 3 && seen.iter().product::<f64>() < DRIFT_P;
        let Summary {
            down,
            up,
            direction,
            there,
        } = grid.summary();
        let posterior = grid.normalised();
        let away = idx
            .iter()
            .filter(|&&i| all[i].class == Class::NoAnswer)
            .filter_map(|&i| {
                let r = &all[i];
                let t = r.predicted.as_ref().and_then(|p| Terms::of(r, p))?;
                Some((i, t.away(&posterior)))
            })
            .collect();
        let per_s = (losses + loss_rate_all(all, weight) * LOSS_PRIOR_S) / (seconds + LOSS_PRIOR_S);
        let modems = &self.learning.efficiency;
        let gw_eta = efficiency(all, weight, Some((&c.callsign, modems)));
        let modem_key = idx
            .last()
            .map(|&i| efficiency_key(&all[i]))
            .unwrap_or_default();
        let efficiency = gw_eta.get(&modem_key).copied().unwrap_or_else(|| Estimate {
            mean: modems
                .get(&modem_key)
                .map_or_else(|| start_eta(&modem_key), |e| e.mean),
            n: 0.0,
        });
        let learned = Learned {
            down,
            up,
            direction,
            there,
            measured,
            calls,
            losses_per_hour: per_s * 3600.0,
            efficiency,
            drift: false,
        };
        (learned, drift, away)
    }
}

/// The first calls of a candidate's last three episodes, when it has four or more: drift is judged
/// on them (§12.7).
fn last_episodes(all: &[Rec], idx: &[usize]) -> Vec<usize> {
    let mut firsts: Vec<usize> = Vec::new();
    let mut last: Option<u64> = None;
    for &i in idx {
        if last.is_none_or(|t| all[i].start - t > EPISODE_S) {
            firsts.push(i);
        }
        last = Some(all[i].start);
    }
    if firsts.len() < 4 {
        return Vec::new();
    }
    firsts.split_off(firsts.len() - 3)
}

/// Whether the episode that begins with record `first` brought a link.
fn episode_linked(all: &[Rec], idx: &[usize], first: usize) -> bool {
    let start = all[first].start;
    let mut last = start;
    idx.iter()
        .copied()
        .filter(|&i| all[i].start >= start)
        .take_while(|&i| {
            let near = all[i].start - last <= EPISODE_S;
            last = all[i].start;
            near
        })
        .any(|i| all[i].linked())
}

/// Losses a second of link over all sessions, under a prior of one an hour.
fn loss_rate_all(all: &[Rec], weight: Weight<'_>) -> f64 {
    let (mut losses, mut seconds) = (0.0, 0.0);
    for (i, r) in all.iter().enumerate().filter(|(_, r)| r.linked()) {
        let w = weight(i, HALF_LOSSES);
        seconds += w * r.link_s;
        if r.class == Class::LinkLost && !not_a_fade(r) {
            losses += w;
        }
    }
    (losses + LOSS_PRIOR_PER_S * LOSS_PRIOR_S) / (seconds + LOSS_PRIOR_S)
}

/// Overheads: the weighted medians of the time to connect and of a failed call, by modem,
/// bandwidth and whether the gateway scans (§12.4).
fn overheads(all: &[Rec], now: u64) -> BTreeMap<String, (Option<f64>, Option<f64>)> {
    let mut connect: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
    let mut failed: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
    for r in all {
        let Some(s) = r.call_s else { continue };
        let key = format!(
            "{} {}",
            efficiency_key(r),
            if r.scanned { "scanned" } else { "fixed" }
        );
        #[allow(clippy::cast_precision_loss, reason = "seconds")]
        let w = decay((now - r.start.min(now)) as f64, HALF_OVERHEADS);
        if r.linked() {
            connect.entry(key).or_default().push((s, w));
        } else if r.class == Class::NoAnswer {
            failed.entry(key).or_default().push((s, w));
        }
    }
    let keys: BTreeSet<String> = connect.keys().chain(failed.keys()).cloned().collect();
    keys.into_iter()
        .map(|k| {
            let c = connect.get(&k).and_then(|v| weighted_median(v));
            let f = failed.get(&k).and_then(|v| weighted_median(v));
            (k, (c, f))
        })
        .collect()
}

fn weighted_median(values: &[(f64, f64)]) -> Option<f64> {
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f64 = v.iter().map(|x| x.1).sum();
    let mut acc = 0.0;
    for (x, w) in &v {
        acc += w;
        if acc >= total / 2.0 {
            return Some(*x);
        }
    }
    None
}
