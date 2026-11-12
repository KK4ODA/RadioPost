// SPDX-License-Identifier: MIT OR Apache-2.0
//! `radiopost-mercury`: the RPMI adapter for the Mercury HF modem.
//!
//! ```text
//! radiopost-mercury [--listen 127.0.0.1:8600] [--mercury 127.0.0.1:8300]
//! ```
//!
//! `--mercury` is Mercury's control port (its `-p`, 8300 unless set); its data port is the next.

use std::process::ExitCode;

use radiopost_mercury::serve::{DEFAULT_MERCURY, Note, listen_noting};

const USAGE: &str = "usage: radiopost-mercury [--listen 127.0.0.1:8600] [--mercury 127.0.0.1:8300]";

fn main() -> ExitCode {
    let mut listen = format!("127.0.0.1:{}", rpmi::DEFAULT_PORT);
    let mut mercury = DEFAULT_MERCURY.to_owned();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next();
        match (arg.as_str(), value) {
            ("--listen", Some(v)) => listen = v,
            ("--mercury", Some(v)) => mercury = v,
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let note: Note = std::sync::Arc::new(|line: &str| eprintln!("radiopost-mercury: {line}"));
    match listen_noting(&listen, &mercury, note) {
        Ok(at) => {
            eprintln!("radiopost-mercury: RPMI on {at}, Mercury's control port at {mercury}");
            loop {
                std::thread::park();
            }
        }
        Err(e) => {
            eprintln!("radiopost-mercury: cannot listen on {listen}: {e}");
            ExitCode::FAILURE
        }
    }
}
