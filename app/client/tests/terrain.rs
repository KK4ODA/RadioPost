// SPDX-License-Identifier: MIT OR Apache-2.0
//! VHF gateways judged over the terrain, through the panel (choosing a gateway, §§8, 9.3, 16.3):
//! the line each VHF gateway gets in the list, a path over a made-up ridge and one over flat
//! ground worked out with the profile to draw, a height tried for the operator's antenna, the
//! states a path cannot be worked out in, and fetching from a stand-in for the Copernicus DEM's
//! bucket: a tile its list does not have is the sea, one that fails is said, and a 30 m tile not
//! released is fetched at 90 m and never asked for at 30 m again.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use radiopost_client::panel::Panel;
use radiopost_client::selection::terrain::Terrain;
use radioprop_terrain::geotiff::Raster;
use radioprop_terrain::store::{Source, Store, TileId};
use serde_json::{Value, json};

use common::{scratch, store_for};

fn http(at: SocketAddr, method: &str, path: &str, body: &Value) -> (u16, Value) {
    let mut s = TcpStream::connect(at).expect("the panel");
    let body = if method == "POST" {
        body.to_string()
    } else {
        String::new()
    };
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-RadioPost: 1\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        at.port(),
        body.len()
    )
    .expect("sent");
    let mut text = String::new();
    s.read_to_string(&mut text).expect("an answer");
    let status = text[9..12].parse().expect("a status");
    let body = text.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    (status, serde_json::from_str(body).expect("JSON"))
}

fn get(at: SocketAddr, path: &str) -> Value {
    let (status, v) = http(at, "GET", path, &Value::Null);
    assert_eq!(status, 200, "{path}: {v}");
    v
}

fn post(at: SocketAddr, path: &str, body: &Value) -> Value {
    let (status, v) = http(at, "POST", path, body);
    assert_eq!(status, 200, "{body}: {v}");
    v
}

fn find<'a>(v: &'a Value, callsign: &str) -> &'a Value {
    v["gateways"]
        .as_array()
        .expect("a list")
        .iter()
        .find(|g| g["callsign"] == callsign)
        .unwrap_or_else(|| panic!("{callsign} listed: {v}"))
}

/// Tile N33W085 at 90 m: flat at 250 m, with a ridge 600 m high running north and south at
/// 84.60° W, about a kilometre wide at its foot.
fn ridge_tile(dir: &Path) {
    let n = 1200;
    let step = 1.0 / 1200.0;
    let mut heights = Vec::with_capacity(n * n);
    for _row in 0..n {
        for col in 0..n {
            let lon = -85.0 + f64::from(u16::try_from(col).expect("a column")) * step;
            let from_crest = (lon + 84.60).abs() / 0.006;
            let height = 250.0 + 350.0 * (1.0 - from_crest).max(0.0);
            #[allow(clippy::cast_possible_truncation, reason = "heights of 250 to 600 m")]
            heights.push(height as f32);
        }
    }
    let raster = Raster {
        width: n,
        height: n,
        west: -85.0,
        north: 34.0,
        dlon: step,
        dlat: step,
        heights,
    };
    Store::new(dir, Source::Glo90)
        .put_raster(TileId { lat: 33, lon: -85 }, &raster)
        .expect("kept");
}

/// A stand-in for the data sets' buckets: each data set's tile list, and every other file
/// answered 404. Each request's path is kept.
fn bucket() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let at = listener.local_addr().expect("bound");
    let asked = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&asked);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut line = String::new();
            let mut reader = BufReader::new(stream.try_clone().expect("a copy"));
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            // the rest of the request's head
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).map_or(true, |n| n == 0) || header == "\r\n" {
                    break;
                }
            }
            let path = line.split(' ').nth(1).unwrap_or("").to_owned();
            kept.lock().expect("kept").push(path.clone());
            let body = match path.as_str() {
                // GLO-90 has N35W086 and N36W086, and not N34W086 (the sea); GLO-30 has none
                "/glo90/tileList.txt" => {
                    "Copernicus_DSM_COG_30_N35_00_W086_00_DEM\n\
                                          Copernicus_DSM_COG_30_N36_00_W086_00_DEM\n"
                }
                "/glo30/tileList.txt" => "Copernicus_DSM_COG_10_N10_00_E010_00_DEM\n",
                _ => "",
            };
            let status = if body.is_empty() {
                "404 Not Found"
            } else {
                "200 OK"
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (format!("http://{at}"), asked)
}

