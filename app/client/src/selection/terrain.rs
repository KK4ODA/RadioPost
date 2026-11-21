// SPDX-License-Identifier: MIT OR Apache-2.0
//! VHF and UHF gateways judged over the terrain (the gateway selection design, §§ 8, 9.3, 16.3).
//!
//! Elevation tiles come from the Copernicus DEM's public bucket only when the operator asks
//! (**Fetch the terrain**, or **Prepare this area**): a tile is about 5 MB at 90 m and 45 MB at
//! 30 m, and a metered connection is the operator's to spend. They are kept in the computer's
//! cache directory. A path to a gateway whose record gives its `site` (RP-5 §4.4) is then profiled
//! at the finest data held for all of it, classed, its diffraction loss worked out by ITU-R
//! P.526-16 § 4.5, and its link budget worked out each way from the station profile in use and the
//! gateway's channel `station`, every value said as set or assumed.

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use radioprop_geo::{LatLon, bearing_deg, distance_km};
use radioprop_place::station::{Resolved, Said};
use radioprop_terrain::budget::{self, Ends};
use radioprop_terrain::path::{self as terrain_path, Class, K_STANDARD};
use radioprop_terrain::profile;
use radioprop_terrain::store::{Have, Store, TileId};
use serde_json::{Value, json};

/// Channels from this frequency up are judged over the terrain; below it the ionosphere decides.
pub const VHF_FROM_HZ: u64 = 30_000_000;
/// Paths longer than this are listed but not profiled (§ 8.1).
pub const FAR_KM: f64 = 150.0;
/// The location variability a margin is read against, dB (§ 8.6).
pub const SPREAD_DB: f64 = 10.0;
/// What a gateway's record does not say, assumed: power, antenna height and feed line.
const GATEWAY_POWER_W: f64 = 25.0;
const GATEWAY_AGL_M: f64 = 10.0;
const GATEWAY_FEED_DB: f64 = 2.0;
/// Points the panel draws a profile with, at most.
const DRAWN: usize = 600;
/// The radii an area may be prepared to, km.
pub const RADII_KM: [f64; 4] = [25.0, 50.0, 100.0, 150.0];

pub use radioprop_terrain::store::Source;

/// A tile by its name, `N33W085`: a hemisphere and two digits of latitude, a hemisphere and three
/// of longitude, the tile's south-west corner.
#[must_use]
pub fn tile_named(name: &str) -> Option<TileId> {
    let n = name.trim().to_ascii_uppercase();
    let b = n.as_bytes();
    let digits = |r: std::ops::Range<usize>| {
        b.get(r.clone())
            .is_some_and(|d| d.iter().all(u8::is_ascii_digit))
    };
    if b.len() != 7 || !digits(1..3) || !digits(4..7) {
        return None;
    }
    let lat: i16 = n[1..3].parse().ok()?;
    let lon: i16 = n[4..7].parse().ok()?;
    let lat = match b[0] {
        b'N' => lat,
        b'S' => -lat,
        _ => return None,
    };
    let lon = match b[3] {
        b'E' => lon,
        b'W' => -lon,
        _ => return None,
    };
    ((-90..90).contains(&lat) && (-180..180).contains(&lon)).then_some(TileId { lat, lon })
}

/// A gateway's first channel at 30 MHz or above that is not blocked: the one judged over the
/// terrain.
#[must_use]
pub fn vhf_channel(gateway: &Value) -> Option<Value> {
    gateway["channels"]
        .as_array()?
        .iter()
        .find(|c| c["dial_hz"].as_u64().is_some_and(|hz| hz >= VHF_FROM_HZ) && c["blocked"] != true)
        .cloned()
}

/// The folder terrain tiles are kept in: `RADIOPOST_TERRAIN` when set, else the computer's own
/// cache folder (`%LOCALAPPDATA%` on Windows, `~/Library/Caches` on macOS, `$XDG_CACHE_HOME` or
/// `~/.cache` elsewhere), under `RadioPost/terrain`.
#[must_use]
pub fn cache_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("RADIOPOST_TERRAIN") {
        return d.into();
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library").join("Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|h| h.join(".cache")))
    };
    base.unwrap_or_else(std::env::temp_dir)
        .join("RadioPost")
        .join("terrain")
}

