// SPDX-License-Identifier: MIT OR Apache-2.0
//! The callers the gateway's operator refuses (wishlist W24): an account by its address, or a
//! station by its callsign, with a note of why for the operator alone.
//!
//! They are kept in `refused.json` beside the settings, not in them: the settings are held while
//! the gateway answers, and an operator who sees a station misuse the gateway wants it refused at
//! once. Each session reads the list when the caller's link comes up, so a change applies from
//! the next call. The engine answers a refused caller's `HELLO` with `refused-by-policy` and a
//! text saying so (RP-3 §5.9, RP-0 §9.3), before anything reaches a post office.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The file, beside `gateway.json`.
pub const FILE: &str = "refused.json";

/// One caller refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refused {
    /// An address (`ana@example.org`) or a callsign; a callsign without an SSID refuses every
    /// SSID of it.
    pub caller: String,
    /// Why, for the operator: never sent to the caller.
    #[serde(default)]
    pub note: String,
    /// When it was added, Unix seconds.
    #[serde(default)]
    pub at: u64,
}

/// The callers refused, oldest first.
#[must_use]
pub fn list(dir: &Path) -> Vec<Refused> {
    std::fs::read_to_string(dir.join(FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// What the engine is given: each entry's caller.
#[must_use]
pub fn callers(dir: &Path) -> Vec<String> {
    list(dir).into_iter().map(|r| r.caller).collect()
}

fn save(dir: &Path, list: &[Refused]) -> Result<(), String> {
    let text = serde_json::to_string_pretty(list).map_err(|e| e.to_string())?;
    crate::station::write(dir, FILE, &text)
}

/// A caller as the list keeps it: an address in lower case, a callsign in upper case.
///
/// # Errors
///
/// Neither an address nor a callsign.
pub fn normal(caller: &str) -> Result<String, String> {
    let caller = caller.trim();
    if let Some((local, domain)) = caller.rsplit_once('@') {
        if local.is_empty() || !domain.contains('.') || caller.contains(char::is_whitespace) {
            return Err(format!(
                "{caller:?} is not an address such as ana@example.org"
            ));
        }
        return Ok(radiopost::address::lowercase(caller));
    }
    let call = caller.to_uppercase();
    if crate::panel::callsign_ok(&call) {
        Ok(call)
    } else {
        Err(format!(
            "{caller:?} is neither an address nor a callsign such as N0CALL or N0CALL-5"
        ))
    }
}

/// Refuses `caller` from the next call on, with `note`; a caller already refused keeps its place
/// and takes the new note. What it says of it.
///
/// # Errors
///
/// Not an address or a callsign, or the file could not be written.
pub fn add(dir: &Path, caller: &str, note: &str, at: u64) -> Result<String, String> {
    let caller = normal(caller)?;
    let mut list = list(dir);
    let note = note.trim().to_owned();
    match list.iter_mut().find(|r| r.caller == caller) {
        Some(r) => r.note = note,
        None => list.push(Refused {
            caller: caller.clone(),
            note,
            at,
        }),
    }
    save(dir, &list)?;
    Ok(if caller.contains('@') || caller.contains('-') {
        format!("refusing {caller} from the next call on")
    } else {
        format!("refusing {caller}, every SSID of it, from the next call on")
    })
}

/// Takes `caller` off the list: whether it was on it.
///
/// # Errors
///
/// The file could not be written.
pub fn remove(dir: &Path, caller: &str) -> Result<bool, String> {
    let caller = normal(caller).unwrap_or_else(|_| caller.trim().to_owned());
    let mut list = list(dir);
    let before = list.len();
    list.retain(|r| r.caller != caller);
    if list.len() == before {
        return Ok(false);
    }
    save(dir, &list)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{add, callers, list, normal, remove};

    /// Callers are kept as the engine matches them, once each, with the operator's note.
    #[test]
    fn callers_are_kept_once_each_with_a_note() {
        let dir = std::env::temp_dir().join(format!("radiopost-refused-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        assert_eq!(list(&dir), Vec::<super::Refused>::new());
        assert_eq!(
            add(&dir, " n0call ", "sent business mail", 10).expect("added"),
            "refusing N0CALL, every SSID of it, from the next call on"
        );
        add(&dir, "Spam@Example.ORG", "", 11).expect("added");
        add(&dir, "N0CALL", "and again", 12).expect("noted");
        assert_eq!(callers(&dir), ["N0CALL", "spam@example.org"]);
        assert_eq!(list(&dir)[0].note, "and again");
        assert_eq!(list(&dir)[0].at, 10);
        assert!(add(&dir, "not a caller", "", 13).is_err());
        assert!(add(&dir, "ana@nowhere", "", 13).is_err());
        assert_eq!(normal("w1aw-5").as_deref(), Ok("W1AW-5"));
        assert!(remove(&dir, "SPAM@example.org").expect("removed"));
        assert!(!remove(&dir, "spam@example.org").expect("gone already"));
        assert_eq!(callers(&dir), ["N0CALL"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
