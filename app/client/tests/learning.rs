// SPDX-License-Identifier: MIT OR Apache-2.0
//! Phase 4's acceptance (choosing a gateway, §18): learning, replayed on simulated attempt streams,
//! reproduces known offsets within their stated spread; a planted station fault is flagged and not
//! blamed on the gateways; and the same records always give the same estimates.
//!
//! The streams are made here from a seeded generator, as the client's recorder writes its records
//! (version 2): a gateway whose real path sits a known number of decibels from VOACAP's prediction
//! each way, and which answers a known share of the time. Each day's SNR is drawn from the
//! prediction's own spread, moved by the true offset; a call links when the gateway is there, the
//! band is open and both ways clear the modem's call; a session reports what it measured each way,
//! with 2 dB of measurement error. A call that does not link is heard only when the downlink
//! carries and the gateway is on the air for another station: one that never heard the call does
//! not answer it.

use radiopost_client::selection::hf::{Class, Profile};
use radiopost_client::selection::learning::{self, Forget, Forgotten, Learning, Seen};
use serde_json::{Value, json};

/// `SplitMix64`: a small generator whose stream is the same everywhere.
struct Draws(u64);

impl Draws {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn uniform(&mut self) -> f64 {
        #[allow(clippy::cast_precision_loss, reason = "53 bits of a draw")]
        let u = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        u
    }

    /// A standard normal, by Box and Muller.
    fn normal(&mut self) -> f64 {
        let u = self.uniform().max(1e-12);
        let v = self.uniform();
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }
}

/// A path as the model predicts it, and as it really is.
struct Path {
    callsign: &'static str,
    /// The predicted medians each way, dB in 3 kHz, and the deciles' distance.
    down: f64,
    up: f64,
    decile: f64,
    open: f64,
    /// How far the real path sits from the prediction, each way.
    true_down: f64,
    true_up: f64,
    /// How often the gateway answers.
    there: f64,
}

const DAY: u64 = 86_400;
/// How often a gateway that did not hear the call is on the air for someone else, and heard.
const BUSY_ELSEWHERE: f64 = 0.3;
const NOW: u64 = 1_800_000_000;
/// The class the predictions read the path as: Moderate, whose call needs −14.5 dB.
const CLASS: &str = "moderate";

fn call_db() -> f64 {
    Profile::aether().call_db(Class::Moderate)
}

/// The split normal's draw: half the days below the median, half above, each with its decile.
fn day_snr(d: &mut Draws, median: f64, decile: f64) -> f64 {
    median + d.normal() * decile / 1.281_551_565_544_6
}

fn predicted(p: &Path) -> Value {
    let way = |m: f64| json!({"snr_db": m, "lower_db": p.decile, "upper_db": p.decile});
    // the chance the model gives (what the panel showed), for station health
    let sigma = p.decile / 1.281_551_565_544_6;
    let above = |m: f64| {
        let z = (call_db() - m) / sigma;
        0.5 * libm_erfc(z / std::f64::consts::SQRT_2)
    };
    let chance = above(p.down) * above(p.up) * p.open;
    json!({"hf": {"state": "predicted", "band": "40 m", "class": CLASS, "open": p.open,
                  "open_model": p.open, "chance": chance, "up": way(p.up), "down": way(p.down)}})
}

/// erfc, to the precision the tests need.
fn libm_erfc(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.5 * z);
    let r = t
        * (-z * z - 1.265_512_23
            + t * (1.000_023_68
                + t * (0.374_091_96
                    + t * (0.096_784_18
                        + t * (-0.186_288_06
                            + t * (0.278_868_07
                                + t * (-1.135_203_98
                                    + t * (1.488_515_87
                                        + t * (-0.822_152_23 + t * 0.170_872_77)))))))))
            .exp();
    if x >= 0.0 { r } else { 2.0 - r }
}

