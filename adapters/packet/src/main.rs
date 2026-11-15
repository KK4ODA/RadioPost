// SPDX-License-Identifier: MIT OR Apache-2.0
//! `radiopost-packet`: the RPMI adapter for AX.25 packet, through a TNC's AGWPE port (RP-4 §9.4).
//!
//! ```text
//! radiopost-packet [--listen 127.0.0.1:8600] [--tnc 127.0.0.1:8000] [--port 0]
//! ```
//!
//! `--tnc` is the TNC's AGWPE port (Dire Wolf's `AGWPORT`, 8000 unless set); `--port` the TNC's
//! radio port, 0 for the first.

use std::process::ExitCode;

use radiopost_packet::serve::{DEFAULT_TNC, Note, listen};

const USAGE: &str =
    "usage: radiopost-packet [--listen 127.0.0.1:8600] [--tnc 127.0.0.1:8000] [--port 0]";

fn main() -> ExitCode {
    let mut at = format!("127.0.0.1:{}", rpmi::DEFAULT_PORT);
    let mut tnc = DEFAULT_TNC.to_owned();
    let mut port = 0u8;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next()) {
            ("--listen", Some(v)) => at = v,
            ("--tnc", Some(v)) => tnc = v,
            ("--port", Some(v)) if v.parse::<u8>().is_ok() => port = v.parse().unwrap_or(0),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let note: Note = std::sync::Arc::new(|line: &str| eprintln!("radiopost-packet: {line}"));
    match listen(&at, &tnc, port, note) {
        Ok(local) => {
            eprintln!(
                "radiopost-packet: RPMI on {local}, the TNC's AGWPE port at {tnc}, its port {port}"
            );
            loop {
                std::thread::park();
            }
        }
        Err(e) => {
            eprintln!("radiopost-packet: cannot listen on {at}: {e}");
            ExitCode::FAILURE
        }
    }
}
