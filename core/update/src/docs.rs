// SPDX-License-Identifier: MIT OR Apache-2.0
//! The documents a release installs with the programs: the user's manual, the server guide and
//! the licences (`tools/stage_docs.py`), in a folder `docs` among each desktop application's
//! resources and beside the programs in the tools archive.
//!
//! Each panel's **Manual and licences** opens them through its own program, since the panel's
//! window opens no other page: the installed file or folder in the system's own viewer, which
//! needs no Internet, or the copy on GitHub when this build carries none (a program built from a
//! checkout).

use std::path::{Path, PathBuf};

/// The user's manual on GitHub, for a build that carries none.
pub const MANUAL_ONLINE: &str =
    "https://github.com/KK4ODA/RadioPost/blob/main/docs/manual/manual.md";
/// The licences on GitHub: the README's table, which names each licence file.
pub const LICENCES_ONLINE: &str = "https://github.com/KK4ODA/RadioPost#licence";

/// What a panel can open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Doc {
    /// The user's manual, `manual.pdf`.
    Manual,
    /// The folder itself: RadioPost's licences and every crate's, beside the books.
    Licences,
}

impl Doc {
    /// The document a panel names: `manual` or `licences`.
    #[must_use]
    pub fn named(name: &str) -> Option<Doc> {
        match name {
            "manual" => Some(Doc::Manual),
            "licences" => Some(Doc::Licences),
            _ => None,
        }
    }
}

/// The folder beside the running program, where the tools archive puts it.
#[must_use]
pub fn beside_program() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("docs"))
}

/// What opening `doc` opens: the installed file or folder when `dir` holds it, else the online
/// copy's address.
#[must_use]
pub fn target(dir: Option<&Path>, doc: Doc) -> String {
    let installed = dir.map(|dir| match doc {
        Doc::Manual => dir.join("manual.pdf"),
        Doc::Licences => dir.to_owned(),
    });
    match (installed, doc) {
        (Some(path), Doc::Manual) if path.is_file() => path.display().to_string(),
        (Some(path), Doc::Licences) if path.join("THIRD-PARTY-LICENSES.txt").is_file() => {
            path.display().to_string()
        }
        (_, Doc::Manual) => MANUAL_ONLINE.to_owned(),
        (_, Doc::Licences) => LICENCES_ONLINE.to_owned(),
    }
}

/// Opens `doc` ([`target`]) with the system's own program for it: a PDF viewer, the file
/// manager, the web browser. Returns what it opened.
///
/// # Errors
///
/// No program to open it could be started.
pub fn open(dir: Option<&Path>, doc: Doc) -> Result<String, String> {
    use std::process::Command;
    let what = target(dir, doc);
    let folder = Path::new(&what).is_dir();
    let (program, args): (&str, Vec<&str>) = if cfg!(windows) {
        if folder {
            ("explorer", vec![what.as_str()])
        } else {
            // `start` would cut an address at its `&`; the URL handler takes it whole, and a
            // file's path too
            (
                "rundll32",
                vec!["url.dll,FileProtocolHandler", what.as_str()],
            )
        }
    } else if cfg!(target_os = "macos") {
        ("open", vec![what.as_str()])
    } else {
        ("xdg-open", vec![what.as_str()])
    };
    Command::new(program)
        .args(&args)
        .spawn()
        .map(|_| what.clone())
        .map_err(|e| format!("{what} could not be opened ({program}: {e})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installed_copy_is_opened_and_the_online_one_without_it() {
        let dir = std::env::temp_dir().join(format!("radiopost-docs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a folder");

        // a build with no documents (a checkout) opens them on GitHub
        assert_eq!(target(None, Doc::Manual), MANUAL_ONLINE);
        assert_eq!(target(Some(&dir), Doc::Manual), MANUAL_ONLINE);
        assert_eq!(target(Some(&dir), Doc::Licences), LICENCES_ONLINE);

        std::fs::write(dir.join("manual.pdf"), b"%PDF").expect("the manual");
        std::fs::write(dir.join("THIRD-PARTY-LICENSES.txt"), b"crates").expect("the licences");
        assert_eq!(
            target(Some(&dir), Doc::Manual),
            dir.join("manual.pdf").display().to_string()
        );
        assert_eq!(target(Some(&dir), Doc::Licences), dir.display().to_string());

        assert_eq!(Doc::named("manual"), Some(Doc::Manual));
        assert_eq!(Doc::named("licences"), Some(Doc::Licences));
        // nothing else: a panel cannot have its program open a path it chose
        assert_eq!(Doc::named("../../etc/passwd"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
