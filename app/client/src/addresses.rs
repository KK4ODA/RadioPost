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

// ── groups (wishlist W2) ─────────────────────────────────────────────────────────────────────
// Kept in `<home>/groups.json`, a file of its own, so a client of an earlier version, which knows
// only `addresses.json`, leaves them as they are.

/// The groups' file, in the home.
pub const GROUPS: &str = "groups.json";

/// A group: a name, and the addresses written to when it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// Its name, as the user gave it.
    pub name: String,
    /// Its members' addresses, in the order they were given.
    pub members: Vec<String>,
}

impl Group {
    /// As the file keeps it.
    #[must_use]
    pub fn json(&self) -> Value {
        json!({"name": self.name, "members": self.members})
    }

    fn from_json(v: &Value) -> Option<Group> {
        Some(Group {
            name: v["name"].as_str()?.to_owned(),
            members: v["members"]
                .as_array()?
                .iter()
                .filter_map(|m| m.as_str().map(str::to_owned))
                .collect(),
        })
    }
}

/// The groups, by name.
#[must_use]
pub fn groups(home: &Path) -> Vec<Group> {
    let mut all: Vec<Group> = fs::read_to_string(home.join(GROUPS))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| {
            v.as_array()
                .map(|a| a.iter().filter_map(Group::from_json).collect())
        })
        .unwrap_or_default();
    all.sort_by_key(|g| g.name.to_lowercase());
    all
}

fn save_groups(home: &Path, all: &[Group]) -> Result<(), Error> {
    let v: Vec<Value> = all.iter().map(Group::json).collect();
    let text = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    crate::store::write(&home.join(GROUPS), text.as_bytes())
}

fn find_group<'a>(all: &'a mut [Group], name: &str) -> Option<&'a mut Group> {
    all.iter_mut()
        .find(|g| g.name.eq_ignore_ascii_case(name.trim()))
}

/// Makes a group, or changes the one named `was`: its name, and its members from typed text
/// (`Name <address>` or addresses, with commas, semicolons or lines between them). A member not
/// on the address list joins it, with the name typed. The groups.
///
/// # Errors
///
/// No name, a name another group has, no members, a member that is not an address, or a file
/// that cannot be written.
pub fn save_group(
    home: &Path,
    was: Option<&str>,
    name: &str,
    typed: &str,
) -> Result<Vec<Group>, Error> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() || name.chars().count() > 64 {
        return Err("a group needs a name, of 64 characters at most".to_owned());
    }
    let written = crate::compose::parse_named(typed)?;
    if written.is_empty() {
        return Err("a group needs at least one address".to_owned());
    }
    let mut all = groups(home);
    let same = |g: &Group| g.name.eq_ignore_ascii_case(&name);
    let renamed_onto = all
        .iter()
        .any(|g| same(g) && !was.is_some_and(|w| g.name.eq_ignore_ascii_case(w)));
    if renamed_onto {
        return Err(format!("there is a group called {name} already"));
    }
    let mut members: Vec<String> = Vec::new();
    for (_, a) in &written {
        if !members.contains(a) {
            members.push(a.clone());
        }
    }
    match was.and_then(|w| find_group(&mut all, w)) {
        Some(g) => {
            g.name = name;
            g.members = members;
        }
        None if was.is_some() => {
            return Err(format!(
                "there is no group called {}",
                was.unwrap_or_default()
            ));
        }
        None => all.push(Group { name, members }),
    }
    // the members join the address list, with the names typed for them
    let mut entries = list(home);
    for (n, a) in written {
        match entries.iter_mut().find(|e| e.address == a) {
            Some(e) => {
                if n.is_some() {
                    e.name = n;
                }
            }
            None => entries.push(Entry {
                address: a,
                name: n,
                used: 0,
                uses: 0,
            }),
        }
    }
    save(home, &entries)?;
    save_groups(home, &all)?;
    Ok(groups(home))
}