/// One call's record, as the recorder writes it.
fn record(id: &str, start: u64, p: &Path, outcome: &Outcome) -> Value {
    let (class, evidence, call, link) = match *outcome {
        Outcome::Linked { down, up } => (
            "completed",
            json!({"opened": true, "connected": true, "over": true}),
            json!({"duration_s": 12.0, "result": "connected", "heard_remote": false,
                   "busy_before": false}),
            json!({"duration_s": 240.0, "bytes_handed": 6000, "bytes_unacknowledged": 0,
                   "bytes_in": 9000, "bandwidth_hz": 2300,
                   "snr_db": {"median": down, "min": down, "max": down, "readings": 20},
                   "peer_snr_db": {"median": up, "min": up, "max": up, "readings": 20},
                   "quality": [[5.0, down, 900.0, true, up]]}),
        ),
        Outcome::Silent => (
            "no-answer",
            json!({"opened": true, "failed": "timeout"}),
            json!({"duration_s": 60.0, "result": "timeout", "heard_remote": false,
                   "heard_said": false, "busy_before": false}),
            Value::Null,
        ),
        Outcome::Heard { down } => (
            "no-answer-heard",
            json!({"opened": true, "failed": "timeout", "heard_remote": true}),
            json!({"duration_s": 60.0, "result": "timeout", "heard_remote": true,
                   "heard_snr_db": down, "heard_said": true, "busy_before": false}),
            Value::Null,
        ),
    };
    json!({"v": 2, "id": id, "start": start, "end": start + 300, "kind": "gateway",
           "remote": p.callsign, "channel": {"dial_hz": 7_101_000, "bandwidth_hz": 2300},
           "modem": "Aether HF 0.2.0", "predicted": predicted(p), "call": call, "link": link,
           "outcome": {"class": class}, "classifier": 1, "evidence": evidence})
}

enum Outcome {
    Linked { down: f64, up: f64 },
    Silent,
    Heard { down: f64 },
}

/// `n` calls to `p` over `days`, ending a day before [`NOW`].
fn stream(d: &mut Draws, p: &Path, n: u64, days: u64, tag: &str) -> Vec<Value> {
    (0..n)
        .map(|k| {
            let start = NOW - DAY - days * DAY + k * days * DAY / n + d.next() % 3600;
            let there = d.uniform() < p.there;
            let open = d.uniform() < p.open;
            let down = day_snr(d, p.down + p.true_down, p.decile);
            let up = day_snr(d, p.up + p.true_up, p.decile);
            let outcome = if there && open && down >= call_db() && up >= call_db() {
                Outcome::Linked {
                    down: down + 2.0 * d.normal(),
                    up: up + 2.0 * d.normal(),
                }
            } else if there && open && down >= call_db() && d.uniform() < BUSY_ELSEWHERE {
                // the gateway never heard the call, so it did not answer; it is heard only when it
                // is on the air for another station
                Outcome::Heard { down }
            } else {
                Outcome::Silent
            };
            record(&format!("a-{tag}-{k:04}"), start, p, &outcome)
        })
        .collect()
}

fn gateway(callsign: &'static str, true_down: f64, true_up: f64, there: f64) -> Path {
    Path {
        callsign,
        down: 4.0,
        up: 2.0,
        decile: 10.0,
        open: 0.95,
        true_down,
        true_up,
        there,
    }
}

#[test]
fn known_offsets_are_learned_within_their_stated_spread() {
    let mut draws = Draws(17);
    // the real path 6 dB under the prediction coming in, 9 dB under going out; answers 4 in 5
    let p = gateway("KK4ODA-1", -6.0, -9.0, 0.8);
    let records = stream(&mut draws, &p, 200, 60, "k");
    let l = Learning::learn(&records, &Forgotten::default(), &Seen::new(), NOW);
    let c = l.channel("KK4ODA-1", "40 m").expect("learned");
    // each within two of its stated standard deviations, and the spreads honest (under 3 dB)
    assert!(
        (c.down.mean - p.true_down).abs() <= 2.0 * c.down.sd.max(0.5),
        "down {:?}",
        c.down
    );
    assert!(
        (c.up.mean - p.true_up).abs() <= 2.0 * c.up.sd.max(0.5),
        "up {:?}",
        c.up
    );
    assert!(c.down.sd < 3.0 && c.up.sd < 3.0, "{c:?}");
    let a = l.availability["KK4ODA-1"].0;
    assert!((a.mean - p.there).abs() < 0.1, "availability {a:?}");
    assert!((c.there - p.there).abs() < 0.15, "on the grid {}", c.there);
    assert!(!c.drift, "a steady path is no drift");
    // the direction learned over all gateways: up is 3 dB under down
    assert!(
        (l.direction.mean + 3.0).abs() <= 2.0 * l.direction.sd,
        "{:?}",
        l.direction
    );
    assert!(l.direction.sd < 2.0, "{:?}", l.direction);
    // no fault: every call was predicted to connect less than 0.7, or worked
    assert!(l.health.warning.is_none(), "{:?}", l.health);
}

