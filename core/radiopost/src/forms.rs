// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-2 §8: forms. A form travels as its template's fingerprint and its values by field number;
//! each station holds the template, and makes the form's text from it.
//!
//! * [`Template`]: a template read from its JSON (`forms/*.json`), its CBOR map and fingerprint,
//!   whether values fill it ([`Template::problem`]), and the text a form message shows as.
//! * [`Form`]: key 13 of a message.
//! * [`shown`]: the text a message's form shows as, with the templates a station holds.

use serde_json::Value as Json;

use crate::cbor::Value;
use crate::message::{Message, key};

/// RadioPost's published templates (RP-2 §8.1), as their JSON files.
pub const PUBLISHED: &[&str] = &[include_str!("../../../forms/ics-213.json")];

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
    /// An integer.
    Number,
    /// The index of a choice.
    Choice,
    /// Yes or no.
    Check,
}

impl Kind {
    const NAMES: [(&'static str, Kind); 7] = [
        ("line", Kind::Line),
        ("text", Kind::Text),
        ("date", Kind::Date),
        ("time", Kind::Time),
        ("number", Kind::Number),
        ("choice", Kind::Choice),
        ("check", Kind::Check),
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

/// One field of a template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    /// Its number, 1 to 255.
    pub n: u64,
    /// As shown.
    pub label: String,
    /// Its type.
    pub kind: Kind,
    /// The form is not filled in without it.
    pub required: bool,
    /// A `choice` field's choices.
    pub choices: Vec<String>,
    /// What it takes from the message, when it is bound.
    pub bound: Option<Bound>,
    /// The most characters of a `line` or `text` value.
    pub max: Option<u64>,
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
        let name = j["name"]
            .as_str()
            .ok_or("a template has a name")?
            .to_owned();
        let note = j["note"].as_str().map(str::to_owned);
        let mut fields = Vec::new();
        for f in j["fields"].as_array().ok_or("a template has fields")? {
            let n = f["n"]
                .as_u64()
                .filter(|n| (1..=255).contains(n))
                .ok_or("a field's number is 1 to 255")?;
            if fields.iter().any(|g: &Field| g.n == n) {
                return Err(format!("field {n} appears twice"));
            }
            let kind = f["type"]
                .as_str()
                .and_then(Kind::named)
                .ok_or_else(|| format!("field {n}: no such type"))?;
            let choices: Vec<String> = f["choices"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c.as_str().map(str::to_owned))
                .collect();
            if kind == Kind::Choice && choices.is_empty() {
                return Err(format!("field {n}: a choice field has choices"));
            }
            let bound = match f["bound"].as_str() {
                None => None,
                Some(b) => Some(Bound::named(b).ok_or_else(|| format!("field {n}: bound to {b}"))?),
            };
            fields.push(Field {
                n,
                label: f["label"]
                    .as_str()
                    .ok_or_else(|| format!("field {n} has a label"))?
                    .to_owned(),
                kind,
                required: f["required"].as_bool().unwrap_or(false),
                choices,
                bound,
                max: f["max"].as_u64(),
            });
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
        let fields = self
            .fields
            .iter()
            .map(|f| {
                let mut m = vec![
                    (1, Value::Uint(f.n)),
                    (2, Value::text(&f.label)),
                    (3, Value::Uint(f.kind.code())),
                ];
                if f.required {
                    m.push((4, Value::Bool(true)));
                }
                if !f.choices.is_empty() {
                    m.push((
                        5,
                        Value::Array(f.choices.iter().map(|c| Value::text(c)).collect()),
                    ));
                }
                if let Some(b) = f.bound {
                    m.push((6, Value::Uint(b.code())));
                }
                if let Some(max) = f.max {
                    m.push((7, Value::Uint(max)));
                }
                Value::int_map(m)
            })
            .collect();
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

    fn field(&self, n: u64) -> Option<&Field> {
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
            let wrong = Some("a value of the wrong type");
            match f.kind {
                Kind::Line | Kind::Text => {
                    let Some(text) = v.as_text() else {
                        return wrong;
                    };
                    if f.kind == Kind::Line && text.contains('\n') {
                        return Some("a line value with a line break");
                    }
                    if f.max.is_some_and(|max| text.chars().count() as u64 > max) {
                        return Some("a value longer than the field's max");
                    }
                    if text.is_empty() {
                        return Some("an empty value: an empty field is left out");
                    }
                    if f.kind == Kind::Text && text.ends_with('\n') {
                        return Some("a text value ending with a line break");
                    }
                }
                Kind::Date | Kind::Time | Kind::Choice => {
                    let Value::Uint(u) = v else {
                        return wrong;
                    };
                    if f.kind == Kind::Time && *u > 1439 {
                        return Some("a time past 23:59");
                    }
                    if f.kind == Kind::Choice && *u >= f.choices.len() as u64 {
                        return Some("a choice the field does not have");
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
        let map = message.map();
        let subject = map.get(key::SUBJECT).and_then(Value::as_text).unwrap_or("");
        let date = map.get(key::DATE).and_then(Value::as_u64).unwrap_or(0);
        let mut lines = vec![self.name.clone(), String::new()];
        for f in &self.fields {
            let shown = match f.bound {
                Some(Bound::Subject) => (!subject.is_empty()).then(|| subject.to_owned()),
                Some(Bound::Date) => Some(ymd(date / 86_400)),
                Some(Bound::Time) => Some(hm((date % 86_400) / 60)),
                None => form.value(f.n).map(|v| shown_value(f, v)),
            };
            let Some(shown) = shown else {
                continue;
            };
            if f.kind == Kind::Text {
                lines.push(format!("{}:", f.label));
                lines.extend(shown.split('\n').map(str::to_owned));
            } else {
                lines.push(format!("{}: {shown}", f.label));
            }
        }
        finish(&lines, message)
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
        let mut lines = vec![
            format!("Form {}", crate::cbor::hex(&self.template)),
            String::new(),
        ];
        for (n, v) in &self.values {
            let shown = match v {
                Value::Text(t) => t.clone(),
                Value::Bool(b) => yes_no(*b).to_owned(),
                Value::Uint(u) => u.to_string(),
                Value::Nint(m) => format!("-{}", u128::from(*m) + 1),
                other => other.diagnostic(),
            };
            if shown.contains('\n') {
                lines.push(format!("Field {n}:"));
                lines.extend(shown.split('\n').map(str::to_owned));
            } else {
                lines.push(format!("Field {n}: {shown}"));
            }
        }
        finish(&lines, message)
    }
}

/// The text a message's form shows as (RP-2 §8.4): with its template when one of `templates` is
/// it and the values fill it, else by number. `None` for a message with no form.
#[must_use]
pub fn shown(message: &Message, templates: &[Template]) -> Option<String> {
    let form = Form::of(message)?;
    let template = templates
        .iter()
        .find(|t| t.fingerprint() == form.template)
        .filter(|t| t.problem(&form.values).is_none());
    Some(match template {
        Some(t) => t.text(message, &form),
        None => form.text_without(message),
    })
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

fn shown_value(f: &Field, v: &Value) -> String {
    match (f.kind, v) {
        (Kind::Date, Value::Uint(d)) => ymd(*d),
        (Kind::Time, Value::Uint(m)) => hm(*m),
        (Kind::Choice, Value::Uint(i)) => usize::try_from(*i)
            .ok()
            .and_then(|i| f.choices.get(i))
            .cloned()
            .unwrap_or_default(),
        (Kind::Check, Value::Bool(b)) => yes_no(*b).to_owned(),
        (_, Value::Text(t)) => t.clone(),
        (_, Value::Uint(u)) => u.to_string(),
        (_, Value::Nint(m)) => format!("-{}", u128::from(*m) + 1),
        (_, other) => other.diagnostic(),
    }
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
}
