// SPDX-License-Identifier: MIT OR Apache-2.0
//! Backups of a post office (wishlist W38, design D35): what cannot be made again.
//!
//! A backup is a zip of the post office's directory less what comes back by itself: its key
//! (`key.hex`, which signs every credential: lose it and every user enrolls again), its settings,
//! its state (accounts, mailboxes, applications), and the attachments held for the radio. The
//! FCC copy and the program are fetched again.
//!
//! The service writes one each night beside itself (`backups/`, the last seven), which guards
//! against a mistake or a broken disk but not a lost server. For that the operator downloads one
//! from the desk to their own computer, sealed with a passphrase they choose: PBKDF2-HMAC-SHA256
//! (600 000 rounds) makes the key, ChaCha20-Poly1305 seals the zip, so a stolen file is useless
//! without the passphrase and a changed one does not open. `radiopost-po restore` opens either.
//! When it was last downloaded is kept (`backups/downloads.json`): a backup that never leaves the
//! server is lost with it (concept §8.5).
//!
//! **The mail server's mailboxes** (the rest of W38): what users read in their mail programs, and
//! the mail server's accounts, passwords and DKIM keys, all in Stalwart's `RocksDB` store. A store
//! copied while it runs is not a backup, and the service may not read it, so the root-owned jobs
//! runner ([`crate::jobs`]) makes one each night: Stalwart stopped, its data archived, Stalwart
//! started again ([`mail_backup`], `backups/mail/`, the last [`MAIL_KEEP`]). A downloaded backup
//! carries the newest, and `radiopost-po restore` puts it back ([`restore_mail`]).

use std::fmt::Write as _;
use std::io::{Cursor, Read as _, Write as _};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value as Json, json};

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

/// Where a backup keeps the mail server's mailboxes.
const MAIL: &str = "mail";

/// The post office in `dir` as a zip; `with_mail`, the newest backup of the mail server's
/// mailboxes too (`mail/`), for a backup that leaves the server. The nightly one beside the
/// service leaves them out: they are beside it already.
///
/// # Errors
///
/// A file could not be read, the post office has no key, or the mailboxes' backup could not be
/// read (a backup that should carry them never leaves without them unsaid).
pub fn archive(dir: &Path, with_mail: bool) -> Result<Vec<u8>, String> {
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
    let mail = if with_mail {
        mail_list(dir).into_iter().next()
    } else {
        None
    };
    if let Some((name, _, _)) = &mail {
        let path = mail_dir(dir).join(name);
        let data = std::fs::read(&path)
            .map_err(|e| format!("the mailboxes' backup {}: {e}", path.display()))?;
        // gzip already: stored as it is
        let stored = options.compression_method(zip::CompressionMethod::Stored);
        zip.start_file(format!("{MAIL}/{name}"), stored)
            .map_err(|e| e.to_string())?;
        zip.write_all(&data).map_err(|e| e.to_string())?;
    }
    let mut note = format!(
        "A backup of a RadioPost post office, by radiopost-po {}.\n\
         key.hex signs every credential it issued: keep this file as you would a password.\n\
         Restore with: radiopost-po --dir DIR restore FILE\n",
        env!("CARGO_PKG_VERSION")
    );
    if let Some((name, _, _)) = &mail {
        let _ = write!(
            note,
            "{MAIL}/{name}: the mail server's mailboxes, accounts, passwords and DKIM keys\n\
             (Stalwart's /var/lib/stalwart and /etc/stalwart, copied while it was stopped).\n\
             restore puts them back too, with the mail server stopped.\n"
        );
    }
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
            owned_like(parent, dir);
        }
        std::fs::write(&path, &data).map_err(|e| format!("{}: {e}", path.display()))?;
        private(&path);
        owned_like(&path, dir);
        written.push(name);
    }
    if !written.iter().any(|n| n == "key.hex") {
        return Err("the backup holds no key.hex".into());
    }
    Ok(written)
}

