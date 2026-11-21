// SPDX-License-Identifier: MIT OR Apache-2.0
//! HF channels predicted through the panel (choosing a gateway, § 10): each HF channel of the
//! gateway list gets its prediction at the hour both ways, run once for the month and kept; the
//! station profile in use is one end and the gateway's record the other; a channel that cannot be
//! predicted says why. The engine here makes its predictions up from the request, so what is
//! checked is what the client does with them; `real_voacap_predicts_a_gateway_both_ways` runs the
//! real engine when `RADIOPOST_VOACAP` names it.

mod common;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use radiopost_client::panel::Panel;
use radiopost_client::selection::hf::{Hf, IN_3_KHZ_DB};
use radioprop_hf::propagation::{
    EngineRun, FrequencyPrediction, HourPrediction, Prediction, PredictionRequest,
    PropagationEngine,
};
use serde_json::{Value, json};

use common::{scratch, store_for};

fn http(at: SocketAddr, method: &str, path: &str, body: &Value) -> Value {
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
    assert_eq!(&text[9..12], "200", "{path}: {text}");
    let body = text.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    serde_json::from_str(body).expect("JSON")
}

/// An engine that says every path is one F hop at 20° whose median SNR is 20 dB-Hz above the
/// transmitter's power in dBW, 10 dB either side to its deciles, open on nine days in ten;
/// the runs it was asked for are counted.
struct MadeUp {
    runs: Arc<AtomicUsize>,
}

impl PropagationEngine for MadeUp {
    fn name(&self) -> &'static str {
        "made-up"
    }

    fn predict(&self, request: &PredictionRequest) -> Result<EngineRun, String> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let snr = 10.0 * request.tx_power_watts.log10() + 20.0;
        let frequencies: Vec<FrequencyPrediction> = request
            .frequencies_mhz
            .iter()
            .map(|&f| FrequencyPrediction {
                freq_mhz: f,
                mode: "1F2".into(),
                takeoff_angle_deg: 20.0,
                muf_day: 0.9,
                snr_db: snr,
                snr_lower_decile_db: 10.0,
                snr_upper_decile_db: 10.0,
                ..FrequencyPrediction::default()
            })
            .collect();
        let hours = (1..=24)
            .map(|utc_hour| HourPrediction {
                utc_hour,
                frequencies: frequencies.clone(),
                ..HourPrediction::default()
            })
            .collect();
        Ok(EngineRun {
            prediction: Prediction {
                engine: "made-up".into(),
                distance_km: 0.0,
                azimuth_tx_deg: 0.0,
                azimuth_rx_deg: 0.0,
                hours,
            },
            input: String::new(),
            output: String::new(),
        })
    }
}

/// A panel at `EM73tr` whose list has a gateway 1 300 km away with 1 kW on 40 m, an Aether channel
/// at each bandwidth, a VARA one, a 160 m one, a VHF one and a blocked one; and one with no site.
fn panel(name: &str, hf: Hf, grid: Option<&str>) -> SocketAddr {
    let dir = scratch(name);
    let home = dir.join("facundo");
    let mut store = store_for(&home, "device:facundo", "facundo");
    store.set_grid(grid).expect("a grid square");
    store.gateway_list = Some(
        json!({"domain": "radiopost.org", "fetched": 1, "gateways": [
            {"callsign": "W1AW-10", "grid": "FN31pr", "site": {"lat": 41.71, "lon": -72.73},
             "channels": [
                {"dial_hz": 7_101_000, "mode": "USB", "modem": "aether", "bandwidth_hz": 2300,
                 "station": {"power_w": 1000}},
                {"dial_hz": 3_590_000, "mode": "USB", "modem": "aether", "bandwidth_hz": 500},
                {"dial_hz": 14_105_000, "mode": "USB", "modem": "vara", "bandwidth_hz": 2300},
                {"dial_hz": 1_850_000, "mode": "USB", "modem": "aether", "bandwidth_hz": 2300},
                {"dial_hz": 145_050_000, "mode": "FM", "modem": "aether-fm", "bandwidth_hz": 12_500},
                {"dial_hz": 10_140_000, "mode": "USB", "modem": "aether", "bandwidth_hz": 2300}]},
            {"callsign": "N0SITE-10", "grid": "EM85",
             "channels": [{"dial_hz": 7_102_000, "mode": "USB", "modem": "aether", "bandwidth_hz": 500}]},
        ]}),
    );
    store.save().expect("saved");
    let at = Panel::new(&home, None)
        .with_hf(hf)
        .start("127.0.0.1:0")
        .expect("a port");
    // the 30 m channel is left out by the operator
    http(
        at,
        "POST",
        "/api/gateway-marks",
        &json!({"op": "block", "callsign": "W1AW-10", "dial_hz": 10_140_000}),
    );
    at
}

