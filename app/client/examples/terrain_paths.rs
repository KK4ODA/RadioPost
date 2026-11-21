// SPDX-License-Identifier: MIT OR Apache-2.0
//! Phase 2's measurements (choosing a gateway, §18): P.526 on real paths, and how long the client
//! takes to judge 100 VHF gateways from tiles already held. Not a test: it needs real elevation
//! tiles, which `tools/terrain/paths.py` fetches.
//!
//! ```text
//! python tools/terrain/paths.py TILES PATHS
//! cargo run --release -p radiopost-client --example terrain_paths -- TILES PATHS OUT
//! ```
//!
//! TILES is a terrain folder as the client keeps one; each `GeoTIFF` in `TILES/tif` is converted into
//! it first. OUT gets each path's profile, exactly as P.526 read it, and P.526's result, so that
//! another model can be run on the same profile.

use std::path::Path;
use std::time::Instant;

use radiopost_client::selection::terrain::Terrain;
use radioprop_geo::LatLon;
use radioprop_place::station::StationProfile;
use radioprop_terrain::path::{self, Class, K_STANDARD};
use radioprop_terrain::profile;
use radioprop_terrain::store::{Have, Source, Store, TileId};
use serde_json::{Value, json};

/// How many gateways the timing judges, and how far they are put from the middle of the first
/// region's two tiles (Georgia's), km: every path inside the tiles held.
const CANDIDATES: usize = 100;
const WITHIN_KM: f64 = 45.0;
const MIDDLE: LatLon = LatLon {
    lat: 33.5,
    lon: -84.0,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, tiles, paths, out] = args.as_slice() else {
        eprintln!("usage: terrain_paths TILES PATHS OUT");
        std::process::exit(2);
    };
    let tiles = Path::new(tiles);
    let store = Store::new(tiles, Source::Glo90);
    convert(tiles, &store);
    let list: Value =
        serde_json::from_str(&std::fs::read_to_string(paths).expect("the paths")).expect("JSON");
    let mut worked = Vec::new();
    for p in list["paths"].as_array().expect("paths") {
        worked.push(work_out(&store, p));
    }
    let timing = time_candidates(tiles, MIDDLE);
    eprintln!("{timing}");
    let all = json!({"source": Source::Glo90.words(), "notice": Source::Glo90.notice(),
                     "k": K_STANDARD, "paths": worked, "timing": timing});
    std::fs::write(
        out,
        serde_json::to_string_pretty(&all).expect("JSON") + "\n",
    )
    .expect("written");
}

/// Every `GeoTIFF` in `TILES/tif` not yet held, converted.
fn convert(tiles: &Path, store: &Store) {
    let Ok(dir) = std::fs::read_dir(tiles.join("tif")) else {
        return;
    };
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Copernicus_DSM_COG_30_N33_00_W085_00_DEM.tif
        let parts: Vec<&str> = name.split('_').collect();
        let (Some(ns), Some(ew)) = (parts.get(4), parts.get(6)) else {
            continue;
        };
        let number = |s: &str| s[1..].parse::<i16>().ok();
        let (Some(lat), Some(lon)) = (number(ns), number(ew)) else {
            continue;
        };
        let t = TileId {
            lat: if ns.starts_with('S') { -lat } else { lat },
            lon: if ew.starts_with('W') { -lon } else { lon },
        };
        if store.have(t) != Have::Missing {
            continue;
        }
        let bytes = std::fs::read(entry.path()).expect("a tile");
        let size = store.put(t, &bytes).expect("converted");
        eprintln!("{}: kept, {size} bytes", t.name());
    }
}

