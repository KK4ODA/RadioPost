// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-2 §8: forms. A form travels as its template's fingerprint and its values by field number;
//! each station holds the template, and makes the form's text from it.
//!
//! * [`Template`]: a template read from its JSON (`forms/*.json`), its CBOR map and fingerprint,
//!   whether values fill it ([`Template::problem`]), and the text a form message shows as.
//! * [`Form`]: key 13 of a message.
//! * [`shown`]: the text a message's form shows as, with the templates a station holds;
//!   [`shown_as`]: the same as rows, label and value, for a program to lay out as the form.
//! * [`layout`]: how a form prints, as the paper form it comes from (`forms/*.print.html`).

use serde_json::Value as Json;

use crate::cbor::Value;
use crate::message::{Message, key};

/// RadioPost's published templates (RP-2 §8.1), as their JSON files: ICS-213 first, FEMA's other
/// ICS forms, then RadioPost's own (`forms/README.md`; the forms design, step 2).
pub const PUBLISHED: &[&str] = &[
    include_str!("../../../forms/ics-213.json"),
    include_str!("../../../forms/ics-202.json"),
    include_str!("../../../forms/ics-204.json"),
    include_str!("../../../forms/ics-205.json"),
    include_str!("../../../forms/ics-205a.json"),
    include_str!("../../../forms/ics-206.json"),
    include_str!("../../../forms/ics-208.json"),
    include_str!("../../../forms/ics-210.json"),
    include_str!("../../../forms/ics-211.json"),
    include_str!("../../../forms/ics-213rr.json"),
    include_str!("../../../forms/ics-214.json"),
    include_str!("../../../forms/check-in.json"),
    include_str!("../../../forms/check-out.json"),
    include_str!("../../../forms/comms-log.json"),
    include_str!("../../../forms/damage-report.json"),
    include_str!("../../../forms/position-report.json"),
    include_str!("../../../forms/situation-report.json"),
    include_str!("../../../forms/storm-report.json"),
];

/// The print layouts the programs hold (`forms/*.print.html`): how a form prints on paper, as the
/// paper form it comes from. Each names the template it lays out by its fingerprint
/// (`data-template`) and has a place for each field it shows (`data-field`, the field's number).
/// A layout is the programs' own, never on the air: a station without one prints the form's rows.
pub const LAYOUTS: &[&str] = &[
    include_str!("../../../forms/ics-213.print.html"),
    include_str!("../../../forms/ics-202.print.html"),
    include_str!("../../../forms/ics-204.print.html"),
    include_str!("../../../forms/ics-205.print.html"),
    include_str!("../../../forms/ics-205a.print.html"),
    include_str!("../../../forms/ics-206.print.html"),
    include_str!("../../../forms/ics-208.print.html"),
    include_str!("../../../forms/ics-210.print.html"),
    include_str!("../../../forms/ics-211.print.html"),
    include_str!("../../../forms/ics-213rr.print.html"),
    include_str!("../../../forms/ics-214.print.html"),
];

/// The print layout for the template with this fingerprint, when the programs hold one.
#[must_use]
pub fn layout(fingerprint: &[u8; 8]) -> Option<&'static str> {
    let named = format!("data-template=\"{}\"", crate::cbor::hex(fingerprint));
    LAYOUTS.iter().copied().find(|l| l.contains(&named))
}

/// A field's type (RP-2 §8.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Text with no line break.
    Line,
    /// Text, lines joined by LF, not ending with one.
    Text,
    /// Days since 1970-01-01, UTC.
    Date,
    /// Minutes after midnight UTC.
    Time,
    /// An integer, with a `scale` a decimal.
    Number,
    /// The index of a choice.
    Choice,
    /// Yes or no.
    Check,
    /// Latitude and longitude in ten-millionths of a degree, and how far off it may be in metres.
    Position,
    /// Minutes since 1970-01-01 00:00 UTC.
    Datetime,
    /// The choices chosen, as bits.
    Multi,
    /// Rows, each a map of a column's number to a value of its type.
    Rows,
    /// No value: a section's name.
    Heading,
}

impl Kind {
    /// The types by name, in the order of their numbers.
    const NAMES: [(&'static str, Kind); 12] = [
        ("line", Kind::Line),
        ("text", Kind::Text),
        ("date", Kind::Date),
        ("time", Kind::Time),
        ("number", Kind::Number),
        ("choice", Kind::Choice),
        ("check", Kind::Check),
        ("position", Kind::Position),
        ("datetime", Kind::Datetime),
        ("multi", Kind::Multi),
        ("rows", Kind::Rows),
        ("heading", Kind::Heading),
    ];

    fn named(name: &str) -> Option<Kind> {
        Kind::NAMES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, k)| *k)
    }

    /// Its name in a template's JSON.
    #[must_use]
    pub fn name(self) -> &'static str {
        Kind::NAMES
            .iter()
            .find(|(_, k)| *k == self)
            .map_or("line", |(n, _)| n)
    }

    fn code(self) -> u64 {
        Kind::NAMES
            .iter()
            .position(|(_, k)| *k == self)
            .map_or(0, |i| i as u64)
    }
}