/// Takes a group away (its members stay on the address list): what it was, and the groups.
///
/// # Errors
///
/// There is no such group, or the file cannot be written.
pub fn remove_group(home: &Path, name: &str) -> Result<(Group, Vec<Group>), Error> {
    let mut all = groups(home);
    let at = all
        .iter()
        .position(|g| g.name.eq_ignore_ascii_case(name.trim()))
        .ok_or_else(|| format!("there is no group called {}", name.trim()))?;
    let gone = all.remove(at);
    save_groups(home, &all)?;
    Ok((gone, groups(home)))
}

/// Puts back a group as it was: what Undo does after [`remove_group`].
///
/// # Errors
///
/// The file cannot be written.
pub fn put_back_group(home: &Path, group: Group) -> Result<Vec<Group>, Error> {
    let mut all = groups(home);
    all.retain(|g| !g.name.eq_ignore_ascii_case(&group.name));
    all.push(group);
    save_groups(home, &all)?;
    Ok(groups(home))
}

/// A group's members as they are written in To, each with its name from the address list.
///
/// # Errors
///
/// There is no such group.
pub fn members(home: &Path, name: &str) -> Result<Vec<(Option<String>, String)>, Error> {
    let all = groups(home);
    let g = all
        .iter()
        .find(|g| g.name.eq_ignore_ascii_case(name.trim()))
        .ok_or_else(|| format!("there is no group called {}", name.trim()))?;
    let entries = list(home);
    Ok(g.members
        .iter()
        .map(|a| {
            let n = entries
                .iter()
                .find(|e| e.address == *a)
                .and_then(|e| e.name.clone());
            (n, a.clone())
        })
        .collect())
}

/// What an import did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    /// Addresses new to the list.
    pub added: usize,
    /// Addresses already there that had no name and took the file's.
    pub named: usize,
    /// Groups made or added to.
    pub groups: usize,
    /// What the file held that is not an email address.
    pub skipped: Vec<String>,
}

impl Imported {
    /// As the panel shows it.
    #[must_use]
    pub fn json(&self) -> Value {
        json!({"added": self.added, "named": self.named, "groups": self.groups,
               "skipped": self.skipped})
    }
}

