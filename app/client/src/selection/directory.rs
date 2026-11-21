// SPDX-License-Identifier: MIT OR Apache-2.0
//! The gateway list as the operator sees it (choosing a gateway, §7.3 and §16.1): favourites and
//! blocks, by gateway or by channel, gateways of the operator's own, the age of what is shown, and
//! what the call records say of each gateway: the last call to it, and whether it was worked from
//! near here.
//!
//! Kept in `selection/gateways.json` under the first home, beside the places. Nothing here is sent
//! anywhere: the lists stay the post offices', and the operator's marks are laid over them.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::selection::classify::Class;

/// Where the marks are kept, under the first home.
pub const FILE: &str = "selection/gateways.json";

/// How near the position in use a completed call at HF must have been made for its gateway to
/// count as worked from here, in kilometres.
pub const WORKED_HF_KM: f64 = 50.0;
/// The same at VHF and above (30 MHz and up), where a few hundred metres can put a ridge between.
pub const WORKED_VHF_KM: f64 = 2.0;
/// A list older than this is shown dimmed, with its age.
pub const OLD_LIST_S: u64 = 7 * 86_400;
/// A gateway its post office has not seen for longer is shown dimmed, with how long (RP-5 §4.4
/// `seen`): its computer may be off, or it may be cut off and still serving by radio.
pub const OLD_SEEN_S: u64 = 2 * 86_400;

type Error = String;

/// A gateway, or one of its channels.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    /// The gateway's callsign, as listed.
    pub callsign: String,
    /// One of its channels; none for the whole gateway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dial_hz: Option<u64>,
}

impl Mark {
    fn covers(&self, callsign: &str, dial_hz: Option<u64>) -> bool {
        self.callsign == callsign && (self.dial_hz.is_none() || self.dial_hz == dial_hz)
    }
}

/// The operator's marks.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Marks {
    /// Ordered first among equals, and shown in the Favourites view.
    pub favourites: Vec<Mark>,
    /// Left out of the list, and counted.
    pub blocks: Vec<Mark>,
    /// Gateways of the operator's own, in RP-5 §4.4's shape.
    pub own: Vec<Value>,
}

impl Marks {
    /// The marks kept under the first home `base`; none when there are none yet.
    ///
    /// # Errors
    ///
    /// The file cannot be read, or holds no marks.
    pub fn load(base: &Path) -> Result<Marks, Error> {
        let path = base.join(FILE);
        match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("{} is not a list of marks: {e}", path.display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Marks::default()),
            Err(e) => Err(format!("cannot read {}: {e}", path.display())),
        }
    }

    /// Writes the marks whole, by way of a temporary file.
    ///
    /// # Errors
    ///
    /// The file cannot be written.
    pub fn save(&self, base: &Path) -> Result<(), Error> {
        let path = base.join(FILE);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("cannot write {}: {e}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())? + "\n";
        let temporary = path.with_extension("json.new");
        fs::write(&temporary, text)
            .and_then(|()| fs::rename(&temporary, &path))
            .map_err(|e| format!("cannot write {}: {e}", path.display()))
    }

    fn favourite(&self, callsign: &str, dial_hz: Option<u64>) -> bool {
        self.favourites.iter().any(|m| m.covers(callsign, dial_hz))
    }

    fn blocked(&self, callsign: &str, dial_hz: Option<u64>) -> bool {
        self.blocks.iter().any(|m| m.covers(callsign, dial_hz))
    }
}