/// Waits for the fetching to finish, and gives what the panel then says of the terrain.
fn fetched(at: SocketAddr) -> Value {
    let start = Instant::now();
    loop {
        let s = get(at, "/api/terrain");
        if s["fetching"].is_null() && s["queued"] == 0 {
            return s;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "still fetching: {s}"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

/// A panel whose gateway list has VHF gateways at sites around a ridge, its terrain kept in a
/// folder of its own and fetched from a stand-in bucket.
struct Bench {
    dir: PathBuf,
    tiles: PathBuf,
    at: SocketAddr,
    asked: Arc<Mutex<Vec<String>>>,
}

fn bench(name: &str) -> Bench {
    let dir = scratch(name);
    let home = dir.join("facundo");
    let tiles = dir.join("terrain");
    let mut store = store_for(&home, "device:facundo", "facundo");
    store.set_grid(Some("EM73tr")).expect("a grid square");
    store.gateway_list = Some(
        json!({"domain": "radiopost.org", "fetched": 1, "gateways": [
            // over the ridge, 37 km east: its HF channel first, its VHF channel judged
            {"callsign": "KK4ODA-10", "grid": "EM73", "site": {"lat": 33.5, "lon": -84.4, "agl_m": 20},
             "channels": [{"dial_hz": 7_101_000, "bandwidth_hz": 2300},
                          {"dial_hz": 145_050_000, "mode": "FM", "bandwidth_hz": 12_500, "modem": "ax25",
                           "station": {"power_w": 50, "antenna": "j-pole"}}]},
            // on the same side of the ridge, 9 km east, a record that says nothing of its station
            {"callsign": "N4FLAT-10", "grid": "EM73", "site": {"lat": 33.5, "lon": -84.7},
             "channels": [{"dial_hz": 145_070_000, "mode": "FM", "bandwidth_hz": 12_500, "modem": "aether-fm"}]},
            // beyond the 150 km of a VHF path
            {"callsign": "W4FAR-10", "grid": "EM85", "site": {"lat": 35.5, "lon": -82.0},
             "channels": [{"dial_hz": 145_090_000, "mode": "FM", "bandwidth_hz": 12_500}]},
            // a grid square and no site
            {"callsign": "N0SITE-10", "grid": "EM73",
             "channels": [{"dial_hz": 145_010_000, "mode": "FM", "bandwidth_hz": 12_500}]},
            // HF only
            {"callsign": "KK4ODA-1", "grid": "EM73tr",
             "channels": [{"dial_hz": 7_101_000, "bandwidth_hz": 2300}]},
        ]}),
    );
    store.save().expect("saved");
    let (origin, asked) = bucket();
    let at = Panel::new(&home, None)
        .with_terrain(Terrain::new(&tiles).fetching_from(&origin))
        .start("127.0.0.1:0")
        .expect("a port");
    Bench {
        dir,
        tiles,
        at,
        asked,
    }
}

/// The place the paths are worked out from, typed to the hundredth of a degree, chosen.
fn on_the_hill(at: SocketAddr) {
    post(
        at,
        "/api/places",
        &json!({"op": "add", "name": "Hill", "where": "33.5, -84.8"}),
    );
    post(
        at,
        "/api/places",
        &json!({"op": "choice", "use": "place", "id": "pl-hill"}),
    );
}

#[test]
fn a_path_is_worked_out_only_from_a_position_and_a_site_good_enough() {
    let b = bench("terrain-states");
    let at = b.at;
    // a grid square is too coarse for a VHF path: no profile, and said why
    let list = get(at, "/api/gateways");
    let judged = &find(&list, "KK4ODA-10")["terrain"];
    assert_eq!(judged["state"], "grid-only", "{judged}");
    assert_eq!(
        judged["dial_hz"], 145_050_000,
        "its VHF channel, not its first"
    );
    assert!(find(&list, "KK4ODA-1").get("terrain").is_none(), "HF only");

    // from a place typed to the hundredth of a degree
    on_the_hill(at);
    let list = get(at, "/api/gateways");
    assert_eq!(find(&list, "N0SITE-10")["terrain"]["state"], "no-site");
    let far = &find(&list, "W4FAR-10")["terrain"];
    assert_eq!(far["state"], "far", "{far}");
    // the distance is measured from the site, not its grid square's centre
    assert_eq!(find(&list, "KK4ODA-10")["km"], 37.0);
    // nothing held yet: the tile to fetch named, with its size
    let judged = &find(&list, "KK4ODA-10")["terrain"];
    assert_eq!(judged["state"], "fetch", "{judged}");
    assert_eq!(judged["tiles"], json!(["N33W085"]));
    assert_eq!(judged["mb"]["glo90"], 5.0);
    assert_eq!(
        judged["words"],
        "Terrain not checked: 1 tile to fetch, about 5 MB"
    );
    let _ = std::fs::remove_dir_all(&b.dir);
}

#[test]
fn paths_over_a_ridge_and_over_flat_ground_are_worked_out() {
    let b = bench("terrain-paths");
    let at = b.at;
    on_the_hill(at);
    ridge_tile(&b.tiles);
    // with the tile held: blocked by the ridge, whose diffraction loss the budget carries
    let list = get(at, "/api/gateways");
    let ridge = &find(&list, "KK4ODA-10")["terrain"];
    assert_eq!(ridge["state"], "judged", "{ridge}");
    assert_eq!(ridge["source"], "glo90");
    assert_eq!(ridge["class"]["kind"], "blocked", "{ridge}");
    let ridge_at = ridge["class"]["at_km"].as_f64().expect("where");
    assert!(
        (ridge_at - 18.6).abs() < 0.3,
        "the crest at 84.60° W: {ridge}"
    );
    let diffraction = ridge["diffraction_db"].as_f64().expect("a loss");
    assert!(diffraction > 30.0, "{ridge}");
    // 145.05 MHz over 37.1 km: 32.45 + 20 log 145.05 + 20 log 37.1
    let fspl =
        32.45 + 20.0 * 145.05_f64.log10() + 20.0 * ridge["length_km"].as_f64().expect("km").log10();
    assert!(
        (ridge["fspl_db"].as_f64().expect("a loss") - fspl).abs() < 0.1,
        "{ridge}"
    );
    // you to it: 25 W assumed (44.0 dBm), your 0 dBi and 2 dB of feed line, its j-pole's 2 dBi
    // and 2 dB of feed line
    let received = 10.0 * 25_000_f64.log10() - 2.0 + 2.0 - 2.0 - fspl - diffraction;
    assert!(
        (ridge["up"]["received_dbm"].as_f64().expect("dBm") - received).abs() < 0.15,
        "{ridge}"
    );
    assert_eq!(ridge["up"]["threshold_dbm"], -117.0, "1 200 baud packet");
    // it to you: its record's 50 W (47.0 dBm) and j-pole, your 0 dBi and feed line
    let received = 10.0 * 50_000_f64.log10() + 2.0 - 2.0 + 0.0 - 2.0 - fspl - diffraction;
    assert!(
        (ridge["down"]["received_dbm"].as_f64().expect("dBm") - received).abs() < 0.15,
        "{ridge}"
    );
    // a single ridge at VHF costs tens of decibels and still leaves a margin at 25 W
    assert_eq!(ridge["chance"], "likely to work", "{ridge}");
    assert!(ridge.get("profile").is_none(), "the list draws nothing");
    // its record's power and antenna are used; what it does not say is assumed and said
    let assumed: Vec<&str> = ridge["assumed"]
        .as_array()
        .expect("listed")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        assumed.contains(&"its antenna's gain 2.0 dBi"),
        "{assumed:?}"
    );
    assert!(
        !assumed.iter().any(|a| a.starts_with("its power")),
        "{assumed:?}"
    );
    assert!(
        !assumed.iter().any(|a| a.starts_with("its antenna ")),
        "its site's 20 m"
    );
    assert_eq!(
        ridge["ends"]["gateway"],
        json!({"agl_m": 20.0, "said": "set"})
    );
    // on the flat side, a short path that works
    let flat = &find(&list, "N4FLAT-10")["terrain"];
    assert_eq!(flat["state"], "judged", "{flat}");
    assert_ne!(flat["class"]["kind"], "blocked", "{flat}");
    assert_eq!(flat["chance"], "likely to work", "{flat}");
    // low antennas over flat ground: P.526 near the two-ray plane-earth loss,
    // 40 log d − 20 log h1 h2 (d and h in metres), less free space
    let d_m = flat["length_km"].as_f64().expect("km") * 1000.0;
    let plane = 40.0 * d_m.log10() - 20.0 * (5.0_f64 * 10.0).log10();
    let excess = plane - flat["fspl_db"].as_f64().expect("a loss");
    let p526 = flat["diffraction_db"].as_f64().expect("a loss");
    assert!(
        (p526 - excess).abs() < 3.0,
        "P.526 {p526} dB, plane earth {excess:.1} dB"
    );
    assert!(
        flat["assumed"]
            .as_array()
            .expect("listed")
            .contains(&json!("its power 25 W")),
        "{flat}"
    );

    let _ = std::fs::remove_dir_all(&b.dir);
}

#[test]
fn a_path_is_drawn_and_a_height_tried_for_your_antenna() {
    let b = bench("terrain-drawn");
    let at = b.at;
    on_the_hill(at);
    ridge_tile(&b.tiles);
    // one path, drawn: at most 600 points, both ends and the crest among them
    let path = get(at, "/api/terrain/path?callsign=KK4ODA-10&dial=145050000");
    assert_eq!(path["state"], "judged");
    assert_eq!(path["callsign"], "KK4ODA-10");
    assert_eq!(path["dial_hz"], 145_050_000);
    let d = path["profile"]["d_km"].as_array().expect("distances");
    assert!(d.len() <= 601 && d.len() > 100, "{}", d.len());
    assert_eq!(d[0], 0.0);
    assert!(
        (d[d.len() - 1].as_f64().expect("km") - path["length_km"].as_f64().expect("km")).abs()
            < 0.1
    );
    let ground = path["profile"]["ground_m"].as_array().expect("the ground");
    let highest = ground.iter().filter_map(Value::as_f64).fold(0.0, f64::max);
    // the crest: 600 m and 20 m of bulge, less what sampling a sharp crest every 90 m misses
    assert!(highest > 560.0, "the crest is drawn: {highest}");
    assert_eq!(path["finer"]["tiles"], json!(["N33W085"]), "30 m on offer");
    // the same with the first VHF channel by default, and a mast tried: higher, less loss
    let first = get(at, "/api/terrain/path?callsign=KK4ODA-10");
    assert_eq!(first["diffraction_db"], path["diffraction_db"]);
    let tried = get(at, "/api/terrain/path?callsign=KK4ODA-10&agl=60");
    assert_eq!(
        tried["ends"]["you"],
        json!({"agl_m": 60.0, "said": "tried"})
    );
    assert!(
        tried["diffraction_db"].as_f64() < path["diffraction_db"].as_f64(),
        "{} against {}",
        tried["diffraction_db"],
        path["diffraction_db"]
    );
    let (status, _) = http(
        at,
        "GET",
        "/api/terrain/path?callsign=NOBODY-1",
        &Value::Null,
    );
    assert_eq!(status, 400);
    let (status, _) = http(
        at,
        "GET",
        "/api/terrain/path?callsign=KK4ODA-1",
        &Value::Null,
    );
    assert_eq!(status, 400, "no VHF channel");

    let _ = std::fs::remove_dir_all(&b.dir);
}

#[test]
fn tiles_are_fetched_as_the_data_sets_have_them() {
    let b = bench("terrain-fetch");
    let (at, tiles, asked) = (b.at, &b.tiles, &b.asked);
    on_the_hill(at);
    ridge_tile(tiles);
    // what is held, and the area around the place in use, with its sizes
    let status = get(at, "/api/terrain");
    assert_eq!(status["held"], json!({"glo30": 0, "glo90": 1}));
    assert_eq!(status["notices"].as_array().map(Vec::len), Some(1));
    assert_eq!(status["notices"][0]["data"], "Copernicus GLO-90");
    let within = |km: f64| {
        status["around"]
            .as_array()
            .expect("radii")
            .iter()
            .find(|a| a["radius_km"] == km)
            .cloned()
            .expect("a radius")
    };
    // 50 km from 33.5° N 84.8° W reaches 33.05° to 33.95° N and 84.26° to 85.34° W: N33W085 and
    // N33W086, 5 MB each at 90 m below 50° of latitude
    assert_eq!(
        within(50.0),
        json!({"radius_km": 50.0, "tiles": 2, "mb": {"glo90": 10.0, "glo30": 90.0}})
    );
    // 100 km reaches 32.6° to 34.4° N and 83.7° to 85.9° W: three rows of three
    assert_eq!(within(100.0)["tiles"], 9);

    // fetching from the stand-in: N34W086 is not in GLO-90's list, so it is the sea; N35W086 is
    // listed and its file is not there
    post(
        at,
        "/api/terrain",
        &json!({"op": "fetch", "tiles": ["N34W086", "N35W086"], "source": "glo90"}),
    );
    let status = fetched(at);
    assert!(tiles.join("copernicus-glo90/N34W086.absent").exists());
    let failed = status["failed"].as_array().expect("failures");
    assert_eq!(failed.len(), 1, "{status}");
    assert!(
        failed[0]
            .as_str()
            .is_some_and(|f| f.contains("N35W086") && f.contains("404")),
        "{status}"
    );
    // a 30 m tile GLO-30 has not released is fetched at 90 m, and marked
    post(
        at,
        "/api/terrain",
        &json!({"op": "fetch", "tiles": ["N36W086"], "source": "glo30"}),
    );
    fetched(at);
    assert!(tiles.join("copernicus-glo30/N36W086.unreleased").exists());
    let tif = |p: &&String| p.contains("_DEM.tif");
    let before: Vec<String> = asked.lock().expect("asked").clone();
    assert!(
        before.contains(&"/glo90/Copernicus_DSM_COG_30_N36_00_W086_00_DEM/Copernicus_DSM_COG_30_N36_00_W086_00_DEM.tif".to_owned()),
        "{before:?}"
    );
    assert!(
        !before.iter().filter(tif).any(|p| p.starts_with("/glo30/")),
        "{before:?}"
    );
    // and not asked for at 30 m again, nor fetched again at 90 m; each list is fetched once
    post(
        at,
        "/api/terrain",
        &json!({"op": "fetch", "tiles": ["N36W086"], "source": "glo30"}),
    );
    fetched(at);
    let after: Vec<String> = asked.lock().expect("asked").clone();
    assert_eq!(after.len(), before.len(), "asked again: {after:?}");
    let lists = asked
        .lock()
        .expect("asked")
        .iter()
        .filter(|p| p.ends_with("tileList.txt"))
        .count();
    assert_eq!(lists, 2, "one a data set");
    assert!(tiles.join("copernicus-glo90/tileList.txt").exists());

    // what makes no sense is refused
    for body in [
        json!({"op": "fetch", "tiles": ["X33W085"]}),
        json!({"op": "fetch", "tiles": ["N33W085"], "source": "srtm"}),
        json!({"op": "prepare", "radius_km": 900}),
        json!({"op": "dig"}),
    ] {
        let (status, v) = http(at, "POST", "/api/terrain", &body);
        assert_eq!(status, 400, "{body}: {v}");
    }
    post(at, "/api/terrain", &json!({"op": "cancel"}));
    let _ = std::fs::remove_dir_all(&b.dir);
}
