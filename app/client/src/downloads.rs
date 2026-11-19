// SPDX-License-Identifier: MIT OR Apache-2.0
//! A received message's attachments saved as files (RP-2 §2, key 8), for the operator to open
//! with whatever program they choose.
//!
//! A file's name comes from the other station, so it is made safe before it is used: its last
//! part only, no character a file system refuses or a shell would read, no name Windows keeps
//! for a device, no leading dot, not too long. A file already there is never written over: the
//! new one takes ` (2)`, ` (3)` and so on before its extension. On Windows the file is marked as
//! coming from outside, as a browser marks what it downloads, so the system warns before running
//! it and opens a document in its protected view.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use radiopost::cbor::Value;
use radiopost::message::{Message, key};

/// The most characters a saved file's name keeps, its extension among them.
const LONGEST: usize = 120;

/// The names Windows keeps for devices, whatever the extension.
const DEVICES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// The usual folder for saved attachments: `Downloads` in the user's home folder (or in
/// `fallback` when there is no home folder).
#[must_use]
pub fn folder(fallback: &Path) -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| fallback.to_owned(), PathBuf::from)
        .join("Downloads")
}

/// A file name from another station, made safe to save under: see the module's comment.
#[must_use]
pub fn safe_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or("");
    let kept: String = last
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let kept = kept
        .trim()
        .trim_start_matches('.')
        .trim_end_matches(['.', ' ']);
    let (stem, ext) = split(kept);
    let device = DEVICES.iter().any(|d| d.eq_ignore_ascii_case(stem.trim()));
    let stem = if device {
        format!("_{stem}")
    } else {
        stem.to_owned()
    };
    // the extension kept whole when the name is too long: it says what opens the file
    let ext: String = ext.chars().take(LONGEST / 4).collect();
    let room = LONGEST.saturating_sub(ext.chars().count() + 1).max(1);
    let stem: String = stem.chars().take(room).collect();
    match (stem.trim().is_empty(), ext.is_empty()) {
        (true, true) => "attachment".to_owned(),
        (true, false) => format!("attachment.{ext}"),
        (false, true) => stem,
        (false, false) => format!("{stem}.{ext}"),
    }
}

/// A name's stem and extension: `report.final.pdf` is `report.final` and `pdf`.
fn split(name: &str) -> (&str, &str) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => (stem, ext),
        _ => (name, ""),
    }
}

/// The `n`th attachment of `message` (counted from 0, as key 8 lists them) saved in `folder`,
/// which is made if it is missing: where it went.
///
/// # Errors
///
/// The message has no such attachment, or the file cannot be written: why, in the user's words.
pub fn save(message: &Message, n: usize, folder: &Path) -> Result<PathBuf, String> {
    let file = message
        .map()
        .get(key::FILES)
        .and_then(Value::as_array)
        .and_then(|files| files.get(n))
        .ok_or("the message has no such attachment")?;
    let name = file.get(1).and_then(Value::as_text).unwrap_or("");
    let data = file
        .get(3)
        .and_then(Value::as_bytes)
        .ok_or("the attachment holds no data")?;
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let name = safe_name(name);
    let (stem, ext) = split(&name);
    for i in 1..=999 {
        let tried = match (i, ext.is_empty()) {
            (1, _) => name.clone(),
            (_, true) => format!("{stem} ({i})"),
            (_, false) => format!("{stem} ({i}).{ext}"),
        };
        let path = folder.join(&tried);
        // never over a file already there, even one made a moment ago
        let mut out = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(out) => out,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        out.write_all(data)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        mark_as_from_outside(&path);
        return Ok(path);
    }
    Err(format!(
        "{}: 999 files of that name are there already",
        folder.join(&name).display()
    ))
}

/// Windows's mark of a file from outside (the Internet zone), as browsers write it; nothing
/// elsewhere. A file system without it (FAT) keeps the file unmarked.
fn mark_as_from_outside(path: &Path) {
    if cfg!(windows) {
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        let _ = std::fs::write(stream, "[ZoneTransfer]\r\nZoneId=3\r\n");
    }
}

#[cfg(test)]
mod tests {
    use super::safe_name;

    #[test]
    fn a_name_from_another_station_is_made_safe() {
        for (given, kept) in [
            ("grid.txt", "grid.txt"),
            ("Shelter roster 3.pdf", "Shelter roster 3.pdf"),
            ("../../.ssh/authorized_keys", "authorized_keys"),
            ("C:\\Windows\\System32\\evil.dll", "evil.dll"),
            ("..", "attachment"),
            ("", "attachment"),
            (".bashrc", "bashrc"),
            ("report.", "report"),
            ("a:b?c*.txt", "a_b_c_.txt"),
            ("tab\there.txt", "tab_here.txt"),
            ("CON", "_CON"),
            ("con.txt", "_con.txt"),
            ("lpt1.log", "_lpt1.log"),
            ("console.txt", "console.txt"),
            (".pdf", "pdf"),
            ("photo.JPEG", "photo.JPEG"),
        ] {
            assert_eq!(safe_name(given), kept, "{given:?}");
        }
        let long = format!("{}.pdf", "x".repeat(300));
        let kept = safe_name(&long);
        assert_eq!(kept.chars().count(), 120);
        assert!(kept.ends_with("x.pdf"), "{kept}");
    }
}
