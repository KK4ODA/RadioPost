// SPDX-License-Identifier: MIT OR Apache-2.0
//! RadioPost's updates.
//!
//! Each release publishes, on its update channel (a release of its own, `channel-beta`), what it
//! offers each kind of program: `client.json` (the RadioPost app), `gateway.json` (RadioPost
//! Gateway) and `tools.json` (the command-line programs, the post office's among them). Each is
//! Tauri's updater manifest: the version, the notes, and for each platform a file's address and
//! its signature. A program compares the version with its own, says when a newer one is out, and
//! takes a file only when the project's update key signed it.
//!
//! The desktop applications install their update with Tauri's updater, from the same manifests
//! and the same key; [`install_tools`] replaces the command-line programs from the signed tools
//! archive. [`Watch`] keeps what a program's panel shows: the offer, checked when the program
//! starts and every six hours, and an install under way.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};

/// Where the update channel's manifests are.
pub const CHANNEL: &str = "https://github.com/KK4ODA/RadioPost/releases/download/channel-beta";
/// The project's releases, where each release's page is.
pub const RELEASES: &str = "https://github.com/KK4ODA/RadioPost/releases";
/// How often a running program asks the channel again.
pub const EVERY: Duration = Duration::from_secs(6 * 3600);
/// The largest file an update may be.
const MAX_BYTES: u64 = 300 << 20;

/// The project's update key: a minisign public key, base64, as Tauri's signer writes it. Empty
/// in a build made before the key existed: such a build installs nothing, and says so.
#[must_use]
pub fn public_key() -> &'static str {
    include_str!("../updater.pub").trim()
}

/// The update channel: `$RADIOPOST_UPDATES` when set (a test's), else [`CHANNEL`].
#[must_use]
pub fn channel() -> String {
    std::env::var("RADIOPOST_UPDATES").unwrap_or_else(|_| CHANNEL.to_owned())
}

/// The kinds of program, each with its own manifest on the channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Product {
    /// RadioPost, the desktop application.
    Client,
    /// RadioPost Gateway, the desktop application.
    Gateway,
    /// The command-line programs: `radiopost`, `radiopost-gateway`, `radiopost-po` and the rest.
    Tools,
}

impl Product {
    /// Its manifest's file name on the channel.
    #[must_use]
    pub fn manifest(self) -> &'static str {
        match self {
            Product::Client => "client.json",
            Product::Gateway => "gateway.json",
            Product::Tools => "tools.json",
        }
    }
}

/// A manifest, as Tauri's updater reads it.
#[derive(Clone, Debug, Deserialize)]
pub struct Manifest {
    /// The version offered.
    pub version: String,
    /// What changed, in words.
    #[serde(default)]
    pub notes: String,
    /// When it was published.
    #[serde(default)]
    pub pub_date: String,
    /// Each platform's file (`linux-x86_64`, `windows-x86_64`, `darwin-aarch64`).
    #[serde(default)]
    pub platforms: BTreeMap<String, Asset>,
}

/// One platform's file.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Asset {
    /// Where to download it.
    pub url: String,
    /// Its signature: a minisign signature, base64, as Tauri's signer writes it.
    pub signature: String,
}

/// A newer version than this program's, on offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    /// The version.
    pub version: String,
    /// What changed.
    pub notes: String,
    /// The release's page, for a download by hand.
    pub page: String,
    /// This platform's file, when the release has one.
    pub asset: Option<Asset>,
}

/// This platform, as manifests name it.
#[must_use]
pub fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

