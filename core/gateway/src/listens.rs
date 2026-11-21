// SPDX-License-Identifier: MIT OR Apache-2.0
//! When the gateway listens, as its operator types it and as its record says it (RP-5 §4.4
//! `listens`): each day's times in UTC, such as `18:00-02:00`, several with commas, made into
//! daily windows for callers' programs and into words for `hours`. Empty, it listens always and
//! the record lists no windows.

use serde_json::{Value, json};

const DAY: u64 = 86_400;

/// The daily windows of what the operator typed: `18:00-02:00`, `0800-1200, 18:00-22:00`.
///
/// # Errors
///
/// A part is not two times of day with a dash between, or a window starts and ends together.
pub fn parse(text: &str) -> Result<Vec<Value>, String> {
    let text = text.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("always") {
        return Ok(Vec::new());
    }
    text.split(',')
        .map(|part| {
            let part = part
                .trim()
                .trim_end_matches("UTC")
                .trim_end_matches('Z')
                .trim();
            let (from, to) = part
                .split_once('-')
                .ok_or_else(|| format!("{part:?}: two times of day, such as 18:00-02:00"))?;
            let (from, to) = (time_of(from)?, time_of(to)?);
            let length = (to + DAY - from) % DAY;
            if length == 0 {
                return Err(format!(
                    "{part:?} starts and ends at the same time: for always, leave the times empty"
                ));
            }
            Ok(json!({"every_s": DAY, "at_s": from, "length_s": length}))
        })
        .collect()
}

/// A time of day, `18:00` or `1800`, as seconds after midnight.
fn time_of(text: &str) -> Result<u64, String> {
    let t = text.trim();
    let digits: String = t.chars().filter(char::is_ascii_digit).collect();
    let ok = t.chars().all(|c| c.is_ascii_digit() || c == ':') && digits.len() == 4;
    let (h, m) = (
        digits.get(..2).and_then(|h| h.parse::<u64>().ok()),
        digits.get(2..).and_then(|m| m.parse::<u64>().ok()),
    );
    match (ok, h, m) {
        (true, Some(h), Some(m)) if h < 24 && m < 60 => Ok(h * 3600 + m * 60),
        _ => Err(format!("{t:?} is not a time of day such as 18:00")),
    }
}

/// The daily windows of a record as the operator types them, for the form: what [`parse`] reads.
/// Windows that are not daily are left out, as the form cannot show them.
#[must_use]
pub fn text_of(record: &Value) -> String {
    daily(record)
        .iter()
        .map(|(from, length)| format!("{}-{}", clock(*from), clock((from + length) % DAY)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The same in words for `hours`: `18:00-02:00 UTC`.
#[must_use]
pub fn words(windows: &[Value]) -> String {
    let record = json!({ "listens": windows });
    format!("{} UTC", text_of(&record))
}

fn daily(record: &Value) -> Vec<(u64, u64)> {
    record["listens"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|w| w["every_s"].as_u64() == Some(DAY))
        .filter_map(|w| Some((w["at_s"].as_u64()?, w["length_s"].as_u64()?)))
        .collect()
}

fn clock(s: u64) -> String {
    format!("{:02}:{:02}", s / 3600, s % 3600 / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_of_day_become_daily_windows_and_back() {
        let w = parse("18:00-02:00, 0800-1000 UTC").expect("two windows");
        assert_eq!(
            w,
            vec![
                json!({"every_s": 86_400, "at_s": 64_800, "length_s": 28_800}),
                json!({"every_s": 86_400, "at_s": 28_800, "length_s": 7_200}),
            ]
        );
        let record = json!({"listens": w});
        assert_eq!(text_of(&record), "18:00-02:00, 08:00-10:00");
        assert_eq!(words(&w), "18:00-02:00, 08:00-10:00 UTC");
        // what the form shows reads back the same
        assert_eq!(parse(&text_of(&record)).expect("again"), w);
    }

    #[test]
    fn empty_is_always_and_nonsense_is_refused() {
        assert_eq!(parse("").expect("always"), Vec::<Value>::new());
        assert_eq!(parse(" always ").expect("always"), Vec::<Value>::new());
        for bad in [
            "18:00",
            "25:00-02:00",
            "18:60-19:00",
            "6pm-2am",
            "18:00-18:00",
            "180-0200",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        // a record with no windows shows nothing, and one with weekly windows shows only its daily
        assert_eq!(text_of(&json!({"hours": "always"})), "");
        let weekly = json!({"listens": [{"every_s": 604_800, "at_s": 0, "length_s": 3600},
                                         {"every_s": 86_400, "at_s": 0, "length_s": 3600}]});
        assert_eq!(text_of(&weekly), "00:00-01:00");
    }
}
