// SPDX-License-Identifier: MIT OR Apache-2.0
//! RPMI against its vectors (RP-4 §2 to §7): `rpmi-lines.json` and `rpmi-conversations.json`.
//!
//! The simulated channel plays every adapter: given the host lines, it must write the adapter
//! lines, as JSON values, `text` aside, each before the next host line and none extra. The
//! client's host plays its side of session-01 over RPMI with the real session engine: given the
//! adapter lines, it must write the host lines, each before the next adapter line.

mod common;

use std::collections::VecDeque;

use rpmi::channel::{Channel, Out, StationConfig};
use rpmi::host::{Carrier, Event, Host};
use rpmi::wire::{Caps, Message, line_of};
use serde_json::Value as Json;

use common::{Calling, vectors, world};

fn object(v: &Json) -> Message {
    v.as_object().expect("a JSON object").clone()
}

/// RP-4's rule for comparing: JSON values, with `text` left out.
fn without_text(m: &Message) -> Message {
    let mut m = m.clone();
    m.remove("text");
    m
}

/// The simulated modem of the vectors (`modem` in `rpmi-conversations.json`), with the caps the
/// station's `hello` gives and the link IDs the vector shows for it.
fn station(name: &str, hello: &Message, prefix: &str, file: &Json) -> StationConfig {
    StationConfig {
        name: name.into(),
        modem: file["modem"]["name"].as_str().expect("a modem").into(),
        caps: Caps::from_json(&hello["caps"]),
        prefix: prefix.into(),
        frame_bytes: usize::try_from(
            file["modem"]["received_bytes_at_most"]
                .as_u64()
                .expect("a size"),
        )
        .expect("a size"),
    }
}

#[test]
fn every_line_is_read_or_refused() {
    let conversations = vectors("rpmi-conversations.json");
    let hello = object(&conversations["conversations"][0]["steps"][2]["message"]);
    let file = vectors("rpmi-lines.json");
    let mut ch = Channel::new();
    let s = ch.add_station(station("KK4ODA", &hello, "c", &conversations));
    ch.open(s, "client");
    for line in [
        r#"{"op":"hello","id":1,"version":[1,1],"app":"RadioPost client 0.1.0"}"#,
        r#"{"op":"callsigns","id":2,"callsigns":["KK4ODA"]}"#,
    ] {
        ch.request_line(s, "client", line);
    }
    assert_eq!(ch.take().len(), 2, "hello and ok");
    for case in file["lines"].as_array().expect("lines") {
        let what = case["what"].as_str().expect("what");
        ch.request_line(s, "client", case["line"].as_str().expect("a line"));
        let got: Vec<Message> = ch
            .take()
            .into_iter()
            .map(|o| match o {
                Out::Line { message, .. } => without_text(&message),
                Out::Close { .. } => panic!("{what}: the adapter closed"),
            })
            .collect();
        let want: Vec<Message> = case["answers"]
            .as_array()
            .expect("answers")
            .iter()
            .map(|a| without_text(&object(a)))
            .collect();
        assert_eq!(got, want, "{what}");
    }
}

/// The step's station and host.
fn who(step: &Json) -> (String, String) {
    (
        step["station"].as_str().expect("station").into(),
        step["host"].as_str().expect("host").into(),
    )
}

/// The link prefix a station's adapter uses in the conversation: its first `connected`'s link,
/// less the count.
fn prefix_of(steps: &[Json], station: &str) -> String {
    steps
        .iter()
        .filter(|s| s["station"] == station && s["to"] == "host")
        .find_map(|s| s["message"]["link"].as_str())
        .map_or("x", |l| l.trim_end_matches(|c: char| c.is_ascii_digit()))
        .into()
}