#[test]
fn a_better_path_than_predicted_is_learned_too() {
    let mut d = Draws(5);
    let p = gateway("W1AW-10", 5.0, 3.0, 0.95);
    let records = stream(&mut d, &p, 120, 45, "b");
    let l = Learning::learn(&records, &Forgotten::default(), &Seen::new(), NOW);
    let c = l.channel("W1AW-10", "40 m").expect("learned");
    assert!(
        (c.down.mean - 5.0).abs() <= 2.0 * c.down.sd.max(0.5),
        "{:?}",
        c.down
    );
    assert!(
        (c.up.mean - 3.0).abs() <= 2.0 * c.up.sd.max(0.5),
        "{:?}",
        c.up
    );
}

/// Two reliable gateways, then four calls in the last day to both of them that all fail with
/// nothing heard, though each was predicted to connect: the station's fault.
fn faulty_day() -> (Vec<Value>, Vec<Value>) {
    let mut d = Draws(99);
    let a = gateway("KK4ODA-1", 0.0, 0.0, 0.97);
    let b = gateway("N1CALL-10", 0.0, 0.0, 0.97);
    let mut good = stream(&mut d, &a, 60, 40, "a");
    good.extend(stream(&mut d, &b, 60, 40, "b"));
    // strong predictions: these calls should all have worked
    let strong = |p: &Path| Path {
        down: 20.0,
        up: 18.0,
        decile: 8.0,
        open: 0.99,
        ..*p
    };
    let mut fault = good.clone();
    for (k, p) in [&a, &b, &a, &b].into_iter().enumerate() {
        let k = u64::try_from(k).expect("a few");
        fault.push(record(
            &format!("a-fault-{k}"),
            NOW - 4 * 3600 + k * 600 + 1,
            &strong(p),
            &Outcome::Silent,
        ));
    }
    (good, fault)
}

#[test]
fn a_planted_station_fault_is_flagged_and_not_blamed_on_the_gateways() {
    let (good, fault) = faulty_day();
    let before = Learning::learn(&good, &Forgotten::default(), &Seen::new(), NOW);
    let after = Learning::learn(&fault, &Forgotten::default(), &Seen::new(), NOW);
    let h = &after.health;
    assert!(h.p_fault > 0.9, "{h:?}");
    assert_eq!((h.failures, h.gateways), (4, 2));
    let warning = h.warning.as_deref().expect("said");
    assert!(
        warning.starts_with("Your last 4 calls failed on 2 gateways"),
        "{warning}"
    );
    // the gateways keep what they had: their availability and offsets hardly move
    for gw in ["KK4ODA-1", "N1CALL-10"] {
        let (a0, a1) = (
            before.availability[gw].0.mean,
            after.availability[gw].0.mean,
        );
        assert!((a0 - a1).abs() < 0.02, "{gw}: availability {a0} → {a1}");
        let (c0, c1) = (
            before.channel(gw, "40 m").expect("before"),
            after.channel(gw, "40 m").expect("after"),
        );
        assert!(
            (c0.down.mean - c1.down.mean).abs() < 0.5,
            "{gw}: {c0:?} → {c1:?}"
        );
        assert!(
            (c0.there - c1.there).abs() < 0.03,
            "{gw}: {} → {}",
            c0.there,
            c1.there
        );
    }
    // a day later the fault has gone from the window, and nothing is flagged
    let later = Learning::learn(&fault, &Forgotten::default(), &Seen::new(), NOW + 2 * DAY);
    assert!(later.health.warning.is_none());
}

