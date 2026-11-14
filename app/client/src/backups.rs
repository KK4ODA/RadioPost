// SPDX-License-Identifier: MIT OR Apache-2.0
//! The post office's backups, fetched by its operator's computer by itself (wishlist W38; the
//! author's choice, 2026-11-14).
//!
//! A backup that stays on the server is lost with it, and one downloaded by hand is one that is
//! forgotten. So an operator turns this on once, on a computer that runs the RadioPost app: the
//! app makes an X25519 key pair (`radiopost_api::sealing`), keeps the private half in
//! [`KEY_FILE`] in the home, and registers the public half with the post office
//! (`/office/backup-keys`). While the app is open it asks every [`EVERY`] whether the server has
//! made a backup it has not fetched, and fetches it sealed to that key, with the newest mailboxes,
//! into the same folder as the backups downloaded by hand. The server keeps nothing secret: the
//! file opens on this computer alone (`radiopost backups open FILE`).
//!
//! The key file is what opens them. Lose the computer and its key, and its backups go with them:
//! the guide says to keep a copy of the key, or another computer of the operator's turned on too.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use radiopost_api::sealing::{self, KeyPair};
use serde_json::{Value, json};

use crate::online;
use crate::store::{Error, Store};

/// The private key, in the home: what opens this computer's backups.
pub const KEY_FILE: &str = "backup.key";
/// What is known of the fetching, in the home.
const STATE_FILE: &str = "backups.json";
/// How often the open app asks.
pub const EVERY: Duration = Duration::from_secs(3600);
/// How many fetched backups are kept: the oldest go, never one downloaded by hand.
pub const KEEP: usize = 10;
/// The largest backup read: the mailboxes grow.
const LIMIT: u64 = 1 << 30;

/// The usual folder for backups: `RadioPost backups` in the user's home folder, where one
/// downloaded from the desk goes too (or in `fallback` when there is no home folder).
#[must_use]
pub fn folder(fallback: &Path) -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| fallback.to_owned(), PathBuf::from)
        .join("RadioPost backups")
}

/// This computer's name, to know it by on the desk.
#[must_use]
pub fn computer_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "this computer".to_owned())
}

