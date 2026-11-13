// SPDX-License-Identifier: MIT OR Apache-2.0
//! The callsigns this computer works (wishlist W46 and W7): each a credential of its own, with its
//! own account's mail, chosen for the next call from a list.
//!
//! Each callsign is a home of its own, as [`Store`] knows homes: the first is the home itself,
//! as it always was, and each one added is a home under it:
//!
//! ```text
//! <home>/                  the first callsign's home, as before
//! <home>/callsigns/2/      a second callsign's home: its credential, mail and state
//! <home>/callsigns/3/      …
//! <home>/chosen            which one is chosen: its number; none, or one gone, is the first
//! ```
//!
//! What is the computer's and not an account's stays in the first home for all of them: the
//! address list ([`crate::addresses`]) and the log ([`crate::LOG_FILE`]). One device key serves
//! them all (the author's decision, 2026-11-13): a home added takes a copy of the first's, so
//! revoking this computer's key at the post office stops every callsign it held.
//!
//! One session carries one credential (RP-3 §5.1), so a call goes as the callsign chosen, and
//! listening answers that callsign alone.

use std::fs;
use std::path::{Path, PathBuf};

use radiopost::credential::Body;
use serde_json::{Value, json};

use crate::store::{Error, Store, write};

/// Where the homes of the callsigns added are kept, in the first home.
pub const DIR: &str = "callsigns";

/// The file in the first home that names the callsign chosen.
const CHOSEN: &str = "chosen";

/// The first callsign's number: its home is the home itself.
pub const FIRST: u32 = 1;

/// One callsign's home: its number, where it is, and what it holds of an identity.
#[derive(Clone, Debug)]
pub struct Entry {
    /// `1` for the first home, else the number of its directory under [`DIR`].
    pub id: u32,
    /// The home.
    pub home: PathBuf,
    /// The credential's body: the account and callsign, and when it runs out.
    pub me: Option<Body>,
    /// An application waiting for the post office's operators (RP-5 §6.7), as the home keeps it.
    pub application: Option<Value>,
    /// Whether it is the one chosen.
    pub chosen: bool,
}

impl Entry {
    /// As the panel shows it: `id`, `address`, `operator`, `expires`, `application` (its
    /// `address`, `operator` and `status`) and `chosen`.
    #[must_use]
    pub fn json(&self) -> Value {
        let mut v = json!({"id": self.id, "chosen": self.chosen});
        if let Some(me) = &self.me {
            v["address"] = me.address.clone().into();
            v["operator"] = me.operator.clone().into();
            v["expires"] = me.expires.into();
        }
        if let Some(a) = &self.application {
            v["application"] = json!({
                "address": a["address"], "operator": a["operator"], "status": a["status"],
            });
        }
        v
    }

    /// How a person names it: `ADDRESS · CALLSIGN`, the callsign applied for, or a new one.
    #[must_use]
    pub fn label(&self) -> String {
        if let Some(me) = &self.me {
            return format!("{} · {}", me.address, me.operator);
        }
        if let Some(op) = self
            .application
            .as_ref()
            .and_then(|a| a["operator"].as_str())
        {
            return format!("{op} (applied for)");
        }
        "a new callsign, not yet enrolled".into()
    }
}

/// The home of callsign `id`.
#[must_use]
pub fn home_of(base: &Path, id: u32) -> PathBuf {
    if id == FIRST {
        base.to_owned()
    } else {
        base.join(DIR).join(id.to_string())
    }
}

/// The numbers of the homes added, in order.
fn added(base: &Path) -> Vec<u32> {
    let mut ids: Vec<u32> = fs::read_dir(base.join(DIR))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str()?.parse().ok())
        .filter(|&n| n > FIRST)
        .collect();
    ids.sort_unstable();
    ids
}

/// The number of the callsign chosen: what [`CHOSEN`] says, when its home is still there; else
/// the first.
#[must_use]
pub fn chosen_id(base: &Path) -> u32 {
    fs::read_to_string(base.join(CHOSEN))
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .filter(|&n| n == FIRST || added(base).contains(&n))
        .unwrap_or(FIRST)
}

/// The home of the callsign chosen: the one a command or the panel works on.
#[must_use]
pub fn chosen(base: &Path) -> PathBuf {
    home_of(base, chosen_id(base))
}

/// Every callsign's home, the first first.
#[must_use]
pub fn list(base: &Path) -> Vec<Entry> {
    let chosen = chosen_id(base);
    std::iter::once(FIRST)
        .chain(added(base))
        .map(|id| {
            let home = home_of(base, id);
            let store = Store::open(&home).ok();
            Entry {
                id,
                me: store.as_ref().and_then(|s| s.me().ok()),
                application: store.and_then(|s| s.application),
                home,
                chosen: id == chosen,
            }
        })
        .collect()
}

