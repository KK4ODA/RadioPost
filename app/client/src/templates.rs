// SPDX-License-Identifier: MIT OR Apache-2.0
//! The form templates this client holds (RP-2 §8.3): RadioPost's published ones, and those its user
//! keeps, imported from a file or fetched by fingerprint from a post office (RP-5 §4.7) for a form
//! that came in.
//!
//! What is kept is the computer's, not an account's: it is in the first home's [`DIR`], whichever
//! callsign is chosen, one file a template, its deterministic CBOR named by its fingerprint
//! (`forms/c071db80be8d9501.cbor`). A file whose bytes do not hash to its name, or do not read as a
//! template, is not held. Each template kept is handed in to the post office of each account
//! ([`hand_in`], RP-5 §6.8), so that its forms read as forms in email; [`HANDED_IN`] remembers
//! where it went.

use std::fs;
use std::path::{Path, PathBuf};

use radiopost::cbor::hex;
use radiopost::forms::Template;
use serde_json::{Value, json};

use crate::callsigns;
use crate::online;
use crate::store::{Error, Store, write};

/// The folder of the templates kept, in the first home.
pub const DIR: &str = "forms";

/// The file in [`DIR`] that says which templates were handed in to which account's post office.
pub const HANDED_IN: &str = "handed-in.json";

fn dir(home: &Path) -> PathBuf {
    callsigns::base_of(home).join(DIR)
}

/// A template's fingerprint in hex, as files and paths name it.
#[must_use]
pub fn fingerprint(t: &Template) -> String {
    hex(&t.fingerprint())
}

/// Whether a template is one of RadioPost's published ones.
#[must_use]
pub fn published(fingerprint: &[u8; 8]) -> bool {
    Template::published()
        .iter()
        .any(|t| t.fingerprint() == *fingerprint)
}