/// Owned as `dir` is: a restore run as root leaves the files the post office's own, so that its
/// service reads them.
fn owned_like(path: &Path, dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if let Ok(m) = std::fs::metadata(dir) {
            let _ = std::os::unix::fs::chown(path, Some(m.uid()), Some(m.gid()));
        }
    }
    #[cfg(not(unix))]
    let _ = (path, dir);
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
    std::fs::write(&part, archive(dir, false)?).map_err(|e| e.to_string())?;
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

// -- the mail server's mailboxes -------------------------------------------------------------------

/// How many backups of the mailboxes are kept beside the service: fewer than the nightly backups
/// of the post office, since mailboxes grow.
pub const MAIL_KEEP: usize = 3;

/// The hours (UTC) in which the service asks for the night's backup of the mailboxes: 08:00 to
/// 12:00 UTC is 03:00 to 07:00 on the US east coast, midnight to 04:00 on the west. Missed, it
/// waits for the next night.
pub const MAIL_HOURS_UTC: std::ops::Range<u64> = 8..12;

/// The mail server whose mailboxes a backup holds: where they are, and how it stops and starts.
#[derive(Clone, Debug)]
pub struct MailServer {
    /// What `paths` are under: `/` on a server, a stand-in directory in a test.
    pub root: PathBuf,
    /// Its data and settings, under `root`.
    pub paths: Vec<String>,
    /// The command that stops it: a program and its arguments.
    pub stop: Vec<String>,
    /// The command that starts it.
    pub start: Vec<String>,
}

impl MailServer {
    /// Stalwart as the guide installs it (its §5): `RocksDB` in `/var/lib/stalwart` holds the mail,
    /// the accounts and their passwords, the DKIM keys and what was set in its administration;
    /// `/etc/stalwart` holds `config.json`, the environment file and the certificate's copy.
    /// Stalwart's own documentation (*Backup and restore*, 1.x) says a running `RocksDB` directory
    /// copied file by file is not a backup, and, without a file system snapshot, to stop it,
    /// copy the directory with `config.json` and the environment, and start it again.
    #[must_use]
    pub fn stalwart() -> MailServer {
        MailServer {
            root: PathBuf::from("/"),
            paths: vec!["var/lib/stalwart".into(), "etc/stalwart".into()],
            stop: vec!["systemctl".into(), "stop".into(), "stalwart".into()],
            start: vec!["systemctl".into(), "start".into(), "stalwart".into()],
        }
    }
}

fn mail_dir(dir: &Path) -> PathBuf {
    dir.join("backups").join(MAIL)
}

