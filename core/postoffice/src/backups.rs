// SPDX-License-Identifier: MIT OR Apache-2.0
//! Backups of a post office (wishlist W38, design D35): what cannot be made again.
//!
//! A backup is a zip of the post office's directory less what comes back by itself: its key
//! (`key.hex`, which signs every credential: lose it and every user enrolls again), its settings,
//! its state (accounts, mailboxes, applications), and the attachments held for the radio. The
//! FCC copy and the program are fetched again; the mail server keeps its own mailboxes.
//!
//! The service writes one each night beside itself (`backups/`, the last seven), which guards
//! against a mistake or a broken disk but not a lost server. For that the operator downloads one
//! from the desk to their own computer, sealed with a passphrase they choose: PBKDF2-HMAC-SHA256
//! (600 000 rounds) makes the key, ChaCha20-Poly1305 seals the zip, so a stolen file is useless
//! without the passphrase and a changed one does not open. `radiopost-po restore` opens either.

use std::io::{Cursor, Read as _, Write as _};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};

/// The first bytes of a sealed backup.
const MAGIC: &[u8] = b"RADIOPOST-BACKUP-1\n";
/// PBKDF2's rounds: OWASP's 2023 figure for HMAC-SHA256.
const ROUNDS: u32 = 600_000;
/// How many nightly backups are kept.
pub const KEEP: usize = 7;
/// The shortest passphrase taken.
pub const MIN_PASSPHRASE: usize = 12;

/// What a backup holds: these files, and every file under these directories.
const FILES: [&str; 3] = ["key.hex", "config.json", "state.json"];
const DIRS: [&str; 1] = ["held"];

/// The post office in `dir` as a zip.
///
/// # Errors
///
/// A file could not be read, or the post office has no key.
pub fn archive(dir: &Path) -> Result<Vec<u8>, String> {
    if !dir.join("key.hex").is_file() {
        return Err(format!("{} holds no post office", dir.display()));
    }
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o600);
    let mut add = |name: &str, path: &Path| -> Result<(), String> {
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        zip.start_file(name, options).map_err(|e| e.to_string())?;
        zip.write_all(&data).map_err(|e| e.to_string())
    };
    for name in FILES {
        let path = dir.join(name);
        if path.is_file() {
            add(name, &path)?;
        }
    }
    for d in DIRS {
        let Ok(entries) = std::fs::read_dir(dir.join(d)) else {
            continue;
        };
        let mut names: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        names.sort();
        for path in names.iter().filter(|p| p.is_file()) {
            let file = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            add(&format!("{d}/{file}"), path)?;
        }
    }
    let note = format!(
        "A backup of a RadioPost post office, by radiopost-po {}.\n\
         key.hex signs every credential it issued: keep this file as you would a password.\n\
         Restore with: radiopost-po --dir DIR restore FILE\n",
        env!("CARGO_PKG_VERSION")
    );
    zip.start_file("README.txt", options.unix_permissions(0o644))
        .map_err(|e| e.to_string())?;
    zip.write_all(note.as_bytes()).map_err(|e| e.to_string())?;
    zip.finish()
        .map(Cursor::into_inner)
        .map_err(|e| e.to_string())
}

fn key_of(passphrase: &str, salt: &[u8]) -> Result<LessSafeKey, String> {
    let mut key = [0u8; 32];
    let rounds = NonZeroU32::new(ROUNDS).ok_or("no rounds")?;
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        rounds,
        salt,
        passphrase.as_bytes(),
        &mut key,
    );
    UnboundKey::new(&CHACHA20_POLY1305, &key)
        .map(LessSafeKey::new)
        .map_err(|_| "the key could not be made".to_owned())
}

/// `zip` sealed with `passphrase`: the magic line, 16 bytes of salt, a 12-byte nonce, then the
/// sealed zip and its tag.
///
/// # Errors
///
/// A passphrase shorter than [`MIN_PASSPHRASE`], or no random numbers.
pub fn seal(zip: &[u8], passphrase: &str) -> Result<Vec<u8>, String> {
    if passphrase.chars().count() < MIN_PASSPHRASE {
        return Err(format!(
            "a passphrase of {MIN_PASSPHRASE} characters or more"
        ));
    }
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut salt).map_err(|e| e.to_string())?;
    getrandom::fill(&mut nonce).map_err(|e| e.to_string())?;
    let key = key_of(passphrase, &salt)?;
    let mut data = zip.to_vec();
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::from(MAGIC),
        &mut data,
    )
    .map_err(|_| "the backup could not be sealed".to_owned())?;
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&data);
    Ok(out)
}

/// The zip in a backup: a sealed one opened with `passphrase`, a plain one as it is.
///
/// # Errors
///
/// A sealed backup whose passphrase is wrong or whose bytes changed, or neither kind.
pub fn open(file: &[u8], passphrase: Option<&str>) -> Result<Vec<u8>, String> {
    if file.starts_with(b"PK") {
        return Ok(file.to_vec());
    }
    let rest = file.strip_prefix(MAGIC).ok_or("not a RadioPost backup")?;
    if rest.len() < 28 + 16 {
        return Err("the backup is cut short".into());
    }
    let passphrase = passphrase.ok_or("the backup is sealed: give its passphrase")?;
    let (salt, rest) = rest.split_at(16);
    let (nonce, sealed) = rest.split_at(12);
    let key = key_of(passphrase, salt)?;
    let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| "a bad nonce")?;
    let mut data = sealed.to_vec();
    let opened = key
        .open_in_place(nonce, Aad::from(MAGIC), &mut data)
        .map_err(|_| "the passphrase is wrong, or the file has changed".to_owned())?;
    Ok(opened.to_vec())
}