fn channel<'a>(v: &'a Value, callsign: &str, dial: u64) -> &'a Value {
    v["gateways"]
        .as_array()
        .expect("a list")
        .iter()
        .find(|g| g["callsign"] == callsign)
        .and_then(|g| g["channels"].as_array())
        .and_then(|c| c.iter().find(|c| c["dial_hz"] == dial))
        .unwrap_or_else(|| panic!("{callsign} on {dial}: {v}"))
}

#[test]
fn hf_channels_are_predicted_both_ways_at_the_hour_and_kept() {
    let runs = Arc::new(AtomicUsize::new(0));
    let hf = Hf::with_engine(Box::new(MadeUp {
        runs: Arc::clone(&runs),
    }));
    let at = panel("hf-both-ways", hf, Some("EM73tr"));
    let v = http(at, "GET", "/api/gateways", &Value::Null);
    assert_eq!(v["hf"]["engine"], "made-up", "{}", v["hf"]);

    // 40 m: your 100 W one way, its 1 kW the other, less each transmitter's 1 dB of feed line
    let forty = &channel(&v, "W1AW-10", 7_101_000)["hf"];
    assert_eq!(forty["state"], "predicted", "{forty}");
    assert_eq!(forty["band"], "40 m");
    assert_eq!(forty["class"], "moderate");
    assert_eq!(forty["class_words"], "one hop (1F2)");
    let close = |a: &Value, b: f64| (a.as_f64().expect("a number") - b).abs() < 0.06;
    assert!(
        close(&forty["up"]["snr_db"], 40.0 - IN_3_KHZ_DB - 1.0),
        "{forty}"
    );
    assert!(
        close(&forty["down"]["snr_db"], 50.0 - IN_3_KHZ_DB - 1.0),
        "{forty}"
    );
    assert!(close(&forty["open"], 0.9));
    let chance = forty["chance"].as_f64().expect("a chance");
    assert!(chance > 0.5 && chance <= 0.9, "{forty}");
    // the stronger way carries more
    let up = forty["up"]["bps"].as_f64().expect("a rate");
    let down = forty["down"]["bps"].as_f64().expect("a rate");
    assert!(down > up && up > 0.0, "{forty}");
    let words = forty["words"].as_str().expect("words");
    assert!(
        words.contains("% of days at") && words.contains("sending"),
        "{words}"
    );
    let assumed: Vec<&str> = forty["assumed"]
        .as_array()
        .expect("assumed")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(assumed.contains(&"your power 100 W"), "{assumed:?}");
    assert!(assumed.contains(&"its noise residential"), "{assumed:?}");
    assert!(
        !assumed.iter().any(|a| a.starts_with("its power")),
        "{assumed:?}"
    );

    // 80 m at 500 Hz: the gateway's 100 W assumed, both ways alike
    let eighty = &channel(&v, "W1AW-10", 3_590_000)["hf"];
    assert_eq!(eighty["state"], "predicted", "{eighty}");
    assert!(
        close(&eighty["down"]["snr_db"], 40.0 - IN_3_KHZ_DB - 1.0),
        "{eighty}"
    );
    // VARA: no model of the modem, the propagation only
    let vara = &channel(&v, "W1AW-10", 14_105_000)["hf"];
    assert_eq!(vara["state"], "no-modem", "{vara}");
    assert!(
        vara["chance"].is_null() && vara["up"]["bps"].is_null(),
        "{vara}"
    );
    assert!(
        vara["words"]
            .as_str()
            .is_some_and(|w| w.starts_with("No model of vara"))
    );
    // 160 m is outside what is predicted; VHF and the blocked channel get nothing
    assert_eq!(channel(&v, "W1AW-10", 1_850_000)["hf"]["state"], "outside");
    assert!(channel(&v, "W1AW-10", 145_050_000).get("hf").is_none());
    assert!(channel(&v, "W1AW-10", 10_140_000).get("hf").is_none());
    // a record with only a grid square: its centre will do at HF
    assert_eq!(
        channel(&v, "N0SITE-10", 7_102_000)["hf"]["state"],
        "predicted"
    );
    // one run each way for each station a gateway has: W1AW-10's 1 kW channel is one, its other
    // channels (100 W assumed) share another, N0SITE-10 a third; then kept
    assert_eq!(runs.load(Ordering::SeqCst), 6);
    assert_eq!(v["hf"]["kept"], 3);
    let again = http(at, "GET", "/api/gateways", &Value::Null);
    assert_eq!(runs.load(Ordering::SeqCst), 6, "kept for the month");
    assert_eq!(
        channel(&again, "W1AW-10", 7_101_000)["hf"],
        channel(&v, "W1AW-10", 7_101_000)["hf"]
    );
}