/// Runs a command, a program and its arguments with no shell: whether it succeeded, or why not.
fn run(command: &[String]) -> Result<(), String> {
    let (program, args) = command.split_first().ok_or("no command")?;
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "`{}` failed: {}",
            command.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// What root writes here, readable by the post office's service: the group of the post office's
/// directory, and `mode`.
fn shared(path: &Path, dir: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        if let Ok(m) = std::fs::metadata(dir) {
            let _ = std::os::unix::fs::chown(path, None, Some(m.gid()));
        }
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = (path, dir, mode);
}

/// Backs up the mail server's mailboxes into `dir/backups/mail/`, named by `day` (`YYYY-MM-DD`):
/// the mail server stopped, its data archived with `tar` (owners and permissions kept), and
/// started again whatever happened, about a minute at night: senders retry for days (RFC 5321).
/// Never copied while it runs: when it does not stop, nothing is copied. The archive holds every
/// password and the DKIM keys, so it is root's, readable by the post office's group alone; the
/// last [`MAIL_KEEP`] are kept. Run as root, by the jobs runner. Returns the archive and its size.
///
/// # Errors
///
/// The mail server's data is not there, it did not stop, the archive could not be made, or it
/// did not start again (the archive is kept then).
pub fn mail_backup(dir: &Path, server: &MailServer, day: &str) -> Result<(PathBuf, u64), String> {
    let present: Vec<&String> = server
        .paths
        .iter()
        .filter(|p| server.root.join(p).exists())
        .collect();
    if present.is_empty() {
        return Err(format!(
            "the mail server's data is not under {}: {}",
            server.root.display(),
            server.paths.join(", ")
        ));
    }
    let folder = mail_dir(dir);
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    shared(&dir.join("backups"), dir, 0o750);
    shared(&folder, dir, 0o750);
    let path = folder.join(format!("stalwart-{day}.tar.gz"));
    let part = folder.join(format!("stalwart-{day}.part"));
    let stopped = run(&server.stop);
    let made = stopped.as_ref().map_err(Clone::clone).and_then(|()| {
        let mut tar = vec![
            "tar".to_owned(),
            "-czf".to_owned(),
            part.display().to_string(),
            "-C".to_owned(),
            server.root.display().to_string(),
        ];
        tar.extend(present.iter().map(|p| (*p).clone()));
        run(&tar)
    });
    let started = run(&server.start);
    if let Err(e) = stopped {
        return Err(format!(
            "the mail server did not stop, so nothing was copied: {e}"
        ));
    }
    if let Err(e) = made {
        let _ = std::fs::remove_file(&part);
        return Err(format!("the mailboxes could not be archived: {e}"));
    }
    shared(&part, dir, 0o640);
    std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
    for (name, _, _) in mail_list(dir).into_iter().skip(MAIL_KEEP) {
        let _ = std::fs::remove_file(folder.join(name));
    }
    started.map_err(|e| {
        format!(
            "the mailboxes were backed up ({}), but the mail server did not start again: {e}",
            path.display()
        )
    })?;
    let bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
    Ok((path, bytes))
}

/// The backups of the mailboxes beside the service: name, size, when written (Unix seconds),
/// newest first.
#[must_use]
pub fn mail_list(dir: &Path) -> Vec<(String, u64, u64)> {
    let mut out: Vec<(String, u64, u64)> = std::fs::read_dir(mail_dir(dir))
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
            name.ends_with(".tar.gz").then_some((name, m.len(), at))
        })
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out
}