/// What a bound field takes from its message (RP-2 §8.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bound {
    /// The message's `subject`.
    Subject,
    /// The date of its `date`.
    Date,
    /// The time of its `date`.
    Time,
}

impl Bound {
    fn named(name: &str) -> Option<Bound> {
        match name {
            "subject" => Some(Bound::Subject),
            "date" => Some(Bound::Date),
            "time" => Some(Bound::Time),
            _ => None,
        }
    }

    /// Its name in a template's JSON.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Bound::Subject => "subject",
            Bound::Date => "date",
            Bound::Time => "time",
        }
    }

    fn code(self) -> u64 {
        match self {
            Bound::Subject => 1,
            Bound::Date => 2,
            Bound::Time => 3,
        }
    }
}

/// One field of a template, or one column of a `rows` field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    /// Its number, 1 to 255.
    pub n: u64,
    /// As shown.
    pub label: String,
    /// Its type.
    pub kind: Kind,
    /// The form is not filled in without it (a row, for a column).
    pub required: bool,
    /// A `choice` or `multi` field's choices.
    pub choices: Vec<String>,
    /// What it takes from the message, when it is bound.
    pub bound: Option<Bound>,
    /// The most characters of a `line` or `text` value; the most rows of a `rows` value.
    pub max: Option<u64>,
    /// A `number`'s decimal places, 1 to 9.
    pub scale: Option<u64>,
    /// A `number`'s unit, shown after it.
    pub unit: Option<String>,
    /// A `rows` field's columns.
    pub columns: Vec<Field>,
}

impl Field {
    /// A field (or, with `column`, a column) from its JSON, keeping RP-2 §8.1's rules.
    fn from_json(f: &Json, column: bool) -> Result<Field, String> {
        let what = if column { "column" } else { "field" };
        let n = f["n"]
            .as_u64()
            .filter(|n| (1..=255).contains(n))
            .ok_or_else(|| format!("a {what}'s number is 1 to 255"))?;
        let kind = f["type"]
            .as_str()
            .and_then(Kind::named)
            .ok_or_else(|| format!("{what} {n}: no such type"))?;
        let has = |key: &str| f.get(key).is_some_and(|v| !v.is_null());
        if column && matches!(kind, Kind::Text | Kind::Rows | Kind::Heading) {
            return Err(format!("column {n}: a column of type {}", kind.name()));
        }
        if column && has("bound") {
            return Err(format!("column {n}: a column is never bound"));
        }
        let choices: Vec<String> = f["choices"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c.as_str().map(str::to_owned))
            .collect();
        if kind == Kind::Choice && choices.is_empty() {
            return Err(format!("{what} {n}: a choice field has choices"));
        }
        if kind == Kind::Multi && !(1..=64).contains(&choices.len()) {
            return Err(format!("{what} {n}: a multi field has 1 to 64 choices"));
        }
        if (has("scale") || has("unit")) && kind != Kind::Number {
            return Err(format!("{what} {n}: a scale or a unit is on a number only"));
        }
        let scale = match f.get("scale").filter(|v| !v.is_null()) {
            None => None,
            Some(s) => Some(
                s.as_u64()
                    .filter(|s| (1..=9).contains(s))
                    .ok_or_else(|| format!("{what} {n}: a scale is 1 to 9"))?,
            ),
        };
        if has("columns") && kind != Kind::Rows {
            return Err(format!("{what} {n}: columns are on a rows field only"));
        }
        let mut columns: Vec<Field> = Vec::new();
        for c in f["columns"].as_array().into_iter().flatten() {
            let c = Field::from_json(c, true)?;
            if columns.iter().any(|d| d.n == c.n) {
                return Err(format!("{what} {n}: column {} appears twice", c.n));
            }
            columns.push(c);
        }
        if kind == Kind::Rows && columns.is_empty() {
            return Err(format!("{what} {n}: a rows field has columns"));
        }
        let required = f["required"].as_bool().unwrap_or(false);
        if kind == Kind::Heading && (required || has("choices") || has("bound") || has("max")) {
            return Err(format!(
                "{what} {n}: a heading has no required, choices, bound or max"
            ));
        }
        let bound = match f["bound"].as_str() {
            None => None,
            Some(b) => Some(Bound::named(b).ok_or_else(|| format!("{what} {n}: bound to {b}"))?),
        };
        Ok(Field {
            n,
            label: f["label"]
                .as_str()
                .ok_or_else(|| format!("{what} {n} has a label"))?
                .to_owned(),
            kind,
            required,
            choices,
            bound,
            max: f["max"].as_u64(),
            scale,
            unit: f["unit"].as_str().map(str::to_owned),
            columns,
        })
    }

