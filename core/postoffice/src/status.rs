// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whether a post office's gateways are on the air (RP-5 §4.6, §5.9): the reports it takes from
//! its own gateways, what it keeps of each, and the status list it publishes. The times it keeps
//! are its own clock's, and it publishes them to the minute and the last session to the hour,
//! whatever the gateway sent.

use radiopost::ErrorCode;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json, json};

use crate::office::{Holder, PostOffice, Response, problem};

/// The states a report may give (RP-5 §5.9).
pub const STATES: [&str; 4] = ["listening", "session", "opening", "stopped"];
/// The most problems one report names (RP-5 §5.9).
pub const MOST_PROBLEMS: usize = 16;

/// A gateway's last report, as kept (RP-5 §5.9).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GatewayStatus {
    /// The report as published: `state`, and `channels`, `problems` and `carried` when it gave
    /// them.
    pub report: Json,
    /// When it came, Unix seconds by this post office's clock.
    pub reported: u64,
}

/// A problem's name (RP-0 §9.7): 1 to 32 characters of lower-case letters, digits and `-`.
fn is_name(n: &Json) -> bool {
    n.as_str().is_some_and(|s| {
        (1..=32).contains(&s.len())
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

/// A channel of a report (RP-5 §5.9): an integer `dial_hz`, a text `modem` and a bool `up`.
fn is_channel(c: &Json) -> bool {
    (c["dial_hz"].is_i64() || c["dial_hz"].is_u64())
        && c["modem"].is_string()
        && c["up"].is_boolean()
}

/// RP-5 §5.9: the report the post office keeps of `body` from the gateway whose record is
/// `record`, at `now`; `None` when the body is malformed. Only the record's channels, matched by
/// `dial_hz` and `modem`, each once, in the report's order; the problems each once, names it does
/// not know kept; `carried` to the hour, no later than `now`; an empty list or an absent member
/// left out; members it does not know ignored.
#[must_use]
pub fn kept(body: &Json, record: &Json, now: u64) -> Option<Json> {
    let state = body.get("state").and_then(Json::as_str)?;
    if !STATES.contains(&state) {
        return None;
    }
    let empty = Vec::new();
    let channels = match body.get("channels") {
        None => &empty,
        Some(c) => c.as_array()?,
    };
    let problems = match body.get("problems") {
        None => &empty,
        Some(p) => p.as_array()?,
    };
    let carried = match body.get("carried") {
        None => None,
        Some(c) => Some(c.as_u64()?),
    };
    if !channels.iter().all(is_channel)
        || problems.len() > MOST_PROBLEMS
        || !problems.iter().all(is_name)
    {
        return None;
    }
    let listed: Vec<(&Json, &Json)> = record["channels"]
        .as_array()
        .map(|cs| cs.iter().map(|c| (&c["dial_hz"], &c["modem"])).collect())
        .unwrap_or_default();
    let mut kept_channels: Vec<Json> = Vec::new();
    for c in channels {
        let key = (&c["dial_hz"], &c["modem"]);
        let again = kept_channels
            .iter()
            .any(|k| (&k["dial_hz"], &k["modem"]) == key);
        if listed.contains(&key) && !again {
            kept_channels
                .push(json!({"dial_hz": c["dial_hz"], "modem": c["modem"], "up": c["up"]}));
        }
    }
    let mut named: Vec<&Json> = Vec::new();
    for p in problems {
        if !named.contains(&p) {
            named.push(p);
        }
    }
    let mut report = Map::new();
    report.insert("state".into(), json!(state));
    if !kept_channels.is_empty() {
        report.insert("channels".into(), Json::Array(kept_channels));
    }
    if !named.is_empty() {
        report.insert("problems".into(), json!(named));
    }
    if let Some(c) = carried {
        report.insert("carried".into(), json!(c.min(now) / 3600 * 3600));
    }
    Some(Json::Object(report))
}

impl PostOffice {
    /// RP-5 §5.9: `PUT /status`, a gateway's report, or `DELETE /status`, its withdrawal; only
    /// from its own credential.
    pub(crate) fn status_report(
        &mut self,
        gateway: &Holder,
        method: &str,
        body: &Json,
    ) -> Response {
        let Some(found) = self.registered(&gateway.operator) else {
            return problem(ErrorCode::UnknownMessage, Some(404));
        };
        if !self.own_credential(&gateway.address) {
            return problem(ErrorCode::NotAuthorized, None);
        }
        if method == "DELETE" {
            self.state.gateways_status.remove(&found);
            return Response::empty();
        }
        let Some(report) = kept(body, &self.state.gateways[&found], self.now()) else {
            return problem(ErrorCode::Malformed, None);
        };
        self.state.gateways_status.insert(
            found,
            GatewayStatus {
                report,
                reported: self.now(),
            },
        );
        self.saw(&gateway.address, &gateway.operator);
        Response::empty()
    }

    /// RP-5 §4.6: the payload of `GET /gateways/status` at `now`: each of its gateways that holds
    /// a report, `seen` and `reported` to the minute.
    #[must_use]
    pub fn status_list(&self, now: u64) -> Json {
        let status: Vec<Json> = self
            .state
            .gateways_status
            .iter()
            .map(|(callsign, s)| {
                let seen = self
                    .state
                    .gateways_seen
                    .get(callsign)
                    .copied()
                    .unwrap_or(s.reported);
                json!({"callsign": callsign, "seen": seen / 60 * 60,
                       "reported": s.reported / 60 * 60, "report": s.report})
            })
            .collect();
        json!({"issued": now, "status": status})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> Json {
        json!({"callsign": "KK4ODA-1", "channels": [
            {"dial_hz": 3_590_000, "mode": "USB", "modem": "aether", "bandwidth_hz": 2300},
            {"dial_hz": 7_101_000, "mode": "USB", "modem": "aether", "bandwidth_hz": 2300}]})
    }

    #[test]
    fn a_report_is_kept_as_rp5_5_9_says() {
        let body = json!({"state": "session",
            "channels": [{"dial_hz": 7_101_000, "modem": "aether", "up": false},
                         {"dial_hz": 7_101_000, "modem": "ardop", "up": true},
                         {"dial_hz": 3_590_000, "modem": "aether", "up": true},
                         {"dial_hz": 7_101_000, "modem": "aether", "up": true}],
            "problems": ["modem-away", "a-later-one", "modem-away"],
            "carried": 7300, "note": "ignored"});
        assert_eq!(
            kept(&body, &record(), 10_000),
            Some(json!({"state": "session",
                "channels": [{"dial_hz": 7_101_000, "modem": "aether", "up": false},
                             {"dial_hz": 3_590_000, "modem": "aether", "up": true}],
                "problems": ["modem-away", "a-later-one"], "carried": 7200}))
        );
        // an empty list and an absent `carried` are not kept; a last session ahead of the clock
        // is the clock's hour
        assert_eq!(
            kept(
                &json!({"state": "listening", "problems": [], "channels": []}),
                &record(),
                10_000
            ),
            Some(json!({"state": "listening"}))
        );
        assert_eq!(
            kept(
                &json!({"state": "opening", "carried": 99_999}),
                &record(),
                10_000
            ),
            Some(json!({"state": "opening", "carried": 7200}))
        );
    }

    #[test]
    fn a_malformed_report_is_refused_whole() {
        let bad = [
            json!({}),
            json!("listening"),
            json!({"state": "dancing"}),
            json!({"state": "listening", "channels": {}}),
            json!({"state": "listening", "channels": [{"dial_hz": "7101000", "modem": "aether", "up": true}]}),
            json!({"state": "listening", "channels": [{"dial_hz": 7_101_000, "modem": 5, "up": true}]}),
            json!({"state": "listening", "channels": [{"dial_hz": 7_101_000, "modem": "aether", "up": 1}]}),
            json!({"state": "listening", "channels": [{"dial_hz": 7.5, "modem": "aether", "up": true}]}),
            json!({"state": "listening", "problems": "modem-away"}),
            json!({"state": "listening", "problems": ["Modem-Away"]}),
            json!({"state": "listening", "problems": [""]}),
            json!({"state": "listening", "problems": ["x".repeat(33)]}),
            json!({"state": "listening", "problems": (0..17).map(|n| format!("p{n}")).collect::<Vec<_>>()}),
            json!({"state": "listening", "carried": -3600}),
            json!({"state": "listening", "carried": 3600.5}),
            json!({"state": "listening", "carried": true}),
        ];
        for b in bad {
            assert_eq!(kept(&b, &record(), 10_000), None, "{b}");
        }
        let sixteen: Vec<String> = (0..16).map(|n| format!("p{n}")).collect();
        let longest = "x".repeat(32);
        assert!(
            kept(
                &json!({"state": "stopped", "problems": sixteen}),
                &record(),
                0
            )
            .is_some()
        );
        assert!(
            kept(
                &json!({"state": "stopped", "problems": [longest]}),
                &record(),
                0
            )
            .is_some()
        );
    }
}