/// Chooses callsign `id` for what comes next.
///
/// # Errors
///
/// There is no such callsign, or the choice cannot be written.
pub fn choose(base: &Path, id: u32) -> Result<(), Error> {
    if id != FIRST && !added(base).contains(&id) {
        return Err(format!("there is no callsign {id} on this computer"));
    }
    fs::create_dir_all(base).map_err(|e| format!("cannot make {}: {e}", base.display()))?;
    write(&base.join(CHOSEN), id.to_string().as_bytes())
}

/// Makes a home for another callsign and chooses it, to be enrolled or applied for there. It
/// takes this computer's device key (made in the first home now if there is none), the post
/// offices the first trusts and its grid square; not the first's post office, which another
/// account's domain may not share. A home added before and still empty (no credential, no
/// application, no mail) is chosen again rather than another made. Returns its number.
///
/// # Errors
///
/// A home cannot be made or written.
pub fn add(base: &Path) -> Result<u32, Error> {
    let first = Store::open(base)?;
    for e in list(base).into_iter().filter(|e| e.id != FIRST) {
        if e.me.is_none() && e.application.is_none() && holds_nothing(&e.home) {
            choose(base, e.id)?;
            return Ok(e.id);
        }
    }
    let key = if let Ok(k) = first.device() {
        k
    } else {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|e| e.to_string())?;
        let k = ed25519_dalek::SigningKey::from_bytes(&seed);
        first.set_device(&k)?;
        k
    };
    let id = added(base).last().map_or(FIRST + 1, |n| n + 1);
    let home = home_of(base, id);
    let mut store = Store::open(&home)?;
    store.set_device(&key)?;
    for (domain, keys) in first.trust()?.keys {
        for k in keys {
            store.add_trust(&domain, &k)?;
        }
    }
    store.grid.clone_from(&first.grid);
    store.save()?;
    choose(base, id)?;
    Ok(id)
}

/// Whether a home holds no mail and nothing still to report: removing it loses nothing but its
/// credential.
fn holds_nothing(home: &Path) -> bool {
    Store::open(home).is_ok_and(|s| {
        s.outbox.is_empty()
            && s.inbox.is_empty()
            && s.sent.is_empty()
            && s.trash.is_empty()
            && s.reports.is_empty()
            && s.partials().is_empty()
    })
}

/// Takes callsign `id` off this computer: its home and its credential, which the post office
/// still honours until it runs out or is revoked. Not the first, whose home holds the others, and
/// not while it holds mail or a session to report: those are deleted first, on purpose. The first
/// is chosen when it was.
///
/// # Errors
///
/// It is the first, or not there, or holds mail, or cannot be removed.
pub fn remove(base: &Path, id: u32) -> Result<(), Error> {
    if id == FIRST {
        return Err("the first callsign's home holds the others: it stays".into());
    }
    if !added(base).contains(&id) {
        return Err(format!("there is no callsign {id} on this computer"));
    }
    let home = home_of(base, id);
    if !holds_nothing(&home) {
        return Err(
            "it still holds mail or sessions to report: empty its folders and trash first".into(),
        );
    }
    let was_chosen = chosen_id(base) == id;
    fs::remove_dir_all(&home).map_err(|e| format!("cannot remove {}: {e}", home.display()))?;
    if was_chosen {
        choose(base, FIRST)?;
    }
    Ok(())
}