fn state(home: &Path) -> Value {
    std::fs::read_to_string(home.join(STATE_FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

/// Where this computer keeps the backups it fetches: the folder chosen when it was turned on,
/// else the usual one ([`folder`]).
#[must_use]
pub fn kept_in(home: &Path) -> PathBuf {
    state(home)["folder"]
        .as_str()
        .filter(|f| !f.is_empty())
        .map_or_else(|| folder(home), PathBuf::from)
}

fn save(home: &Path, state: &Value) -> Result<(), Error> {
    crate::store::write(&home.join(STATE_FILE), state.to_string().as_bytes())
}

/// This computer's key pair: read from the home, or made and kept there, readable by its user
/// alone.
///
/// # Errors
///
/// The key file is not a key, or a new one could not be written.
pub fn key_pair(home: &Path) -> Result<KeyPair, Error> {
    let path = home.join(KEY_FILE);
    if let Ok(text) = std::fs::read_to_string(&path) {
        return KeyPair::from_hex(&text).map_err(|e| format!("{}: {e}", path.display()));
    }
    let keys = KeyPair::generate()?;
    crate::store::write(&path, keys.secret_hex().as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(keys)
}

/// What the panel shows: whether it is on, this computer's key ID and name, the folder, the key
/// file, and the last backup fetched (`name`, `at`, `bytes`, `path`).
#[must_use]
pub fn status(home: &Path) -> Value {
    let s = state(home);
    json!({
        "on": s["on"].as_bool().unwrap_or(false),
        "key": s["key"], "computer": s["computer"],
        "folder": kept_in(home).display().to_string(),
        "key_file": home.join(KEY_FILE).display().to_string(),
        "fetched": s["fetched"],
    })
}

fn refusal(status: u16, answer: &Value) -> String {
    answer["title"].as_str().map_or_else(
        || format!("the post office answered {status}"),
        str::to_owned,
    )
}

/// Turns fetching on: this computer's key registered with the post office as `computer`, the
/// backups kept in `into` (a drive of its own, a folder another program copies elsewhere), or in
/// the usual folder.
///
/// # Errors
///
/// No credential, not one of the post office's operators, or no answer.
pub fn turn_on(
    store: &Store,
    token: &Arc<Mutex<Option<String>>>,
    home: &Path,
    computer: &str,
    into: Option<&Path>,
) -> Result<Value, Error> {
    let keys = key_pair(home)?;
    let sent = json!({"public": radiopost::cbor::hex(&keys.public()), "label": computer});
    let (code, answer) = online::office(store, token, "POST", "/office/backup-keys", Some(&sent))?;
    if code != 200 {
        return Err(refusal(code, &answer));
    }
    let mut s = state(home);
    s["on"] = true.into();
    s["key"] = keys.id().into();
    s["computer"] = computer.into();
    s["folder"] = into.map_or(Value::Null, |f| f.display().to_string().into());
    // what was fetched before stays fetched; a new key fetches the next at once
    s["last"] = Value::Null;
    save(home, &s)?;
    Ok(status(home))
}

/// Turns fetching off: the post office forgets this computer. Backups already fetched stay, and
/// still open here.
///
/// # Errors
///
/// No answer, or the post office refused.
pub fn turn_off(
    store: &Store,
    token: &Arc<Mutex<Option<String>>>,
    home: &Path,
) -> Result<Value, Error> {
    let mut s = state(home);
    if let Some(id) = s["key"].as_str() {
        let (code, answer) = online::office(
            store,
            token,
            "DELETE",
            &format!("/office/backup-keys/{id}"),
            None,
        )?;
        // already forgotten there is forgotten
        if code != 200 && code != 404 {
            return Err(refusal(code, &answer));
        }
    }
    s["on"] = false.into();
    save(home, &s)?;
    Ok(status(home))
}

/// Fetches the server's newest backup if this computer has not got it: what to say of it, or
/// `None` when it is off or there is nothing new. `now` is the clock, Unix seconds.
///
/// # Errors
///
/// No answer, the post office refused, or the file could not be kept. A post office that no
/// longer knows this computer turns fetching off, and says so.
pub fn fetch_if_new(
    store: &Store,
    token: &Arc<Mutex<Option<String>>>,
    home: &Path,
    now: u64,
) -> Result<Option<String>, Error> {
    let mut s = state(home);
    let (true, Some(id)) = (
        s["on"].as_bool().unwrap_or(false),
        s["key"].as_str().map(str::to_owned),
    ) else {
        return Ok(None);
    };
    let (code, listed) = online::office(store, token, "GET", "/office/backups", None)?;
    if code != 200 {
        return Err(refusal(code, &listed));
    }
    // what the server has made since: last night's backup, and the mailboxes' newest, each by its
    // name, when it was written and its size, since a backup made again the same day keeps its name
    let newest = |k: &str| {
        let b = &listed[k][0];
        b["name"]
            .as_str()
            .map_or_else(String::new, |n| format!("{n} {} {}", b["at"], b["bytes"]))
    };
    let stamp = format!("{}|{}", newest("backups"), newest("mail"));
    if stamp == "|" || s["last"].as_str() == Some(stamp.as_str()) {
        return Ok(None);
    }
    let (code, answer) = online::office_reading(
        store,
        token,
        "POST",
        "/office/backup",
        Some(&json!({"key": id})),
        LIMIT,
    )?;
    if code == 404 {
        s["on"] = false.into();
        save(home, &s)?;
        return Err(
            "the post office no longer knows this computer, so its backups stopped: turn them \
             on again on the desk"
                .into(),
        );
    }
    if code != 200 {
        return Err(refusal(code, &answer));
    }
    let data = answer["data"]
        .as_str()
        .and_then(radiopost::base64url::decode)
        .ok_or("the post office's answer held no backup")?;
    if sealing::recipient(&data).as_deref() != Some(id.as_str()) {
        return Err("the post office sent a backup sealed to another key".into());
    }
    let name: String = answer["name"]
        .as_str()
        .unwrap_or("post-office.rpbackup")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect();
    let dir = kept_in(home);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(&name);
    crate::store::write(&path, &data)?;
    // the oldest fetched go; one downloaded by hand never does
    let mut kept: Vec<String> = s["kept"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    kept.retain(|n| *n != name);
    kept.push(name.clone());
    while kept.len() > KEEP {
        let gone = kept.remove(0);
        let _ = std::fs::remove_file(dir.join(gone));
    }
    s["kept"] = json!(kept);
    s["last"] = stamp.into();
    s["fetched"] = json!({"name": name, "at": now, "bytes": data.len(),
                          "path": path.display().to_string(), "mail": answer["mail"]});
    save(home, &s)?;
    Ok(Some(format!(
        "kept the post office's backup on this computer{}: {} ({} kB)",
        if answer["mail"].is_string() {
            ", with the mailboxes"
        } else {
            ""
        },
        path.display(),
        data.len().div_ceil(1000)
    )))
}

/// What a backup fetched by this computer holds: the zip `radiopost-po restore` takes.
///
/// # Errors
///
/// No key on this computer, or a file sealed to another one, or changed.
pub fn open(home: &Path, file: &[u8]) -> Result<Vec<u8>, Error> {
    let path = home.join(KEY_FILE);
    let text = std::fs::read_to_string(&path)
        .map_err(|_| format!("no backup key on this computer ({})", path.display()))?;
    let keys = KeyPair::from_hex(&text)?;
    sealing::open(file, &keys)
}