/// The templates kept at `home` (any callsign's), by name.
#[must_use]
pub fn kept(home: &Path) -> Vec<Template> {
    let mut out: Vec<Template> = fs::read_dir(dir(home))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_stem()?.to_str()?.to_owned();
            if path.extension()? != "cbor" {
                return None;
            }
            let bytes = fs::read(&path).ok()?;
            let t = Template::from_cbor(&bytes).ok()?;
            (fingerprint(&t) == name).then_some(t)
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Every template this client holds: the published ones first, as the programs order them, then
/// those kept.
#[must_use]
pub fn held(home: &Path) -> Vec<Template> {
    let mut all = Template::published();
    for t in kept(home) {
        if !all.iter().any(|p| p.fingerprint() == t.fingerprint()) {
            all.push(t);
        }
    }
    all
}

/// Keeps a template given as its deterministic CBOR (RP-2 §8.3): read only as RP-2 §8.1 writes
/// one. A published one is held already, and kept nowhere.
///
/// # Errors
///
/// The bytes are not a template, or the file cannot be written.
pub fn keep(home: &Path, bytes: &[u8]) -> Result<Template, Error> {
    let t = Template::from_cbor(bytes).map_err(|why| format!("not a form's template: {why}"))?;
    if published(&t.fingerprint()) {
        return Ok(t);
    }
    let folder = dir(home);
    fs::create_dir_all(&folder).map_err(|e| format!("cannot write {}: {e}", folder.display()))?;
    write(&folder.join(format!("{}.cbor", fingerprint(&t))), bytes)?;
    Ok(t)
}

/// Keeps a template from a file: its JSON (as `forms/*.json` are written) or its CBOR. The JSON
/// must keep RP-2 §8.1's rules; what is kept is its CBOR, so the fingerprint is the template's
/// whichever way it came.
///
/// # Errors
///
/// The file holds no template, or it cannot be kept.
pub fn import(home: &Path, data: &[u8]) -> Result<Template, Error> {
    let looks_like_json = data
        .iter()
        .find(|b| !b.is_ascii_whitespace())
        .is_some_and(|b| *b == b'{');
    let bytes = if looks_like_json {
        let text = std::str::from_utf8(data).map_err(|_| "the file is not UTF-8 text")?;
        let t = Template::from_json(text).map_err(|why| format!("not a form's template: {why}"))?;
        t.to_cbor().encode().map_err(|_| "not a form's template")?
    } else {
        data.to_vec()
    };
    keep(home, &bytes)
}

/// Stops holding a template kept. A published one cannot be: the programs hold it.
///
/// # Errors
///
/// It is published, or not kept here.
pub fn remove(home: &Path, fingerprint: &str) -> Result<Template, Error> {
    let t = kept(home)
        .into_iter()
        .find(|t| self::fingerprint(t) == fingerprint)
        .ok_or_else(|| format!("no form {fingerprint} kept here: a published form stays"))?;
    let path = dir(home).join(format!("{fingerprint}.cbor"));
    fs::remove_file(&path).map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
    Ok(t)
}

/// A template as a file to give someone: its JSON, as RadioPost's own `forms/*.json` are.
#[must_use]
pub fn exported(t: &Template) -> String {
    let mut text = serde_json::to_string_pretty(&t.to_json()).unwrap_or_default();
    text.push('\n');
    text
}

/// Fetches the template a form names, which this client does not hold (RP-5 §4.7): from the
/// user's own post office, then from the post office of the form's sender (`sender`, an
/// address), found by its domain (RP-1 §7.1). Keeps it, and returns it.
///
/// # Errors
///
/// Neither post office holds it, or it cannot be kept: what each said.
pub fn fetch(store: &Store, fingerprint: &str, sender: Option<&str>) -> Result<Template, Error> {
    let mut asked: Vec<String> = Vec::new();
    let mut said: Vec<String> = Vec::new();
    let own = store.me().ok().map(|me| (domain_of(&me.address), true));
    let domains = own.into_iter().chain(sender.map(|s| (domain_of(s), false)));
    for (domain, ours) in domains {
        // the home names its own post office's address; any other is found in DNS
        let api = if ours {
            online::post_office(store, &domain)
        } else {
            radiopost_api::lookup(radiopost_api::RESOLVER, &domain)
                .ok_or_else(|| format!("no post office found for {domain} in DNS"))
        };
        let api = match api {
            Ok(api) if !asked.contains(&api) => api,
            Ok(_) => continue,
            Err(why) => {
                said.push(why);
                continue;
            }
        };
        asked.push(api.clone());
        match online::template(&api, fingerprint) {
            Ok(bytes) => return keep(store.home(), &bytes),
            Err(why) => said.push(why),
        }
    }
    Err(if said.is_empty() {
        "no post office to ask".into()
    } else {
        said.join("; ")
    })
}

fn domain_of(address: &str) -> String {
    address
        .rsplit_once('@')
        .map_or(address, |(_, d)| d)
        .to_lowercase()
}

/// Hands each template kept in to the post office of the account at `home`, once (RP-5 §6.8):
/// the fingerprints handed in this time.
///
/// # Errors
///
/// No credential, or the post office could not be reached or refused: what was handed in before
/// it stays remembered.
pub fn hand_in(store: &Store) -> Result<Vec<String>, Error> {
    let me = store.me()?;
    let path = dir(store.home()).join(HANDED_IN);
    let mut record: Value = fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));
    let mut done: Vec<String> = Vec::new();
    for t in kept(store.home()) {
        let fp = fingerprint(&t);
        let already = record[&me.address]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x == &fp));
        if already {
            continue;
        }
        let bytes = t.to_cbor().encode().map_err(|_| "not a form's template")?;
        online::hand_in_template(store, &bytes)?;
        let list = record
            .as_object_mut()
            .ok_or("handed-in.json is not an object")?
            .entry(me.address.clone())
            .or_insert_with(|| json!([]));
        if let Some(a) = list.as_array_mut() {
            a.push(fp.clone().into());
        }
        let text = serde_json::to_string_pretty(&record).map_err(|e| e.to_string())?;
        write(&path, text.as_bytes())?;
        done.push(fp);
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::{exported, held, import, kept, remove};
    use radiopost::forms::Template;

    fn scratch(name: &str) -> std::path::PathBuf {
        let home = std::env::temp_dir().join(format!("radiopost-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("a home");
        home
    }

    const OURS: &str = r#"{"name": "Shelter roll", "fields": [
        {"n": 1, "label": "Shelter", "type": "line", "required": true},
        {"n": 2, "label": "Cots", "type": "number"}]}"#;

    #[test]
    fn a_template_is_kept_from_its_json_or_its_cbor_and_held_beside_the_published() {
        let home = scratch("templates-kept");
        let t = import(&home, OURS.as_bytes()).expect("imported");
        assert_eq!(kept(&home), vec![t.clone()]);
        let all = held(&home);
        assert_eq!(all.len(), Template::published().len() + 1);
        assert_eq!(all.last(), Some(&t));
        // what is exported reads back as the same template, and so does its CBOR
        let again = Template::from_json(&exported(&t)).expect("its JSON");
        assert_eq!(again, t);
        let cbor = t.to_cbor().encode().expect("encodes");
        assert_eq!(import(&home, &cbor).expect("its CBOR"), t);
        assert_eq!(kept(&home).len(), 1, "kept once");
        // a published template is held already, and kept nowhere
        let ics = &Template::published()[0];
        import(&home, &ics.to_cbor().encode().expect("encodes")).expect("read");
        assert_eq!(kept(&home).len(), 1);
        assert!(remove(&home, &super::fingerprint(ics)).is_err());
        remove(&home, &super::fingerprint(&t)).expect("removed");
        assert_eq!(kept(&home), Vec::new());
    }

    #[test]
    fn a_file_that_is_no_template_is_not_kept() {
        let home = scratch("templates-refused");
        let line_with_choices = r#"{"name": "T", "fields": [
            {"n": 1, "label": "A", "type": "line", "choices": ["x"]}]}"#;
        assert!(import(&home, line_with_choices.as_bytes()).is_err());
        assert!(import(&home, b"\xa1\x01\x61T").is_err());
        assert!(import(&home, b"not a template").is_err());
        // a file whose name is not its bytes' fingerprint is not held
        let t = import(&home, OURS.as_bytes()).expect("imported");
        let folder = home.join(super::DIR);
        std::fs::rename(
            folder.join(format!("{}.cbor", super::fingerprint(&t))),
            folder.join("0000000000000000.cbor"),
        )
        .expect("renamed");
        assert_eq!(kept(&home), Vec::new());
    }
}