/// Puts back the mailboxes a backup holds (`mail/`), when it holds them: the mail server
/// stopped, what it had moved aside as `<directory>.before-restore-<now>` (kept, never deleted),
/// the archive unpacked under `server.root` with its owners, and the mail server started again.
/// An archive naming anything outside the mail server's directories is refused before anything
/// is touched. Run as root. Returns what was done, or `None` when the backup holds no mailboxes.
///
/// # Errors
///
/// The archive is not one, names something outside the mail server's directories, the mail
/// server did not stop (nothing is touched then), or the archive could not be unpacked (what was
/// there is put back).
pub fn restore_mail(zip: &[u8], server: &MailServer, now: u64) -> Result<Option<String>, String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(zip)).map_err(|e| format!("not a backup: {e}"))?;
    let mut found = None;
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).map_err(|e| e.to_string())?;
        let Some(name) = f
            .name()
            .strip_prefix(&format!("{MAIL}/"))
            .map(str::to_owned)
        else {
            continue;
        };
        if name.is_empty() || name.contains(['/', '\\']) || !name.ends_with(".tar.gz") {
            continue;
        }
        let mut data = Vec::new();
        f.read_to_end(&mut data).map_err(|e| e.to_string())?;
        found = Some((name, data));
    }
    let Some((name, data)) = found else {
        return Ok(None);
    };
    // the archive holds every password: a file of root's alone while it is unpacked
    let tarball = std::env::temp_dir().join(format!("radiopost-restore-{now}-{name}"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options
        .open(&tarball)
        .and_then(|mut f| f.write_all(&data))
        .map_err(|e| format!("{}: {e}", tarball.display()))?;
    let result = unpack(&tarball, &name, server, now);
    let _ = std::fs::remove_file(&tarball);
    result.map(Some)
}

fn unpack(tarball: &Path, name: &str, server: &MailServer, now: u64) -> Result<String, String> {
    let listed = Command::new("tar")
        .arg("-tzf")
        .arg(tarball)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("tar: {e}"))?;
    if !listed.status.success() {
        return Err(format!("{name} is not an archive tar can read"));
    }
    let inside = |member: &str| {
        let member = member.trim_start_matches("./");
        !member.split('/').any(|part| part == "..")
            && server.paths.iter().any(|p| {
                member == p.as_str()
                    || member
                        .strip_prefix(p.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            })
    };
    if let Some(stray) = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .find(|m| !inside(m))
    {
        return Err(format!(
            "{name} holds {stray}, outside the mail server's directories ({}): nothing restored",
            server.paths.join(", ")
        ));
    }
    run(&server.stop).map_err(|e| {
        format!("the mail server did not stop, so its data was left as it was: {e}")
    })?;
    let mut moved = Vec::new();
    for p in &server.paths {
        let at = server.root.join(p);
        if at.exists() {
            let aside = server.root.join(format!("{p}.before-restore-{now}"));
            if let Err(e) = std::fs::rename(&at, &aside) {
                for (from, to) in moved.iter().rev() {
                    let _ = std::fs::rename(to, from);
                }
                let _ = run(&server.start);
                return Err(format!("{}: {e}", at.display()));
            }
            moved.push((at, aside));
        }
    }
    let unpacked = run(&[
        "tar".to_owned(),
        "-xzf".to_owned(),
        tarball.display().to_string(),
        "-C".to_owned(),
        server.root.display().to_string(),
    ]);
    if let Err(e) = unpacked {
        for (from, to) in moved.iter().rev() {
            let _ = std::fs::remove_dir_all(from);
            let _ = std::fs::rename(to, from);
        }
        let _ = run(&server.start);
        return Err(format!(
            "{name} could not be unpacked, so the mail server's data was put back as it was: {e}"
        ));
    }
    let started = run(&server.start);
    let kept: Vec<String> = moved
        .iter()
        .map(|(_, to)| to.display().to_string())
        .collect();
    let mut said = format!("the mailboxes of {name} are back");
    if !kept.is_empty() {
        let _ = write!(said, "; what was there is kept as {}", kept.join(" and "));
    }
    match started {
        Ok(()) => Ok(said),
        Err(e) => Err(format!(
            "{said}, but the mail server did not start again: {e}"
        )),
    }
}

// -- backups that left the server ------------------------------------------------------------------

/// Where the service writes down the backups downloaded from the desk.
fn downloads_file(dir: &Path) -> PathBuf {
    dir.join("backups").join("downloads.json")
}

/// What is known of backups that left the server: since when it has been watched, and the last
/// one downloaded (`at`, `by`, `bytes`, and `mail`, the mailboxes' backup it carried, if any).
#[must_use]
pub fn offsite(dir: &Path, now: u64) -> Json {
    let path = downloads_file(dir);
    let known = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<Json>(&t).ok());
    if let Some(k) = known.filter(|k| k["since"].is_u64()) {
        return k;
    }
    // the clock starts the first time anyone looks: a post office that never sent one away
    // turns red after as long as one that sent one long ago
    let fresh = json!({"since": now, "last": null});
    let _ = std::fs::create_dir_all(dir.join("backups"));
    let _ = std::fs::write(&path, fresh.to_string());
    fresh
}

