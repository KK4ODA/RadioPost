// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reports (the forms design, §4.3): the forms this computer holds, received or sent, that carry
//! a position, for the Reports view's map and list, and written out as CSV (RFC 4180) for a
//! spreadsheet or `GeoJSON` (RFC 7946) for a map program.
//!
//! Nothing is kept apart from the messages: the reports are read from the Inbox and Sent each
//! time, so a message deleted is a report gone.

use radiopost::cbor::Value as Cbor;
use radiopost::forms::{Form, Kind, Place, Template};
use radiopost::message::{Message, key};
use serde_json::{Value, json};

use crate::store::Store;

/// A form that carries one or more positions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// The message's content ID, as text.
    pub id: String,
    /// `inbox` or `sent`; `circle` for a report sent or heard by broadcast (RP-8).
    pub folder: &'static str,
    /// The sender's address.
    pub from: String,
    /// Its recipients, `to` then `cc`.
    pub to: Vec<String>,
    /// When it was written, Unix seconds.
    pub date: u64,
    /// The message's subject.
    pub subject: String,
    /// The template's fingerprint.
    pub template: [u8; 8],
    /// The template's name.
    pub form: String,
    /// Its positions, in the form's order (RP-6 §3.1).
    pub places: Vec<Place>,
    /// Each `choice` field with a value: its number and the index of its choice, for colouring.
    pub choices: Vec<(u64, u64)>,
    /// Each field shown, but a heading: its label and its value as text (RP-2 §8.4).
    pub fields: Vec<(String, String)>,
}

/// The reports among the Inbox and Sent, newest first: the forms whose template this computer
/// holds and whose values fill it, that carry a position. A message that cannot be read is left
/// out, as the folders leave it out.
#[must_use]
pub fn gather(store: &Store, templates: &[Template]) -> Vec<Report> {
    let ids = store
        .inbox
        .iter()
        .map(|a| (a.id, "inbox"))
        .chain(store.sent.iter().map(|id| (*id, "sent")));
    let mut out: Vec<Report> = ids
        .filter_map(|(id, folder)| report(&store.message(&id).ok()?, folder, templates))
        .collect();
    out.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| a.id.cmp(&b.id)));
    out
}

/// A message as a report, when it is a form with a position.
#[must_use]
pub fn report(m: &Message, folder: &'static str, templates: &[Template]) -> Option<Report> {
    let form = Form::of(m)?;
    let t = templates
        .iter()
        .find(|t| t.fingerprint() == form.template)
        .filter(|t| t.problem(&form.values).is_none())?;
    let places = radiopost::forms::places(t, &form);
    if places.is_empty() {
        return None;
    }
    let map = m.map();
    let text = |k| map.get(k).and_then(Cbor::as_text).unwrap_or("").to_owned();
    let list = |k| -> Vec<String> {
        map.get(k)
            .and_then(Cbor::as_array)
            .map_or_else(Vec::new, |a| {
                a.iter()
                    .filter_map(Cbor::as_text)
                    .map(str::to_owned)
                    .collect()
            })
    };
    let choices = t
        .fields
        .iter()
        .filter(|f| f.kind == Kind::Choice)
        .filter_map(|f| Some((f.n, form.value(f.n)?.as_u64()?)))
        .collect();
    let fields = t
        .shown(m, &form)
        .rows
        .into_iter()
        .filter(|r| !r.heading)
        .map(|r| (r.label, r.value))
        .collect();
    Some(Report {
        id: m.content_id().text(),
        folder,
        from: text(key::FROM),
        to: [list(key::TO), list(key::CC)].concat(),
        date: map.get(key::DATE).and_then(Cbor::as_u64).unwrap_or(0),
        subject: text(key::SUBJECT),
        template: form.template,
        form: t.name.clone(),
        places,
        choices,
        fields,
    })
}

