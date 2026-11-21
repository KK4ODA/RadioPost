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
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use radiopost_client::panel::Panel;
use radiopost_client::selection::evidence::Evidence;
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
/// at each bandwidth, a VARA one, a 160 m one, a VHF one and a blocked one; and one with no site,
/// listening from 02:00 to 04:00 UTC.
fn panel(name: &str, hf: Hf, grid: Option<&str>) -> SocketAddr {
    panel_at(name, hf, grid).0
}

/// [`panel`], and its home.
fn panel_at(name: &str, hf: Hf, grid: Option<&str>) -> (SocketAddr, PathBuf) {
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
             "listens": [{"every_s": 86_400, "at_s": 7200, "length_s": 7200}],
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
    (at, home)
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

/// Twenty sessions with W1AW-10 on 40 m, a day apart over the last twenty days, each measuring
/// the path 10 dB under what the model said each way.
fn sessions_ten_under(home: &std::path::Path, model: &Value) -> usize {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock")
        .as_secs();
    let down = model["down"]["snr_db"].as_f64().expect("down") - 10.0;
    let up = model["up"]["snr_db"].as_f64().expect("up") - 10.0;
    let lines: Vec<String> = (0..20u64)
        .map(|k| {
            let start = now - (20 - k) * 86_400;
            json!({"v": 2, "id": format!("a-{k:02}"), "start": start, "end": start + 300,
                   "kind": "gateway", "remote": "W1AW-10",
                   "channel": {"dial_hz": 7_101_000, "bandwidth_hz": 2300},
                   "modem": "Aether HF 0.2.0", "predicted": {"hf": model},
                   "call": {"duration_s": 12.0, "result": "connected"},
                   "link": {"duration_s": 240.0, "bytes_handed": 6000, "bytes_unacknowledged": 0,
                            "bytes_in": 9000, "bandwidth_hz": 2300,
                            "snr_db": {"median": down, "readings": 20},
                            "peer_snr_db": {"median": up, "readings": 20}},
                   "outcome": {"class": "completed"}, "classifier": 1,
                   "evidence": {"opened": true, "connected": true, "over": true}})
            .to_string()
        })
        .collect();
    let dir = home.join("selection/attempts");
    std::fs::create_dir_all(&dir).expect("a folder");
    std::fs::write(dir.join("2026-11.jsonl"), lines.join("\n") + "\n").expect("written");
    lines.len()
}

#[test]
fn what_was_learned_goes_beside_the_model_and_can_be_forgotten() {
    let hf = Hf::with_engine(Box::new(MadeUp {
        runs: Arc::new(AtomicUsize::new(0)),
    }));
    let (at, home) = panel_at("hf-learned", hf, Some("EM73tr"));
    let before = http(at, "GET", "/api/gateways", &Value::Null);
    let model = channel(&before, "W1AW-10", 7_101_000)["hf"].clone();
    assert!(
        model.get("learned").is_none(),
        "nothing learned yet: {model}"
    );
    assert_eq!(sessions_ten_under(&home, &model), 20);

    // the line keeps the model's figures, and what was learned goes beside them
    let v = http(at, "GET", "/api/gateways", &Value::Null);
    let forty = &channel(&v, "W1AW-10", 7_101_000)["hf"];
    for way in ["up", "down"] {
        assert_eq!(
            forty[way]["snr_db"], model[way]["snr_db"],
            "the model's {way} kept"
        );
        let learned = forty["learned"][way]["snr_db"].as_f64().expect("learned");
        let said = model[way]["snr_db"].as_f64().expect("the model");
        assert!(
            (learned - (said - 10.0)).abs() < 2.0,
            "{way}: {learned} against {said}"
        );
    }
    let learned = &forty["learned"];
    assert!(
        learned["chance"].as_f64() < forty["chance"].as_f64(),
        "{learned}"
    );
    assert!(
        learned["calls"].as_f64().is_some_and(|c| c > 0.0),
        "{learned}"
    );
    assert!(
        learned["words"]
            .as_str()
            .is_some_and(|w| w.ends_with("learned from your calls")),
        "{learned}"
    );
    // a channel never called has nothing learned
    assert!(
        channel(&v, "W1AW-10", 3_590_000)["hf"]
            .get("learned")
            .is_none()
    );
    assert_eq!(v["learning"]["records"], 20, "{}", v["learning"]);
    assert!(v["learning"]["health"]["warning"].is_null());
    // the cache: kept under the learning's version
    let kept: Value = serde_json::from_str(
        &std::fs::read_to_string(home.join("selection/learned.json")).expect("kept"),
    )
    .expect("JSON");
    assert_eq!(kept["version"], 1);
    details_then_forgetting(at, &model);
}

/// The details of W1AW-10, then its history forgotten and brought back.
fn details_then_forgetting(at: SocketAddr, model: &Value) {
    // the details: the model, what was learned and what was measured, side by side
    let d = http(at, "GET", "/api/learned?callsign=w1aw-10", &Value::Null);
    assert_eq!(d["callsign"], "W1AW-10");
    let channels = d["channels"].as_array().expect("channels");
    let forty = channels
        .iter()
        .find(|c| c["channel"] == "40 m")
        .expect("40 m");
    assert_eq!(forty["model"]["hf"]["down"], model["down"], "{forty}");
    let offset = forty["learned"]["down"]["mean"]
        .as_f64()
        .expect("an offset");
    assert!((offset + 10.0).abs() < 2.0, "{forty}");
    let m = &forty["measured"];
    assert_eq!(
        (m["calls"].as_u64(), m["answered"].as_u64()),
        (Some(20), Some(20)),
        "{m}"
    );
    assert_eq!(m["down"]["n"], 20, "{m}");
    assert!(m["bps"]["min"].as_f64().is_some_and(|b| b > 0.0), "{m}");
    assert!(
        channels
            .iter()
            .any(|c| c["channel"] == "80 m" && c["learned"].is_null())
    );
    assert_eq!(d["recent"].as_array().map(Vec::len), Some(10));
    assert!(
        d["availability"][0]["mean"]
            .as_f64()
            .is_some_and(|a| a > 0.8),
        "{d}"
    );

    // forgotten: the records stay, learning leaves them out, and they come back on undo
    let f = http(
        at,
        "POST",
        "/api/forget",
        &json!({"what": "gateway", "callsign": "w1aw-10"}),
    );
    assert_eq!(
        (f["records"].as_u64(), f["forgotten"].as_u64()),
        (Some(20), Some(20)),
        "{f}"
    );
    let v = http(at, "GET", "/api/gateways", &Value::Null);
    assert!(
        channel(&v, "W1AW-10", 7_101_000)["hf"]
            .get("learned")
            .is_none()
    );
    assert_eq!(v["learning"]["forgotten"], 20);
    let recent = http(at, "GET", "/api/attempts", &Value::Null);
    assert!(
        recent["recent"]
            .as_array()
            .expect("calls")
            .iter()
            .all(|r| r["forgotten"] == true),
        "{recent}"
    );
    let d = http(at, "GET", "/api/learned?callsign=W1AW-10", &Value::Null);
    assert_eq!(d["forgotten"], true);
    let f = http(at, "POST", "/api/forget", &json!({"undo": 0}));
    assert_eq!(f["forgotten"], 0, "{f}");
    let v = http(at, "GET", "/api/gateways", &Value::Null);
    assert!(
        channel(&v, "W1AW-10", 7_101_000)["hf"]
            .get("learned")
            .is_some()
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

#[test]
fn another_time_is_judged_at_its_own_hour_and_month() {
    let runs = Arc::new(AtomicUsize::new(0));
    let hf = Hf::with_engine(Box::new(MadeUp {
        runs: Arc::clone(&runs),
    }));
    let at = panel("hf-another-time", hf, Some("EM73tr"));
    let now = http(at, "GET", "/api/gateways", &Value::Null);
    assert!(now["at"].is_null());
    // 03:00 UTC two months on: that month's runs, the hour 03Z, and N0SITE-10 listening then
    let day = 86_400;
    let today = radioprop_hf::timeutil::now() / day * day;
    let then = u64::try_from(today + 62 * day + 3 * 3600).expect("after 1970");
    let (year, month, _, _) = radioprop_hf::timeutil::civil(today + 62 * day);
    let v = http(at, "GET", &format!("/api/gateways?at={then}"), &Value::Null);
    assert_eq!(v["at"], then);
    let forty = &channel(&v, "W1AW-10", 7_101_000)["hf"];
    assert_eq!(forty["month"], format!("{year}-{month:02}"), "{forty}");
    assert_eq!(forty["hour"], 3);
    assert!(
        forty["words"]
            .as_str()
            .is_some_and(|w| w.contains("at 03Z")),
        "{forty}"
    );
    let gw = v["gateways"]
        .as_array()
        .and_then(|g| g.iter().find(|g| g["callsign"] == "N0SITE-10"))
        .expect("listed");
    assert_eq!(
        gw["listening"],
        json!({"open": true, "changes": then + 3600})
    );
    // half past the hour reads the next one
    let v = http(
        at,
        "GET",
        &format!("/api/gateways?at={}", then + 1800),
        &Value::Null,
    );
    assert_eq!(channel(&v, "W1AW-10", 7_101_000)["hf"]["hour"], 4);
    // another month is another set of runs, kept like the first
    let first = runs.load(Ordering::SeqCst);
    assert_eq!(first, 12, "this month's and that one's");
    http(at, "GET", &format!("/api/gateways?at={then}"), &Value::Null);
    assert_eq!(runs.load(Ordering::SeqCst), first);
}

/// A stand-in for hf-predict's evidence API: each place asked about gets 40 m, the first with three
/// stations heard near it (moderate) and the others nothing; each request's head is kept.
fn hf_predict() -> (SocketAddr, Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
    let at = listener.local_addr().expect("bound");
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let kept = Arc::clone(&asked);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut head = String::new();
            let mut reader = std::io::BufReader::new(stream.try_clone().expect("a copy"));
            loop {
                let mut line = String::new();
                if std::io::BufRead::read_line(&mut reader, &mut line).map_or(true, |n| n == 0)
                    || line == "\r\n"
                {
                    break;
                }
                head.push_str(&line);
            }
            let places = head.matches("toward=").count();
            let body = json!({"places": (0..places).map(|i| json!({"bands": [
                {"band": "40 m", "tier": if i == 0 { "moderate" } else { "none" },
                 "stations": if i == 0 { 3 } else { 0 }, "best_snr_db": if i == 0 { json!(-8) } else { json!(null) },
                 "periods_listened": 40.0, "examples": ["W1AAA"], "hearing_stations": 1, "hearing_you": 0}]}))
                .collect::<Vec<_>>()})
            .to_string();
            kept.lock().expect("kept").push(head);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (at, asked)
}

#[test]
fn what_hf_predict_heard_in_the_last_hour_raises_the_band() {
    let runs = Arc::new(AtomicUsize::new(0));
    let (stand_in, asked) = hf_predict();
    let hf = Hf::with_engine(Box::new(MadeUp {
        runs: Arc::clone(&runs),
    }))
    .with_evidence(Evidence::at(Some(stand_in)));
    let at = panel("hf-evidence", hf, Some("EM73tr"));
    let v = http(at, "GET", "/api/gateways", &Value::Null);
    assert_eq!(v["hf"]["heard"], true, "{}", v["hf"]);
    // one request for the whole list: each run's gateway a place, every band predicted
    let heads = asked.lock().expect("asked").clone();
    assert_eq!(heads.len(), 1, "{heads:?}");
    let head = &heads[0];
    assert!(head.starts_with("GET /v1/evidence?from="), "{head}");
    assert_eq!(head.matches("toward=").count(), 3, "{head}");
    assert!(head.contains("&bands=20m,40m,80m"), "{head}");
    assert!(head.contains(&format!("Host: {stand_in}\r\n")), "{head}");
    // the list is nearest first, so the first place asked about is N0SITE-10: its 40 m channel
    // open on 0.9 of days by the model, raised 2:1 in odds by three stations heard near it
    let near = &channel(&v, "N0SITE-10", 7_102_000)["hf"];
    assert_eq!(near["heard"]["tier"], "moderate", "{near}");
    assert_eq!(near["open_model"], 0.9);
    let raised = 2.0 * 0.9 / 1.9;
    assert!(
        (near["open"].as_f64().expect("open") - raised).abs() < 0.006,
        "{near}"
    );
    assert!(
        near["heard"]["words"]
            .as_str()
            .is_some_and(|w| w.starts_with("FT8 from 3 stations near it heard here on 40 m"))
    );
    // nothing heard toward W1AW-10: said, and the band left as modelled
    let far = &channel(&v, "W1AW-10", 7_101_000)["hf"];
    assert_eq!(far["heard"]["tier"], "none", "{far}");
    assert_eq!(far["open"], far["open_model"]);
    // 80 m: hf-predict said nothing of it
    assert!(
        channel(&v, "W1AW-10", 3_590_000)["hf"]
            .get("heard")
            .is_none()
    );
    // another time: today's evidence does not count, and hf-predict is not asked
    let later = radioprop_hf::timeutil::now() + 86_400;
    let v = http(
        at,
        "GET",
        &format!("/api/gateways?at={later}"),
        &Value::Null,
    );
    let near = &channel(&v, "N0SITE-10", 7_102_000)["hf"];
    assert!(near.get("heard").is_none(), "{near}");
    assert_eq!(near["open"], near["open_model"]);
    assert_eq!(asked.lock().expect("asked").len(), 1);
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