/// Whether `offered` is a newer version than `current` (semantic versions: `0.1.0-beta.10` is
/// newer than `0.1.0-beta.9`, and `0.1.0` than both). Either one not a version: no.
#[must_use]
pub fn newer(current: &str, offered: &str) -> bool {
    match (
        semver::Version::parse(current.trim_start_matches('v')),
        semver::Version::parse(offered.trim_start_matches('v')),
    ) {
        (Ok(c), Ok(o)) => o > c,
        _ => false,
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .build()
        .into()
}

fn get(url: &str) -> Result<Vec<u8>, String> {
    let mut response = agent()
        .get(url)
        .call()
        .map_err(|e| format!("cannot fetch {url}: {e}"))?;
    response
        .body_mut()
        .with_config()
        .limit(MAX_BYTES)
        .read_to_vec()
        .map_err(|e| format!("cannot read {url}: {e}"))
}

/// The manifest `channel` offers for `product`.
///
/// # Errors
///
/// It cannot be fetched, or is not a manifest.
pub fn fetch(channel: &str, product: Product) -> Result<Manifest, String> {
    let url = format!("{}/{}", channel.trim_end_matches('/'), product.manifest());
    serde_json::from_slice(&get(&url)?).map_err(|e| format!("{url} is not a manifest: {e}"))
}

/// What `channel` offers `product` that is newer than `current`; `None` when nothing is.
///
/// # Errors
///
/// The channel cannot be reached, or its manifest read.
pub fn check(channel: &str, product: Product, current: &str) -> Result<Option<Offer>, String> {
    let m = fetch(channel, product)?;
    if !newer(current, &m.version) {
        return Ok(None);
    }
    // this platform's file: `{os}-{arch}`, or an installer's own (`linux-x86_64-deb`), as
    // Tauri's updater names them
    let here = platform();
    let asset = m.platforms.get(&here).cloned().or_else(|| {
        m.platforms
            .iter()
            .find(|(k, _)| k.starts_with(&format!("{here}-")))
            .map(|(_, a)| a.clone())
    });
    Ok(Some(Offer {
        page: format!("{RELEASES}/tag/v{}", m.version.trim_start_matches('v')),
        asset,
        version: m.version,
        notes: m.notes,
    }))
}

fn unbase64(text: &str) -> Result<String, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|_| "not base64".to_owned())?;
    String::from_utf8(bytes).map_err(|_| "not text".to_owned())
}

/// Whether `public_key` (a minisign public key, base64, as Tauri's signer writes it) signed
/// `data` with `signature` (a minisign signature, base64): Tauri's updater's own check.
///
/// # Errors
///
/// No key, a key or signature that does not read, or a signature that does not match.
pub fn verify(data: &[u8], signature: &str, public_key: &str) -> Result<(), String> {
    if public_key.trim().is_empty() {
        return Err("this build has no update key, so it installs nothing".into());
    }
    let key = minisign_verify::PublicKey::decode(&unbase64(public_key)?)
        .map_err(|e| format!("the update key does not read: {e}"))?;
    let sig = minisign_verify::Signature::decode(&unbase64(signature)?)
        .map_err(|e| format!("the signature does not read: {e}"))?;
    key.verify(data, &sig, true)
        .map_err(|_| "the signature does not match: the file is not the project's".into())
}

/// The programs in the tools archive `zip` that are also in `dir`, replaced by the archive's, each
/// kept executable. Returns their names. Nothing else in `dir` changes; a program the archive
/// does not hold, or one `dir` does not have, is left alone.
///
/// # Errors
///
/// The archive does not read, or a program cannot be written (a system directory needs root).
pub fn replace_programs(zip: &[u8], dir: &Path) -> Result<Vec<String>, String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(zip)).map_err(|e| format!("not an archive: {e}"))?;
    let mut fresh: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("the archive: {e}"))?;
        let Some(name) = entry
            .enclosed_name()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        else {
            continue;
        };
        if entry.is_dir() || !dir.join(&name).is_file() || !is_program(&name) {
            continue;
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|e| format!("{name}: {e}"))?;
        fresh.push((name, bytes));
    }
    // every program written beside its old one first, so a failure leaves the old ones whole
    let mut staged: Vec<(String, PathBuf)> = Vec::new();
    for (name, bytes) in &fresh {
        let new = dir.join(format!("{name}.new"));
        if let Err(e) = std::fs::write(&new, bytes).and_then(|()| keep_mode(&dir.join(name), &new))
        {
            for (_, path) in &staged {
                let _ = std::fs::remove_file(path);
            }
            let _ = std::fs::remove_file(&new);
            let hint = if e.kind() == std::io::ErrorKind::PermissionDenied {
                " (run it as the directory's owner: sudo)"
            } else {
                ""
            };
            return Err(format!("cannot write {}: {e}{hint}", new.display()));
        }
        staged.push((name.clone(), new));
    }
    for (name, new) in &staged {
        let target = dir.join(name);
        // a running program on Windows cannot be overwritten, but it can be moved aside
        if cfg!(windows) {
            let old = dir.join(format!("{name}.old"));
            let _ = std::fs::remove_file(&old);
            std::fs::rename(&target, &old).map_err(|e| format!("{name}: {e}"))?;
        }
        std::fs::rename(new, &target).map_err(|e| format!("{name}: {e}"))?;
    }
    Ok(staged.into_iter().map(|(n, _)| n).collect())
}