/// A report sent to a circle or heard from one (RP-8 §3, §7), as a report: its tag is its ID, its
/// folder `circle`, its subject the circle's name, its date its own time; the sender's and the
/// station's callsigns stand for addresses. A form whose template this computer does not hold, or
/// whose values do not fill it, or that has no position, is no report, as for mail.
#[must_use]
pub fn of_circle(r: &crate::alerts::CircleReport, templates: &[Template]) -> Option<Report> {
    let map = Cbor::int_map(vec![
        (1, Cbor::Uint(1)),
        (2, Cbor::text(&r.from)),
        (3, Cbor::Array(r.to.iter().map(|t| Cbor::text(t)).collect())),
        (5, Cbor::Uint(r.time)),
        (6, Cbor::text(&r.circle)),
        (7, Cbor::text("")),
        (key::FORM, r.form.to_cbor()),
    ]);
    let m = Message::from_map(&map).ok()?;
    let mut report = report(&m, "circle", templates)?;
    report.id = radiopost::cbor::hex(&r.tag);
    Some(report)
}

/// A position's OpenStreetMap page (RP-6 §3.1's link), from degrees as the Reports view has them:
/// made here from the numbers, so nothing but openstreetmap.org can be opened this way.
///
/// # Errors
///
/// The numbers are not a latitude and a longitude.
pub fn osm_link(lat: f64, lon: f64, within: Option<u64>) -> Result<String, String> {
    if !(lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0) {
        return Err("lat and lon: a latitude and a longitude in degrees".into());
    }
    // within ±180°, so well inside an i128
    #[allow(clippy::cast_possible_truncation)]
    let e7 = |d: f64| (d * 1e7).round() as i128;
    let place = Place {
        label: String::new(),
        n: 0,
        lat: e7(lat),
        lon: e7(lon),
        within,
    };
    Ok(place.osm_link())
}

/// Opens `url` in the system's web browser, for the desktop app, whose window opens no other
/// site. Only a link [`osm_link`] made is handed here.
///
/// # Errors
///
/// No program to open it could be started.
pub fn open_in_browser(url: &str) -> Result<(), String> {
    use std::process::Command;
    let tries: Vec<(&str, Vec<&str>)> = if cfg!(windows) {
        // `start` would cut the address at its `&`; the URL handler takes it whole
        vec![("rundll32", vec!["url.dll,FileProtocolHandler", url])]
    } else if cfg!(target_os = "macos") {
        vec![("open", vec![url])]
    } else {
        vec![("xdg-open", vec![url])]
    };
    for (program, args) in tries {
        if Command::new(program).args(&args).spawn().is_ok() {
            return Ok(());
        }
    }
    Err("no web browser could be started: copy the link instead".into())
}

/// A coordinate in degrees, for JSON: ten-millionths made a number.
#[allow(
    clippy::cast_precision_loss,
    reason = "a coordinate in ten-millionths of a degree is under 2^31"
)]
fn degrees(e7: i128) -> f64 {
    e7 as f64 / 1e7
}

impl Report {
    /// The report for the Reports view: its message, form, places (with their text and map
    /// links) and the choices made, by field number.
    #[must_use]
    pub fn json(&self) -> Value {
        let places: Vec<Value> = self
            .places
            .iter()
            .map(|p| {
                json!({"label": p.label, "lat": degrees(p.lat), "lon": degrees(p.lon),
                       "within": p.within,
                       "text": radiopost::forms::position(p.lat, p.lon, p.within),
                       "osm": p.osm_link()})
            })
            .collect();
        let choices: serde_json::Map<String, Value> = self
            .choices
            .iter()
            .map(|(n, i)| (n.to_string(), json!(i)))
            .collect();
        json!({"id": self.id, "folder": self.folder, "from": self.from, "to": self.to,
               "date": self.date, "subject": self.subject,
               "template": radiopost::cbor::hex(&self.template), "form": self.form,
               "places": places, "choices": choices})
    }
}

/// A Unix time as `YYYY-MM-DD HH:MM`, UTC.
fn stamp(seconds: u64) -> String {
    format!(
        "{} {}",
        radiopost::forms::ymd(seconds / 86_400),
        radiopost::forms::hm(seconds % 86_400 / 60).trim_end_matches(" UTC")
    )
}

