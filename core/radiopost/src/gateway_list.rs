// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-5 §4.3 and §4.4: a gateway list read from the home's list and the list of every post office
//! the home names in `/post-offices`. A client reads it so; a post office reads it for the
//! gateways its users name (RP-5 §5.8). No input or output here: the reader fetches.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;

use crate::base64url;
use crate::crypto;
use crate::error::ErrorCode;
use crate::jws;

/// One entry of a `/post-offices` document (RP-5 §4.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The domain it serves.
    pub domain: String,
    /// Its API's base URL.
    pub api: String,
    /// Its keys by ID in hex, each kept only when the ID is that key's (RP-0 §5).
    pub keys: HashMap<String, [u8; 32]>,
    /// Every domain whose entry names this `api`, this one first: one post office serving
    /// several domains (RP-5 §4.3). From [`entries`], only its own.
    pub domains: Vec<String>,
}

/// The entries of a `/post-offices` payload, in its order; one with no `api` is left out.
#[must_use]
pub fn entries(payload: &Value) -> Vec<Entry> {
    payload["post_offices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let domain = e["domain"].as_str()?.to_lowercase();
            let api = e["api"].as_str().filter(|a| !a.is_empty())?.to_owned();
            let keys = e["keys"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|k| {
                    let public: [u8; 32] =
                        base64url::decode(k["public"].as_str()?)?.try_into().ok()?;
                    let kid = k["kid"].as_str()?;
                    (crate::cbor::hex(&crypto::kid(&public)) == kid)
                        .then(|| (kid.to_owned(), public))
                })
                .collect();
            Some(Entry {
                domains: vec![domain.clone()],
                domain,
                api,
                keys,
            })
        })
        .collect()
}

/// Each API once (RP-5 §4.4): one post office serving several domains is asked once, its list
/// counted once and kept under the first domain naming it, checked with the keys of any entry
/// naming it, and left out for every domain naming it (`domains`). An entry for the home's own
/// domain is passed over: the home's list is taken already.
#[must_use]
pub fn by_api(entries: &[Entry], home: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for e in entries.iter().filter(|e| e.domain != home) {
        match out.iter_mut().find(|o| o.api == e.api) {
            Some(first) => {
                first.domains.push(e.domain.clone());
                for (kid, public) in &e.keys {
                    first.keys.entry(kid.clone()).or_insert(*public);
                }
            }
            None => out.push(Entry {
                domains: vec![e.domain.clone()],
                ..e.clone()
            }),
        }
    }
    out
}

/// A post office's `/gateways` checked with its entry's keys and those its info document adds,
/// signed by one of them (RP-1 §7.2): its records.
///
/// # Errors
///
/// The list is not a JWS those keys verify (RP-0 §5's codes).
pub fn verify_list(entry: &Entry, info: Option<&str>, list: &str) -> Result<Vec<Value>, ErrorCode> {
    let keys = jws::accepted_keys(&entry.keys, info);
    let payload = jws::verify(list, &keys)?;
    Ok(payload["gateways"].as_array().cloned().unwrap_or_default())
}

/// The list a reader makes: every record, each with `post_office`, and the callsigns listed by
/// two post offices neither of which is the home.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Merged {
    /// The home's records first, then each other list's, in order.
    pub gateways: Vec<Value>,
    /// Callsigns in more than one list other than the home's (RP-5 §4.4): a client keeps each
    /// record, and a post office places no copies at them.
    pub ambiguous: Vec<String>,
}

fn tagged(record: &Value, domain: &str) -> Value {
    let mut r = record.clone();
    if let Some(o) = r.as_object_mut() {
        o.insert("post_office".into(), Value::String(domain.to_owned()));
    }
    r
}

/// RP-5 §4.4: the home's records, and those of the `others` lists (each with its post office's
/// domain, as checked by [`verify_list`]) for callsigns the home does not list. A reader whose
/// home says `gateways: "listed"` passes no others.
#[must_use]
pub fn merge(home: &str, home_gateways: &[Value], others: &[(String, Vec<Value>)]) -> Merged {
    let callsign = |r: &Value| r["callsign"].as_str().unwrap_or("").to_owned();
    let home_calls: HashSet<String> = home_gateways.iter().map(callsign).collect();
    // a list that names a callsign twice counts once, by its first record
    let firsts = |records: &'_ [Value]| -> Vec<Value> {
        let mut seen = HashSet::new();
        records
            .iter()
            .filter(|r| seen.insert(callsign(r)))
            .cloned()
            .collect()
    };
    let mut gateways: Vec<Value> = firsts(home_gateways)
        .iter()
        .map(|r| tagged(r, home))
        .collect();
    let mut counted: BTreeMap<String, usize> = BTreeMap::new();
    for (domain, records) in others {
        for r in &firsts(records) {
            let c = callsign(r);
            if home_calls.contains(&c) {
                continue; // the home's record wins
            }
            *counted.entry(c).or_default() += 1;
            gateways.push(tagged(r, domain));
        }
    }
    Merged {
        gateways,
        ambiguous: counted
            .into_iter()
            .filter(|(_, n)| *n > 1)
            .map(|(c, _)| c)
            .collect(),
    }
}