/// The version a tools archive says it is (its `VERSION` file): the signature covers the
/// archive, and this binds it to a version, so an older signed archive cannot pass for a newer.
///
/// # Errors
///
/// It is not an archive, or says no version.
pub fn archive_version(zip: &[u8]) -> Result<String, String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(zip)).map_err(|e| format!("not an archive: {e}"))?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("the archive: {e}"))?;
        if entry
            .enclosed_name()
            .and_then(|p| p.file_name().map(|n| n == "VERSION"))
            == Some(true)
        {
            let mut text = String::new();
            entry
                .read_to_string(&mut text)
                .map_err(|e| format!("VERSION: {e}"))?;
            return Ok(text.trim().to_owned());
        }
    }
    Err("the archive says no version".into())
}

/// RadioPost's command-line programs, as the tools archive names them.
fn is_program(name: &str) -> bool {
    let stem = name.strip_suffix(".exe").unwrap_or(name);
    stem.starts_with("radiopost") || stem == "rpmi-loopback"
}

#[cfg(unix)]
fn keep_mode(old: &Path, new: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(old).map_or(0o755, |m| m.permissions().mode()) | 0o111;
    std::fs::set_permissions(new, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn keep_mode(_old: &Path, _new: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Updates the command-line programs in `dir` (the running program's own directory, normally)
/// to what `channel` offers, when it is newer than `current`: the tools archive for this
/// platform, its signature checked with `key`, and each program `dir` holds replaced. Says what
/// it did.
///
/// # Errors
///
/// The channel cannot be reached, the release has no archive for this platform, the signature
/// does not match, or a program cannot be written.
pub fn install_tools(
    channel: &str,
    current: &str,
    dir: &Path,
    key: &str,
) -> Result<String, String> {
    let Some(offer) = check(channel, Product::Tools, current)? else {
        return Ok(format!("{current} is the newest"));
    };
    let asset = offer.asset.ok_or_else(|| {
        format!(
            "{} has no tools for {}: download them from {}",
            offer.version,
            platform(),
            offer.page
        )
    })?;
    let zip = get(&asset.url)?;
    verify(&zip, &asset.signature, key)?;
    let says = archive_version(&zip)?;
    if says != offer.version {
        return Err(format!(
            "the archive is {says}, not the {} on offer: nothing replaced",
            offer.version
        ));
    }
    let replaced = replace_programs(&zip, dir)?;
    if replaced.is_empty() {
        return Err(format!("nothing in {} to replace", dir.display()));
    }
    Ok(format!(
        "updated {} from {current} to {}",
        replaced.join(", "),
        offer.version
    ))
}

/// Opens a release's page in the user's browser, for a download by hand: only the project's own
/// release pages.
///
/// # Errors
///
/// Not a release page of the project's, or no browser could be started.
pub fn open_page(url: &str) -> Result<(), String> {
    let ours = url
        .strip_prefix(RELEASES)
        .is_some_and(|rest| rest.starts_with("/tag/v") && !rest.contains(['"', ' ', '&', '|']));
    if !ours {
        return Err(format!("{url} is not a RadioPost release page"));
    }
    let mut command = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    command
        .spawn()
        .map(drop)
        .map_err(|e| format!("cannot open a browser ({e}): the release is at {url}"))
}

/// Installs a desktop application's update and restarts it: what the application gives its
/// panel. It returns only when the install failed.
pub type Install = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

#[derive(Default)]
struct Seen {
    offer: Option<Offer>,
    checked: u64,
    error: Option<String>,
    installing: bool,
    failed: Option<String>,
}

/// What a program's panel shows of its updates: the offer, checked at start and every six hours,
/// and an install under way.
pub struct Watch {
    product: Product,
    current: String,
    channel: String,
    seen: Mutex<Seen>,
    install: Mutex<Option<Install>>,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl Watch {
    /// A watch for `product` at version `current`, on `channel`.
    #[must_use]
    pub fn new(product: Product, current: &str, channel: &str) -> Arc<Watch> {
        Arc::new(Watch {
            product,
            current: current.to_owned(),
            channel: channel.to_owned(),
            seen: Mutex::new(Seen::default()),
            install: Mutex::new(None),
        })
    }

    fn seen(&self) -> MutexGuard<'_, Seen> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Asks the channel now.
    pub fn check(&self) {
        let result = check(&self.channel, self.product, &self.current);
        let mut s = self.seen();
        s.checked = now();
        match result {
            Ok(offer) => {
                s.offer = offer;
                s.error = None;
            }
            Err(e) => s.error = Some(e),
        }
    }

    /// Asks the channel now and every six hours, on a thread of its own.
    pub fn watch(self: &Arc<Self>) {
        let me = Arc::clone(self);
        std::thread::spawn(move || {
            loop {
                me.check();
                std::thread::sleep(EVERY);
            }
        });
    }

    /// Lets the panel install an update: the desktop application's own updater.
    pub fn set_install(&self, install: Install) {
        *self.install.lock().unwrap_or_else(PoisonError::into_inner) = Some(install);
    }

    /// The newer version on offer, if any.
    #[must_use]
    pub fn offer(&self) -> Option<Offer> {
        self.seen().offer.clone()
    }

    /// What the panel shows: this version, the offer, when the channel was last asked and what
    /// went wrong, whether this program can install the update itself, and an install under way.
    #[must_use]
    pub fn status(&self) -> Value {
        let s = self.seen();
        let can_install = self
            .install
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
            && s.offer.as_ref().is_some_and(|o| o.asset.is_some());
        json!({
            "current": self.current,
            "offer": s.offer.as_ref().map(|o| json!({
                "version": o.version, "notes": o.notes, "page": o.page,
            })),
            "checked": s.checked,
            "error": s.error,
            "can_install": can_install,
            "installing": s.installing,
            "failed": s.failed,
        })
    }

    /// Opens the release's page in the user's browser, for a download by hand.
    ///
    /// # Errors
    ///
    /// Nothing is on offer, or no browser could be started.
    pub fn download(&self) -> Result<(), String> {
        let offer = self.offer().ok_or("no update is on offer")?;
        open_page(&offer.page)
    }

    /// Installs the update on offer, on a thread of its own: the application restarts when it
    /// is done.
    ///
    /// # Errors
    ///
    /// Nothing is on offer, an install is under way, or this program cannot install it itself
    /// (it is then downloaded by hand from the release's page).
    pub fn install(self: &Arc<Self>) -> Result<(), String> {
        let install = self
            .install
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut s = self.seen();
        let Some(offer) = s.offer.clone() else {
            return Err("no update is on offer".into());
        };
        if s.installing {
            return Err("the update is being installed".into());
        }
        let Some(install) = install.filter(|_| offer.asset.is_some()) else {
            return Err(format!("download {} from {}", offer.version, offer.page));
        };
        s.installing = true;
        s.failed = None;
        drop(s);
        let me = Arc::clone(self);
        std::thread::spawn(move || {
            let result = install();
            let mut s = me.seen();
            s.installing = false;
            s.failed = result.err();
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{newer, platform};

    #[test]
    fn versions_in_order() {
        assert!(newer("0.1.0-beta.4", "0.1.0-beta.5"));
        assert!(newer("0.1.0-beta.9", "0.1.0-beta.10"), "numbers, not text");
        assert!(
            newer("0.1.0-beta.10", "0.1.0"),
            "the release after its betas"
        );
        assert!(newer("0.1.0", "0.1.1-beta.1"));
        assert!(!newer("0.1.0-beta.5", "0.1.0-beta.5"));
        assert!(!newer("0.1.0-beta.5", "0.1.0-beta.4"));
        assert!(newer("v0.1.0-beta.4", "v0.1.0-beta.5"), "a tag's v");
        assert!(!newer("0.1.0", "not a version"));
        assert!(!newer("", "0.1.0"));
    }

    #[test]
    fn only_release_pages_are_opened() {
        for bad in [
            "https://example.com/",
            "https://github.com/KK4ODA/RadioPost/releases/download/x",
            "https://github.com/KK4ODA/RadioPost/releases/tag/v1 & calc",
            "file:///etc/passwd",
        ] {
            assert!(super::open_page(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn platforms_as_manifests_name_them() {
        let p = platform();
        assert!(
            ["linux-", "windows-", "darwin-"]
                .iter()
                .any(|os| p.starts_with(os)),
            "{p}"
        );
        assert!(!p.contains("macos"), "{p}");
    }
}
