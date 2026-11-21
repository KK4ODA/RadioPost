// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where the station is (choosing a gateway, §6): the places kept in `selection/places.json` under
//! the first home, the GPS chosen there, read while the panel runs, and the position in use.
//!
//! The places, the readers and the rule that picks the position in use are hf-predict's shared
//! library, `radioprop-place`; this module keeps its book for the client, and turns what it says
//! into the panel's answers and a call's record. The precise position stays on this computer: the
//! log carries the 6-character grid square only.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use radioprop_geo::parse_position;
use radioprop_place::{
    Book, Choice, Chosen, Gps, Hearing, Link, Origin, Place, Source, StationProfile, choose,
    now_ms, serial_ports,
};
use serde_json::{Value, json};

/// Where the book is kept, under the first home.
pub const FILE: &str = "selection/places.json";

/// The places, the GPS and the position in use, for the panel.
pub struct Position {
    path: PathBuf,
    inner: Mutex<Inner>,
}

struct Inner {
    book: Book,
    /// Why the file could not be read: nothing is written over it until it is mended or removed.
    broken: Option<String>,
    gps: Option<Gps>,
    /// What the log last said of the GPS, so each change is said once.
    said: String,
}

type Error = String;

impl Position {
    /// The book kept under the first home `base`, its GPS started.
    #[must_use]
    pub fn open(base: &Path) -> Position {
        let path = base.join(FILE);
        let (book, broken) = match Book::load(&path) {
            Ok(book) => (book, None),
            Err(why) => {
                radiopost::logfile::line(&format!("places: {why}"));
                (Book::default(), Some(why))
            }
        };
        let gps = book.gps.clone().map(Gps::start);
        Position {
            path,
            inner: Mutex::new(Inner {
                book,
                broken,
                gps,
                said: String::new(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// With no place kept yet, the grid square set before places existed becomes the first: the
    /// default, "Home", a square's centre.
    pub fn adopt_grid(&self, grid: Option<&str>) {
        let mut g = self.lock();
        let Some(grid) = grid.filter(|_| g.book.places.is_empty() && g.broken.is_none()) else {
            return;
        };
        if let Ok(place) = Place::from_grid("Home", grid, now_ms() / 1000) {
            g.book.add(place);
            if let Err(why) = g.book.save(&self.path) {
                radiopost::logfile::line(&format!("places: {why}"));
            }
        }
    }

    /// The position in use now, and why it is not the GPS's present fix when it is not.
    #[must_use]
    pub fn here(&self) -> Chosen {
        let mut g = self.lock();
        let status = g.gps.as_ref().map(Gps::status);
        let chosen = choose(&g.book, status.as_ref(), now_ms());
        // each change of what the GPS is doing is said once in the log, with the grid only
        let key = status.as_ref().map_or("", state_key);
        if key != g.said {
            if let Some(s) = &status {
                let using = chosen
                    .here
                    .as_ref()
                    .map(|h| format!(", using {}", h.grid))
                    .unwrap_or_default();
                let line = format!(
                    "places: GPS ({}) {}{using}",
                    s.source.describe(),
                    state_words(s)
                );
                radiopost::logfile::line(&line);
            }
            key.clone_into(&mut g.said);
        }
        chosen
    }

    /// What the terrain needs (§ 8): where the station is and whether that is better than a grid
    /// square's centre, and the station profile in use with every value resolved.
    #[must_use]
    pub fn for_terrain(
        &self,
    ) -> (
        Option<(radioprop_geo::LatLon, bool)>,
        radioprop_place::station::Resolved,
    ) {
        let chosen = self.here();
        let station = self
            .lock()
            .book
            .station_in_use(&chosen)
            .cloned()
            .unwrap_or_default()
            .resolve();
        let here = chosen.here.map(|h| {
            let exact = !matches!(
                h.from,
                radioprop_place::place::From::Place {
                    source: Origin::Grid,
                    ..
                }
            );
            (
                radioprop_geo::LatLon {
                    lat: h.lat,
                    lon: h.lon,
                },
                exact,
            )
        });
        (here, station)
    }

    /// The position in use as latitude and longitude, to rank gateways by.
    #[must_use]
    pub fn lat_lon(&self) -> Option<(f64, f64)> {
        self.here().here.map(|h| (h.lat, h.lon))
    }

    /// The station profile in use, by its id, as a call's record keeps it (§15.2's `station`).
    #[must_use]
    pub fn station_for_record(&self) -> Option<String> {
        let chosen = self.here();
        let g = self.lock();
        g.book.station_in_use(&chosen).map(|st| st.id.clone())
    }

    /// The position in use as a call's record keeps it (choosing a gateway, §15.2).
    #[must_use]
    pub fn for_record(&self) -> Option<Value> {
        let chosen = self.here();
        let here = chosen.here?;
        let mut record = json!({"lat": round(here.lat), "lon": round(here.lon), "grid": here.grid});
        match &here.from {
            radioprop_place::place::From::Gps { age_s, .. } => {
                let reader = self.lock().book.gps.as_ref().map_or("gps", source_kind);
                record["source"] = json!(format!("gps-{reader}"));
                record["age_s"] = json!(age_s);
            }
            radioprop_place::place::From::Place { id, source, .. } => {
                record["source"] = json!(match source {
                    Origin::Grid => "grid",
                    Origin::Gps => "place-from-gps",
                    Origin::Map => "map",
                    Origin::Typed => "typed",
                });
                record["place"] = json!(id);
            }
        }
        Some(record)
    }

    /// The panel's view: the places with their squares, the choice, the GPS and what it is doing,
    /// the position in use; with `ports`, the serial ports this computer has.
    #[must_use]
    pub fn json(&self, ports: bool) -> Value {
        let chosen = self.here();
        let g = self.lock();
        let places: Vec<Value> = g
            .book
            .places
            .iter()
            .map(|p| {
                let mut v = serde_json::to_value(p).unwrap_or(Value::Null);
                v["square"] = json!(p.square());
                v
            })
            .collect();
        let status = g.gps.as_ref().map(|gps| {
            let s = gps.status();
            let mut v = serde_json::to_value(&s).unwrap_or(Value::Null);
            v["words"] = json!(state_words(&s));
            v
        });
        // each profile with every value a model would use, set or assumed (§6.5)
        let stations: Vec<Value> = g
            .book
            .stations
            .iter()
            .map(|st| {
                let mut v = serde_json::to_value(st).unwrap_or(Value::Null);
                v["used"] = serde_json::to_value(st.resolve()).unwrap_or(Value::Null);
                v
            })
            .collect();
        let in_use = g.book.station_in_use(&chosen).map(|st| st.id.clone());
        let mut out = json!({
            "places": places,
            "stations": stations,
            "station": g.book.station,
            "station_in_use": in_use,
            "choice": g.book.choice,
            "gps": g.book.gps,
            "status": status,
            "here": chosen.here,
            "note": chosen.note,
            "broken": g.broken,
        });
        if ports {
            out["ports"] = json!(serial_ports());
        }
        out
    }

    /// A change from the panel: `op` is `add` (`name`, `where`: a grid square or a latitude and
    /// longitude; `from` `map` when it was picked on the map), `add-gps` (`name`: the GPS's present fix), `remove` or `default` (`id`),
    /// `choice` (`use`: `automatic`, `gps` or `place` with `id`), `gps` (`source`, or null for
    /// none), or one of the station profiles' (`station_request`).
    ///
    /// # Errors
    ///
    /// The request is not one of these, names nothing, or the book cannot be written.
    pub fn request(&self, body: &Value) -> Result<Value, Error> {
        let text = |name: &str| body[name].as_str().unwrap_or("").trim().to_owned();
        {
            let mut g = self.lock();
            if let Some(why) = &g.broken {
                return Err(format!("{why}: mend or remove it first"));
            }
            let now_s = now_ms() / 1000;
            match text("op").as_str() {
                "add" => {
                    let name = text("name");
                    if name.is_empty() {
                        return Err("a name for the place".into());
                    }
                    let picked = if text("from") == "map" {
                        Origin::Map
                    } else {
                        Origin::Typed
                    };
                    let place = place_at(&name, &text("where"), picked, now_s)?;
                    g.book.add(place);
                }
                "add-gps" => {
                    let fix = g
                        .gps
                        .as_ref()
                        .and_then(|gps| gps.status().usable_fix())
                        .ok_or("the GPS has no present fix to save")?;
                    let name = text("name");
                    let name = if name.is_empty() {
                        "From the GPS".into()
                    } else {
                        name
                    };
                    let mut place = Place::at(&name, fix.lat, fix.lon, Origin::Gps, now_s)?;
                    place.ground_m = fix.alt_m;
                    g.book.add(place);
                }
                "remove" => {
                    if !g.book.remove(&text("id")) {
                        return Err("no such place".into());
                    }
                }
                "default" => {
                    if !g.book.set_default(&text("id")) {
                        return Err("no such place".into());
                    }
                }
                "choice" => {
                    g.book.choice = match text("use").as_str() {
                        "automatic" => Choice::Automatic,
                        "gps" => Choice::Gps,
                        "place" if g.book.get(&text("id")).is_some() => {
                            Choice::Place { id: text("id") }
                        }
                        "place" => return Err("no such place".into()),
                        _ => return Err("use: automatic, gps or place".into()),
                    };
                }
                "gps" => {
                    let source: Option<Source> = match &body["source"] {
                        Value::Null => None,
                        s => Some(
                            serde_json::from_value(s.clone())
                                .map_err(|e| format!("not a GPS this program reads: {e}"))?,
                        ),
                    };
                    if let Some(s) = &source {
                        check_source(s)?;
                    }
                    if source != g.book.gps {
                        g.gps = None; // the reader before stops as it is dropped
                        g.gps = source.clone().map(Gps::start);
                        g.book.gps = source;
                        g.said.clear();
                    }
                }
                op if op.starts_with("station") || op == "place-station" => {
                    station_request(&mut g.book, op, body)?;
                }
                other => return Err(format!("no place request {other:?}")),
            }
            g.book.save(&self.path)?;
        }
        Ok(self.json(false))
    }
}

/// The station profiles' part of [`Position::request`] (§6.5): one added or changed
/// (`station-put`, `profile`), removed (`station-remove`, `id`), chosen (`station-choice`, `id` or
/// empty for none), or named by a place (`place-station`, `id` and `station`).
fn station_request(book: &mut Book, op: &str, body: &Value) -> Result<(), Error> {
    let text = |name: &str| body[name].as_str().unwrap_or("").trim().to_owned();
    match op {
        "station-put" => {
            let profile: StationProfile = serde_json::from_value(body["profile"].clone())
                .map_err(|e| format!("not a station profile: {e}"))?;
            book.put_station(profile)?;
        }
        "station-remove" => {
            if !book.remove_station(&text("id")) {
                return Err("no such station profile".into());
            }
        }
        "station-choice" => {
            let id = text("id");
            if !id.is_empty() && book.station(&id).is_none() {
                return Err("no such station profile".into());
            }
            book.station = (!id.is_empty()).then_some(id);
        }
        "place-station" => {
            let station = text("station");
            if !station.is_empty() && book.station(&station).is_none() {
                return Err("no such station profile".into());
            }
            let place = text("id");
            let p = book
                .places
                .iter_mut()
                .find(|p| p.id == place)
                .ok_or("no such place")?;
            p.station = (!station.is_empty()).then_some(station);
        }
        other => return Err(format!("no place request {other:?}")),
    }
    Ok(())
}

/// The position the places saved under the first home `base` give, with no GPS: for the command
/// line, which runs none.
#[must_use]
pub fn saved(base: &Path) -> Option<radioprop_place::Here> {
    let book = Book::load(&base.join(FILE)).ok()?;
    if book.places.is_empty() {
        return None;
    }
    choose(&book, None, now_ms()).here
}

/// A place from what was typed or picked: a grid square, or a latitude and longitude, which keeps
/// `picked` as its origin (typed, or picked on the map).
fn place_at(name: &str, at: &str, picked: Origin, now_s: u64) -> Result<Place, Error> {
    if at.is_empty() {
        return Err("where: a grid square such as EM73tr, or a latitude and longitude".into());
    }
    if at.starts_with(|c: char| c.is_ascii_alphabetic()) {
        Place::from_grid(name, at, now_s)
    } else {
        let p = parse_position(at)?;
        Place::at(name, p.lat, p.lon, picked, now_s)
    }
}

/// A GPS's settings, checked before its reader starts.
fn check_source(s: &Source) -> Result<(), Error> {
    match s {
        Source::Serial { port, baud } => {
            if port.trim().is_empty() {
                return Err("which serial port".into());
            }
            if baud.is_some_and(|b| !radioprop_place::read::BAUDS.contains(&b)) {
                return Err("a speed of 4800, 9600, 38400 or 115200 baud".into());
            }
        }
        Source::Tcp { host, port } | Source::Gpsd { host, port } => {
            if host.trim().is_empty() || *port == 0 {
                return Err("a host and a port".into());
            }
        }
        Source::Udp { port } => {
            if *port == 0 {
                return Err("a UDP port".into());
            }
        }
    }
    Ok(())
}

fn source_kind(s: &Source) -> &'static str {
    match s {
        Source::Serial { .. } => "serial",
        Source::Tcp { .. } => "tcp",
        Source::Udp { .. } => "udp",
        Source::Gpsd { .. } => "gpsd",
    }
}

/// What a GPS is doing, as a word that changes only when that does: the log says each change once.
fn state_key(s: &radioprop_place::Status) -> &'static str {
    match (&s.link, s.hearing) {
        (Link::Opening, _) => "opening",
        (Link::Failed { .. }, _) => "failed",
        (_, Hearing::Nothing) => "nothing",
        (_, Hearing::Silent { .. }) => "silent",
        (_, Hearing::NoFix { .. }) => "no-fix",
        (Link::Open { .. }, Hearing::Positions) => "reading",
    }
}

/// What a GPS is doing, in the operator's words.
fn state_words(s: &radioprop_place::Status) -> String {
    let now = now_ms();
    let ago = |ms: u64| radioprop_place::place::span(now.saturating_sub(ms) / 1000);
    match (&s.link, s.hearing) {
        (Link::Opening, _) => "opening".into(),
        (Link::Failed { text, .. }, _) => format!("not connected: {text}"),
        (_, Hearing::Nothing) => "connected, nothing heard yet".into(),
        (_, Hearing::Silent { since_ms }) => format!("silent for {}", ago(since_ms)),
        (_, Hearing::NoFix { since_ms }) => format!("no fix for {}", ago(since_ms)),
        (Link::Open { detail, .. }, Hearing::Positions) => {
            let age = s.age_s.map_or(String::new(), |a| {
                format!(", {} ago", radioprop_place::place::span(a))
            });
            let at = detail
                .as_ref()
                .map_or(String::new(), |d| format!(" at {d}"));
            format!("reading{at}: fix{age}")
        }
    }
}

/// Five decimal places: about a metre, more than a GPS's fix is worth.
fn round(x: f64) -> f64 {
    (x * 1e5).round() / 1e5
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rp-places-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_grid_set_before_places_becomes_home() {
        let base = scratch("adopt");
        let p = Position::open(&base);
        p.adopt_grid(Some("em73tt"));
        let v = p.json(false);
        assert_eq!(v["places"][0]["name"], "Home");
        assert_eq!(v["places"][0]["grid"], "EM73tt");
        assert_eq!(v["places"][0]["default"], true);
        assert_eq!(v["here"]["grid"], "EM73tt");
        // once there are places, a grid square is not taken again
        p.adopt_grid(Some("FN31pr"));
        assert_eq!(p.json(false)["places"].as_array().map(Vec::len), Some(1));
        let again = Position::open(&base);
        assert_eq!(again.json(false)["places"][0]["grid"], "EM73tt");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn places_are_added_by_square_or_by_latitude_and_longitude() {
        let base = scratch("add");
        let p = Position::open(&base);
        p.request(&json!({"op": "add", "name": "Field Day", "where": "33.98, -84.21"}))
            .expect("added");
        p.request(&json!({"op": "add", "name": "EOC", "where": "EM74ua"}))
            .expect("added");
        // one picked on the map keeps where it came from
        p.request(&json!({"op": "add", "name": "Hill", "where": "34.2, -84.1", "from": "map"}))
            .expect("added");
        let v = p.json(false);
        assert_eq!(v["places"][0]["source"], "typed");
        assert_eq!(v["places"][0]["square"], "EM73vx");
        assert_eq!(v["places"][1]["source"], "grid");
        assert_eq!(v["places"][2]["source"], "map");
        p.request(&json!({"op": "choice", "use": "place", "id": "pl-eoc"}))
            .expect("chosen");
        assert_eq!(p.json(false)["here"]["grid"], "EM74ua");
        let record = p.for_record().expect("a position");
        assert_eq!(record["source"], "grid");
        assert_eq!(record["place"], "pl-eoc");
        assert!(
            p.request(&json!({"op": "add", "name": "x", "where": "91, 0"}))
                .is_err()
        );
        assert!(
            p.request(&json!({"op": "add", "name": "", "where": "EM73"}))
                .is_err()
        );
        assert!(
            p.request(
                &json!({"op": "gps", "source": {"kind": "serial", "port": "COM5", "baud": 1200}})
            )
            .is_err()
        );
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn station_profiles_are_kept_chosen_and_named_by_places() {
        let base = scratch("stations");
        let p = Position::open(&base);
        p.adopt_grid(Some("EM73tr"));
        let profile = json!({"name": "Mobile, TS-480", "vehicle": true,
                             "hf": {"power_w": 65, "antenna": {"kind": "whip", "agl_m": 2.5}}});
        let v = p
            .request(&json!({"op": "station-put", "profile": profile}))
            .expect("kept");
        let st = &v["stations"][0];
        assert_eq!(st["id"], "st-mobile-ts-480");
        assert_eq!(
            st["used"]["hf"]["power_w"],
            json!({"value": 65.0, "said": "set"})
        );
        assert_eq!(
            st["used"]["vhf"]["agl_m"],
            json!({"value": 2.0, "said": "assumed"})
        );
        assert_eq!(v["station_in_use"], "st-mobile-ts-480", "the only one");
        p.request(&json!({"op": "station-put", "profile": {"name": "Home, FTDX10"}}))
            .expect("kept");
        p.request(&json!({"op": "place-station", "id": "pl-home", "station": "st-home-ftdx10"}))
            .expect("named");
        assert_eq!(p.station_for_record().as_deref(), Some("st-home-ftdx10"));
        p.request(&json!({"op": "station-choice", "id": "st-mobile-ts-480"}))
            .expect("chosen");
        assert_eq!(p.station_for_record().as_deref(), Some("st-mobile-ts-480"));
        p.request(&json!({"op": "station-remove", "id": "st-mobile-ts-480"}))
            .expect("removed");
        let v = p.json(false);
        assert_eq!(v["station"], Value::Null);
        assert_eq!(v["station_in_use"], "st-home-ftdx10");
        assert!(
            p.request(
                &json!({"op": "station-put", "profile": {"name": "x", "hf": {"power_w": -1}}})
            )
            .is_err()
        );
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn a_file_that_cannot_be_read_is_not_written_over() {
        let base = scratch("broken");
        std::fs::create_dir_all(base.join("selection")).expect("a folder");
        std::fs::write(base.join(FILE), "{ not json").expect("written");
        let p = Position::open(&base);
        assert!(p.json(false)["broken"].is_string());
        assert!(
            p.request(&json!({"op": "add", "name": "Home", "where": "EM73"}))
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(base.join(FILE)).expect("kept"),
            "{ not json"
        );
        let _ = std::fs::remove_dir_all(base);
    }
}