/// A change from the panel: `op` is `favourite`, `unfavourite`, `block` or `unblock` (`callsign`,
/// and `dial_hz` for one channel), `add-own` (`callsign`, `grid`, `place`, `hours`, and a channel:
/// `dial_hz`, `bandwidth_hz`; again with the same callsign adds a channel), or `remove-own`
/// (`callsign`). `listed` are the callsigns the post offices list: a gateway of one's own is one
/// they do not.
///
/// # Errors
///
/// The request is not one of these, names nothing, or the marks cannot be written.
pub fn request(base: &Path, body: &Value, listed: &[String]) -> Result<(), Error> {
    let text = |name: &str| body[name].as_str().unwrap_or("").trim().to_owned();
    let mut marks = Marks::load(base)?;
    let callsign = text("callsign").to_ascii_uppercase();
    if callsign.is_empty() {
        return Err("which gateway: its callsign".into());
    }
    let mark = Mark {
        callsign: callsign.clone(),
        dial_hz: body["dial_hz"].as_u64(),
    };
    match text("op").as_str() {
        "favourite" => {
            if !marks.favourites.contains(&mark) {
                marks.favourites.push(mark);
            }
        }
        "unfavourite" => marks.favourites.retain(|m| *m != mark),
        "block" => {
            if !marks.blocks.contains(&mark) {
                marks.blocks.push(mark);
            }
        }
        "unblock" => marks.blocks.retain(|m| *m != mark),
        "add-own" => add_own(&mut marks, &callsign, body, listed)?,
        "remove-own" => {
            let before = marks.own.len();
            marks.own.retain(|g| g["callsign"] != callsign.as_str());
            if marks.own.len() == before {
                return Err(format!("{callsign} is not one of your own gateways"));
            }
        }
        other => return Err(format!("no gateway request {other:?}")),
    }
    marks.save(base)
}

fn add_own(
    marks: &mut Marks,
    callsign: &str,
    body: &Value,
    listed: &[String],
) -> Result<(), Error> {
    let text = |name: &str| body[name].as_str().unwrap_or("").trim().to_owned();
    if listed.iter().any(|c| c == callsign) {
        return Err(format!(
            "{callsign} is in a post office's list already: mark it a favourite instead"
        ));
    }
    let dial_hz = body["dial_hz"]
        .as_u64()
        .filter(|&hz| (1_800_000..=1_300_000_000).contains(&hz))
        .ok_or("a channel's dial, between 1.8 and 1300 MHz")?;
    let bandwidth_hz = body["bandwidth_hz"].as_u64().unwrap_or(2300);
    let grid = text("grid");
    if !grid.is_empty() && crate::grid::centre(&grid).is_none() {
        return Err(format!(
            "{grid} is not a grid square, such as EM73 or EM73tt"
        ));
    }
    let channel = json!({"dial_hz": dial_hz, "mode": if dial_hz >= 30_000_000 { "FM" } else { "USB" },
                         "modem": "aether", "bandwidth_hz": bandwidth_hz});
    if let Some(g) = marks.own.iter_mut().find(|g| g["callsign"] == callsign) {
        let channels = g["channels"]
            .as_array_mut()
            .ok_or("a broken gateway of your own")?;
        if !channels.iter().any(|c| c["dial_hz"] == dial_hz) {
            channels.push(channel);
        }
        return Ok(());
    }
    let mut record = json!({"callsign": callsign, "channels": [channel], "own": true});
    for name in ["grid", "place", "hours"] {
        let v = text(name);
        if !v.is_empty() {
            record[name] = json!(v);
        }
    }
    marks.own.push(record);
    Ok(())
}

/// Where a gateway is: its `site` when its record gives one (RP-5 §4.4), else its grid square's
/// centre.
#[must_use]
pub fn position_of(gateway: &Value) -> Option<(f64, f64)> {
    radiopost::gateway_list::site(gateway)
        .map(|s| (s.lat, s.lon))
        .or_else(|| gateway["grid"].as_str().and_then(crate::grid::centre))
}