/// One CSV field (RFC 4180): quoted when it holds a comma, a quote or a line break, its quotes
/// doubled.
fn field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

/// The reports as CSV (RFC 4180): one line for each position, in UTF-8 with a byte order mark,
/// which spreadsheet programs read as UTF-8, and CRLF line ends. The columns are the message's
/// and the position's; when every report is of one form, that form's fields follow, each label a
/// column.
#[must_use]
pub fn csv(reports: &[&Report]) -> String {
    let one_form = reports
        .first()
        .filter(|first| reports.iter().all(|r| r.template == first.template));
    let mut head: Vec<String> = [
        "Date (UTC)",
        "From",
        "To",
        "Subject",
        "Form",
        "Message",
        "Place",
        "Latitude",
        "Longitude",
        "Within (m)",
        "Locator",
    ]
    .iter()
    .map(|h| (*h).to_owned())
    .collect();
    if let Some(first) = one_form {
        head.extend(first.fields.iter().map(|(label, _)| label.clone()));
    }
    let mut lines = vec![head.iter().map(|h| field(h)).collect::<Vec<_>>().join(",")];
    for r in reports {
        for p in &r.places {
            let mut cells = vec![
                stamp(r.date),
                r.from.clone(),
                r.to.join(", "),
                r.subject.clone(),
                r.form.clone(),
                r.id.clone(),
                p.label.clone(),
                radiopost::forms::degrees(p.lat),
                radiopost::forms::degrees(p.lon),
                p.within.map(|w| w.to_string()).unwrap_or_default(),
                radiopost::forms::locator(p.lat, p.lon),
            ];
            if let Some(first) = one_form {
                // by label, in the form's order: a field the report leaves empty is an empty cell
                cells.extend(first.fields.iter().map(|(label, _)| {
                    r.fields
                        .iter()
                        .find(|(l, _)| l == label)
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default()
                }));
            }
            lines.push(cells.iter().map(|c| field(c)).collect::<Vec<_>>().join(","));
        }
    }
    format!("\u{feff}{}\r\n", lines.join("\r\n"))
}

/// The reports as `GeoJSON` (RFC 7946): a feature for each position, a point at its longitude and
/// latitude, its properties the message's, the position's and the form's fields, by label.
#[must_use]
pub fn geojson(reports: &[&Report]) -> Value {
    let features: Vec<Value> = reports
        .iter()
        .flat_map(|r| {
            r.places.iter().map(move |p| {
                let fields: serde_json::Map<String, Value> = r
                    .fields
                    .iter()
                    .map(|(label, value)| (label.clone(), json!(value)))
                    .collect();
                json!({
                    "type": "Feature",
                    "geometry": {"type": "Point",
                                 "coordinates": [degrees(p.lon), degrees(p.lat)]},
                    "properties": {
                        "date": format!("{}Z", stamp(r.date).replace(' ', "T") + ":00"),
                        "from": r.from, "to": r.to, "subject": r.subject, "form": r.form,
                        "message": r.id, "place": p.label, "within_m": p.within,
                        "locator": radiopost::forms::locator(p.lat, p.lon),
                        "fields": fields,
                    },
                })
            })
        })
        .collect();
    json!({"type": "FeatureCollection", "features": features})
}

#[cfg(test)]
mod tests {
    use super::{csv, field, geojson, report};
    use radiopost::forms::Template;

    /// A Storm Report from the vectors' messages (`shelter3-storm`), and an ICS-206 with a
    /// helipad in its hospital table (`shelter3-206`).
    fn message(name: &str) -> radiopost::message::Message {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vectors/messages.json");
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("messages.json"))
                .expect("JSON");
        let hex = v["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .find(|m| m["name"] == name)
            .and_then(|m| m["canonical_hex"].as_str())
            .expect("the message")
            .to_owned();
        let bytes = crate::store::unhex(&hex).expect("hex");
        radiopost::message::Message::from_canonical(&bytes).expect("canonical")
    }