/// The callsign `who` names: its number in [`list`], or a callsign or an account's address as its
/// credential or application has it, any case.
///
/// # Errors
///
/// None has it, or more than one does.
pub fn find(base: &Path, who: &str) -> Result<Entry, Error> {
    let who = who.trim();
    let names = |e: &Entry| -> Vec<String> {
        let mut n = vec![e.id.to_string()];
        if let Some(me) = &e.me {
            n.extend([me.operator.clone(), me.address.clone()]);
        }
        if let Some(a) = &e.application {
            n.extend(
                ["operator", "address"]
                    .iter()
                    .filter_map(|k| a[*k].as_str().map(str::to_owned)),
            );
        }
        n
    };
    let mut found: Vec<Entry> = list(base)
        .into_iter()
        .filter(|e| names(e).iter().any(|n| n.eq_ignore_ascii_case(who)))
        .collect();
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(format!(
            "no callsign or address {who} on this computer: `radiopost callsigns` lists them"
        )),
        _ => Err(format!(
            "{who} names more than one: give the account's address, or the number \
             `radiopost callsigns` lists"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{FIRST, add, choose, chosen, chosen_id, find, home_of, list, remove};
    use crate::store::Store;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("radiopost-callsigns-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A home as one is today is the first callsign, chosen, and nothing in it moves.
    #[test]
    fn a_home_of_today_is_the_first_callsign() {
        let base = scratch("today");
        Store::open(&base).expect("a home");
        assert_eq!(chosen(&base), base);
        let all = list(&base);
        assert_eq!(all.len(), 1);
        assert_eq!((all[0].id, all[0].chosen), (FIRST, true));
        assert_eq!(all[0].label(), "a new callsign, not yet enrolled");
        assert!(!base.join("callsigns").exists(), "nothing made");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A callsign added has a home of its own with the first's device key, trust and grid, is
    /// chosen, and is chosen again rather than another made while it is empty.
    #[test]
    fn a_callsign_added_takes_the_computers_key_and_is_chosen() {
        let base = scratch("add");
        let mut first = Store::open(&base).expect("a home");
        first.grid = Some("EM73tt".into());
        first.post_office = Some("https://po.example".into());
        first.save().expect("saved");
        first
            .add_trust(
                "radiopost.org",
                &radiopost::credential::PostOfficeKey {
                    public: [7; 32],
                    not_before: 0,
                    not_after: u64::MAX,
                },
            )
            .expect("trusted");
        let id = add(&base).expect("added");
        assert_eq!(id, 2);
        assert_eq!(chosen_id(&base), 2);
        assert_eq!(chosen(&base), base.join("callsigns").join("2"));
        let first = Store::open(&base).expect("the first");
        let second = Store::open(&home_of(&base, 2)).expect("the second");
        let public = |s: &Store| s.device().expect("a key").verifying_key().to_bytes();
        assert_eq!(public(&second), public(&first), "one device key");
        assert_eq!(
            second.trust().expect("trust").keys,
            first.trust().expect("trust").keys
        );
        assert_eq!(second.grid.as_deref(), Some("EM73tt"));
        assert_eq!(second.post_office, None, "another account's post office");
        // empty, it is chosen again rather than another made
        choose(&base, FIRST).expect("chosen");
        assert_eq!(add(&base).expect("added"), 2);
        assert_eq!(list(&base).len(), 2);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The choice is kept; one whose home has gone falls back to the first.
    #[test]
    fn a_choice_whose_home_has_gone_is_the_first() {
        let base = scratch("gone");
        Store::open(&base).expect("a home");
        add(&base).expect("added");
        assert!(choose(&base, 5).is_err(), "no such callsign");
        assert_eq!(chosen_id(&base), 2);
        std::fs::remove_dir_all(home_of(&base, 2)).expect("removed by hand");
        assert_eq!(chosen_id(&base), FIRST);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Removing: never the first, never a home with mail in it; the first is chosen again.
    #[test]
    fn a_callsign_is_removed_only_when_it_holds_no_mail() {
        let base = scratch("remove");
        Store::open(&base).expect("a home");
        assert!(remove(&base, FIRST).is_err(), "the first stays");
        let id = add(&base).expect("added");
        let mut second = Store::open(&home_of(&base, id)).expect("the second");
        second.reports.push(serde_json::json!({"session": "x"}));
        second.save().expect("saved");
        let held = remove(&base, id).expect_err("a session to report");
        assert!(held.contains("empty its folders"), "{held}");
        second.reports.clear();
        second.save().expect("saved");
        remove(&base, id).expect("removed");
        assert!(!home_of(&base, id).exists());
        assert_eq!(chosen_id(&base), FIRST);
        assert!(remove(&base, id).is_err(), "gone");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// `--as` finds a callsign by its number or what its application names, in any case; an
    /// unknown one is refused.
    #[test]
    fn a_callsign_is_found_by_callsign_or_address() {
        let base = scratch("find");
        Store::open(&base).expect("a home");
        let id = add(&base).expect("added");
        let mut second = Store::open(&home_of(&base, id)).expect("the second");
        second.application = Some(serde_json::json!({
            "address": "club@radiopost.org", "operator": "WD5EMA", "status": "waiting",
        }));
        second.save().expect("saved");
        assert_eq!(
            find(&base, "wd5ema").expect("found").home,
            home_of(&base, id)
        );
        assert_eq!(find(&base, "CLUB@radiopost.org").expect("found").id, id);
        assert_eq!(find(&base, "2").expect("by its number").id, id);
        assert_eq!(find(&base, "1").expect("the first").home, base);
        assert!(find(&base, "N0CALL").is_err());
        assert_eq!(list(&base)[1].label(), "WD5EMA (applied for)");
        let _ = std::fs::remove_dir_all(&base);
    }
}