/// A typical gain for an antenna kind (RP-0 §9.6), dBi, when a record names the kind and no gain.
fn typical_gain(kind: &str) -> f64 {
    match kind {
        "dipole" => 2.15,
        "j-pole" => 2.0,
        "collinear" => 5.0,
        "yagi" => 9.0,
        _ => 0.0,
    }
}

/// What the receiving modem needs at the radio's input, dBm, and why: assumed, since no threshold
/// is published as a measurement (§ 8.6). A good FM radio's 12 dB SINAD point is about −122 dBm.
fn threshold_dbm(modem: &str) -> (f64, &'static str) {
    match modem {
        "ax25" => (
            -117.0,
            "1 200 baud packet needs about −117 dBm: assumed, 5 dB above a good FM radio's 12 dB SINAD point",
        ),
        "vara-fm" | "aether-fm" => (
            -117.0,
            "the modem needs about −117 dBm: assumed, 5 dB above a good FM radio's 12 dB SINAD point",
        ),
        _ => (
            -112.0,
            "the modem needs about −112 dBm: assumed, 10 dB above a good FM radio's 12 dB SINAD point",
        ),
    }
}

/// About how many MB fetching `t` from `source` takes: tiles narrow towards the poles.
fn tile_mb(t: TileId, source: Source) -> f64 {
    let full = match source {
        Source::Glo30 => 45.0,
        Source::Glo90 => 5.0,
    };
    let lat = f64::from(t.lat.abs().max((t.lat + 1).abs()));
    let share = match lat {
        x if x <= 50.0 => 1.0,
        x if x <= 60.0 => 2.0 / 3.0,
        x if x <= 70.0 => 0.5,
        x if x <= 80.0 => 1.0 / 3.0,
        x if x <= 85.0 => 0.2,
        _ => 0.1,
    };
    full * share
}

#[derive(Default)]
struct Fetching {
    queue: VecDeque<(TileId, Source)>,
    running: bool,
    /// The tile being fetched: its name, the bytes read, and how many there are when said.
    now: Option<(String, u64, Option<u64>)>,
    done: u32,
    failed: Vec<String>,
}

/// The terrain tiles kept, and their fetching.
pub struct Terrain {
    dir: PathBuf,
    glo30: Store,
    glo90: Store,
    fetching: Mutex<Fetching>,
    /// The tile list of each data set, read once (`tileList.txt`).
    lists: Mutex<[Option<Arc<HashSet<String>>>; 2]>,
    /// Where the data sets are fetched from in place of their public buckets: tests serve them.
    origin: Option<String>,
}

impl Terrain {
    /// The tiles kept under `dir`.
    #[must_use]
    pub fn new(dir: &Path) -> Terrain {
        Terrain {
            dir: dir.to_owned(),
            glo30: Store::new(dir, Source::Glo30),
            glo90: Store::new(dir, Source::Glo90),
            fetching: Mutex::new(Fetching::default()),
            lists: Mutex::new([None, None]),
            origin: None,
        }
    }

    /// The same, fetching from `origin` (`<origin>/glo90/tileList.txt`,
    /// `<origin>/glo90/<key>/<key>.tif`, and the same under `glo30`) in place of the public
    /// buckets: for tests.
    #[must_use]
    pub fn fetching_from(mut self, origin: &str) -> Terrain {
        self.origin = Some(origin.trim_end_matches('/').to_owned());
        self
    }

    /// The address of a data set's file: `tileList.txt`, or a tile's `<key>/<key>.tif`.
    fn url(&self, source: Source, file: &str) -> String {
        match &self.origin {
            Some(o) => format!("{o}/{}/{file}", short(source)),
            None => format!("{}/{file}", source.bucket()),
        }
    }

    fn store(&self, source: Source) -> &Store {
        match source {
            Source::Glo30 => &self.glo30,
            Source::Glo90 => &self.glo90,
        }
    }

    /// A tile GLO-30 Public has not released, marked when found: its 90 m tile stands in.
    fn unreleased(&self, t: TileId) -> PathBuf {
        self.dir
            .join(Source::Glo30.name())
            .join(format!("{}.unreleased", t.name()))
    }

