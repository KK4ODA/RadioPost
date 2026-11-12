// SPDX-License-Identifier: MIT OR Apache-2.0
//! The address list: the addresses this client has written to, and any added by hand, each with
//! the name it was last written with, offered as To and Cc are typed.
//!
//! It is kept in `<home>/addresses.json`, apart from `state.json`, so a session under way, which
//! writes the folders when it ends, never overwrites a change made meanwhile. It is this
//! computer's own: the post office's accept list (RP-5 §6.5), which says who may email the
//! account, is another list, and changing one leaves the other as it was.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::compose::parse_named;
use crate::store::Error;

/// The file, in the home.
pub const FILE: &str = "addresses.json";

/// One address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The address (RP-0 §6.2), its domain lowercase.
    pub address: String,
    /// The name written with it, if any.
    pub name: Option<String>,
    /// When a message to it was last written, Unix seconds; 0 if never.
    pub used: u64,
    /// How many messages have been written to it.
    pub uses: u64,
}

impl Entry {
    /// As the panel and the file write it.
    #[must_use]
    pub fn json(&self) -> Value {
        json!({"address": self.address, "name": self.name, "used": self.used, "uses": self.uses})
    }

    fn from_json(v: &Value) -> Option<Entry> {
        Some(Entry {
            address: v["address"].as_str()?.to_owned(),
            name: v["name"].as_str().map(str::to_owned),
            used: v["used"].as_u64().unwrap_or(0),
            uses: v["uses"].as_u64().unwrap_or(0),
        })
    }

    /// `Name <address>`, or the address alone: what goes in To.
    #[must_use]
    pub fn written(&self) -> String {
        match &self.name {
            Some(n) if n.contains([',', ';', '"', '<', '>']) => {
                format!("\"{}\" <{}>", n.replace('"', ""), self.address)
            }
            Some(n) => format!("{n} <{}>", self.address),
            None => self.address.clone(),
        }
    }
}

fn path(home: &Path) -> PathBuf {
    home.join(FILE)
}

/// The list, by name (or address where there is none), as an address book reads.
#[must_use]
pub fn list(home: &Path) -> Vec<Entry> {
    let mut entries: Vec<Entry> = fs::read_to_string(path(home))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| {
            v.as_array()
                .map(|a| a.iter().filter_map(Entry::from_json).collect())
        })
        .unwrap_or_default();
    entries.sort_by_key(|e| e.name.as_deref().unwrap_or(&e.address).to_lowercase());
    entries
}

fn save(home: &Path, entries: &[Entry]) -> Result<(), Error> {
    let v: Vec<Value> = entries.iter().map(Entry::json).collect();
    let text = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    crate::store::write(&path(home), text.as_bytes())
}

/// Keeps the addresses a message was written to, at `now`: each one new is added, and each one
/// known counts the message and takes the name it was written with this time, if any.
///
/// # Errors
///
/// The file cannot be written.
pub fn remember(home: &Path, written: &[(Option<String>, String)], now: u64) -> Result<(), Error> {
    if written.is_empty() {
        return Ok(());
    }
    let mut entries = list(home);
    for (name, address) in written {
        match entries.iter_mut().find(|e| e.address == *address) {
            Some(e) => {
                e.used = now;
                e.uses += 1;
                if name.is_some() {
                    e.name.clone_from(name);
                }
            }
            None => entries.push(Entry {
                address: address.clone(),
                name: name.clone(),
                used: now,
                uses: 1,
            }),
        }
    }
    save(home, &entries)
}

/// Adds what was typed, `Name <address>` or an address alone, or renames an address already
/// there: the list.
///
/// # Errors
///
/// It is not one address, or the file cannot be written.
pub fn add(home: &Path, typed: &str) -> Result<Vec<Entry>, Error> {
    let [(name, address)] = <[_; 1]>::try_from(parse_named(typed)?)
        .map_err(|_| "one address at a time: Maria <maria@example.com>".to_owned())?;
    let mut entries = list(home);
    match entries.iter_mut().find(|e| e.address == address) {
        Some(e) => {
            if name.is_some() {
                e.name = name;
            }
        }
        None => entries.push(Entry {
            address,
            name,
            used: 0,
            uses: 0,
        }),
    }
    save(home, &entries)?;
    Ok(list(home))
}

/// Puts back an entry as it was, its count and date with it: what Undo does after [`remove`].
///
/// # Errors
///
/// It is not an address, or the file cannot be written.
pub fn put_back(home: &Path, entry: Entry) -> Result<Vec<Entry>, Error> {
    let address = crate::compose::checked_address(&entry.address)?;
    let mut entries = list(home);
    entries.retain(|e| e.address != address);
    entries.push(Entry { address, ..entry });
    save(home, &entries)?;
    Ok(list(home))
}

/// Takes an address off the list: what it was, and the list.
///
/// # Errors
///
/// It is not on the list, or the file cannot be written.
pub fn remove(home: &Path, address: &str) -> Result<(Entry, Vec<Entry>), Error> {
    let address = crate::compose::address(address.trim());
    let mut entries = list(home);
    let at = entries
        .iter()
        .position(|e| e.address == address)
        .ok_or_else(|| format!("{address} is not on the address list"))?;
    let gone = entries.remove(at);
    save(home, &entries)?;
    Ok((gone, list(home)))
}

#[cfg(test)]
mod tests {
    use super::{Entry, add, list, put_back, remember, remove};

    /// Addresses written to are kept with their names, counted and renamed; one added by hand
    /// joins them; a removed one goes and comes back as it was; and the list reads by name.
    #[test]
    fn the_address_list() {
        let home = std::env::temp_dir().join(format!("radiopost-addresses-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("a home");
        assert_eq!(list(&home), [], "none yet");
        let written = crate::compose::parse_named(
            r#"Zoe Lee <zoe@Example.com>, "Perez, Ana" <ana@hampost.org>; eoc@radiopost.org"#,
        )
        .expect("addresses");
        remember(&home, &written, 100).expect("kept");
        remember(&home, &[(None, "zoe@example.com".into())], 200).expect("kept");
        let names: Vec<String> = list(&home).iter().map(Entry::written).collect();
        assert_eq!(
            names,
            [
                "eoc@radiopost.org",
                "\"Perez, Ana\" <ana@hampost.org>",
                "Zoe Lee <zoe@example.com>",
            ],
            "by name, the address where there is none; a name kept when not written again"
        );
        let zoe = list(&home)
            .into_iter()
            .find(|e| e.address == "zoe@example.com")
            .expect("zoe");
        assert_eq!((zoe.uses, zoe.used), (2, 200));
        assert!(add(&home, "a, b@example.com").is_err(), "one at a time");
        assert!(add(&home, "not an address").is_err());
        let after = add(&home, "Bob <bob@example.net>").expect("added");
        assert_eq!(after.len(), 4);
        add(&home, "Zoe <zoe@example.com>").expect("renamed");
        let (gone, left) = remove(&home, "zoe@EXAMPLE.com").expect("removed");
        assert_eq!(
            (gone.name.as_deref(), gone.uses, left.len()),
            (Some("Zoe"), 2, 3)
        );
        assert!(remove(&home, "zoe@example.com").is_err(), "gone");
        put_back(&home, gone.clone()).expect("put back");
        assert!(list(&home).contains(&gone), "as it was");
        let _ = std::fs::remove_dir_all(&home);
    }
}
