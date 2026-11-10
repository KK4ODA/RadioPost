// SPDX-License-Identifier: MIT OR Apache-2.0
//! `radiopost-aether`: the RPMI adapter for Aether HF.
//!
//! ```text
//! radiopost-aether [--listen 127.0.0.1:8600] [--aether ws://127.0.0.1:8515/v1]
//! ```

use std::process::ExitCode;

const USAGE: &str =
    "usage: radiopost-aether [--listen 127.0.0.1:8600] [--aether ws://127.0.0.1:8515/v1]";

fn main() -> ExitCode {
    let mut listen = format!("127.0.0.1:{}", rpmi::DEFAULT_PORT);
    let mut aether = "ws://127.0.0.1:8515/v1".to_owned();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next();
        match (arg.as_str(), value) {
            ("--listen", Some(v)) => listen = v,
            ("--aether", Some(v)) => aether = v,
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    match radiopost_aether::serve::listen(&listen, &aether) {
        Ok(at) => {
            eprintln!("radiopost-aether: RPMI on {at}, Aether at {aether}");
            loop {
                std::thread::park();
            }
        }
        Err(e) => {
            eprintln!("radiopost-aether: cannot listen on {listen}: {e}");
            ExitCode::FAILURE
        }
    }
}