    /// Its CBOR map (RP-2 §8.1).
    fn to_cbor(&self) -> Value {
        let mut m = vec![
            (1, Value::Uint(self.n)),
            (2, Value::text(&self.label)),
            (3, Value::Uint(self.kind.code())),
        ];
        if self.required {
            m.push((4, Value::Bool(true)));
        }
        if !self.choices.is_empty() {
            m.push((
                5,
                Value::Array(self.choices.iter().map(|c| Value::text(c)).collect()),
            ));
        }
        if let Some(b) = self.bound {
            m.push((6, Value::Uint(b.code())));
        }
        if let Some(max) = self.max {
            m.push((7, Value::Uint(max)));
        }
        if let Some(scale) = self.scale {
            m.push((8, Value::Uint(scale)));
        }
        if let Some(unit) = &self.unit {
            m.push((9, Value::text(unit)));
        }
        if !self.columns.is_empty() {
            m.push((
                10,
                Value::Array(self.columns.iter().map(Field::to_cbor).collect()),
            ));
        }
        Value::int_map(m)
    }

    /// Why `v` is not a value of this field (or column), or `None` when it is (RP-2 §8.2).
    fn problem(&self, v: &Value) -> Option<&'static str> {
        let wrong = Some("a value of the wrong type");
        let empty = Some("an empty value: an empty field is left out");
        match self.kind {
            Kind::Line | Kind::Text => {
                let Some(text) = v.as_text() else {
                    return wrong;
                };
                if self.kind == Kind::Line && text.contains('\n') {
                    return Some("a line value with a line break");
                }
                if self
                    .max
                    .is_some_and(|max| text.chars().count() as u64 > max)
                {
                    return Some("a value longer than the field's max");
                }
                if text.is_empty() {
                    return empty;
                }
                if self.kind == Kind::Text && text.ends_with('\n') {
                    return Some("a text value ending with a line break");
                }
            }
            Kind::Date | Kind::Time | Kind::Choice | Kind::Datetime | Kind::Multi => {
                let Value::Uint(u) = v else {
                    return wrong;
                };
                if self.kind == Kind::Time && *u > 1439 {
                    return Some("a time past 23:59");
                }
                if self.kind == Kind::Choice && *u >= self.choices.len() as u64 {
                    return Some("a choice the field does not have");
                }
                if self.kind == Kind::Multi {
                    if *u == 0 {
                        return empty;
                    }
                    if self.choices.len() < 64 && *u >> self.choices.len() != 0 {
                        return Some("a choice the field does not have");
                    }
                }
            }
            Kind::Number => {
                if !matches!(v, Value::Uint(_) | Value::Nint(_)) {
                    return wrong;
                }
            }
            Kind::Check => {
                if !matches!(v, Value::Bool(_)) {
                    return wrong;
                }
            }
            Kind::Position => {
                let Some(parts) = v.as_array().filter(|a| a.len() == 2 || a.len() == 3) else {
                    return wrong;
                };
                let Some(ints) = parts.iter().map(integer).collect::<Option<Vec<i128>>>() else {
                    return wrong;
                };
                if ints[0].abs() > LAT_MAX
                    || ints[1].abs() > LON_MAX
                    || ints.get(2).is_some_and(|within| *within < 1)
                {
                    return Some("a position out of range");
                }
            }
            Kind::Rows => return self.rows_problem(v),
            Kind::Heading => return Some("a heading among the values"),
        }
        None
    }

    /// Why `v` is not a value of this `rows` field, or `None` when it is (RP-2 §8.2): each row a
    /// map of its columns' numbers to values that fill them, every required column filled.
    fn rows_problem(&self, v: &Value) -> Option<&'static str> {
        let Some(rows) = v.as_array() else {
            return Some("a value of the wrong type");
        };
        if rows.is_empty() {
            return Some("an empty value: an empty field is left out");
        }
        if self.max.is_some_and(|max| rows.len() as u64 > max) {
            return Some("more rows than the field's max");
        }
        for row in rows {
            let Some(cells) = row.as_map() else {
                return Some("a value of the wrong type");
            };
            if cells.is_empty() {
                return Some("an empty row");
            }
            for (k, cell) in cells {
                let Some(column) = k
                    .as_u64()
                    .and_then(|n| self.columns.iter().find(|c| c.n == n))
                else {
                    return Some("a column the field does not have");
                };
                if let Some(why) = column.problem(cell) {
                    return Some(why);
                }
            }
            let has = |n: u64| cells.iter().any(|(k, _)| k.as_u64() == Some(n));
            if self.columns.iter().any(|c| c.required && !has(c.n)) {
                return Some("a required column missing");
            }
        }
        None
    }

    /// A value of this field (or column) as text (RP-2 §8.4); a `rows` value's lines joined by LF.
    fn shown(&self, v: &Value) -> String {
        match (self.kind, v) {
            (Kind::Date, Value::Uint(d)) => ymd(*d),
            (Kind::Time, Value::Uint(m)) => hm(*m),
            (Kind::Datetime, Value::Uint(m)) => format!("{} {}", ymd(m / 1440), hm(m % 1440)),
            (Kind::Choice, Value::Uint(i)) => usize::try_from(*i)
                .ok()
                .and_then(|i| self.choices.get(i))
                .cloned()
                .unwrap_or_default(),
            (Kind::Multi, Value::Uint(bits)) => self
                .choices
                .iter()
                .enumerate()
                .filter(|(i, _)| bits >> i & 1 == 1)
                .map(|(_, c)| c.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            (Kind::Check, Value::Bool(b)) => yes_no(*b).to_owned(),
            (Kind::Number, _) if integer(v).is_some() => {
                let n = integer(v).unwrap_or_default();
                let digits = match self.scale {
                    Some(scale) => decimal(n, u32::try_from(scale).unwrap_or(0)),
                    None => n.to_string(),
                };
                match &self.unit {
                    Some(unit) => format!("{digits} {unit}"),
                    None => digits,
                }
            }
            (Kind::Position, Value::Array(parts)) => {
                let ints: Vec<i128> = parts.iter().filter_map(integer).collect();
                match ints[..] {
                    [lat, lon] => position(lat, lon, None),
                    [lat, lon, within] => position(lat, lon, u64::try_from(within).ok()),
                    _ => v.diagnostic(),
                }
            }
            (Kind::Rows, Value::Array(rows)) => {
                let mut lines = vec![
                    self.columns
                        .iter()
                        .map(|c| c.label.as_str())
                        .collect::<Vec<_>>()
                        .join(" | "),
                ];
                for cells in table_cells(self, rows) {
                    lines.push(cells.join(" | ").trim_end_matches(' ').to_owned());
                }
                lines.join("\n")
            }
            (_, Value::Text(t)) => t.clone(),
            (_, Value::Uint(u)) => u.to_string(),
            (_, Value::Nint(m)) => format!("-{}", u128::from(*m) + 1),
            (_, other) => other.diagnostic(),
        }
    }
}