#[test]
fn the_station_profile_in_use_is_this_end() {
    let runs = Arc::new(AtomicUsize::new(0));
    let hf = Hf::with_engine(Box::new(MadeUp {
        runs: Arc::clone(&runs),
    }));
    let at = panel("hf-station", hf, Some("EM73tr"));
    let before = http(at, "GET", "/api/gateways", &Value::Null);
    // 400 W, no feed line, a 9.15 dBi Yagi: 6 dB more power and 7 dB of gain over a dipole
    let profile = json!({"name": "Home", "noise": "rural", "bands": ["40m"],
                         "hf": {"power_w": 400, "feedline_db": 0,
                                "antenna": {"kind": "yagi", "gain_dbi": 9.15, "agl_m": 15}}});
    http(
        at,
        "POST",
        "/api/places",
        &json!({"op": "station-put", "profile": profile}),
    );
    let after = http(at, "GET", "/api/gateways", &Value::Null);
    let snr = |v: &Value, way: &str| {
        channel(v, "W1AW-10", 7_101_000)["hf"][way]["snr_db"]
            .as_f64()
            .expect("an SNR")
    };
    let more = 10.0 * 4.0_f64.log10() + 1.0 + 7.0;
    assert!(
        (snr(&after, "up") - snr(&before, "up") - more).abs() < 0.1,
        "{after}"
    );
    // the Yagi's gain counts on receive too
    assert!((snr(&after, "down") - snr(&before, "down") - 7.0).abs() < 0.1);
    let forty = &channel(&after, "W1AW-10", 7_101_000)["hf"];
    assert!(
        !forty["assumed"]
            .as_array()
            .expect("assumed")
            .iter()
            .any(|a| a.as_str().is_some_and(|a| a.starts_with("your"))),
        "{forty}"
    );
    // its bands are 40 m only: 80 m is not predicted for it
    assert_eq!(
        channel(&after, "W1AW-10", 3_590_000)["hf"]["state"],
        "not-your-bands"
    );
    // a new station is a new run: on 40 m only, W1AW-10's 1 kW channel and N0SITE-10's
    assert_eq!(runs.load(Ordering::SeqCst), 6 + 4);
}

#[test]
fn without_an_engine_or_a_position_a_channel_says_why() {
    let at = panel(
        "hf-no-engine",
        Hf::without("the VOACAP engine is not installed with this program"),
        Some("EM73tr"),
    );
    let v = http(at, "GET", "/api/gateways", &Value::Null);
    let forty = &channel(&v, "W1AW-10", 7_101_000)["hf"];
    assert_eq!(forty["state"], "no-engine");
    assert_eq!(
        forty["words"],
        "HF not predicted: the VOACAP engine is not installed with this program"
    );
    assert!(v["hf"]["engine"].is_null() && v["hf"]["why"].is_string());

    let runs = Arc::new(AtomicUsize::new(0));
    let hf = Hf::with_engine(Box::new(MadeUp {
        runs: Arc::clone(&runs),
    }));
    let at = panel("hf-nowhere", hf, None);
    let v = http(at, "GET", "/api/gateways", &Value::Null);
    assert_eq!(
        channel(&v, "W1AW-10", 7_101_000)["hf"]["state"],
        "no-position"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 0);
}

/// The real engine, when `RADIOPOST_VOACAP` names it (hf-predict's `.work/engine`, say): a
/// gateway 1 300 km away both ways, and the same run kept.
#[test]
#[ignore = "needs the VOACAP engine: RADIOPOST_VOACAP=<its folder>"]
fn real_voacap_predicts_a_gateway_both_ways() {
    let at = panel("hf-real", Hf::new(), Some("EM73tr"));
    let v = http(at, "GET", "/api/gateways", &Value::Null);
    assert_eq!(v["hf"]["engine"], "voacapl", "{}", v["hf"]);
    let forty = &channel(&v, "W1AW-10", 7_101_000)["hf"];
    assert_eq!(forty["state"], "predicted", "{forty}");
    assert!(
        forty["chance"].is_number() && forty["up"]["snr_db"].is_number(),
        "{forty}"
    );
    println!("{}", serde_json::to_string_pretty(forty).expect("JSON"));
}