    #[test]
    fn a_map_link_is_made_from_numbers_only() {
        assert_eq!(
            super::osm_link(33.78123, -84.38765, Some(25)).as_deref(),
            Ok(
                "https://www.openstreetmap.org/?mlat=33.78123&mlon=-84.38765#map=17/33.78123/-84.38765"
            )
        );
        assert!(super::osm_link(91.0, 0.0, None).is_err());
        assert!(super::osm_link(f64::NAN, 0.0, None).is_err());
    }

    #[test]
    fn a_form_with_positions_is_a_report_and_one_without_is_not() {
        let templates = Template::published();
        let storm = report(&message("shelter3-storm"), "inbox", &templates).expect("a report");
        assert_eq!(storm.form, "Storm Report");
        assert_eq!(storm.places.len(), 1);
        assert_eq!(storm.places[0].within, Some(25));
        // "The wind speed was" (field 7) chose its first choice, Measured
        assert_eq!(storm.choices, vec![(7, 0)]);
        let medical = report(&message("shelter3-206"), "sent", &templates).expect("a report");
        assert_eq!(
            medical.places[0].label,
            "Hospitals, row 1, Helipad position"
        );
        // ICS-213 holds no position: no report
        assert!(report(&message("shelter3-213"), "inbox", &templates).is_none());
        // a message with no form is none either
        assert!(report(&message("maria-419"), "inbox", &templates).is_none());
    }

    #[test]
    fn csv_quotes_as_rfc_4180_says_and_gives_one_forms_fields() {
        assert_eq!(field("plain"), "plain");
        assert_eq!(field("a, b"), "\"a, b\"");
        assert_eq!(field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(field("two\nlines"), "\"two\nlines\"");
        let templates = Template::published();
        let storm = report(&message("shelter3-storm"), "inbox", &templates).expect("a report");
        let text = csv(&[&storm]);
        assert!(text.starts_with('\u{feff}'), "a byte order mark");
        let lines: Vec<&str> = text.trim_start_matches('\u{feff}').split("\r\n").collect();
        assert_eq!(
            lines.len(),
            3,
            "the head, one place, and the last line's end: {lines:?}"
        );
        assert!(lines[0].starts_with("Date (UTC),From,To,Subject,Form,Message,Place,Latitude,"));
        assert!(
            lines[0].ends_with(",Spotter number or callsign"),
            "{}",
            lines[0]
        );
        assert!(
            lines[1].starts_with(
                "2026-11-09 14:25,shelter3@radiopost.org,reports@weather.example,\
                 \"Hail and wind, Atlanta\",Storm Report,"
            ),
            "{}",
            lines[1]
        );
        assert!(
            lines[1].contains(",Location,33.78123,-84.38765,25,EM73ts,"),
            "{}",
            lines[1]
        );
        assert!(
            lines[1].contains(",\"Hail, Damaging wind\","),
            "{}",
            lines[1]
        );
        // two forms: no form's fields, only the message's and the position's columns
        let medical = report(&message("shelter3-206"), "sent", &templates).expect("a report");
        let both = csv(&[&storm, &medical]);
        assert!(both.contains(",Locator\r\n"), "{both}");
    }

    #[test]
    fn geojson_has_a_point_for_each_position_longitude_first() {
        let templates = Template::published();
        let storm = report(&message("shelter3-storm"), "inbox", &templates).expect("a report");
        let g = geojson(&[&storm]);
        assert_eq!(g["type"], "FeatureCollection");
        let f = &g["features"][0];
        assert_eq!(
            f["geometry"]["coordinates"],
            serde_json::json!([-84.38765, 33.78123])
        );
        assert_eq!(f["properties"]["date"], "2026-11-09T14:25:00Z");
        assert_eq!(f["properties"]["within_m"], 25);
        assert_eq!(f["properties"]["fields"]["Hail size"], "1.75 in");
    }
}