/// A position's ranges (RP-2 §8.2), in ten-millionths of a degree.
const LAT_MAX: i128 = 900_000_000;
const LON_MAX: i128 = 1_800_000_000;

/// An integer value of any size, or `None` for anything else.
fn integer(v: &Value) -> Option<i128> {
    match v {
        Value::Uint(u) => Some(i128::from(*u)),
        Value::Nint(n) => Some(-1 - i128::from(*n)),
        _ => None,
    }
}

/// An integer read as having `places` decimal places, written with exactly that many (RP-2 §8.4).
fn decimal(value: i128, places: u32) -> String {
    let sign = if value < 0 { "-" } else { "" };
    let width = usize::try_from(places).unwrap_or(0) + 1;
    let digits = format!("{:0width$}", value.unsigned_abs());
    let (whole, part) = digits.split_at(digits.len() - (width - 1));
    format!("{sign}{whole}.{part}")
}

/// A coordinate in decimal degrees (RP-2 §8.4): seven places, less trailing zeros but one.
#[must_use]
pub fn degrees(e7: i128) -> String {
    let full = decimal(e7, 7);
    let trimmed = full.trim_end_matches('0');
    if trimmed.ends_with('.') {
        format!("{trimmed}0")
    } else {
        trimmed.to_owned()
    }
}

/// A position's 6-character Maidenhead locator, from its integers (RP-2 §8.4).
#[must_use]
pub fn locator(lat: i128, lon: i128) -> String {
    const UPPER: &[u8; 18] = b"ABCDEFGHIJKLMNOPQR";
    const LOWER: &[u8; 24] = b"abcdefghijklmnopqrstuvwx";
    let x = (lon + LON_MAX).clamp(0, 3_599_999_999);
    let y = (lat + LAT_MAX).clamp(0, 1_799_999_999);
    let letter = |set: &[u8], i: i128| char::from(set[usize::try_from(i).unwrap_or(0)]);
    let digit = |i: i128| char::from(b'0' + u8::try_from(i).unwrap_or(0));
    [
        letter(UPPER, x / 200_000_000),
        letter(UPPER, y / 100_000_000),
        digit(x % 200_000_000 / 20_000_000),
        digit(y % 100_000_000 / 10_000_000),
        letter(LOWER, x % 20_000_000 * 24 / 20_000_000),
        letter(LOWER, y % 10_000_000 * 24 / 10_000_000),
    ]
    .iter()
    .collect()
}

/// How far a position may be off (RP-2 §8.4): metres under 1000, else kilometres to a tenth.
#[must_use]
pub fn distance(metres: u64) -> String {
    if metres < 1000 {
        return format!("±{metres} m");
    }
    let tenths = (u128::from(metres) + 50) / 100;
    if tenths % 10 == 0 {
        format!("±{} km", tenths / 10)
    } else {
        format!("±{}.{} km", tenths / 10, tenths % 10)
    }
}