/// Puts a backup's files into `dir`. Refuses a directory that holds a post office already,
/// unless `over`. Returns the files written.
///
/// # Errors
///
/// Not a backup, a name that would leave the directory, or a file that could not be written.
pub fn restore(zip: &[u8], dir: &Path, over: bool) -> Result<Vec<String>, String> {
    if dir.join("key.hex").exists() && !over {
        return Err(format!(
            "{} holds a post office already: stop it, and give --force to put the backup over it",
            dir.display()
        ));
    }
    let mut archive =
        zip::ZipArchive::new(Cursor::new(zip)).map_err(|e| format!("not a backup: {e}"))?;
    let mut written = Vec::new();
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = f.name().to_owned();
        let allowed = FILES.contains(&name.as_str())
            || DIRS.iter().any(|d| {
                name.strip_prefix(&format!("{d}/"))
                    .is_some_and(|n| !n.is_empty() && !n.contains(['/', '\\']) && n != "..")
            });
        if !allowed {
            continue;
        }
        let mut data = Vec::new();
        f.read_to_end(&mut data).map_err(|e| e.to_string())?;
        let path = dir.join(&name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, &data).map_err(|e| format!("{}: {e}", path.display()))?;
        private(&path);
        written.push(name);
    }
    if !written.iter().any(|n| n == "key.hex") {
        return Err("the backup holds no key.hex".into());
    }
    Ok(written)
}

/// Readable by the post office's own user only.
fn private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Writes tonight's backup into `dir/backups/`, named by `day` (`YYYY-MM-DD`), and keeps the
/// last [`KEEP`]. Returns its path.
///
/// # Errors
///
/// The backup could not be made or written.
pub fn nightly(dir: &Path, day: &str) -> Result<PathBuf, String> {
    let folder = dir.join("backups");
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let path = folder.join(format!("radiopost-{day}.zip"));
    let part = path.with_extension("part");
    std::fs::write(&part, archive(dir)?).map_err(|e| e.to_string())?;
    private(&part);
    std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
    let mut kept = list(dir);
    kept.sort_by(|a, b| b.0.cmp(&a.0));
    for (name, _, _) in kept.into_iter().skip(KEEP) {
        let _ = std::fs::remove_file(folder.join(name));
    }
    Ok(path)
}

/// The nightly backups: name, size, when written (Unix seconds), newest first.
#[must_use]
pub fn list(dir: &Path) -> Vec<(String, u64, u64)> {
    let mut out: Vec<(String, u64, u64)> = std::fs::read_dir(dir.join("backups"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_owned();
            let m = e.metadata().ok()?;
            let at = m
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_secs();
            let zip = Path::new(&name)
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("zip"));
            zip.then_some((name, m.len(), at))
        })
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn office(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("radiopost-backup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("held")).expect("a directory");
        std::fs::write(dir.join("key.hex"), "07".repeat(32)).expect("written");
        std::fs::write(dir.join("config.json"), "{}").expect("written");
        std::fs::write(dir.join("state.json"), "{\"next_serial\": 9}").expect("written");
        std::fs::write(dir.join("held/ab12"), b"an attachment").expect("written");
        std::fs::write(dir.join("unrelated.log"), b"not kept").expect("written");
        dir
    }

    #[test]
    fn a_sealed_backup_opens_only_with_its_passphrase_and_restores_whole() {
        let dir = office("seal");
        let zip = archive(&dir).expect("made");
        let sealed = seal(&zip, "correct horse battery").expect("sealed");
        assert!(sealed.starts_with(MAGIC));
        assert!(seal(&zip, "short").is_err());
        assert!(open(&sealed, Some("wrong horse battery")).is_err());
        let mut bent = sealed.clone();
        let last = bent.len() - 1;
        bent[last] ^= 1;
        assert!(open(&bent, Some("correct horse battery")).is_err());
        let opened = open(&sealed, Some("correct horse battery")).expect("opened");
        assert_eq!(opened, zip);
        let to = office("restore-to");
        std::fs::remove_file(to.join("key.hex")).expect("removed");
        std::fs::write(to.join("state.json"), "{}").expect("written");
        let written = restore(&opened, &to, false).expect("restored");
        assert_eq!(
            written,
            ["key.hex", "config.json", "state.json", "held/ab12"]
        );
        assert_eq!(
            std::fs::read_to_string(to.join("state.json")).expect("read"),
            "{\"next_serial\": 9}"
        );
        assert!(
            restore(&opened, &to, false).is_err(),
            "a post office is there now"
        );
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(to);
    }

    #[test]
    fn nightly_backups_keep_a_week() {
        let dir = office("nightly");
        for d in 1..=9 {
            nightly(&dir, &format!("2026-11-{d:02}")).expect("written");
        }
        let names: Vec<String> = list(&dir).into_iter().map(|(n, _, _)| n).collect();
        assert_eq!(names.len(), KEEP);
        assert_eq!(names[0], "radiopost-2026-11-09.zip");
        assert_eq!(names[KEEP - 1], "radiopost-2026-11-03.zip");
        let zip = std::fs::read(dir.join("backups").join(&names[0])).expect("read");
        assert!(
            open(&zip, None).is_ok(),
            "a plain zip opens without a passphrase"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
