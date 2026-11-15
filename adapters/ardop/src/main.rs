// SPDX-License-Identifier: MIT OR Apache-2.0
//! `radiopost-ardop`: the RPMI adapter for an ARDOP TNC (RP-4 §9.3).
//!
//! ```text
//! radiopost-ardop [--listen 127.0.0.1:8600] [--ardop 127.0.0.1:8515]
//! ```
//!
//! `--ardop` is the TNC's command port (ardopcf's, 8515 unless set); its data port is the next.
//! Aether's control API takes 8515 too: on a computer that runs both, start ardopcf on another.

use std::process::ExitCode;

use radiopost_ardop::{DEFAULT_ARDOP, listen};
use radiopost_mercury::serve::Note;

const USAGE: &str = "usage: radiopost-ardop [--listen 127.0.0.1:8600] [--ardop 127.0.0.1:8515]";

fn main() -> ExitCode {
    let mut at = format!("127.0.0.1:{}", rpmi::DEFAULT_PORT);
    let mut ardop = DEFAULT_ARDOP.to_owned();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next()) {
            ("--listen", Some(v)) => at = v,
            ("--ardop", Some(v)) => ardop = v,
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let note: Note = std::sync::Arc::new(|line: &str| eprintln!("radiopost-ardop: {line}"));
    match listen(&at, &ardop, note) {
        Ok(local) => {
            eprintln!("radiopost-ardop: RPMI on {local}, ARDOP's command port at {ardop}");
            loop {
                std::thread::park();
            }
        }
        Err(e) => {
            eprintln!("radiopost-ardop: cannot listen on {at}: {e}");
            ExitCode::FAILURE
        }
    }
}
