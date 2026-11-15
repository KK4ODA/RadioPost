// SPDX-License-Identifier: MIT OR Apache-2.0
//! `radiopost-vara`: the RPMI adapter for VARA HF and VARA FM (RP-4 §9.2).
//!
//! ```text
//! radiopost-vara [--listen 127.0.0.1:8600] [--vara 127.0.0.1:8300] [--fm]
//! ```
//!
//! `--vara` is VARA's command port (8300 unless set in VARA's settings); its data port is the
//! next. `--fm` says the modem is VARA FM, which has no bandwidths to choose.
//!
//! VARA's TNC interface is the one Mercury copied, so the adapter is Mercury's bridge
//! (`radiopost-mercury`) speaking VARA's published commands: EA5HVK's *VARA Protocol Native TNC
//! Commands*. Nothing else about VARA is assumed.

use std::process::ExitCode;

use radiopost_mercury::bridge::Dialect;
use radiopost_mercury::serve::{DEFAULT_MERCURY, Note, listen_speaking};

const USAGE: &str =
    "usage: radiopost-vara [--listen 127.0.0.1:8600] [--vara 127.0.0.1:8300] [--fm]";

fn main() -> ExitCode {
    let mut listen = format!("127.0.0.1:{}", rpmi::DEFAULT_PORT);
    // VARA's command port is 8300 by default, as Mercury's is
    let mut vara = DEFAULT_MERCURY.to_owned();
    let mut dialect = Dialect::VaraHf;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--fm" => dialect = Dialect::VaraFm,
            "--listen" | "--vara" => {
                let Some(value) = args.next() else {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                };
                if arg == "--listen" {
                    listen = value;
                } else {
                    vara = value;
                }
            }
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let note: Note = std::sync::Arc::new(|line: &str| eprintln!("radiopost-vara: {line}"));
    match listen_speaking(&listen, &vara, dialect, note) {
        Ok(at) => {
            eprintln!(
                "radiopost-vara: RPMI on {at}, {}'s command port at {vara}",
                dialect.name()
            );
            loop {
                std::thread::park();
            }
        }
        Err(e) => {
            eprintln!("radiopost-vara: cannot listen on {listen}: {e}");
            ExitCode::FAILURE
        }
    }
}
