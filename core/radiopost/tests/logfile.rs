// SPDX-License-Identifier: MIT OR Apache-2.0
//! The programs' log file: nothing until a file is named, dated lines after, the file set aside
//! when it grows too large, and a panic on any thread written down. One test, in a process of
//! its own, since the file is the process's.

use radiopost::logfile;

#[test]
fn a_program_keeps_its_lines_and_its_panics_in_one_file() {
    let dir = std::env::temp_dir().join(format!("radiopost-logfile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("nested").join("radiopost.log");

    // no file named: nothing is written anywhere
    logfile::line("before any file");
    assert!(logfile::path().is_none());

    logfile::keep_in(&path);
    logfile::line("the connection to Aether ended: Connection reset\n");
    let text = std::fs::read_to_string(&path).expect("the log");
    let first = text.lines().next().expect("a line");
    let (when, what) = first.split_once(' ').expect("a time, then the text");
    assert_eq!(when.len(), "2026-11-12T01:00:31Z".len(), "{first}");
    assert!(when.ends_with('Z'), "{first}");
    assert_eq!(what, "the connection to Aether ended: Connection reset");
    assert!(!text.contains("before any file"));

    // a panic on another thread is written before the thread ends
    logfile::record_panics();
    let thread = std::thread::Builder::new()
        .name("adapter".into())
        .spawn(|| panic!("a bridge that cannot go on"))
        .expect("a thread");
    assert!(thread.join().is_err());
    let text = std::fs::read_to_string(&path).expect("the log");
    assert!(
        text.lines()
            .any(|l| l.contains("panic on thread adapter:")
                && l.contains("a bridge that cannot go on")),
        "{text}"
    );

    // past its size the log is set aside, once, and a new one begun
    let long = "x".repeat(64 * 1024);
    for _ in 0..=(logfile::MAX_BYTES / long.len() as u64) {
        logfile::line(&long);
    }
    logfile::line("after the turn");
    let old = logfile::old(&path);
    assert!(old.exists(), "no {}", old.display());
    let new = std::fs::read_to_string(&path).expect("the new log");
    assert!(
        new.ends_with("after the turn\n"),
        "{}",
        &new[new.len().saturating_sub(80)..]
    );
    assert!(std::fs::metadata(&path).expect("the new log").len() < logfile::MAX_BYTES);
    let _ = std::fs::remove_dir_all(&dir);
}