#[test]
fn without_the_health_check_the_same_failures_would_have_been_blamed() {
    // the planted failures, weighed in full as an ordinary silence, move the gateways: the check
    // above is what keeps them still (the test of the test)
    let (good, fault) = faulty_day();
    let before = Learning::learn(&good, &Forgotten::default(), &Seen::new(), NOW);
    // the same failures spread over the month: no run in the last day, so no fault
    let spread: Vec<Value> = fault
        .into_iter()
        .map(|mut r| {
            if r["id"].as_str().is_some_and(|i| i.starts_with("a-fault")) {
                let start = r["start"].as_u64().expect("a start") - 20 * DAY;
                r["start"] = json!(start);
            }
            r
        })
        .collect();
    let blamed = Learning::learn(&spread, &Forgotten::default(), &Seen::new(), NOW);
    assert!(blamed.health.warning.is_none());
    let moved = ["KK4ODA-1", "N1CALL-10"].iter().any(|gw| {
        (before.availability[*gw].0.mean - blamed.availability[*gw].0.mean).abs() >= 0.02
    });
    assert!(
        moved,
        "four silent calls predicted to work do count against the gateways"
    );
}

#[test]
fn the_same_records_always_give_the_same_estimates() {
    let (_, fault) = faulty_day();
    let mut d = Draws(3);
    let mut records = fault;
    records.extend(stream(
        &mut d,
        &gateway("W1AW-10", -3.0, -1.0, 0.7),
        80,
        30,
        "w",
    ));
    let one = Learning::learn(&records, &Forgotten::default(), &Seen::new(), NOW).json();
    let two = Learning::learn(&records, &Forgotten::default(), &Seen::new(), NOW).json();
    assert_eq!(one.to_string(), two.to_string());
    // in any order
    records.reverse();
    let reversed = Learning::learn(&records, &Forgotten::default(), &Seen::new(), NOW).json();
    assert_eq!(one.to_string(), reversed.to_string());
}

