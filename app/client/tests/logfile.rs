// SPDX-License-Identifier: MIT OR Apache-2.0
//! The client's log file says what happened and never what was said: no correspondent's address,
//! no subject. The window says them; the file goes with the files a station shares when
//! something went wrong (Aether's `share.include`), to people the correspondents never chose.
//!
//! A process of its own: the log file is the program's, one for the whole process.

mod common;

use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use radiopost_client::session::{self, Fetch, Kind, Options, Packing};

use common::{loopback, message, scratch, store_for};

#[test]
fn the_log_file_names_no_correspondent() {
    let dir = scratch("logfile");
    let log = dir.join("radiopost.log");
    radiopost::logfile::keep_in(&log);
    let mut facundo = store_for(&dir.join("facundo"), "device:facundo", "facundo");
    let mut ana = store_for(&dir.join("ana"), "device:ana", "ana");
    facundo.queue(&message("facundo-direct")).expect("queued");
    ana.queue(&message("ana-to-facundo")).expect("queued");

    let (_server, at_f, at_a) = loopback();
    let mut ana_opts = Options::new(&at_a.to_string());
    ana_opts.packing = Packing::None;
    let mut facundo_opts = Options::new(&at_f.to_string());
    facundo_opts.packing = Packing::None;
    facundo_opts.fetch = Fetch::Nothing;

    let window = Arc::new(Mutex::new(Vec::<String>::new()));
    let (tx, rx) = mpsc::channel();
    let ana_window = Arc::clone(&window);
    let ana_runs = thread::spawn(move || {
        let mut say = |s: &str| {
            ana_window.lock().expect("window").push(s.to_owned());
            if s.starts_with("listening") {
                let _ = tx.send(());
            }
        };
        session::answer(&mut ana, &ana_opts, &mut say).expect("answered")
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("Ana's station listens");
    let called = session::call(
        &mut facundo,
        "N0CALL",
        Kind::Station,
        &facundo_opts,
        &mut |s| window.lock().expect("window").push(s.to_owned()),
    )
    .expect("called");
    let answered = ana_runs.join().expect("Ana's side");
    let window = window.lock().expect("window").clone();
    assert_eq!(called.failed, None, "{window:?}");
    assert_eq!(answered.failed, None, "{window:?}");

    // the window names who sent what, and who the other station is
    let from = message("ana-to-facundo")
        .from()
        .expect("a sender")
        .to_owned();
    assert!(
        window
            .iter()
            .any(|l| l.starts_with("received ") && l.contains(&from)),
        "{window:?}"
    );
    assert!(
        window
            .iter()
            .any(|l| l.starts_with("the other station is ") && l.contains('@')),
        "{window:?}"
    );
    // the file has every line, without them
    let filed = std::fs::read_to_string(&log).expect("the log file");
    let lines: Vec<&str> = filed.lines().collect();
    assert!(
        lines.iter().any(|l| l.ends_with(&format!(
            "received {}",
            &message("ana-to-facundo").content_id().text()[..6]
        ))),
        "{filed}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("the other station is ") && l.contains("checked")),
        "{filed}"
    );
    assert!(
        lines.iter().all(|l| !l.contains('@')),
        "an address in the log file:\n{filed}"
    );
    assert!(
        lines.len() >= window.len(),
        "a line the window said is missing from the file:\n{filed}\n{window:?}"
    );
}