/// A backup downloaded from the desk, at `at`, by `by`, `bytes` long, carrying the mailboxes'
/// backup `mail`.
///
/// # Errors
///
/// It could not be written down.
pub fn downloaded(
    dir: &Path,
    at: u64,
    by: &str,
    bytes: usize,
    mail: Option<&str>,
) -> Result<(), String> {
    let mut known = offsite(dir, at);
    known["last"] = json!({"at": at, "by": by, "bytes": bytes, "mail": mail});
    std::fs::write(downloads_file(dir), known.to_string()).map_err(|e| e.to_string())
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
        let zip = archive(&dir, true).expect("made");
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

    /// A stand-in for Stalwart under `root`: its `RocksDB` directory and its settings, and the
    /// commands that stop and start it, which write down that they ran.
    #[cfg(unix)]
    fn stand_in(root: &Path) -> MailServer {
        let data = root.join("var/lib/stalwart");
        std::fs::create_dir_all(data.join("db")).expect("a directory");
        std::fs::create_dir_all(root.join("etc/stalwart")).expect("a directory");
        std::fs::write(data.join("db/000012.sst"), b"mail of the 12th").expect("written");
        std::fs::write(data.join("db/CURRENT"), b"MANIFEST-000012\n").expect("written");
        std::fs::write(root.join("etc/stalwart/config.json"), b"{\"store\": 1}").expect("written");
        std::fs::write(root.join("etc/stalwart/stalwart.env"), b"A=1\n").expect("written");
        let said = |what: &str| {
            vec![
                "sh".to_owned(),
                "-c".to_owned(),
                format!("echo {what} >> '{}'", root.join("said").display()),
            ]
        };
        MailServer {
            root: root.to_path_buf(),
            paths: vec!["var/lib/stalwart".into(), "etc/stalwart".into()],
            stop: said("stop"),
            start: said("start"),
        }
    }

    #[cfg(unix)]
    fn said(root: &Path) -> String {
        std::fs::read_to_string(root.join("said")).unwrap_or_default()
    }

    /// W38: the mailboxes backed up with the mail server stopped and started again, kept three
    /// nights, carried by a backup that leaves the server, and put back by a restore, what was
    /// there moved aside.
    #[test]
    #[cfg(unix)]
    fn the_mailboxes_are_backed_up_stopped_and_restored_whole() {
        let dir = office("mail");
        let root = dir.join("stand-in");
        let server = stand_in(&root);
        for d in 1..=5 {
            let (path, bytes) =
                mail_backup(&dir, &server, &format!("2026-11-{d:02}")).expect("made");
            assert!(path.ends_with(format!("backups/mail/stalwart-2026-11-{d:02}.tar.gz")));
            assert!(bytes > 0);
        }
        assert_eq!(
            said(&root),
            "stop\nstart\n".repeat(5),
            "stopped, then started"
        );
        let names: Vec<String> = mail_list(&dir).into_iter().map(|(n, _, _)| n).collect();
        assert_eq!(
            names,
            [
                "stalwart-2026-11-05.tar.gz",
                "stalwart-2026-11-04.tar.gz",
                "stalwart-2026-11-03.tar.gz"
            ]
        );
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(mail_dir(&dir).join(&names[0]))
                .expect("there")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o640, "every password is in it");
        }

        // the nightly backup of the post office leaves them out; one that leaves the server not
        assert!(
            !archive(&dir, false)
                .expect("made")
                .windows(5)
                .any(|w| w == b"mail/")
        );
        let zip = archive(&dir, true).expect("made");
        let sealed = seal(&zip, "correct horse battery").expect("sealed");
        let zip = open(&sealed, Some("correct horse battery")).expect("opened");

        // the mail server changes after the backup; the restore puts the backup's back
        std::fs::write(root.join("var/lib/stalwart/db/000013.sst"), b"newer").expect("written");
        std::fs::write(root.join("etc/stalwart/config.json"), b"{}").expect("written");
        std::fs::write(root.join("said"), b"").expect("written");
        let to = office("mail-restore-to");
        std::fs::remove_file(to.join("key.hex")).expect("removed");
        let written = restore(&zip, &to, false).expect("restored");
        assert!(
            !written.iter().any(|w| w.starts_with("mail")),
            "{written:?}"
        );
        let done = restore_mail(&zip, &server, 1_794_600_000)
            .expect("restored")
            .expect("it held mailboxes");
        assert!(done.contains("stalwart-2026-11-05.tar.gz"), "{done}");
        assert_eq!(said(&root), "stop\nstart\n");
        let read = |p: &str| std::fs::read(root.join(p)).unwrap_or_default();
        assert_eq!(read("var/lib/stalwart/db/000012.sst"), b"mail of the 12th");
        assert!(
            !root.join("var/lib/stalwart/db/000013.sst").exists(),
            "replaced, not merged"
        );
        assert_eq!(read("etc/stalwart/config.json"), b"{\"store\": 1}");
        assert_eq!(
            read("var/lib/stalwart.before-restore-1794600000/db/000013.sst"),
            b"newer",
            "what was there is kept"
        );

        // a backup without mailboxes leaves the mail server alone
        let plain = archive(&dir, false).expect("made");
        std::fs::write(root.join("said"), b"").expect("written");
        assert_eq!(restore_mail(&plain, &server, 1_794_600_100), Ok(None));
        assert_eq!(said(&root), "");
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(to);
    }

    /// Never a copy of a mail server that runs: one that does not stop is not copied, and is
    /// started again; an archive naming anything outside its directories is not restored.
    #[test]
    #[cfg(unix)]
    fn a_mail_server_that_does_not_stop_is_not_copied() {
        let dir = office("mail-running");
        let root = dir.join("stand-in");
        let mut server = stand_in(&root);
        let start = server.start.clone();
        server.stop = vec!["false".into()];
        let refused = mail_backup(&dir, &server, "2026-11-13").expect_err("refused");
        assert!(refused.contains("did not stop"), "{refused}");
        assert_eq!(mail_list(&dir), Vec::new());
        assert!(
            !mail_dir(&dir).join("stalwart-2026-11-13.part").exists(),
            "nothing half made"
        );
        assert_eq!(said(&root), "start\n", "started again all the same");

        // an archive with a member outside the mail server's directories
        server.stop = start.clone();
        std::fs::create_dir_all(root.join("root/.ssh")).expect("a directory");
        std::fs::write(root.join("root/.ssh/authorized_keys"), b"a key").expect("written");
        let bad = dir.join("bad.tar.gz");
        let made = Command::new("tar")
            .arg("-czf")
            .arg(&bad)
            .arg("-C")
            .arg(&root)
            .args(["var/lib/stalwart", "root/.ssh"])
            .status()
            .expect("tar");
        assert!(made.success());
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(
            "mail/stalwart-2026-11-13.tar.gz",
            zip::write::SimpleFileOptions::default(),
        )
        .expect("a file");
        zip.write_all(&std::fs::read(&bad).expect("read"))
            .expect("written");
        let zip = zip.finish().expect("made").into_inner();
        std::fs::write(root.join("said"), b"").expect("written");
        let refused = restore_mail(&zip, &server, 1).expect_err("refused");
        assert!(refused.contains("root/.ssh"), "{refused}");
        assert_eq!(said(&root), "", "nothing stopped, nothing touched");
        assert!(root.join("var/lib/stalwart/db/000012.sst").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_download_is_written_down() {
        let dir = office("downloads");
        let first = offsite(&dir, 100);
        assert_eq!(first, json!({"since": 100, "last": null}));
        assert_eq!(offsite(&dir, 200)["since"], 100, "the clock starts once");
        downloaded(
            &dir,
            300,
            "KK4ODA",
            1234,
            Some("stalwart-2026-11-13.tar.gz"),
        )
        .expect("kept");
        let known = offsite(&dir, 400);
        assert_eq!(known["since"], 100);
        assert_eq!(
            known["last"],
            json!({"at": 300, "by": "KK4ODA", "bytes": 1234, "mail": "stalwart-2026-11-13.tar.gz"})
        );
        let _ = std::fs::remove_dir_all(dir);
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