    /// What is held, and what is being fetched.
    ///
    /// # Panics
    ///
    /// A thread panicked while holding the fetching's lock.
    #[must_use]
    pub fn status(&self) -> Value {
        let f = self.fetching.lock().expect("terrain");
        let held = |s: Source| {
            std::fs::read_dir(self.dir.join(s.name())).map_or((0, 0), |d| {
                d.flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "rpt"))
                    .map(|e| e.metadata().map_or(0, |m| m.len()))
                    .fold((0u32, 0u64), |(n, b), len| (n + 1, b + len))
            })
        };
        let (n30, b30) = held(Source::Glo30);
        let (n90, b90) = held(Source::Glo90);
        // the notice each data set asks for, of the data held or being fetched
        let fetching_from = |s: Source| f.queue.iter().any(|&(_, q)| q == s);
        let notices: Vec<Value> = [(Source::Glo30, n30), (Source::Glo90, n90)]
            .into_iter()
            .filter(|&(s, n)| n > 0 || fetching_from(s))
            .map(|(s, _)| json!({"data": data_name(s), "notice": s.notice()}))
            .collect();
        json!({
            "dir": self.dir.display().to_string(),
            "held": {"glo30": n30, "glo90": n90},
            "held_mb": round1(mb(b30 + b90)),
            "fetching": f.now.as_ref().map(|(name, read, of)| json!({"tile": name, "read": read, "of": of})),
            "queued": f.queue.len(),
            "done": f.done,
            "failed": f.failed,
            "notices": notices,
        })
    }

    /// Starts fetching the tiles not held yet, on a thread of its own.
    ///
    /// # Panics
    ///
    /// A thread panicked while holding the fetching's lock.
    pub fn fetch(self: &Arc<Self>, tiles: &[TileId], source: Source) {
        let mut f = self.fetching.lock().expect("terrain");
        for &t in tiles {
            let queued = f.queue.iter().any(|&(q, s)| q == t && s == source);
            let unreleased = source == Source::Glo30 && self.unreleased(t).exists();
            if self.store(source).have(t) == Have::Missing && !queued && !unreleased {
                f.queue.push_back((t, source));
            }
        }
        if !f.running && !f.queue.is_empty() {
            f.running = true;
            f.failed.clear();
            let me = Arc::clone(self);
            std::thread::spawn(move || me.run());
        }
    }

    /// Drops what is queued; the tile being fetched finishes.
    ///
    /// # Panics
    ///
    /// A thread panicked while holding the fetching's lock.
    pub fn cancel(&self) {
        self.fetching.lock().expect("terrain").queue.clear();
    }

    fn run(&self) {
        loop {
            let next = {
                let mut f = self.fetching.lock().expect("terrain");
                let Some(next) = f.queue.pop_front() else {
                    f.running = false;
                    f.now = None;
                    return;
                };
                f.now = Some((next.0.name(), 0, None));
                next
            };
            let result = self.get(next.0, next.1);
            let mut f = self.fetching.lock().expect("terrain");
            match result {
                Ok(()) => f.done += 1,
                Err(why) => {
                    let line = format!("terrain: {}: {why}", next.0.name());
                    radiopost::logfile::line(&line);
                    f.failed.push(line);
                }
            }
        }
    }

    /// The data set's tile list, fetched once and kept beside its tiles.
    fn listed(&self, source: Source) -> Result<Arc<HashSet<String>>, String> {
        let k = usize::from(source == Source::Glo90);
        if let Some(l) = &self.lists.lock().expect("terrain")[k] {
            return Ok(Arc::clone(l));
        }
        let file = self.dir.join(source.name()).join("tileList.txt");
        let text = if let Ok(t) = std::fs::read_to_string(&file) {
            t
        } else {
            let bytes = self.download(&self.url(source, "tileList.txt"))?;
            let _ = std::fs::create_dir_all(self.dir.join(source.name()));
            std::fs::write(&file, &bytes).map_err(|e| format!("{}: {e}", file.display()))?;
            String::from_utf8_lossy(&bytes).into_owned()
        };

        let list: Arc<HashSet<String>> = Arc::new(
            text.lines()
                .map(|l| l.trim().to_owned())
                .filter(|l| !l.is_empty())
                .collect(),
        );
        self.lists.lock().expect("terrain")[k] = Some(Arc::clone(&list));
        Ok(list)
    }

    /// Fetches tile `t` and keeps it: one its data set does not list is the sea, and a 30 m tile
    /// not released yet has its 90 m tile fetched in its place.
    fn get(&self, t: TileId, source: Source) -> Result<(), String> {
        if !self.listed(source)?.contains(&source.key(t)) {
            if source == Source::Glo30
                && self.listed(Source::Glo90)?.contains(&Source::Glo90.key(t))
            {
                let mark = self.unreleased(t);
                let _ = std::fs::create_dir_all(mark.parent().unwrap_or(&self.dir));
                std::fs::write(&mark, b"").map_err(|e| e.to_string())?;
                return self.get(t, Source::Glo90);
            }
            return self.store(source).put_absent(t);
        }
        let key = source.key(t);
        let bytes = self.download(&self.url(source, &format!("{key}/{key}.tif")))?;
        self.store(source).put(t, &bytes).map(|_| ())
    }

    /// One HTTPS download, its progress in `now`.
    fn download(&self, url: &str) -> Result<Vec<u8>, String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .timeout_global(Some(Duration::from_secs(1800)))
            .build()
            .into();
        let mut answer = agent.get(url).call().map_err(|e| e.to_string())?;
        let status = answer.status().as_u16();
        if status != 200 {
            return Err(format!("{url} answered {status}"));
        }
        let of = answer
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let mut reader = answer
            .body_mut()
            .with_config()
            .limit(200 * 1024 * 1024)
            .reader();
        let mut out = Vec::new();
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = reader.read(&mut buf).map_err(|e| format!("{url}: {e}"))?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
            if let Some(now) = &mut self.fetching.lock().expect("terrain").now {
                now.1 = out.len() as u64;
                now.2 = of;
            }
        }
        Ok(out)
    }

    /// The tiles within `radius_km` of `here`, to fetch ahead ("prepare this area"): each tile
    /// whose nearest point is that close.
    #[must_use]
    pub fn around(here: LatLon, radius_km: f64) -> Vec<TileId> {
        let dlat = radius_km / 111.2;
        // a circle that holds a pole reaches every longitude
        let to_pole = (90.0 - here.lat.abs()) * 111.2;
        let dlon = if radius_km >= to_pole {
            180.0
        } else {
            (radius_km / (111.2 * here.lat.to_radians().cos())).min(180.0)
        };
        #[allow(
            clippy::cast_possible_truncation,
            reason = "degrees floored, inside ±540 by the bounds above"
        )]
        let floor = |x: f64| x.floor() as i32;
        let (lat0, lat1) = (
            floor((here.lat - dlat).max(-90.0)),
            floor((here.lat + dlat).min(89.999)),
        );
        let (lon0, lon1) = (
            floor(here.lon - dlon),
            floor(here.lon + dlon).min(floor(here.lon - dlon) + 359),
        );
        let wrap = |lon: f64| (lon + 540.0).rem_euclid(360.0) - 180.0;
        let mut out = BTreeSet::new();
        for lat in lat0..=lat1 {
            for lon in lon0..=lon1 {
                let (south, west) = (f64::from(lat), f64::from(lon));
                // the tile's nearest longitude, and its nearest point there: inside it, or on its
                // south or north edge (across a pole, the pole)
                let lon_c = wrap(here.lon.clamp(west, west + 1.0));
                let nearest = [here.lat.clamp(south, south + 1.0), south, south + 1.0]
                    .into_iter()
                    .map(|lat| distance_km(here, LatLon { lat, lon: lon_c }))
                    .fold(f64::INFINITY, f64::min);
                if nearest <= radius_km {
                    #[allow(
                        clippy::cast_possible_truncation,
                        reason = "a tile's corner, inside ±180"
                    )]
                    let lon = wrap(west).round() as i16;
                    #[allow(clippy::cast_possible_truncation, reason = "inside ±90")]
                    out.insert(TileId {
                        lat: lat as i16,
                        lon,
                    });
                }
            }
        }
        out.into_iter().collect()
    }

    /// What fetching `tiles` from `source` would take, about, in MB.
    #[must_use]
    pub fn size_mb(tiles: &[TileId], source: Source) -> f64 {
        (tiles.iter().map(|&t| tile_mb(t, source)).sum::<f64>()).round()
    }

    /// The finest data held for every tile a path reads: GLO-30 where each is held at 30 m or is
    /// the sea there, else GLO-90. A path reads one data set, so one tile GLO-30 has not released
    /// puts the whole path on GLO-90.
    fn source_for(&self, tiles: &[TileId]) -> Option<Source> {
        let all = |s: Source| {
            tiles
                .iter()
                .all(|&t| self.store(s).have(t) != Have::Missing)
        };
        let no_unreleased = tiles.iter().all(|&t| !self.unreleased(t).exists());
        if all(Source::Glo30) && no_unreleased {
            Some(Source::Glo30)
        } else if all(Source::Glo90) {
            Some(Source::Glo90)
        } else {
            None
        }
    }

    /// A VHF channel of a gateway judged from `here`: the path over the terrain and the link
    /// budget each way. `here` is the position in use and whether it is better than a grid
    /// square's centre; `station` the station profile in use; `your_agl_m` a height to try in
    /// place of the profile's; `drawn` adds the profile for the panel to draw.
    #[must_use]
    pub fn judge(
        &self,
        here: Option<(LatLon, bool)>,
        station: &Resolved,
        gateway: &Value,
        channel: &Value,
        your_agl_m: Option<f64>,
        drawn: bool,
    ) -> Value {
        let found = match self.locate(here, gateway) {
            Ok(found) => found,
            Err(why) => return why,
        };
        let hz = channel["dial_hz"].as_u64().unwrap_or(VHF_FROM_HZ);
        #[allow(
            clippy::cast_precision_loss,
            reason = "a dial in hertz, well inside 2^52"
        )]
        let f_mhz = hz as f64 / 1e6;
        let s = sides(station, channel, found.site_agl, your_agl_m);
        let path = terrain_path::analyse(&found.profile, f_mhz, s.your_agl, s.gw_agl, K_STANDARD);
        let fspl = budget::fspl_db(f_mhz, found.length);
        let loss = fspl + path.diffraction.loss_db;
        let up = budget::one_way(s.up, loss, s.threshold, SPREAD_DB);
        let down = budget::one_way(s.down, loss, s.threshold, SPREAD_DB);
        let weaker = if up.margin_db <= down.margin_db {
            up
        } else {
            down
        };
        let source = found.source;
        let mut out = json!({
            "state": "judged",
            "source": short(source),
            "data": source.words(),
            "notice": source.notice(),
            "length_km": round1(found.length),
            "bearing_deg": bearing_deg(found.here, found.there).round(),
            "f_mhz": f_mhz,
            "class": class_json(path.class),
            "class_words": path.words(),
            "diffraction_db": round1(path.diffraction.loss_db),
            "fspl_db": round1(fspl),
            "up": budget_json(up, s.threshold),
            "down": budget_json(down, s.threshold),
            "margin_db": round1(weaker.margin_db),
            "chance": likely(weaker.p_viable),
            "words": format!(
                "{} · diffraction about {:.0} dB (P.526) · margin about {:.0} dB · {}",
                path.words(),
                path.diffraction.loss_db,
                weaker.margin_db,
                likely(weaker.p_viable)
            ),
            "ends": {"you": {"agl_m": s.your_agl, "said": s.your_said},
                     "gateway": {"agl_m": s.gw_agl, "said": s.gw_said}},
            "assumed": s.assumed,
            "k": K_STANDARD,
        });
        // finer data to fetch for this path, when it is read at 90 m and GLO-30 has all of it
        let tiles = &found.tiles;
        if source == Source::Glo90 && tiles.iter().all(|&t| !self.unreleased(t).exists()) {
            let finer: Vec<TileId> = tiles
                .iter()
                .copied()
                .filter(|&t| self.glo30.have(t) == Have::Missing)
                .collect();
            out["finer"] = json!({"tiles": names(&finer),
                                  "mb": Self::size_mb(&finer, Source::Glo30).max(1.0)});
        }
        if drawn {
            out["profile"] = drawing(&path);
        }
        out
    }

    /// The path to a gateway's site and its profile from the data held, or why there is none: the
    /// answer [`Terrain::judge`] gives then.
    fn locate(&self, here: Option<(LatLon, bool)>, gateway: &Value) -> Result<Located, Value> {
        let Some((here, exact)) = here else {
            return Err(json!({"state": "no-position",
                              "words": "Terrain not checked: where you are is not known"}));
        };
        let Some(site) = radiopost::gateway_list::site(gateway) else {
            return Err(json!({"state": "no-site",
                              "words": "Terrain not checked: the gateway's record gives only its grid square"}));
        };
        if !exact {
            return Err(json!({"state": "grid-only",
                              "words": "Terrain not checked: your position is only a grid square's centre"}));
        }
        let there = LatLon {
            lat: site.lat,
            lon: site.lon,
        };
        let length = distance_km(here, there);
        if length > FAR_KM {
            return Err(json!({"state": "far", "length_km": length.round(),
                              "words": format!("Terrain not checked: {length:.0} km is beyond {FAR_KM:.0} km")}));
        }
        let tiles = profile::tiles(here, there);
        let Some(source) = self.source_for(&tiles) else {
            let missing: Vec<TileId> = tiles
                .iter()
                .copied()
                .filter(|&t| self.glo90.have(t) == Have::Missing)
                .collect();
            let mb = Self::size_mb(&missing, Source::Glo90).max(1.0);
            return Err(json!({
                "state": "fetch",
                "tiles": names(&missing),
                "all_tiles": names(&tiles),
                "mb": {"glo90": mb, "glo30": Self::size_mb(&tiles, Source::Glo30).max(1.0)},
                "words": format!(
                    "Terrain not checked: {} to fetch, about {mb:.0} MB",
                    if missing.len() == 1 { "1 tile".to_owned() } else { format!("{} tiles", missing.len()) },
                ),
            }));
        };
        match profile::profile(self.store(source), here, there) {
            Ok(profile) => Ok(Located {
                here,
                there,
                length,
                tiles,
                source,
                profile,
                site_agl: site.agl_m,
            }),
            Err(Ok(missing)) => Err(json!({"state": "fetch", "tiles": names(&missing),
                                           "all_tiles": names(&tiles),
                                           "words": "Terrain not checked: tiles to fetch"})),
            Err(Err(why)) => {
                Err(json!({"state": "error", "words": format!("Terrain not checked: {why}")}))
            }
        }
    }

    /// The tiles a path from `here` to a gateway's site reads, those not held at 90 m.
    #[must_use]
    pub fn missing(&self, here: LatLon, gateway: &Value) -> Vec<TileId> {
        radiopost::gateway_list::site(gateway)
            .map(|s| {
                profile::tiles(
                    here,
                    LatLon {
                        lat: s.lat,
                        lon: s.lon,
                    },
                )
            })
            .unwrap_or_default()
            .into_iter()
            .filter(|&t| self.glo90.have(t) == Have::Missing)
            .collect()
    }
}