/// Reads a contact file (vCard or CSV, [`crate::contacts`]) into the address list and the
/// groups: each address new to the list is added with the contact's name, one already there
/// keeps its own name unless it had none, and each group the contact was in gains its
/// addresses. An address that does not check is skipped and said.
///
/// # Errors
///
/// The files cannot be written.
pub fn import(home: &Path, text: &str) -> Result<Imported, Error> {
    let mut done = Imported::default();
    let mut entries = list(home);
    let mut all = groups(home);
    let mut touched: Vec<String> = Vec::new();
    for contact in crate::contacts::read(text) {
        let mut good = Vec::new();
        for e in &contact.emails {
            match crate::compose::checked_address(e) {
                Ok(a) => good.push(a),
                Err(_) => done.skipped.push(e.clone()),
            }
        }
        for a in &good {
            if let Some(e) = entries.iter_mut().find(|e| e.address == *a) {
                if e.name.is_none() && contact.name.is_some() {
                    e.name.clone_from(&contact.name);
                    done.named += 1;
                }
            } else {
                entries.push(Entry {
                    address: a.clone(),
                    name: contact.name.clone(),
                    used: 0,
                    uses: 0,
                });
                done.added += 1;
            }
        }
        for g in &contact.groups {
            if good.is_empty() {
                continue;
            }
            let at = all
                .iter()
                .position(|x| x.name.eq_ignore_ascii_case(g.trim()))
                .unwrap_or_else(|| {
                    all.push(Group {
                        name: g.clone(),
                        members: Vec::new(),
                    });
                    all.len() - 1
                });
            let group = &mut all[at];
            for a in &good {
                if !group.members.contains(a) {
                    group.members.push(a.clone());
                }
            }
            if !touched.iter().any(|t| t.eq_ignore_ascii_case(g)) {
                touched.push(g.clone());
            }
        }
    }
    done.groups = touched.len();
    save(home, &entries)?;
    save_groups(home, &all)?;
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::{
        Entry, Group, add, groups, import, list, members, put_back, put_back_group, remember,
        remove, remove_group, save_group,
    };

    /// A group is made from typed addresses (its members joining the address list with their
    /// names), written to with those names, renamed onto no other group's name, removed and put
    /// back; its members stay on the list when it goes.
    #[test]
    fn groups_of_addresses() {
        let home = std::env::temp_dir().join(format!("radiopost-groups-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("a home");
        add(&home, "Ana <ana@hampost.org>").expect("added");
        assert!(
            save_group(&home, None, " ", "ana@hampost.org").is_err(),
            "a name"
        );
        assert!(save_group(&home, None, "Net", "").is_err(), "members");
        assert!(
            save_group(&home, None, "Net", "not one").is_err(),
            "addresses"
        );
        save_group(
            &home,
            None,
            "Shelter  team",
            "ana@hampost.org, Zoe Lee <zoe@Example.com>; ana@hampost.org",
        )
        .expect("made");
        save_group(&home, None, "Net", "net@radiopost.org").expect("made");
        assert!(
            save_group(&home, None, "shelter team", "x@example.com").is_err(),
            "a name taken"
        );
        assert_eq!(
            groups(&home),
            [
                Group {
                    name: "Net".into(),
                    members: vec!["net@radiopost.org".into()],
                },
                Group {
                    name: "Shelter team".into(),
                    members: vec!["ana@hampost.org".into(), "zoe@example.com".into()],
                },
            ]
        );
        assert_eq!(
            members(&home, "SHELTER team").expect("members"),
            [
                (Some("Ana".into()), "ana@hampost.org".into()),
                (Some("Zoe Lee".into()), "zoe@example.com".into()),
            ]
        );
        assert!(
            save_group(&home, Some("Net"), "Shelter team", "a@example.com").is_err(),
            "renamed onto another"
        );
        save_group(&home, Some("Net"), "Tuesday net", "net@radiopost.org").expect("renamed");
        let (gone, left) = remove_group(&home, "tuesday NET").expect("removed");
        assert_eq!(left.len(), 1);
        assert!(list(&home).iter().any(|e| e.address == "net@radiopost.org"));
        put_back_group(&home, gone).expect("put back");
        assert_eq!(groups(&home).len(), 2);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A contact file adds its addresses with their names, names an address that had none,
    /// leaves a name already there, makes and fills groups from its categories, and says what
    /// it skipped.
    #[test]
    fn importing_contacts() {
        let home = std::env::temp_dir().join(format!("radiopost-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("a home");
        add(&home, "Ana P. <ana@hampost.org>").expect("added");
        add(&home, "zoe@example.com").expect("added");
        save_group(&home, None, "Net", "net@radiopost.org").expect("made");
        let vcf = "BEGIN:VCARD\nFN:Ana Perez\nEMAIL:ana@hampost.org\nCATEGORIES:Shelter team\n\
            END:VCARD\nBEGIN:VCARD\nFN:Zoe Lee\nEMAIL:zoe@example.com\nEMAIL:not-an-address\n\
            CATEGORIES:Net,Shelter team\nEND:VCARD\nBEGIN:VCARD\nFN:Jim\nEMAIL:jim@Example.NET\n\
            END:VCARD\n";
        let done = import(&home, vcf).expect("imported");
        assert_eq!(
            (done.added, done.named, done.groups, done.skipped),
            (1, 1, 2, vec!["not-an-address".to_owned()])
        );
        let names: Vec<String> = list(&home).iter().map(Entry::written).collect();
        assert_eq!(
            names,
            [
                "Ana P. <ana@hampost.org>",
                "Jim <jim@example.net>",
                "net@radiopost.org",
                "Zoe Lee <zoe@example.com>",
            ]
        );
        let shelter = groups(&home)
            .into_iter()
            .find(|g| g.name == "Shelter team")
            .expect("made");
        assert_eq!(shelter.members, ["ana@hampost.org", "zoe@example.com"]);
        let net = groups(&home)
            .into_iter()
            .find(|g| g.name == "Net")
            .expect("kept");
        assert_eq!(net.members, ["net@radiopost.org", "zoe@example.com"]);
        let _ = std::fs::remove_dir_all(&home);
    }

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