/// RP-5 §4.4: the windows a record lists in `listens`, those that do not hold left out. A window
/// holds with an integer `every_s` of 1 or more, `at_s` below it and `length_s` from 1 to
/// `every_s`. `None` when no window holds or the record lists none: `hours` says, for people.
#[must_use]
pub fn listens(record: &Value) -> Option<Vec<crate::forwarding::Window>> {
    let kept: Vec<crate::forwarding::Window> = record
        .get("listens")?
        .as_array()?
        .iter()
        .filter_map(|w| {
            let n = |k: &str| w.get(k).and_then(Value::as_u64);
            let (every_s, at_s, length_s) = (n("every_s")?, n("at_s")?, n("length_s")?);
            (every_s >= 1 && at_s < every_s && (1..=every_s).contains(&length_s)).then_some(
                crate::forwarding::Window {
                    every_s,
                    at_s,
                    length_s,
                },
            )
        })
        .collect();
    (!kept.is_empty()).then_some(kept)
}

/// The kinds of antenna a channel's `station` may name (RP-0 §9.6).
pub const ANTENNAS: [&str; 9] = [
    "vertical",
    "dipole",
    "yagi",
    "j-pole",
    "collinear",
    "loop",
    "nvis",
    "whip",
    "other",
];

/// Where a gateway's antennas are (RP-5 §4.4, `site`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Site {
    /// Latitude, degrees on WGS 84, north positive, at the precision the operator gave.
    pub lat: f64,
    /// Longitude, east positive, likewise.
    pub lon: f64,
    /// The antennas' height above the ground there, metres.
    pub agl_m: Option<f64>,
}

/// A JSON number from `lo` to `hi`, its ends included unless `above` or `below` says not.
fn number(v: Option<&Value>, lo: f64, hi: f64, above: bool, below: bool) -> Option<f64> {
    let x = v?.as_f64()?;
    let low_ok = if above { x > lo } else { x >= lo };
    let high_ok = if below { x < hi } else { x <= hi };
    (low_ok && high_ok).then_some(x)
}

/// The record's `site`, read: `lat` from −90 to 90 and `lon` from −180 to 180, both needed;
/// `agl_m` from 0 to 1 000 kept when it holds. `None` when there is none or it does not hold.
#[must_use]
pub fn site(record: &Value) -> Option<Site> {
    let s = record.get("site")?.as_object()?;
    Some(Site {
        lat: number(s.get("lat"), -90.0, 90.0, false, false)?,
        lon: number(s.get("lon"), -180.0, 180.0, false, false)?,
        agl_m: number(s.get("agl_m"), 0.0, 1000.0, false, false),
    })
}

/// How a gateway transmits and receives on a channel (RP-5 §4.4, a channel's `station`): each
/// member `None` when the record does not give it, or gives one that does not hold.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Station {
    /// The transmitter's power, watts, above 0.
    pub power_w: Option<f64>,
    /// A kind from [`ANTENNAS`].
    pub antenna: Option<String>,
    /// The antenna's height above the ground, metres: this channel's, in place of the site's.
    pub agl_m: Option<f64>,
    /// The antenna's gain over isotropic, dBi.
    pub gain_dbi: Option<f64>,
    /// Where a directional antenna points, degrees east of true north; `None`, all round.
    pub azimuth_deg: Option<f64>,
}

/// The channel's `station`, read: each member kept when it holds (`power_w` above 0, `antenna` a
/// kind of RP-0 §9.6, `agl_m` from 0 to 1 000, `gain_dbi` from −30 to 60, `azimuth_deg` from 0 to
/// below 360), the others left out.
#[must_use]
pub fn station(channel: &Value) -> Station {
    let Some(s) = channel.get("station").and_then(Value::as_object) else {
        return Station::default();
    };
    Station {
        power_w: number(s.get("power_w"), 0.0, f64::INFINITY, true, false),
        antenna: s
            .get("antenna")
            .and_then(Value::as_str)
            .filter(|a| ANTENNAS.contains(a))
            .map(str::to_owned),
        agl_m: number(s.get("agl_m"), 0.0, 1000.0, false, false),
        gain_dbi: number(s.get("gain_dbi"), -30.0, 60.0, false, false),
        azimuth_deg: number(s.get("azimuth_deg"), 0.0, 360.0, false, true),
    }
}

/// Whether a gateway listens at a moment, and when that changes (RP-5 §4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Listening {
    /// It listens now.
    pub open: bool,
    /// When that changes; `None` when it never does (no `listens`, or windows that cover every
    /// moment).
    pub changes: Option<u64>,
}

/// Whether the gateway of `record` listens at `now`, and when that changes. Windows that follow
/// on, or overlap, are one stretch of listening.
#[must_use]
pub fn listening(record: &Value, now: u64) -> Listening {
    let Some(windows) = listens(record) else {
        return Listening {
            open: true,
            changes: None,
        };
    };
    // when the window open at `t` closes, if one is: the period's start counted from `at_s`
    let start = |w: &crate::forwarding::Window, t: u64| {
        let t = i128::from(t);
        let phase = (t - i128::from(w.at_s)).rem_euclid(i128::from(w.every_s));
        u64::try_from(t - phase).unwrap_or(0)
    };
    let mut t = now;
    let mut closes = false;
    for _ in 0..64 {
        let ends = windows
            .iter()
            .map(|w| start(w, t) + w.length_s)
            .filter(|&end| t < end)
            .max();
        let Some(end) = ends else {
            closes = true;
            break;
        };
        t = end;
    }
    if !closes {
        return Listening {
            open: true,
            changes: None,
        };
    }
    if t > now {
        return Listening {
            open: true,
            changes: Some(t),
        };
    }
    let opens = windows
        .iter()
        .map(|w| {
            let s = start(w, now);
            if s <= now { s + w.every_s } else { s }
        })
        .min();
    Listening {
        open: false,
        changes: opens,
    }
}