#[test]
fn the_cache_gives_exactly_what_learning_gives() {
    // learned.json holds the last result: read back, it must be the same to the last bit, and a
    // change to the records or the rules must be learned again, not read
    let base = std::env::temp_dir().join(format!("radiopost-learned-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let dir = base.join("selection/attempts");
    std::fs::create_dir_all(&dir).expect("a folder");
    let (_, mut records) = faulty_day();
    let mut d = Draws(11);
    records.extend(stream(
        &mut d,
        &gateway("W1AW-10", -3.0, -5.0, 0.85),
        60,
        30,
        "w",
    ));
    let write = |records: &[Value]| {
        let lines: Vec<String> = records.iter().map(Value::to_string).collect();
        std::fs::write(dir.join("2026-11.jsonl"), lines.join("\n") + "\n").expect("written");
    };
    write(&records);
    let seen: Seen = [("KK4ODA-1".to_owned(), NOW - 3600)].into_iter().collect();
    let fresh = Learning::learn(&records, &Forgotten::default(), &seen, NOW);
    let first = Learning::cached(&base, &seen, NOW);
    assert!(base.join("selection/learned.json").exists());
    let kept = Learning::cached(&base, &seen, NOW);
    assert_eq!(first, fresh);
    assert_eq!(kept, fresh, "read back from the cache");
    assert_eq!(kept.json().to_string(), fresh.json().to_string());
    // a record more, a rule, another sighting, an hour later: each learned again
    records.truncate(records.len() - 1);
    write(&records);
    let fewer = Learning::cached(&base, &seen, NOW);
    assert_eq!(
        fewer,
        Learning::learn(&records, &Forgotten::default(), &seen, NOW)
    );
    assert_ne!(fewer, kept);
    let forget = Forgotten {
        rules: vec![Forget::Gateway {
            callsign: "W1AW-10".into(),
            at: NOW,
        }],
    };
    forget.save(&base).expect("kept");
    let forgotten = Learning::cached(&base, &seen, NOW);
    assert!(forgotten.channel("W1AW-10", "40 m").is_none());
    std::fs::remove_file(base.join(learning::FORGOTTEN)).expect("removed");
    let other: Seen = [("KK4ODA-1".to_owned(), NOW - 9 * DAY)]
        .into_iter()
        .collect();
    assert_eq!(
        Learning::cached(&base, &other, NOW),
        Learning::learn(&records, &Forgotten::default(), &other, NOW)
    );
    let later = NOW + 2 * 3600;
    assert_eq!(
        Learning::cached(&base, &seen, later),
        Learning::learn(&records, &Forgotten::default(), &seen, later)
    );
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn forgotten_records_are_left_out_and_counted() {
    let mut d = Draws(8);
    let mut records = stream(&mut d, &gateway("KK4ODA-1", -6.0, -9.0, 0.8), 50, 20, "k");
    records.extend(stream(
        &mut d,
        &gateway("W1AW-10", 2.0, 1.0, 0.9),
        50,
        20,
        "w",
    ));
    let forget = Forgotten {
        rules: vec![Forget::Gateway {
            callsign: "kk4oda-1".into(),
            at: NOW,
        }],
    };
    let l = Learning::learn(&records, &forget, &Seen::new(), NOW);
    assert!(l.channel("KK4ODA-1", "40 m").is_none());
    assert!(l.channel("W1AW-10", "40 m").is_some());
    assert_eq!((l.records, l.forgotten), (50, 50));
    // calls after the gateway was forgotten count again
    let after = Forgotten {
        rules: vec![Forget::Gateway {
            callsign: "KK4ODA-1".into(),
            at: NOW - 10 * DAY,
        }],
    };
    let l = Learning::learn(&records, &after, &Seen::new(), NOW);
    assert!(l.channel("KK4ODA-1", "40 m").is_some_and(|c| c.calls > 0.0));
    let everything = Forgotten {
        rules: vec![Forget::Everything { at: NOW }],
    };
    let l = Learning::learn(&records, &everything, &Seen::new(), NOW);
    assert_eq!((l.records, l.forgotten), (0, 100));
}

/// The acceptance's stronger form, too slow for a debug build: four paths, twenty streams each.
/// Each offset's error over its stated spread has a root mean square near one (the spread is
/// honest) and a mean near nought (no bias); availability within a few hundredths. Measured
/// (2026-11-21): RMS 0.45–1.00, means −0.47 to +0.38, availability's mean error −0.07 to −0.005.
/// Run with `cargo test --release -p radiopost-client --test learning -- --ignored`.
#[test]
#[ignore = "eighty streams: run in a release build"]
fn the_spreads_are_honest_over_many_streams() {
    for (down, up, there) in [
        (-6.0, -9.0, 0.8),
        (0.0, 0.0, 0.95),
        (3.0, -4.0, 0.6),
        (-10.0, -10.0, 0.9),
    ] {
        let mut errors = Vec::new();
        for seed in 1..=20u64 {
            let mut draws = Draws(seed);
            let p = gateway("KK4ODA-1", down, up, there);
            let records = stream(&mut draws, &p, 200, 60, "k");
            let l = Learning::learn(&records, &Forgotten::default(), &Seen::new(), NOW);
            let c = l.channel("KK4ODA-1", "40 m").expect("learned");
            errors.push([
                (c.down.mean - down) / c.down.sd,
                (c.up.mean - up) / c.up.sd,
                (c.direction.mean - (up - down)) / c.direction.sd,
                l.availability["KK4ODA-1"].0.mean - there,
            ]);
        }
        #[allow(clippy::cast_precision_loss, reason = "twenty")]
        let n = errors.len() as f64;
        let mean = |k: usize| errors.iter().map(|v| v[k]).sum::<f64>() / n;
        let rms = |k: usize| (errors.iter().map(|v| v[k] * v[k]).sum::<f64>() / n).sqrt();
        let path = format!("({down}, {up}, {there})");
        for (k, what) in [(0, "down"), (1, "up"), (2, "direction")] {
            println!("{path} {what}: mean {:+.2} rms {:.2}", mean(k), rms(k));
            assert!(mean(k).abs() < 0.5 && rms(k) < 1.25, "{path} {what}");
        }
        println!(
            "{path} availability: mean {:+.3} rms {:.3}",
            mean(3),
            rms(3)
        );
        assert!(mean(3).abs() < 0.08 && rms(3) < 0.1, "{path} availability");
    }
}