/// One path: its profile and P.526's loss, with the parts of § 4.5 it is made of.
fn work_out(store: &Store, p: &Value) -> Value {
    let at = |v: &Value| LatLon {
        lat: v[0].as_f64().expect("lat"),
        lon: v[1].as_f64().expect("lon"),
    };
    let (from, to) = (at(&p["from"]), at(&p["to"]));
    let f_mhz = p["f_mhz"].as_f64().expect("f");
    let (h1, h2) = (
        p["agl_m"][0].as_f64().expect("h"),
        p["agl_m"][1].as_f64().expect("h"),
    );
    let prof = match profile::profile(store, from, to) {
        Ok(prof) => prof,
        Err(Ok(missing)) => {
            let names: Vec<String> = missing.iter().map(|t| t.name()).collect();
            panic!("{}: tiles missing: {names:?}", p["id"]);
        }
        Err(Err(why)) => panic!("{}: {why}", p["id"]),
    };
    let worked = path::analyse(&prof, f_mhz, h1, h2, K_STANDARD);
    let class = match worked.class {
        Class::Clear => "clear",
        Class::Partly { .. } => "partly",
        Class::Blocked { .. } => "blocked",
    };
    let d = &worked.diffraction;
    #[allow(clippy::cast_precision_loss, reason = "a few thousand samples at most")]
    let spacing_m = prof.length_km * 1000.0 / (prof.h_m.len() - 1) as f64;
    let mut out = p.clone();
    out["length_km"] = json!(prof.length_km);
    out["spacing_m"] = json!(spacing_m);
    out["h_m"] = json!(
        prof.h_m
            .iter()
            .map(|h| (h * 10.0).round() / 10.0)
            .collect::<Vec<_>>()
    );
    out["class"] = json!(class);
    out["class_words"] = json!(worked.words());
    out["fspl_db"] = json!(radioprop_terrain::budget::fspl_db(f_mhz, prof.length_km));
    out["p526_db"] = json!(d.loss_db);
    out["bullington_db"] = json!(d.actual.loss_db);
    out["smooth_db"] = json!(d.smooth.loss_db);
    out["spherical_db"] = json!(d.spherical_db);
    out["line_of_sight"] = json!(d.actual.line_of_sight);
    // where the client says the prediction is less certain
    out["close"] = json!(radiopost_client::selection::terrain::close_ground(
        &prof,
        (h1, h2),
        worked.re_km
    ));
    out
}

/// The client's own judgment (`Terrain::judge`, as the gateway list calls it) of 100 gateways up
/// to 45 km from `here`, each on a 2 m FM channel, for a station profile that sets nothing: once
/// from cold, the tiles not yet opened, and once again.
fn time_candidates(tiles: &Path, here: LatLon) -> Value {
    // a fixed spread of sites, the same each run
    let mut state: u64 = 0x2026_1121;
    let mut next = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        #[allow(
            clippy::cast_precision_loss,
            reason = "53 bits, which a double holds exactly"
        )]
        let unit = (state >> 11) as f64 / (1u64 << 53) as f64;
        unit
    };
    let gateways: Vec<Value> = (0..CANDIDATES)
        .map(|i| {
            let r = WITHIN_KM * next().sqrt();
            let a = std::f64::consts::TAU * next();
            let lat = here.lat + r * a.cos() / 111.2;
            let lon = here.lon + r * a.sin() / (111.2 * here.lat.to_radians().cos());
            json!({"callsign": format!("N{i}TEST-10"), "site": {"lat": lat, "lon": lon},
                   "channels": [{"dial_hz": 145_050_000, "mode": "FM", "bandwidth_hz": 12_500,
                                 "modem": "aether-fm"}]})
        })
        .collect();
    let station = StationProfile::default().resolve();
    let terrain = Terrain::new(tiles);
    let pass = || {
        let start = Instant::now();
        let judged = gateways
            .iter()
            .filter(|g| {
                terrain.judge(
                    Some((here, true)),
                    &station,
                    g,
                    &g["channels"][0],
                    None,
                    false,
                )["state"]
                    == "judged"
            })
            .count();
        (start.elapsed().as_secs_f64() * 1000.0, judged)
    };
    let (cold_ms, judged) = pass();
    let (warm_ms, _) = pass();
    json!({"candidates": CANDIDATES, "within_km": WITHIN_KM, "judged": judged,
           "cold_ms": cold_ms.round(), "warm_ms": warm_ms.round()})
}
