// SPDX-License-Identifier: MIT OR Apache-2.0
//! Phase 3's measurements (choosing a gateway, §18): a gateway list's HF channels predicted with
//! the real VOACAP engine, and how long the client takes to predict a hundred gateways both ways,
//! first and then from what it kept. Not a test: it needs the engine, which hf-predict's
//! `engines/voacapl/build.sh` builds.
//!
//! ```text
//! RADIOPOST_VOACAP=ENGINE cargo run --release -p radiopost-client --example hf_gateways -- \
//!     LIST GRID UNIX [COUNT]
//! ```
//!
//! LIST is a JSON file with a `gateways` array as a post office's list gives them; GRID is where
//! you are; UNIX the moment, in seconds since 1970. It prints each HF channel's prediction, by
//! callsign and dial, as the panel would show it with a station profile that says nothing. With
//! COUNT, it then times COUNT gateways spread over North America, each with an 80 m and a 40 m
//! channel.

use std::time::Instant;

use radiopost_client::selection::hf::Hf;
use radioprop_place::station::StationProfile;
use serde_json::{Map, Value, json};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (list, grid, unix, count) = match args.as_slice() {
        [_, list, grid, unix] => (list, grid, unix, None),
        [_, list, grid, unix, count] => (list, grid, unix, count.parse::<usize>().ok()),
        _ => {
            eprintln!("usage: hf_gateways LIST GRID UNIX [COUNT]");
            std::process::exit(2);
        }
    };
    let here = radioprop_geo::from_maidenhead(grid).expect("a grid square");
    let unix: i64 = unix.parse().expect("seconds since 1970");
    let text = std::fs::read_to_string(list).expect("the list");
    let list: Value = serde_json::from_str(&text).expect("JSON");
    let mut gateways = list["gateways"].as_array().cloned().unwrap_or_default();
    let station = StationProfile::default().resolve();
    let hf = Hf::new();
    hf.judge(Some(here), &station, &[], &mut gateways, unix);
    let mut out = Map::new();
    for gw in &gateways {
        for c in gw["channels"].as_array().into_iter().flatten() {
            if let Some(h) = c.get("hf") {
                let key = format!("{} {}", gw["callsign"].as_str().unwrap_or(""), c["dial_hz"]);
                out.insert(key, h.clone());
            }
        }
    }
    println!("{}", serde_json::to_string_pretty(&out).expect("JSON"));

    if let Some(count) = count {
        let mut many: Vec<Value> = (0..count)
            .map(|i| {
                // a lattice over North America, 25° to 50° N and 125° to 70° W
                #[allow(clippy::cast_precision_loss, reason = "a few hundred at most")]
                let (row, col) = ((i / 10) as f64, (i % 10) as f64);
                json!({"callsign": format!("T{i}"), "grid": "EM73",
                       "site": {"lat": 25.0 + row * 2.5, "lon": -125.0 + col * 6.0},
                       "channels": [
                          {"dial_hz": 3_591_500, "mode": "USB", "modem": "aether", "bandwidth_hz": 2300},
                          {"dial_hz": 7_101_000, "mode": "USB", "modem": "aether", "bandwidth_hz": 2300}]})
            })
            .collect();
        let start = Instant::now();
        hf.judge(Some(here), &station, &[], &mut many, unix);
        let first = start.elapsed();
        let start = Instant::now();
        hf.judge(Some(here), &station, &[], &mut many, unix);
        let kept = start.elapsed();
        let predicted = many
            .iter()
            .flat_map(|g| g["channels"].as_array().cloned().unwrap_or_default())
            .filter(|c| c["hf"]["state"] == "predicted")
            .count();
        eprintln!(
            "{count} gateways, {predicted} channels predicted, on {} threads: {:.2} s first, \
             {:.3} s kept",
            std::thread::available_parallelism().map_or(1, usize::from),
            first.as_secs_f64(),
            kept.as_secs_f64()
        );
    }
}