/// A position as text (RP-2 §8.4): `33.78123, -84.38765 (EM73ts)`, and `±…` with a distance.
#[must_use]
pub fn position(lat: i128, lon: i128, within: Option<u64>) -> String {
    let at = format!("{}, {} ({})", degrees(lat), degrees(lon), locator(lat, lon));
    match within {
        Some(m) => format!("{at} {}", distance(m)),
        None => at,
    }
}

/// A position's value (RP-2 §8.1): ten-millionths of a degree, and how far off it may be.
#[must_use]
pub fn position_value(lat_e7: i64, lon_e7: i64, within_m: Option<u64>) -> Value {
    let mut parts = vec![Value::int(lat_e7), Value::int(lon_e7)];
    parts.extend(within_m.map(Value::Uint));
    Value::Array(parts)
}

/// A `rows` value's cells as text, one list a row in the columns' order, empty where a row has no
/// cell for a column.
fn table_cells(f: &Field, rows: &[Value]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| {
            f.columns
                .iter()
                .map(|c| {
                    row.as_map()
                        .and_then(|cells| cells.iter().find(|(k, _)| k.as_u64() == Some(c.n)))
                        .map(|(_, v)| c.shown(v))
                        .unwrap_or_default()
                })
                .collect()
        })
        .collect()
}

/// A template (RP-2 §8.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    /// The form's name.
    pub name: String,
    /// Where it comes from.
    pub note: Option<String>,
    /// Its fields, in the order shown.
    pub fields: Vec<Field>,
}

impl Template {
    /// A template from its JSON (`forms/*.json`).
    ///
    /// # Errors
    ///
    /// The JSON is not a template RP-2 §8.1 allows: what is wrong.
    pub fn from_json(text: &str) -> Result<Template, String> {
        let j: Json = serde_json::from_str(text).map_err(|e| e.to_string())?;
        Template::from_value(&j)
    }

    /// A template from its JSON, already parsed.
    ///
    /// # Errors
    ///
    /// The JSON is not a template RP-2 §8.1 allows: what is wrong.
    pub fn from_value(j: &Json) -> Result<Template, String> {
        let name = j["name"]
            .as_str()
            .ok_or("a template has a name")?
            .to_owned();
        let note = j["note"].as_str().map(str::to_owned);
        let mut fields: Vec<Field> = Vec::new();
        for f in j["fields"].as_array().ok_or("a template has fields")? {
            let f = Field::from_json(f, false)?;
            if fields.iter().any(|g| g.n == f.n) {
                return Err(format!("field {} appears twice", f.n));
            }
            fields.push(f);
        }
        Ok(Template { name, note, fields })
    }

    /// RadioPost's published templates.
    ///
    /// # Panics
    ///
    /// A published template does not read, which its tests catch.
    #[must_use]
    pub fn published() -> Vec<Template> {
        PUBLISHED
            .iter()
            .map(|t| Template::from_json(t).expect("a published template reads"))
            .collect()
    }

    /// Its CBOR map (RP-2 §8.1).
    #[must_use]
    pub fn to_cbor(&self) -> Value {
        let fields = self.fields.iter().map(Field::to_cbor).collect();
        let mut m = vec![(1, Value::text(&self.name)), (2, Value::Array(fields))];
        if let Some(note) = &self.note {
            m.push((3, Value::text(note)));
        }
        Value::int_map(m)
    }

    /// Its fingerprint: the first 8 bytes of SHA-256 of its deterministic encoding.
    ///
    /// # Panics
    ///
    /// Never: a template's map has no repeated key.
    #[must_use]
    pub fn fingerprint(&self) -> [u8; 8] {
        let encoded = self.to_cbor().encode().expect("no key twice");
        let digest = crate::crypto::sha256(&encoded);
        let mut out = [0u8; 8];
        out.copy_from_slice(&digest[..8]);
        out
    }

    /// The field with this number.
    #[must_use]
    pub fn field(&self, n: u64) -> Option<&Field> {
        self.fields.iter().find(|f| f.n == n)
    }