/// The gateways as the panel lists them: the post offices' records ranked by distance (`ranked`,
/// from the store), the operator's own added in order, and on each `own`, `favourite`, `blocked`
/// (the gateway, and each channel), the age of its list (`list_fetched`, `list_old`), the last call
/// to it (`last`), whether it was `worked` from within reach of `here`, where it is (`at`, for the
/// map: its site, else its grid square's centre) and its `bearing` from `here` in degrees from
/// true north.
#[must_use]
pub fn annotate(
    ranked: Vec<Value>,
    gateway_list: &Value,
    marks: &Marks,
    records: &[Value],
    here: Option<(f64, f64)>,
    now: u64,
) -> Vec<Value> {
    let mut all = ranked;
    for own in &marks.own {
        let mut g = own.clone();
        let there = g["grid"].as_str().and_then(crate::grid::centre);
        g["km"] = here.zip(there).map_or(Value::Null, |(a, b)| {
            json!(crate::grid::km_between(a, b).round())
        });
        all.push(g);
    }
    // the operator's own take their place by distance; those with none stay after, as listed
    all.sort_by(|a, b| match (a["km"].as_f64(), b["km"].as_f64()) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    let home = gateway_list["domain"].as_str().unwrap_or("");
    for g in &mut all {
        let callsign = g["callsign"].as_str().unwrap_or("").to_owned();
        let there = position_of(g);
        g["at"] = there.map_or(Value::Null, |(lat, lon)| json!({"lat": lat, "lon": lon}));
        g["bearing"] = here.zip(there).map_or(Value::Null, |(a, b)| {
            json!(crate::grid::bearing(a, b).round().rem_euclid(360.0))
        });
        g["own"] = json!(g["own"] == true);
        g["favourite"] = json!(marks.favourite(&callsign, None));
        g["blocked"] = json!(marks.blocked(&callsign, None));
        if let Some(channels) = g["channels"].as_array_mut() {
            for c in channels {
                let dial = c["dial_hz"].as_u64();
                c["favourite"] = json!(marks.favourite(&callsign, dial));
                c["blocked"] = json!(marks.blocked(&callsign, dial));
            }
        }
        let fetched = match g["post_office"].as_str() {
            Some(po) if po != home => gateway_list["lists"][po]["fetched"].as_u64(),
            _ => gateway_list["fetched"].as_u64(),
        }
        .filter(|_| g["own"] != true);
        g["list_fetched"] = json!(fetched);
        g["list_old"] = json!(fetched.is_some_and(|f| now.saturating_sub(f) > OLD_LIST_S));
        g["last"] = last_call(records, &callsign);
        g["worked"] = json!(worked(records, &callsign, here));
        listening_at(g, now);
        g["seen_old"] = json!(
            g["seen"]
                .as_u64()
                .is_some_and(|seen| now.saturating_sub(seen) > OLD_SEEN_S)
        );
    }
    all
}

/// Whether a gateway listens at `t`, seconds since 1970, and when that changes, from the windows
/// its record lists (RP-5 §4.4), as its `listening`; none when it lists none (`hours` says).
pub fn listening_at(g: &mut Value, t: u64) {
    g["listening"] = if radiopost::gateway_list::listens(g).is_some() {
        let l = radiopost::gateway_list::listening(g, t);
        json!({"open": l.open, "changes": l.changes})
    } else {
        Value::Null
    };
}

/// The last call to `callsign`: when it ended, its class and the class in words.
fn last_call(records: &[Value], callsign: &str) -> Value {
    records
        .iter()
        .rev()
        .find(|r| r["remote"] == callsign)
        .map_or(Value::Null, |r| {
            let class = r["outcome"]["class"].as_str().unwrap_or("unknown");
            json!({"at": r["end"].as_u64().or(r["start"].as_u64()), "class": class,
                   "words": words(class)})
        })
}

/// Whether a call to `callsign` completed from within reach of `here`: 50 km at HF, 2 km on a
/// channel of 30 MHz or more.
fn worked(records: &[Value], callsign: &str, here: Option<(f64, f64)>) -> bool {
    let Some(here) = here else {
        return false;
    };
    records.iter().any(|r| {
        let completed = Class::from_name(r["outcome"]["class"].as_str().unwrap_or(""))
            .is_some_and(Class::completed);
        let vhf = r["channel"]["dial_hz"]
            .as_u64()
            .is_some_and(|hz| hz >= 30_000_000);
        let reach = if vhf { WORKED_VHF_KM } else { WORKED_HF_KM };
        completed
            && r["remote"] == callsign
            && from_where(&r["place"]).is_some_and(|at| crate::grid::km_between(at, here) <= reach)
    })
}

/// Where a call was made from: its position, or its grid square's centre.
fn from_where(place: &Value) -> Option<(f64, f64)> {
    match (place["lat"].as_f64(), place["lon"].as_f64()) {
        (Some(lat), Some(lon)) => Some((lat, lon)),
        _ => place["grid"].as_str().and_then(crate::grid::centre),
    }
}

fn words(class: &str) -> &'static str {
    Class::from_name(class).map_or("unknown", Class::words)
}