/// A path found, with its profile read from the finest data held for all of it.
struct Located {
    here: LatLon,
    there: LatLon,
    length: f64,
    tiles: Vec<TileId>,
    source: Source,
    profile: radioprop_terrain::profile::Profile,
    /// The site's antenna height, when its record gives one.
    site_agl: Option<f64>,
}

/// Both ends of a path as used, each value set or assumed, and what was assumed in words.
struct Sides {
    /// You transmitting to the gateway, and the gateway to you.
    up: Ends,
    down: Ends,
    your_agl: f64,
    your_said: &'static str,
    gw_agl: f64,
    gw_said: &'static str,
    threshold: f64,
    assumed: Vec<String>,
}

/// Your end from the station profile in use (or the height being tried), the gateway's from its
/// channel's `station` and its `site`, and what neither says assumed.
fn sides(
    station: &Resolved,
    channel: &Value,
    site_agl: Option<f64>,
    your_agl_m: Option<f64>,
) -> Sides {
    let st = radiopost::gateway_list::station(channel);
    let mine = station.vhf;
    let mut assumed: Vec<String> = Vec::new();
    let (your_agl, your_said) = match your_agl_m {
        Some(h) => (h, "tried"),
        None => (mine.agl_m.value, said(mine.agl_m.said)),
    };
    if your_agl_m.is_none() && mine.agl_m.said == Said::Assumed {
        assumed.push(format!("your antenna {:.0} m up", mine.agl_m.value));
    }
    for (what, s) in [
        (
            format!("your power {:.0} W", mine.power_w.value),
            mine.power_w.said,
        ),
        (
            format!("your antenna's gain {:.1} dBi", mine.gain_dbi.value),
            mine.gain_dbi.said,
        ),
        (
            format!("your feed line {:.0} dB", mine.feedline_db.value),
            mine.feedline_db.said,
        ),
    ] {
        if s == Said::Assumed {
            assumed.push(what);
        }
    }
    let (gw_agl, gw_said) = if let Some(h) = st.agl_m.or(site_agl) {
        (h, "set")
    } else {
        assumed.push(format!("its antenna {GATEWAY_AGL_M:.0} m up"));
        (GATEWAY_AGL_M, "assumed")
    };
    let gw_power = st.power_w.unwrap_or_else(|| {
        assumed.push(format!("its power {GATEWAY_POWER_W:.0} W"));
        GATEWAY_POWER_W
    });
    let gw_gain = st.gain_dbi.unwrap_or_else(|| {
        let g = st.antenna.as_deref().map_or(0.0, typical_gain);
        assumed.push(format!("its antenna's gain {g:.1} dBi"));
        g
    });
    assumed.push(format!("its feed line {GATEWAY_FEED_DB:.0} dB"));
    let (threshold, why) = threshold_dbm(channel["modem"].as_str().unwrap_or(""));
    assumed.push(why.to_owned());
    Sides {
        up: Ends {
            power_w: mine.power_w.value,
            tx_gain_dbi: mine.gain_dbi.value,
            tx_feed_db: mine.feedline_db.value,
            rx_gain_dbi: gw_gain,
            rx_feed_db: GATEWAY_FEED_DB,
        },
        down: Ends {
            power_w: gw_power,
            tx_gain_dbi: gw_gain,
            tx_feed_db: GATEWAY_FEED_DB,
            rx_gain_dbi: mine.gain_dbi.value,
            rx_feed_db: mine.feedline_db.value,
        },
        your_agl,
        your_said,
        gw_agl,
        gw_said,
        threshold,
        assumed,
    }
}