    /// Why these values do not fill the template (RP-2 §8.2), or `None` when they do.
    #[must_use]
    pub fn problem(&self, values: &[(u64, Value)]) -> Option<&'static str> {
        for (n, v) in values {
            let Some(f) = self.field(*n) else {
                return Some("a field the template does not have");
            };
            if f.bound.is_some() {
                return Some("a bound field among the values");
            }
            if let Some(why) = f.problem(v) {
                return Some(why);
            }
        }
        self.fields
            .iter()
            .any(|f| f.required && f.bound.is_none() && !values.iter().any(|(n, _)| *n == f.n))
            .then_some("a required field missing")
    }

    /// The text a form message shows as with this template (RP-2 §8.4).
    #[must_use]
    pub fn text(&self, message: &Message, form: &Form) -> String {
        self.shown(message, form).text(message)
    }

    /// The form as it shows with this template (RP-2 §8.4): its name, and a row for each field in
    /// the template's order that has a value, a bound field the message's, with a heading's row
    /// before the first of its section's that shows.
    #[must_use]
    pub fn shown(&self, message: &Message, form: &Form) -> Shown {
        let map = message.map();
        let subject = map.get(key::SUBJECT).and_then(Value::as_text).unwrap_or("");
        let date = map.get(key::DATE).and_then(Value::as_u64).unwrap_or(0);
        let mut rows = Vec::new();
        let mut heading: Option<&Field> = None;
        for f in &self.fields {
            if f.kind == Kind::Heading {
                heading = Some(f);
                continue;
            }
            let raw = form.value(f.n);
            let value = match f.bound {
                Some(Bound::Subject) => (!subject.is_empty()).then(|| subject.to_owned()),
                Some(Bound::Date) => Some(ymd(date / 86_400)),
                Some(Bound::Time) => Some(hm((date % 86_400) / 60)),
                None => raw.map(|v| f.shown(v)),
            };
            let Some(value) = value else {
                continue;
            };
            if let Some(h) = heading.take() {
                rows.push(Row {
                    n: h.n,
                    label: h.label.clone(),
                    value: String::new(),
                    lines: false,
                    heading: true,
                    table: None,
                });
            }
            let table = match (f.kind, raw) {
                (Kind::Rows, Some(Value::Array(cells))) => Some(Table {
                    columns: f.columns.iter().map(|c| c.label.clone()).collect(),
                    rows: table_cells(f, cells),
                }),
                _ => None,
            };
            rows.push(Row {
                n: f.n,
                label: f.label.clone(),
                value,
                lines: matches!(f.kind, Kind::Text | Kind::Rows),
                heading: false,
                table,
            });
        }
        Shown {
            title: self.name.clone(),
            filled: true,
            rows,
        }
    }
}

/// A form as a station shows it (RP-2 §8.4): the lines of its text, which a program may also lay
/// out as a form, each label beside its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    /// The template's name; `Form <the fingerprint in hex>` without the template.
    pub title: String,
    /// The template is held and the values fill it, so each label is the template's own.
    pub filled: bool,
    /// One for each field with a value: in the template's order, or by number without it; and
    /// one for each heading shown.
    pub rows: Vec<Row>,
}

/// One field of a form as it shows (RP-2 §8.4), or a heading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The field's number (RP-2 §8.1), by which a print layout places it.
    pub n: u64,
    /// The field's label, or `Field <n>` without the template.
    pub label: String,
    /// Its value as text: a date `YYYY-MM-DD`, a time `HH:MM UTC`, a choice its text, a check
    /// `Yes` or `No`, a position `33.78123, -84.38765 (EM73ts)`, a table's lines; empty for a
    /// heading.
    pub value: String,
    /// The label stands alone and the value follows on the lines after it: a `text` or `rows`
    /// field, or a value with a line break when the template is not held.
    pub lines: bool,
    /// A heading: its label alone, after an empty line unless it opens the form.
    pub heading: bool,
    /// A `rows` field's table, for a program to lay out as one.
    pub table: Option<Table>,
}

/// A `rows` value as a table (RP-2 §8.4): the columns' labels, and each row's cells as text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    /// The columns' labels, in order.
    pub columns: Vec<String>,
    /// Each row's cells in the columns' order, empty where it has none.
    pub rows: Vec<Vec<String>>,
}

impl Shown {
    /// Its text (RP-2 §8.4), the message's `body` after it when there is one.
    #[must_use]
    pub fn text(&self, message: &Message) -> String {
        finish(&self.lines(), message)
    }

    /// Its lines (RP-2 §8.4), before the message's `body`.
    fn lines(&self) -> Vec<String> {
        let mut lines = vec![self.title.clone(), String::new()];
        for r in &self.rows {
            if r.heading {
                if lines.len() > 2 {
                    lines.push(String::new());
                }
                lines.push(r.label.clone());
            } else if r.lines {
                lines.push(format!("{}:", r.label));
                lines.extend(r.value.split('\n').map(str::to_owned));
            } else {
                lines.push(format!("{}: {}", r.label, r.value));
            }
        }
        lines
    }
}

/// A message's form: key 13 (RP-2 §8.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Form {
    /// The template's fingerprint.
    pub template: [u8; 8],
    /// The values, by field number, in the order of their numbers.
    pub values: Vec<(u64, Value)>,
}