/// The last `limit` calls, newest first, as the Recent view shows them.
#[must_use]
pub fn recent(records: &[Value], limit: usize) -> Vec<Value> {
    records
        .iter()
        .rev()
        .take(limit)
        .map(|r| {
            let class = r["outcome"]["class"].as_str().unwrap_or("unknown");
            let took = r["end"]
                .as_u64()
                .zip(r["start"].as_u64())
                .map(|(e, s)| e.saturating_sub(s));
            json!({
                "id": r["id"], "at": r["start"], "kind": r["kind"], "remote": r["remote"],
                "as": r["as"], "dial_hz": r["channel"]["dial_hz"],
                "bandwidth_hz": r["channel"]["bandwidth_hz"], "grid": r["place"]["grid"],
                "class": class, "words": words(class),
                "completed": Class::from_name(class).is_some_and(Class::completed),
                "took_s": took, "why": r["outcome"]["why"],
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rp-directory-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn record(remote: &str, class: &str, end: u64, place: &Value, dial_hz: u64) -> Value {
        json!({"id": format!("a-{end}"), "start": end - 60, "end": end, "kind": "gateway",
               "remote": remote, "channel": {"dial_hz": dial_hz, "bandwidth_hz": 2300},
               "place": place, "outcome": {"class": class}})
    }

    #[test]
    fn favourites_and_blocks_cover_a_gateway_or_one_channel() {
        let base = scratch("marks");
        let listed = vec!["KK4ODA-1".to_owned()];
        request(
            &base,
            &json!({"op": "favourite", "callsign": "kk4oda-1"}),
            &listed,
        )
        .expect("ok");
        request(
            &base,
            &json!({"op": "block", "callsign": "KK4ODA-1", "dial_hz": 3_591_500}),
            &listed,
        )
        .expect("ok");
        let marks = Marks::load(&base).expect("kept");
        let list = vec![json!({"callsign": "KK4ODA-1", "grid": "EM73", "km": 14,
            "channels": [{"dial_hz": 3_591_500}, {"dial_hz": 7_101_000}]})];
        let g = &annotate(list, &json!({}), &marks, &[], None, 0)[0];
        assert_eq!(
            (g["favourite"].as_bool(), g["blocked"].as_bool()),
            (Some(true), Some(false))
        );
        assert_eq!(g["channels"][0]["blocked"], true);
        assert_eq!(g["channels"][1]["blocked"], false);
        assert_eq!(
            g["channels"][1]["favourite"], true,
            "a gateway's favourite covers its channels"
        );
        request(
            &base,
            &json!({"op": "unblock", "callsign": "KK4ODA-1", "dial_hz": 3_591_500}),
            &listed,
        )
        .expect("ok");
        assert_eq!(Marks::load(&base).expect("kept").blocks, Vec::<Mark>::new());
        assert!(request(&base, &json!({"op": "favourite"}), &listed).is_err());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn a_gateway_of_ones_own_takes_its_place_by_distance() {
        let base = scratch("own");
        let listed = vec!["KK4ODA-1".to_owned()];
        let own = json!({"op": "add-own", "callsign": "N0CALL-10", "grid": "EM74",
                         "dial_hz": 145_050_000, "bandwidth_hz": 12_500});
        request(&base, &own, &listed).expect("added");
        // the same callsign again adds a channel
        let more = json!({"op": "add-own", "callsign": "N0CALL-10", "dial_hz": 7_101_000});
        request(&base, &more, &listed).expect("a channel added");
        assert!(
            request(
                &base,
                &json!({"op": "add-own", "callsign": "KK4ODA-1", "dial_hz": 7_101_000}),
                &listed
            )
            .is_err(),
            "a listed gateway is not one's own"
        );
        let marks = Marks::load(&base).expect("kept");
        assert_eq!(marks.own[0]["channels"][0]["mode"], "FM");
        assert_eq!(marks.own[0]["channels"].as_array().map(Vec::len), Some(2));
        let list = vec![
            json!({"callsign": "KK4ODA-1", "grid": "EM73", "km": 14.0}),
            json!({"callsign": "N1CALL-10", "grid": "EM10", "km": 1309.0}),
        ];
        let here = crate::grid::centre("EM73");
        let all = annotate(list, &json!({}), &marks, &[], here, 0);
        let order: Vec<&str> = all.iter().filter_map(|g| g["callsign"].as_str()).collect();
        assert_eq!(order, ["KK4ODA-1", "N0CALL-10", "N1CALL-10"]);
        assert_eq!(all[1]["own"], true);
        // each carries its square's centre, for the map, and its bearing from here: EM74 is due
        // north of EM73, EM10 west-south-west
        assert_eq!(all[1]["at"], json!({"lat": 34.5, "lon": -85.0}));
        assert_eq!(all[1]["bearing"].as_f64(), Some(0.0));
        let west = all[2]["bearing"].as_f64().expect("a bearing");
        assert!((240.0..265.0).contains(&west), "{west}");
        let all = annotate(
            vec![json!({"callsign": "N9X"})],
            &json!({}),
            &marks,
            &[],
            here,
            0,
        );
        let nowhere = all.iter().find(|g| g["callsign"] == "N9X").expect("listed");
        assert_eq!(
            (&nowhere["at"], &nowhere["bearing"]),
            (&Value::Null, &Value::Null)
        );
        request(
            &base,
            &json!({"op": "remove-own", "callsign": "N0CALL-10"}),
            &listed,
        )
        .expect("gone");
        assert_eq!(Marks::load(&base).expect("kept").own, Vec::<Value>::new());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn worked_from_here_needs_a_completed_call_within_reach() {
        let near = json!({"lat": 33.75, "lon": -84.39, "grid": "EM73tr"});
        let far = json!({"lat": 34.75, "lon": -84.39, "grid": "EM74ts"});
        let records = vec![
            record("KK4ODA-1", "no-answer", 1000, &near, 7_101_000),
            record("KK4ODA-1", "completed", 2000, &far, 7_101_000),
            record("N1CALL-10", "completed-in-part", 3000, &near, 7_101_000),
            record(
                "N4VHF-1",
                "completed",
                4000,
                &json!({"lat": 33.80, "lon": -84.39}),
                145_050_000,
            ),
        ];
        let here = Some((33.751, -84.391));
        assert!(
            !worked(&records, "KK4ODA-1", here),
            "completed only 111 km away"
        );
        assert!(worked(&records, "N1CALL-10", here));
        assert!(
            !worked(&records, "N4VHF-1", here),
            "5.5 km is too far at VHF"
        );
        assert!(
            !worked(&records, "N1CALL-10", None),
            "no position, no 'here'"
        );
        let last = last_call(&records, "KK4ODA-1");
        assert_eq!(
            (last["class"].as_str(), last["at"].as_u64()),
            (Some("completed"), Some(2000))
        );
        let r = recent(&records, 2);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0]["remote"], "N4VHF-1");
        assert_eq!(r[1]["words"], "completed, some mail left in the outbox");
        assert_eq!(r[1]["took_s"], 60);
    }

    #[test]
    fn when_a_gateway_listens_and_was_seen_are_shown() {
        let day = 86_400;
        let now = 20_000 * day + 20 * 3600; // 20:00 UTC
        let gateways = vec![
            json!({"callsign": "A-1", "seen": now - 120,
                   "listens": [{"every_s": day, "at_s": 18 * 3600, "length_s": 8 * 3600}]}),
            json!({"callsign": "B-1", "seen": now - 3 * day,
                   "listens": [{"every_s": day, "at_s": 8 * 3600, "length_s": 3600}]}),
            json!({"callsign": "C-1"}),
        ];
        let all = annotate(gateways, &json!({}), &Marks::default(), &[], None, now);
        assert_eq!(
            all[0]["listening"],
            json!({"open": true, "changes": now + 6 * 3600})
        );
        assert_eq!(all[0]["seen_old"], false);
        assert_eq!(
            all[1]["listening"],
            json!({"open": false, "changes": now + 12 * 3600})
        );
        assert_eq!(all[1]["seen_old"], true);
        assert_eq!(
            all[2]["listening"],
            Value::Null,
            "no windows: as its hours say"
        );
        assert_eq!(all[2]["seen_old"], false, "never seen is not old");
    }

    #[test]
    fn a_list_a_week_old_is_marked_old() {
        let list = json!({"domain": "radiopost.org", "fetched": 1000,
                          "lists": {"hampost.org": {"fetched": 1000 + OLD_LIST_S}}});
        let gateways = vec![
            json!({"callsign": "A-1", "post_office": "radiopost.org"}),
            json!({"callsign": "B-1", "post_office": "hampost.org"}),
        ];
        let all = annotate(
            gateways,
            &list,
            &Marks::default(),
            &[],
            None,
            1001 + OLD_LIST_S,
        );
        assert_eq!(
            (all[0]["list_old"].as_bool(), all[1]["list_old"].as_bool()),
            (Some(true), Some(false))
        );
    }
}
