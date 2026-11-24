// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-2 §8 against `vectors/forms.json`: each published template's encoding and fingerprint, the
//! text `shelter3-213` shows as with its template and without it, and values checked; and the test
//! template with every type (`every_type`): its encoding, its messages' text, values written,
//! values checked, and templates that break §8.1's rules.

use std::path::PathBuf;

use radiopost::cbor::Value;
use radiopost::forms::{Form, Template, shown, shown_as};
use radiopost::message::Message;
use serde_json::Value as Json;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn vectors(name: &str) -> Json {
    let path = root().join("vectors").join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("a vector file is JSON")
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

fn template(file: &str) -> Template {
    let text = std::fs::read_to_string(root().join(file)).expect("a template");
    Template::from_json(&text).expect("it reads")
}

fn message(name: &str) -> Message {
    let hex = vectors("messages.json")["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|m| m["name"] == name)
        .and_then(|m| m["canonical_hex"].as_str())
        .map(str::to_owned)
        .expect("the message");
    Message::from_canonical(&unhex(&hex)).expect("canonical")
}

#[test]
fn templates_encode_and_fingerprint_as_rp2_8_1_says() {
    let file = vectors("forms.json");
    let published = Template::published();
    for t in file["templates"].as_array().expect("templates") {
        let ours = template(t["file"].as_str().expect("a file"));
        let encoded = ours.to_cbor().encode().expect("encodes");
        assert_eq!(encoded, unhex(t["canonical_hex"].as_str().expect("hex")));
        assert_eq!(
            ours.to_cbor().diagnostic(),
            t["diagnostic"].as_str().expect("diag")
        );
        assert_eq!(
            ours.fingerprint().to_vec(),
            unhex(t["fingerprint_hex"].as_str().expect("hex"))
        );
        assert!(published.contains(&ours), "the programs hold {}", ours.name);
        // RP-2 §8.3: read back from its CBOR (RP-5 §4.7) and from its JSON, the same template
        assert_eq!(Template::from_cbor(&encoded).as_ref(), Ok(&ours));
        assert_eq!(Template::from_value(&ours.to_json()).as_ref(), Ok(&ours));
    }
}

#[test]
fn a_template_reads_from_its_cbor_only_as_rp2_8_1_writes_it() {
    let e = vectors("forms.json")["every_type"].clone();
    let bytes = unhex(e["canonical_hex"].as_str().expect("hex"));
    let t = Template::from_cbor(&bytes).expect("the test template");
    assert_eq!(
        t.fingerprint().to_vec(),
        unhex(e["fingerprint_hex"].as_str().expect("hex"))
    );
    assert_eq!(t.to_json(), e["template"], "its JSON is the vector's");
    // what RP-5 §6.8 refuses as not a template: each handed-in body the post office refused
    // as malformed under the fingerprint of its own bytes is not read here either
    let account = vectors("api-account.json");
    let mut seen = 0;
    for c in account["forms_cases"].as_array().expect("forms_cases") {
        for x in c["exchanges"].as_array().expect("exchanges") {
            let (Some(path), Some(given)) = (
                x["request"]["path"].as_str(),
                x["request"]["body"]["template"].as_str(),
            ) else {
                continue;
            };
            let Some(data) = radiopost::base64url::decode(given) else {
                continue;
            };
            let own = radiopost::cbor::hex(&radiopost::crypto::sha256(&data)[..8]);
            if !path.ends_with(&own) || data.len() > 65_536 {
                continue;
            }
            let status = x["response"]["status"].as_u64().expect("status");
            let read = Template::from_cbor(&data);
            assert_eq!(read.is_ok(), status != 400, "{}: {read:?}", c["what"]);
            seen += 1;
        }
    }
    assert!(seen >= 5, "the refusals were found");
}

#[test]
fn a_form_message_shows_as_rp2_8_4_says() {
    let file = vectors("forms.json");
    for s in file["shown"].as_array().expect("shown") {
        let m = message(s["message"].as_str().expect("a name"));
        let t = template(s["template_file"].as_str().expect("a file"));
        let form = Form::of(&m).expect("a form");
        assert_eq!(form.template, t.fingerprint());
        assert_eq!(Message::from_map(&rebuilt(&m, &form)).expect("rebuilt"), m);
        assert_eq!(
            shown(&m, std::slice::from_ref(&t)).as_deref(),
            s["text"].as_str()
        );
        assert_eq!(
            shown(&m, &[]).as_deref(),
            s["text_without_template"].as_str()
        );
        assert_eq!(
            form.text_without(&m),
            s["text_without_template"].as_str().expect("text")
        );
        // the rows a program lays out as the form are the text's lines, each label with its value
        let with = shown_as(&m, std::slice::from_ref(&t)).expect("a form");
        assert_eq!((with.title.as_str(), with.filled), (t.name.as_str(), true));
        let text = s["text"].as_str().expect("text");
        for r in &with.rows {
            let line = if r.heading {
                format!("\n{}\n", r.label)
            } else if r.lines {
                format!("\n{}:\n{}\n", r.label, r.value)
            } else {
                format!("\n{}: {}\n", r.label, r.value)
            };
            assert!(text.contains(&line), "{line:?} in {text:?}");
        }
        let without = shown_as(&m, &[]).expect("a form");
        assert!(!without.filled);
        assert_eq!(without.rows.len(), form.values.len());
    }
}

/// The message again, its form written back from [`Form`]: the same bytes.
fn rebuilt(m: &Message, form: &Form) -> Value {
    let entries = m
        .map()
        .as_map()
        .expect("a map")
        .iter()
        .map(|(k, v)| {
            if k.as_u64() == Some(13) {
                (k.clone(), form.to_cbor())
            } else {
                (k.clone(), v.clone())
            }
        })
        .collect();
    Value::Map(entries)
}

#[test]
fn values_fill_the_template_or_say_why_not() {
    let file = vectors("forms.json");
    let t = template("forms/ics-213.json");
    for c in file["checks"].as_array().expect("checks") {
        let values = numbered(&c["values"]);
        let got = t.problem(&values).unwrap_or("fills the template");
        assert_eq!(got, c["expect"].as_str().expect("expect"), "{}", c["what"]);
    }
}

/// A value as the vectors write it in JSON, back to CBOR: a map's keys are its numbers as text.
fn cbor(v: &Json) -> Value {
    match v {
        Json::String(s) => Value::text(s),
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => Value::int(n.as_i64().expect("an integer")),
        Json::Array(items) => Value::Array(items.iter().map(cbor).collect()),
        Json::Object(entries) => Value::Map(
            entries
                .iter()
                .map(|(k, v)| (Value::Uint(k.parse().expect("a number")), cbor(v)))
                .collect(),
        ),
        Json::Null => Value::Null,
    }
}

/// Values by field number, from the vectors' JSON.
fn numbered(values: &Json) -> Vec<(u64, Value)> {
    let mut out: Vec<(u64, Value)> = values
        .as_object()
        .expect("values")
        .iter()
        .map(|(n, v)| (n.parse().expect("a number"), cbor(v)))
        .collect();
    out.sort_by_key(|(n, _)| *n);
    out
}

#[test]
fn every_type_encodes_shows_and_checks_as_rp2_8_says() {
    let e = &vectors("forms.json")["every_type"];
    let t = Template::from_value(&e["template"]).expect("the test template reads");
    let encoded = t.to_cbor().encode().expect("encodes");
    assert_eq!(encoded, unhex(e["canonical_hex"].as_str().expect("hex")));
    assert_eq!(
        t.to_cbor().diagnostic(),
        e["diagnostic"].as_str().expect("diag")
    );
    assert_eq!(
        t.fingerprint().to_vec(),
        unhex(e["fingerprint_hex"].as_str().expect("hex"))
    );
    for m in e["messages"].as_array().expect("messages") {
        let what = m["what"].as_str().expect("what");
        let message = Message::from_canonical(&unhex(m["canonical_hex"].as_str().expect("hex")))
            .expect("canonical");
        let form = Form::of(&message).expect("a form");
        assert_eq!(t.problem(&form.values), None, "{what}");
        assert_eq!(
            shown(&message, std::slice::from_ref(&t)).as_deref(),
            m["text"].as_str(),
            "{what}"
        );
        assert_eq!(
            shown(&message, &[]).as_deref(),
            m["text_without_template"].as_str(),
            "{what}"
        );
        // a rows field's table holds the cells its text shows
        let with = shown_as(&message, std::slice::from_ref(&t)).expect("a form");
        for r in with.rows.iter().filter(|r| r.table.is_some()) {
            let table = r.table.as_ref().expect("a table");
            assert_eq!(
                r.value.lines().next(),
                Some(table.columns.join(" | ").as_str())
            );
            assert_eq!(r.value.lines().count(), table.rows.len() + 1);
        }
    }
    for c in e["values_shown"].as_array().expect("values shown") {
        let what = c["what"].as_str().expect("what");
        let n = c["field"].as_u64().expect("a field");
        let value = cbor(&c["value"]);
        // the value alone in a form, shown with the template: its field's line, or lines
        let field = t.field(n).expect("the field");
        let form = Form {
            template: t.fingerprint(),
            values: vec![(n, value)],
        };
        let rows = t.shown(&message_with(&form), &form).rows;
        let row = rows.iter().find(|r| r.n == n).expect("its row");
        assert_eq!(
            row.value,
            c["shown"].as_str().expect("shown"),
            "{what} ({})",
            field.label
        );
    }
    for c in e["checks"].as_array().expect("checks") {
        let got = t
            .problem(&numbered(&c["values"]))
            .unwrap_or("fills the template");
        assert_eq!(got, c["expect"].as_str().expect("expect"), "{}", c["what"]);
    }
    for c in e["templates"].as_array().expect("templates") {
        let what = c["what"].as_str().expect("what");
        let read = Template::from_value(&c["template"]);
        if c["expect"] == "keeps the rules" {
            assert!(read.is_ok(), "{what}: {read:?}");
        } else {
            assert!(read.is_err(), "{what}: read as a template");
        }
    }
}

/// A message carrying this form, for showing it.
fn message_with(form: &Form) -> Message {
    let map = Value::int_map(vec![
        (1, Value::Uint(1)),
        (2, Value::text("shelter3@radiopost.org")),
        (3, Value::Array(vec![Value::text("eoc@radiopost.org")])),
        (5, Value::Uint(1_794_235_200)),
        (6, Value::text("")),
        (7, Value::text("")),
        (13, form.to_cbor()),
    ]);
    Message::from_map(&map).expect("a message")
}