impl Form {
    /// The form a message carries, if it carries one of RP-2 §8.2's shape.
    #[must_use]
    pub fn of(message: &Message) -> Option<Form> {
        let form = message.map().get(key::FORM)?;
        let template: [u8; 8] = form.get(1)?.as_bytes()?.try_into().ok()?;
        let mut values = Vec::new();
        for (k, v) in form.get(2)?.as_map()? {
            values.push((k.as_u64()?, v.clone()));
        }
        values.sort_by_key(|(n, _)| *n);
        Some(Form { template, values })
    }

    /// Its CBOR map, key 13's value.
    #[must_use]
    pub fn to_cbor(&self) -> Value {
        Value::int_map(vec![
            (1, Value::bytes(&self.template)),
            (
                2,
                Value::Map(
                    self.values
                        .iter()
                        .map(|(n, v)| (Value::Uint(*n), v.clone()))
                        .collect(),
                ),
            ),
        ])
    }

    /// A field's value.
    #[must_use]
    pub fn value(&self, n: u64) -> Option<&Value> {
        self.values.iter().find(|(m, _)| *m == n).map(|(_, v)| v)
    }

    /// The text it shows as without its template (RP-2 §8.4).
    #[must_use]
    pub fn text_without(&self, message: &Message) -> String {
        self.shown_without().text(message)
    }

    /// The form as it shows without its template (RP-2 §8.4): its values by number.
    #[must_use]
    pub fn shown_without(&self) -> Shown {
        let rows = self
            .values
            .iter()
            .map(|(n, v)| {
                let value = match v {
                    Value::Text(t) => t.clone(),
                    Value::Bool(b) => yes_no(*b).to_owned(),
                    Value::Uint(u) => u.to_string(),
                    Value::Nint(m) => format!("-{}", u128::from(*m) + 1),
                    other => other.diagnostic(),
                };
                Row {
                    n: *n,
                    label: format!("Field {n}"),
                    lines: value.contains('\n'),
                    value,
                    heading: false,
                    table: None,
                }
            })
            .collect();
        Shown {
            title: format!("Form {}", crate::cbor::hex(&self.template)),
            filled: false,
            rows,
        }
    }
}

/// The text a message's form shows as (RP-2 §8.4): with its template when one of `templates` is
/// it and the values fill it, else by number. `None` for a message with no form.
#[must_use]
pub fn shown(message: &Message, templates: &[Template]) -> Option<String> {
    shown_as(message, templates).map(|s| s.text(message))
}

/// A message's form as it shows (RP-2 §8.4), its rows for a program to lay out as a form: with its
/// template when one of `templates` is it and the values fill it, else by number. `None` for a
/// message with no form.
#[must_use]
pub fn shown_as(message: &Message, templates: &[Template]) -> Option<Shown> {
    let form = Form::of(message)?;
    let template = templates
        .iter()
        .find(|t| t.fingerprint() == form.template)
        .filter(|t| t.problem(&form.values).is_none());
    Some(match template {
        Some(t) => t.shown(message, &form),
        None => form.shown_without(),
    })
}

/// A position in a form, as RP-6 §3.1 links it to a map: its label (a field's, or a cell's: the
/// field's label, its row's number from 1 and the column's label), and its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// What it is in the form.
    pub label: String,
    /// The field's number (a `rows` field's, for a cell).
    pub n: u64,
    /// Latitude, ten-millionths of a degree.
    pub lat: i128,
    /// Longitude, ten-millionths of a degree.
    pub lon: i128,
    /// How far off it may be, in metres, when the sender said.
    pub within: Option<u64>,
}

impl Place {
    /// How close a map opens on it (RP-6 §3.1): 17, or by how far off it may be.
    #[must_use]
    pub fn zoom(&self) -> u8 {
        match self.within {
            Some(m) if m >= 100_000 => 6,
            Some(m) if m >= 10_000 => 9,
            Some(m) if m >= 1000 => 12,
            Some(m) if m >= 100 => 15,
            _ => 17,
        }
    }

    /// Its OpenStreetMap link (RP-6 §3.1): a marker on it, at [`Place::zoom`].
    #[must_use]
    pub fn osm_link(&self) -> String {
        let (lat, lon) = (degrees(self.lat), degrees(self.lon));
        format!(
            "https://www.openstreetmap.org/?mlat={lat}&mlon={lon}#map={}/{lat}/{lon}",
            self.zoom()
        )
    }

    /// Its `geo:` URI (RFC 5870), with `;u=` how far off it may be.
    #[must_use]
    pub fn geo_uri(&self) -> String {
        let uri = format!("geo:{},{}", degrees(self.lat), degrees(self.lon));
        match self.within {
            Some(m) => format!("{uri};u={m}"),
            None => uri,
        }
    }
}

