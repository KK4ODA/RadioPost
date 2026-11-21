// SPDX-License-Identifier: MIT OR Apache-2.0
//! Today's evidence (choosing a gateway, §§10.5, 11.4): what this station's own FT8 receiver heard
//! in the last hour toward each gateway, asked of hf-predict's evidence API when hf-predict runs on
//! this computer. RadioPost works the same without it: the rows then have no observation line.
//!
//! One request carries every gateway (`toward` repeated). It is a one-shot exchange on loopback
//! that gives up at its limits, never a read resumed. An answer is kept a minute, and so is
//! hf-predict's absence: a closed port on Windows takes about two seconds to refuse.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{Read, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use radioprop_geo::LatLon;
use serde_json::Value;

/// hf-predict's evidence API, unless `RADIOPOST_HF_PREDICT` names another address (or `off`).
pub const DEFAULT_AT: &str = "127.0.0.1:8625";
/// How far back the evidence counts: the last hour (§ 10.5).
pub const SPAN_S: i64 = 3600;
/// How long an answer, or hf-predict's absence, is kept.
const KEEP: Duration = Duration::from_secs(60);
const CONNECT_LIMIT: Duration = Duration::from_millis(300);
const READ_LIMIT: Duration = Duration::from_secs(4);
/// An answer larger than this is not read.
const MAX_ANSWER: u64 = 4 * 1024 * 1024;

/// What hf-predict said of each band toward each place asked about, by the place's index.
pub type Heard = Vec<HashMap<String, Value>>;

/// The evidence API's address, and what it last said.
pub struct Evidence {
    at: Option<SocketAddr>,
    kept: Mutex<Option<(Instant, String, Option<Heard>)>>,
}

impl Evidence {
    /// At [`DEFAULT_AT`], or where `RADIOPOST_HF_PREDICT` says; `off` there asks nothing.
    #[must_use]
    pub fn new() -> Self {
        match std::env::var("RADIOPOST_HF_PREDICT") {
            Ok(v) if v == "off" => Self::off(),
            Ok(v) => Self::at(v.parse().ok()),
            Err(_) => Self::at(DEFAULT_AT.parse().ok()),
        }
    }

    /// At `at`: tests give a stand-in's address.
    #[must_use]
    pub fn at(at: Option<SocketAddr>) -> Self {
        Self {
            at,
            kept: Mutex::new(None),
        }
    }

    /// Asking nothing.
    #[must_use]
    pub fn off() -> Self {
        Self::at(None)
    }

    /// What was heard in the last hour before `now` toward each of `places` on `bands` (named
    /// `40 m`), from `from`; `None` when hf-predict is not there or did not answer.
    pub fn ask(
        &self,
        from: LatLon,
        places: &[LatLon],
        bands: &[String],
        now: i64,
    ) -> Option<Heard> {
        let at = self.at?;
        if places.is_empty() || bands.is_empty() {
            return None;
        }
        let mut query = format!(
            "from={:.4},{:.4}&since={}&bands={}",
            from.lat,
            from.lon,
            // to the minute, so that a page refreshed within it reads what was kept
            (now - SPAN_S) / 60 * 60,
            bands
                .iter()
                .map(|b| b.replace(' ', ""))
                .collect::<Vec<_>>()
                .join(",")
        );
        for p in places {
            let _ = write!(query, "&toward={:.4},{:.4}", p.lat, p.lon);
        }
        if let Ok(kept) = self.kept.lock() {
            if let Some((when, q, heard)) = kept.as_ref() {
                // hf-predict's absence is kept whatever was asked
                if when.elapsed() < KEEP && (q == &query || heard.is_none()) {
                    return heard.clone();
                }
            }
        }
        let heard = get(at, &format!("/v1/evidence?{query}")).and_then(|v| read(&v, places.len()));
        if let Ok(mut kept) = self.kept.lock() {
            *kept = Some((Instant::now(), query, heard.clone()));
        }
        heard
    }
}

impl Evidence {
    /// Whether hf-predict answered when last asked, in the last minute; `None` when not asked.
    #[must_use]
    pub fn answered(&self) -> Option<bool> {
        let kept = self.kept.lock().ok()?;
        let (when, _, heard) = kept.as_ref()?;
        (when.elapsed() < KEEP).then_some(heard.is_some())
    }
}

impl Default for Evidence {
    fn default() -> Self {
        Self::new()
    }
}

