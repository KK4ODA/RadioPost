// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-2 §8 against `vectors/forms.json`: each published template's encoding and fingerprint, the
//! text `shelter3-213` shows as with its template and without it, and values checked.

use std::path::PathBuf;

use radiopost::cbor::Value;
use radiopost::forms::{Form, Template, shown};
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
    }
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
        assert_eq!(shown(&m, &[t]).as_deref(), s["text"].as_str());
        assert_eq!(
            shown(&m, &[]).as_deref(),
            s["text_without_template"].as_str()
        );
        assert_eq!(
            form.text_without(&m),
            s["text_without_template"].as_str().expect("text")
        );
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
        let values: Vec<(u64, Value)> = c["values"]
            .as_object()
            .expect("values")
            .iter()
            .map(|(n, v)| {
                let value = match v {
                    Json::String(s) => Value::text(s),
                    Json::Bool(b) => Value::Bool(*b),
                    Json::Number(n) => Value::int(n.as_i64().expect("an integer")),
                    other => panic!("{other}"),
                };
                (n.parse().expect("a number"), value)
            })
            .collect();
        let got = t.problem(&values).unwrap_or("fills the template");
        assert_eq!(got, c["expect"].as_str().expect("expect"), "{}", c["what"]);
    }
}