/// The positions a form holds (RP-6 §3.1), in its order: a `position` field's value, and a `rows`
/// field's `position` cells, row by row, its columns in order.
#[must_use]
pub fn places(template: &Template, form: &Form) -> Vec<Place> {
    let place = |label: String, n: u64, v: &Value| {
        let parts: Vec<i128> = v.as_array()?.iter().filter_map(integer).collect();
        let (&lat, &lon) = (parts.first()?, parts.get(1)?);
        Some(Place {
            label,
            n,
            lat,
            lon,
            within: parts.get(2).and_then(|w| u64::try_from(*w).ok()),
        })
    };
    let mut out = Vec::new();
    for f in &template.fields {
        let Some(v) = form.value(f.n) else {
            continue;
        };
        match f.kind {
            Kind::Position => out.extend(place(f.label.clone(), f.n, v)),
            Kind::Rows => {
                for (i, row) in v.as_array().unwrap_or(&[]).iter().enumerate() {
                    for c in f.columns.iter().filter(|c| c.kind == Kind::Position) {
                        let cell = row
                            .as_map()
                            .and_then(|cells| cells.iter().find(|(k, _)| k.as_u64() == Some(c.n)));
                        if let Some((_, v)) = cell {
                            let label = format!("{}, row {}, {}", f.label, i + 1, c.label);
                            out.extend(place(label, f.n, v));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The text a post office writes in the email it makes of a message with a form (RP-6 §3.1): the
/// form's text with its template, a map link for each position it holds, then the `body`; without
/// the template, or with values that do not fill it, the values by number. `None` for a message
/// with no form.
#[must_use]
pub fn email_text(message: &Message, templates: &[Template]) -> Option<String> {
    let form = Form::of(message)?;
    let Some(t) = templates
        .iter()
        .find(|t| t.fingerprint() == form.template)
        .filter(|t| t.problem(&form.values).is_none())
    else {
        return Some(form.text_without(message));
    };
    let mut lines = t.shown(message, &form).lines();
    let found = places(t, &form);
    if !found.is_empty() {
        lines.push(String::new());
        lines.push("Maps:".to_owned());
        for p in found {
            lines.push(format!("{}: {} {}", p.label, p.osm_link(), p.geo_uri()));
        }
    }
    Some(finish(&lines, message))
}

/// The lines joined, ending with a line break, then a non-empty body after an empty line.
fn finish(lines: &[String], message: &Message) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    if let Some(body) = message
        .map()
        .get(key::BODY)
        .and_then(Value::as_text)
        .filter(|b| !b.is_empty())
    {
        out.push('\n');
        out.push_str(body);
    }
    out
}

fn yes_no(b: bool) -> &'static str {
    if b { "Yes" } else { "No" }
}

/// Days since 1970-01-01 as `YYYY-MM-DD` (the proleptic Gregorian calendar).
#[must_use]
pub fn ymd(days: u64) -> String {
    // Howard Hinnant's civil_from_days, for days on or after the epoch
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + u64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Minutes after midnight as `HH:MM UTC`.
#[must_use]
pub fn hm(minutes: u64) -> String {
    format!("{:02}:{:02} UTC", minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_times_read_as_the_calendar_does() {
        assert_eq!(ymd(0), "1970-01-01");
        assert_eq!(ymd(59), "1970-03-01");
        assert_eq!(ymd(11_016), "2000-02-29");
        assert_eq!(ymd(20_766), "2026-11-09");
        assert_eq!(hm(0), "00:00 UTC");
        assert_eq!(hm(1439), "23:59 UTC");
    }

    #[test]
    fn the_published_templates_read() {
        let t = Template::published();
        assert_eq!(t[0].name, "ICS-213 General Message");
        assert_eq!(t[0].fields.len(), 11);
    }

    /// The numbers in a layout's `attribute="…"`, in order.
    fn numbers(layout: &str, attribute: &str) -> Vec<String> {
        layout
            .split(&format!("{attribute}=\""))
            .skip(1)
            .filter_map(|rest| rest.split_once('"').map(|(n, _)| n.to_owned()))
            .collect()
    }

    /// Each layout lays out a published template, found by its fingerprint, with one place for
    /// every field of it, bound or not, but a heading: nothing a form holds is left off the paper,
    /// and no place waits for a field the template does not have.
    #[test]
    fn each_layout_has_a_place_for_every_field_of_its_template() {
        let published = Template::published();
        for l in LAYOUTS {
            let named = numbers(l, "data-template");
            assert_eq!(named.len(), 1, "a layout names one template");
            let t = published
                .iter()
                .find(|t| crate::cbor::hex(&t.fingerprint()) == named[0])
                .unwrap_or_else(|| panic!("no published template is {}", named[0]));
            assert_eq!(layout(&t.fingerprint()), Some(*l));
            let mut places = numbers(l, "data-field");
            places.sort();
            let mut fields: Vec<String> = t
                .fields
                .iter()
                .filter(|f| f.kind != Kind::Heading)
                .map(|f| f.n.to_string())
                .collect();
            fields.sort();
            assert_eq!(places, fields, "{}", t.name);
        }
        assert_eq!(layout(&[0; 8]), None);
    }
}