/// A data set's name, before the notice its licence asks for.
fn data_name(source: Source) -> &'static str {
    match source {
        Source::Glo30 => "Copernicus GLO-30",
        Source::Glo90 => "Copernicus GLO-90",
    }
}

/// A data set as the panel's requests and answers name it: `glo90`, `glo30`.
fn short(source: Source) -> &'static str {
    match source {
        Source::Glo30 => "glo30",
        Source::Glo90 => "glo90",
    }
}

fn names(tiles: &[TileId]) -> Vec<String> {
    tiles.iter().map(|t| t.name()).collect()
}

fn said(s: Said) -> &'static str {
    match s {
        Said::Set => "set",
        Said::Assumed => "assumed",
    }
}

/// Bytes in MB.
#[allow(
    clippy::cast_precision_loss,
    reason = "a size on disk, far inside 2^52 bytes"
)]
fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1e6
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// A chance in words (§ 8.6).
fn likely(p: f64) -> &'static str {
    if p >= 0.8 {
        "likely to work"
    } else if p >= 0.4 {
        "may work"
    } else {
        "unlikely to work"
    }
}

fn class_json(c: Class) -> Value {
    match c {
        Class::Clear => json!({"kind": "clear"}),
        Class::Partly { at_km } => json!({"kind": "partly", "at_km": round1(at_km)}),
        Class::Blocked { at_km, above_ray_m } => {
            json!({"kind": "blocked", "at_km": round1(at_km), "above_ray_m": above_ray_m.round()})
        }
    }
}

