// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the post office found of an applicant's callsign, for its operators (design D35).
//!
//! The FCC's licence database and QRZ.com's callbook show that a callsign exists and in whose
//! name, which anyone can read, so the evidence is shown beside the decision, never in its place.
//! The judgement here only sorts what was found into lines, flags and a colour: `good` (an
//! active US licence in the applicant's name), `warn` (something to check by hand), `bad` (no
//! licence, or one no longer valid).

use serde_json::{Value as Json, json};
use unicode_normalization::UnicodeNormalization as _;

use crate::licences::{Licence, us_callsign};
use crate::qrz::Entry;

/// What was looked up, and what came back. `None`: not looked up (no copy, no account).
#[derive(Debug, Default)]
pub struct Found {
    /// The FCC copy's answer.
    pub fcc: Option<Result<Option<Licence>, String>>,
    /// QRZ's answer.
    pub qrz: Option<Result<Option<Entry>, String>>,
}

/// A name as plain lowercase words: accents, case and punctuation aside.
fn words(name: &str) -> Vec<String> {
    let plain: String = name
        .nfkd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect();
    plain.split_whitespace().map(str::to_owned).collect()
}

/// Whether `given` (as the applicant wrote it) is the person `first` `last`: every word of the
/// last name is there, and the first name or its initial.
#[must_use]
pub fn same_person(given: &str, first: &str, last: &str) -> bool {
    let given = words(given);
    let (first, last) = (words(first), words(last));
    if last.is_empty() {
        return false;
    }
    let last_ok = last.iter().all(|w| given.contains(w));
    let first_ok = first.first().is_none_or(|f| {
        given.contains(f)
            || given
                .iter()
                .any(|w| w.len() == 1 && f.starts_with(w.as_str()))
    });
    last_ok && first_ok
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Weight {
    Note,
    Warn,
    Bad,
}

/// What was gathered while judging: lines, weighted flags, and whether a name matched.
#[derive(Default)]
struct Notes {
    lines: Vec<String>,
    flags: Vec<(Weight, String)>,
    matched: bool,
}

/// The FCC copy's part: the licence, its state, its holder; the address to post a card to.
fn from_fcc(
    name: &str,
    call: &str,
    fcc: Option<&Result<Option<Licence>, String>>,
    today: &str,
    n: &mut Notes,
) -> Option<(Json, Json)> {
    let us = us_callsign(call);
    let l = match fcc {
        None if us => {
            n.flags.push((
                Weight::Warn,
                "No copy of the FCC's licence database here: turn it on in Settings".into(),
            ));
            return None;
        }
        None => return None,
        Some(Err(e)) => {
            n.flags
                .push((Weight::Warn, format!("The FCC copy could not be read: {e}")));
            return None;
        }
        Some(Ok(None)) => {
            if us {
                n.flags
                    .push((Weight::Bad, format!("No FCC licence for {call}")));
            } else {
                n.lines.push(format!(
                    "{call} is not a US callsign: the FCC's database does not cover it"
                ));
            }
            return None;
        }
        Some(Ok(Some(l))) => l,
    };
    let until = if l.expires.is_empty() {
        String::new()
    } else {
        format!(" until {}", l.expires)
    };
    n.lines.push(format!(
        "FCC: {}, {}, {}{until}. {}, {} {}",
        l.call,
        l.class_name(),
        l.status_name(),
        l.name,
        l.city,
        l.state
    ));
    if l.status != "A" {
        let when = if l.cancelled.is_empty() {
            &l.expires
        } else {
            &l.cancelled
        };
        n.flags.push((
            Weight::Bad,
            format!(
                "The licence is {} ({when}): it does not allow transmitting",
                l.status_name()
            ),
        ));
    } else if !l.expires.is_empty() && l.expires.as_str() < today {
        n.flags
            .push((Weight::Bad, format!("The licence ran out on {}", l.expires)));
    }
    if l.class.is_empty() {
        let trustee = if l.trustee.is_empty() {
            "unknown"
        } else {
            &l.trustee
        };
        n.flags.push((
            Weight::Warn,
            format!(
                "A club callsign, its trustee {trustee}: a role mailbox, with the trustee's word"
            ),
        ));
    } else if same_person(name, &l.first, &l.last) {
        n.matched = true;
    } else {
        n.flags.push((
            Weight::Warn,
            format!("The licence is in the name of {}, not {name}", l.name),
        ));
    }
    Some((
        serde_json::to_value(l).unwrap_or(Json::Null),
        json!(l.mail_to()),
    ))
}

/// QRZ's part: what its user wrote, and whether the name matches when the FCC said nothing.
fn from_qrz(
    name: &str,
    call: &str,
    qrz: Option<&Result<Option<Entry>, String>>,
    fcc_said: bool,
    n: &mut Notes,
) -> Option<Json> {
    let e = match qrz {
        None => return None,
        Some(Err(e)) => {
            n.lines.push(format!("QRZ: {e}"));
            return None;
        }
        Some(Ok(None)) => {
            n.lines.push(format!("QRZ: {call} is not listed"));
            return None;
        }
        Some(Ok(Some(e))) => e,
    };
    let place: Vec<&str> = [e.city.as_str(), e.state.as_str(), e.country.as_str()]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect();
    let mut line = format!("QRZ: {} {}, {}", e.first, e.last, place.join(", "));
    if e.lotw == "1" {
        line.push_str(", uploads to LoTW");
    }
    n.lines.push(line);
    if !fcc_said && !e.last.is_empty() {
        if same_person(name, &e.first, &e.last) {
            n.matched = true;
        } else {
            n.flags.push((
                Weight::Warn,
                format!("QRZ lists {} {}, not {name}", e.first, e.last),
            ));
        }
    }
    Some(serde_json::to_value(e).unwrap_or(Json::Null))
}

/// The evidence for an applicant giving `name` and `call`, from what was `found`, with `history`
/// (earlier applications and accounts of the callsign) said as it is. `today` is `YYYY-MM-DD`.
#[must_use]
pub fn judge(name: &str, call: &str, found: &Found, history: &[String], today: &str) -> Json {
    let mut n = Notes::default();
    let fcc = from_fcc(name, call, found.fcc.as_ref(), today, &mut n);
    let qrz = from_qrz(name, call, found.qrz.as_ref(), fcc.is_some(), &mut n);
    for h in history {
        n.flags.push((Weight::Warn, h.clone()));
    }
    let worst = n
        .flags
        .iter()
        .map(|(w, _)| *w)
        .max()
        .unwrap_or(Weight::Note);
    let verdict = match worst {
        Weight::Bad => "bad",
        Weight::Note if n.matched => "good",
        _ => "warn",
    };
    if verdict == "good" {
        n.lines.push(
            "The name matches the licence. Still evidence, not proof: anyone can read it.".into(),
        );
    }
    let mut out = json!({
        "verdict": verdict,
        "lines": n.lines,
        "flags": n.flags.into_iter().map(|(_, f)| f).collect::<Vec<_>>(),
    });
    if let Some((licence, mail_to)) = fcc {
        out["fcc"] = licence;
        out["mail_to"] = mail_to;
    }
    if let Some(entry) = qrz {
        out["qrz"] = entry;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn licence(status: &str, class: &str, first: &str, last: &str) -> Licence {
        Licence {
            call: "N5CALL".into(),
            status: status.into(),
            class: class.into(),
            granted: "2024-03-04".into(),
            expires: "2034-03-04".into(),
            name: format!("{first} {last}"),
            first: first.into(),
            last: last.into(),
            city: "ATLANTA".into(),
            state: "GA".into(),
            zip: "30303".into(),
            street: "12 ELM ST".into(),
            ..Licence::default()
        }
    }

    fn fcc(l: Option<Licence>) -> Found {
        Found {
            fcc: Some(Ok(l)),
            qrz: None,
        }
    }

    #[test]
    fn names_match_through_accents_and_initials() {
        assert!(same_person("José Pérez", "JOSE", "PEREZ"));
        assert!(same_person("J. Perez", "JOSE", "PEREZ"));
        assert!(same_person("Maria de la Cruz", "MARIA", "DE LA CRUZ"));
        assert!(!same_person("Lena Ortiz", "JOSE", "PEREZ"));
        assert!(!same_person("Jose", "JOSE", "PEREZ"));
    }

    #[test]
    fn an_active_licence_in_the_name_is_good_and_anything_else_is_said() {
        let today = "2026-11-12";
        let good = judge(
            "José Pérez",
            "N5CALL",
            &fcc(Some(licence("A", "G", "JOSE", "PEREZ"))),
            &[],
            today,
        );
        assert_eq!(good["verdict"], "good", "{good}");
        assert_eq!(good["mail_to"][1], "12 ELM ST");
        let other = judge(
            "Lena Ortiz",
            "N5CALL",
            &fcc(Some(licence("A", "G", "JOSE", "PEREZ"))),
            &[],
            today,
        );
        assert_eq!(other["verdict"], "warn", "{other}");
        let expired = judge(
            "José Pérez",
            "N5CALL",
            &fcc(Some(licence("E", "G", "JOSE", "PEREZ"))),
            &[],
            today,
        );
        assert_eq!(expired["verdict"], "bad", "{expired}");
        let none = judge("José Pérez", "N5CALL", &fcc(None), &[], today);
        assert_eq!(none["verdict"], "bad", "{none}");
        let abroad = judge("Ana", "VE3ABC", &fcc(None), &[], today);
        assert_eq!(abroad["verdict"], "warn", "{abroad}");
        let unchecked = judge("José Pérez", "N5CALL", &Found::default(), &[], today);
        assert_eq!(unchecked["verdict"], "warn", "{unchecked}");
        let again = judge(
            "José Pérez",
            "N5CALL",
            &fcc(Some(licence("A", "G", "JOSE", "PEREZ"))),
            &["Applied before: declined".into()],
            today,
        );
        assert_eq!(again["verdict"], "warn", "{again}");
        let club = judge(
            "Shelter",
            "N5CALL",
            &fcc(Some(licence("A", "", "", ""))),
            &[],
            today,
        );
        assert_eq!(club["verdict"], "warn", "{club}");
    }
}