/// One GET on loopback, its JSON if it answered 200.
fn get(at: SocketAddr, target: &str) -> Option<Value> {
    let mut s = TcpStream::connect_timeout(&at, CONNECT_LIMIT).ok()?;
    s.set_read_timeout(Some(READ_LIMIT)).ok()?;
    s.set_write_timeout(Some(READ_LIMIT)).ok()?;
    write!(
        s,
        "GET {target} HTTP/1.1\r\nHost: {at}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut text = String::new();
    s.take(MAX_ANSWER).read_to_string(&mut text).ok()?;
    let (head, body) = text.split_once("\r\n\r\n")?;
    head.starts_with("HTTP/1.1 200").then_some(())?;
    serde_json::from_str(body).ok()
}

/// Each place's bands by name, if the answer has one entry for each place asked about.
fn read(v: &Value, places: usize) -> Option<Heard> {
    let answered = v["places"].as_array()?;
    (answered.len() == places).then_some(())?;
    Some(
        answered
            .iter()
            .map(|p| {
                p["bands"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|b| Some((b["band"].as_str()?.to_owned(), b.clone())))
                    .collect()
            })
            .collect(),
    )
}

/// The odds by which moderate or strong evidence raises the chance the band is open toward a
/// gateway (§ 10.5): a fixed, labelled ratio until the station's own records replace it.
pub const OPEN_ODDS: f64 = 2.0;

/// The share of days the band is open, `open`, moved by what was heard on it toward the gateway:
/// raised by [`OPEN_ODDS`] when the evidence is moderate or strong, else left.
#[must_use]
pub fn open_with(open: f64, band: &Value) -> f64 {
    if matches!(band["tier"].as_str(), Some("moderate" | "strong")) && open < 1.0 {
        OPEN_ODDS * open / (1.0 + (OPEN_ODDS - 1.0) * open)
    } else {
        open
    }
}

/// What was heard on a band toward a gateway, in words for its row; `None` when the band was not
/// listened to long enough to say.
#[must_use]
pub fn words(band: &Value) -> Option<String> {
    let name = band["band"].as_str().unwrap_or("the band");
    let stations = band["stations"].as_u64().unwrap_or(0);
    let tier = band["tier"].as_str()?;
    let minutes = band["periods_listened"].as_f64().unwrap_or(0.0) / 4.0;
    Some(match tier {
        "notSampled" => return None,
        "none" => format!(
            "Nothing from near it heard here on {name} in the last hour, in {minutes:.0} min of \
             listening"
        ),
        _ => {
            let best = band["best_snr_db"]
                .as_i64()
                .map(|s| format!(", the strongest at {s} dB"))
                .unwrap_or_default();
            let raised = if matches!(tier, "moderate" | "strong") {
                ": the band is taken as more likely open"
            } else {
                ""
            };
            format!(
                "FT8 from {stations} station{} near it heard here on {name} in the last hour{best}{raised}",
                if stations == 1 { "" } else { "s" }
            )
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_band_is_raised_only_by_moderate_or_strong_evidence() {
        let band = |tier: &str| json!({"band": "40 m", "tier": tier, "stations": 3});
        assert!((open_with(0.5, &band("moderate")) - 2.0 / 3.0).abs() < 1e-12);
        assert!((open_with(0.5, &band("strong")) - 2.0 / 3.0).abs() < 1e-12);
        for tier in ["limited", "none", "notSampled"] {
            assert!((open_with(0.5, &band(tier)) - 0.5).abs() < 1e-12, "{tier}");
        }
        // odds: a sure band stays sure, a closed one closed
        assert!((open_with(1.0, &band("strong")) - 1.0).abs() < 1e-12);
        assert!(open_with(0.0, &band("strong")).abs() < 1e-12);
    }

    #[test]
    fn what_was_heard_reads_plainly() {
        let w = |v: Value| words(&v);
        assert_eq!(
            w(
                json!({"band": "40 m", "tier": "moderate", "stations": 3, "best_snr_db": -8,
                     "periods_listened": 40.0})
            ),
            Some(
                "FT8 from 3 stations near it heard here on 40 m in the last hour, the strongest \
                  at -8 dB: the band is taken as more likely open"
                    .into()
            )
        );
        assert_eq!(
            w(json!({"band": "20 m", "tier": "limited", "stations": 1, "best_snr_db": -20})),
            Some("FT8 from 1 station near it heard here on 20 m in the last hour, the strongest at -20 dB".into())
        );
        assert_eq!(
            w(json!({"band": "20 m", "tier": "none", "stations": 0, "periods_listened": 40.0})),
            Some(
                "Nothing from near it heard here on 20 m in the last hour, in 10 min of listening"
                    .into()
            )
        );
        assert_eq!(w(json!({"band": "20 m", "tier": "notSampled"})), None);
    }

    #[test]
    fn nobody_there_is_none_and_kept() {
        // a port nothing listens on
        let free = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        let at = free.local_addr().expect("bound");
        drop(free);
        let e = Evidence::at(Some(at));
        let here = LatLon {
            lat: 33.75,
            lon: -84.39,
        };
        assert!(
            e.ask(here, &[here], &["40 m".into()], 1_800_000_000)
                .is_none()
        );
        assert!(
            Evidence::off()
                .ask(here, &[here], &["40 m".into()], 1_800_000_000)
                .is_none()
        );
    }
}