fn budget_json(b: budget::Budget, threshold: f64) -> Value {
    json!({"received_dbm": round1(b.received_dbm), "threshold_dbm": threshold,
           "margin_db": round1(b.margin_db), "p": (b.p_viable * 100.0).round() / 100.0,
           "words": likely(b.p_viable)})
}

/// The profile as the panel draws it: at most [`DRAWN`] points, the one standing highest above the
/// direct ray always among them.
fn drawing(p: &terrain_path::Path) -> Value {
    let n = p.d_km.len();
    let step = n.div_ceil(DRAWN).max(1);
    let worst = (0..n)
        .max_by(|&a, &b| (-p.clearance_m[a]).total_cmp(&(-p.clearance_m[b])))
        .unwrap_or(0);
    let mut keep: Vec<usize> = (0..n).step_by(step).collect();
    if keep.last() != Some(&(n - 1)) {
        keep.push(n - 1);
    }
    if !keep.contains(&worst) {
        keep.push(worst);
        keep.sort_unstable();
    }
    let pick = |v: &[f64]| keep.iter().map(|&i| round1(v[i])).collect::<Vec<_>>();
    let f60: Vec<f64> = (0..n).map(|i| p.ray_m[i] - 0.6 * p.fresnel_m[i]).collect();
    json!({
        "d_km": keep.iter().map(|&i| (p.d_km[i] * 1000.0).round() / 1000.0).collect::<Vec<_>>(),
        "ground_m": pick(&p.ground_m),
        "ray_m": pick(&p.ray_m),
        "f60_m": pick(&f60),
        "ends_m": [round1(p.ends_m.0), round1(p.ends_m.1)],
        "agl_m": [p.agl_m.0, p.agl_m.1],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_are_named_as_the_data_sets_name_them() {
        assert_eq!(tile_named("N33W085"), Some(TileId { lat: 33, lon: -85 }));
        assert_eq!(tile_named(" s01e010 "), Some(TileId { lat: -1, lon: 10 }));
        assert_eq!(
            tile_named("S90W180"),
            Some(TileId {
                lat: -90,
                lon: -180
            })
        );
        assert_eq!(
            tile_named("N33W180"),
            Some(TileId { lat: 33, lon: -180 }),
            "180° W to 179° W"
        );
        for bad in [
            "", "N33", "X33W085", "N90W000", "N33E180", "N3W085", "N33W85",
        ] {
            assert_eq!(tile_named(bad), None, "{bad}");
        }
    }

    #[test]
    fn an_area_takes_every_tile_the_circle_reaches() {
        // the corners a circle only clips are taken, the ones it misses are not
        let here = LatLon {
            lat: 33.5,
            lon: -84.8,
        };
        let names = |r: f64| {
            Terrain::around(here, r)
                .iter()
                .map(|t| t.name())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(25.0), ["N33W086", "N33W085"]);
        // N34W084's nearest corner, 34° N 84° W, is 92.7 km away: in at 100 km, out at 90
        assert!(names(100.0).contains(&"N34W084".to_owned()));
        assert!(!names(90.0).contains(&"N34W084".to_owned()));
        // across the antimeridian, and the equator
        let pacific = Terrain::around(
            LatLon {
                lat: 0.1,
                lon: 179.9,
            },
            30.0,
        );
        assert_eq!(
            pacific,
            [
                TileId { lat: -1, lon: -180 },
                TileId { lat: -1, lon: 179 },
                TileId { lat: 0, lon: -180 },
                TileId { lat: 0, lon: 179 },
            ]
        );
        // near a pole every longitude may be in reach, each tile once
        let polar = Terrain::around(
            LatLon {
                lat: 89.6,
                lon: 10.0,
            },
            60.0,
        );
        assert!(polar.iter().all(|t| t.lat == 89), "{polar:?}");
        assert_eq!(polar.len(), 360);
    }

    #[test]
    fn a_tile_is_smaller_towards_the_poles() {
        let t = |lat| TileId { lat, lon: 0 };
        assert!((tile_mb(t(33), Source::Glo90) - 5.0).abs() < 1e-9);
        assert!(
            (tile_mb(t(-50), Source::Glo30) - 45.0).abs() < 1e-9,
            "50° to 49° S"
        );
        assert!((tile_mb(t(55), Source::Glo30) - 30.0).abs() < 1e-9);
        assert!((tile_mb(t(-89), Source::Glo90) - 0.5).abs() < 1e-9);
    }
}