fn replay_adapters(conversation: &Json, file: &Json) {
    let name = conversation["name"].as_str().expect("a name");
    let steps = conversation["steps"].as_array().expect("steps");
    let mut ch = Channel::new();
    let mut stations: Vec<String> = Vec::new();
    let mut produced: VecDeque<Out> = VecDeque::new();
    for (i, step) in steps.iter().enumerate() {
        if step.get("note").is_some() {
            continue;
        }
        let (station, host) = who(step);
        let s = if let Some(s) = stations.iter().position(|x| *x == station) {
            s
        } else {
            let prefix = prefix_of(steps, &station);
            let hello = steps
                .iter()
                .find(|s| {
                    s["station"] == station.as_str()
                        && s["to"] == "host"
                        && s["message"]["op"] == "hello"
                })
                .map(|s| object(&s["message"]))
                .expect("a hello from each adapter");
            stations.push(station.clone());
            ch.add_station(self::station(&station, &hello, &prefix, file))
        };
        let at = format!("{name}, step {i}");
        let input = match step["connection"].as_str() {
            Some("opened") => true,
            Some("closed") => {
                let ours = matches!(produced.front(),
                    Some(Out::Close { station: st, host: h }) if *st == s && *h == host);
                if ours {
                    produced.pop_front();
                    continue;
                }
                true
            }
            _ => step["to"] == "adapter",
        };
        if input {
            assert!(
                produced.is_empty(),
                "{at}: the adapter said more first: {produced:?}"
            );
            match step["connection"].as_str() {
                Some("opened") => ch.open(s, &host),
                Some(_) => ch.close(s, &host),
                None => ch.request_line(s, &host, &line_of(&object(&step["message"]))),
            }
            produced.extend(ch.take());
            continue;
        }
        if produced.is_empty() {
            // Nothing is due from the host's lines: the modem's time passes.
            ch.give_up();
            produced.extend(ch.take());
        }
        let want = without_text(&object(&step["message"]));
        match produced.pop_front() {
            Some(Out::Line {
                station: st,
                host: h,
                message,
            }) => {
                assert_eq!(
                    (stations[st].as_str(), h.as_str()),
                    (station.as_str(), host.as_str()),
                    "{at}: said to the wrong host: {message:?}"
                );
                assert_eq!(without_text(&message), want, "{at}");
            }
            other => panic!("{at}: wanted {want:?}, got {other:?}"),
        }
    }
    assert!(produced.is_empty(), "{name}: said more: {produced:?}");
    ch.give_up();
    assert!(ch.take().is_empty(), "{name}: a call left waiting");
}

#[test]
fn every_conversation_from_the_adapters() {
    let file = vectors("rpmi-conversations.json");
    let conversations = file["conversations"].as_array().expect("conversations");
    assert_eq!(conversations.len(), 4);
    for c in conversations {
        replay_adapters(c, &file);
    }
}

/// A RadioPost client: says hello, names its callsign, calls, and carries the session.
struct ClientProgram {
    host: Host,
    carrier: Carrier<Calling>,
    events: Vec<radiopost::session::Event>,
}

impl ClientProgram {
    fn start(&mut self) -> Vec<Message> {
        vec![self.host.hello("RadioPost client 0.1.0")]
    }

    fn on_line(&mut self, m: &Message) -> Vec<Message> {
        let event = Event::read(m).expect("an event");
        match &event {
            Event::Hello { .. } => vec![self.host.callsigns(&["KK4ODA"])],
            Event::Ok { id } if *id == 2 => {
                vec![self.host.connect("N1CALL-10", None, Some(2300))]
            }
            _ => {
                let (out, said) = self.carrier.on_event(&mut self.host, &event);
                self.events.extend(said);
                out
            }
        }
    }
}

#[test]
fn session_01_over_rpmi_from_the_clients_host() {
    let w = world();
    let file = vectors("rpmi-conversations.json");
    let steps = file["conversations"][0]["steps"].as_array().expect("steps");
    let mut program = ClientProgram {
        host: Host::new(),
        carrier: Carrier::new(Calling(common::session_01_client(&w))),
        events: Vec::new(),
    };
    let mut written: VecDeque<Message> = VecDeque::new();
    for (i, step) in steps.iter().enumerate() {
        if step.get("note").is_some() || who(step) != ("KK4ODA".into(), "client".into()) {
            continue;
        }
        if step["connection"] == "opened" {
            written.extend(program.start());
        } else if step["to"] == "host" {
            assert!(
                written.is_empty(),
                "step {i}: the host wrote more first: {written:?}"
            );
            written.extend(program.on_line(&object(&step["message"])));
        } else {
            let want = object(&step["message"]);
            assert_eq!(written.pop_front(), Some(want), "step {i}");
        }
    }
    assert!(written.is_empty(), "the host wrote more: {written:?}");
    assert!(program.carrier.is_ended(), "the link is down");
    let ids = |names: &[&str]| -> Vec<radiopost::ContentId> {
        names.iter().map(|n| w.messages[*n].content_id()).collect()
    };
    assert_eq!(
        common::delivered(&program.events),
        ids(&["facundo-reply", "facundo-to-ana"])
    );
    assert_eq!(
        common::received(&program.events),
        ids(&["maria-419", "eoc-420"])
    );
}
