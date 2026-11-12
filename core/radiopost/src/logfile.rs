// SPDX-License-Identifier: MIT OR Apache-2.0
//! The programs' own log on disk: dated lines in one file, the one before it kept.
//!
//! Not one of RadioPost's objects, but every program needs it, and they all depend on this
//! library. A program names its file once, at its start ([`keep_in`]); everything it says after
//! that goes there as well as to its window ([`line`]), and so does a panic
//! ([`record_panics`]). Until a file is named, [`line`] does nothing: a test or a library caller
//! writes no log of its own.
//!
//! Why: on 2026-11-12 a client's connection to its modem ended twice in mid-session, and the
//! client's log, kept only in its window, was gone by the time anyone asked what it said.
//!
//! The file goes with the files a station shares when something went wrong (the Aether adapter
//! offers it to Aether, RP-4 §9.1), so it says what happened and never what was said: no
//! correspondent's address and no subject. Where the window names them, the program writes the
//! file a line without them.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

/// How large a log grows before it is set aside as `<name>.old` and a new one begun: a week of
/// a busy gateway's lines, and small enough to attach to a report.
pub const MAX_BYTES: u64 = 2 * 1024 * 1024;

static FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// From now on, [`line`] writes to `path`, its folder made if need be.
pub fn keep_in(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    *FILE.lock().unwrap_or_else(PoisonError::into_inner) = Some(path.to_owned());
}

/// The file lines go to, if one was named.
#[must_use]
pub fn path() -> Option<PathBuf> {
    FILE.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// One line, after the time in UTC, to the file named by [`keep_in`]; nothing when none was.
/// A line that cannot be written is dropped: a full disk must not stop the program.
pub fn line(text: &str) {
    let guard = FILE.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(path) = guard.as_ref() else {
        return;
    };
    if fs::metadata(path).is_ok_and(|m| m.len() >= MAX_BYTES) {
        let _ = fs::rename(path, old(path));
    }
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{} {}", utc(secs), text.trim_end());
    }
}

/// Where the log before this one is kept.
#[must_use]
pub fn old(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    path.with_file_name(name)
}

/// A panic is written to the log, where and what, before the usual message: a panic on one of
/// a program's threads ends only that thread, and leaves no other trace in a program without a
/// console.
pub fn record_panics() {
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        // a panic says where on one line and what on the next: one dated line here
        let said = info.to_string().replace('\n', " ");
        line(&format!(
            "panic on thread {}: {said}",
            thread.name().unwrap_or("unnamed")
        ));
        before(info);
    }));
}

/// A time as the log writes it: `2026-11-12T02:00:31Z`.
#[must_use]
pub fn utc(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    // civil from days (H. Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::utc;

    #[test]
    fn times_are_written_in_utc_to_the_second() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(1_794_445_231), "2026-11-12T01:00:31Z");
        assert_eq!(utc(951_782_400), "2000-02-29T00:00:00Z");
    }
}
