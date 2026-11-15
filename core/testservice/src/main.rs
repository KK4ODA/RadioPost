// SPDX-License-Identifier: MIT OR Apache-2.0
//! `radiopost-test-service`: the public test service (wishlist W43).

use std::path::PathBuf;
use std::process::ExitCode;

use radiopost_test_service::{GATEWAY, STATION, Settings, start};

const USAGE: &str = "\
usage: radiopost-test-service --dir DIR --domain DOMAIN [--url URL] [--web ADDRESS]
                              [--air ADDRESS] [--plain]

  --dir      where it keeps the test post office, its stations' keys, its TLS certificate and
             the sessions' reports
  --domain   the test post office's domain, such as test.radiopost.org. Publish nothing for it in
             DNS: no real gateway or post office is to trust its credentials (RP-1 §7.3)
  --url      the service as the world reaches it, such as https://test.radiopost.org, behind an
             HTTPS front that hands requests to --web; its API is under /rp/v1
  --web      where it serves HTTP (127.0.0.1:8630)
  --air      where it takes calls from other TCP adapters (0.0.0.0:8774, RP-4 §9.5)
  --plain    plain TCP links instead of TLS: a test on one computer only

It answers as RPTEST-1, a gateway of the test post office, and RPTEST-2, a client's station, one
session at a time, and keeps a report of each.";

fn one(words: &[String], flag: &str) -> Option<String> {
    let i = words.iter().position(|w| w == flag)?;
    words.get(i + 1).cloned()
}

fn main() -> ExitCode {
    let words: Vec<String> = std::env::args().skip(1).collect();
    let (Some(dir), Some(domain)) = (one(&words, "--dir"), one(&words, "--domain")) else {
        println!("{USAGE}");
        return ExitCode::from(2);
    };
    let settings = Settings {
        dir: PathBuf::from(dir),
        domain,
        url: one(&words, "--url").unwrap_or_default(),
        web: one(&words, "--web").unwrap_or_else(|| "127.0.0.1:8630".into()),
        air: one(&words, "--air").unwrap_or_else(|| "0.0.0.0:8774".into()),
        plain: words.iter().any(|w| w == "--plain"),
    };
    match start(&settings) {
        Ok(service) => {
            println!(
                "the page and the test post office's API: {} (at {})",
                service.url, service.web
            );
            println!("calls to {GATEWAY} and {STATION}: {}", service.air);
            match &service.fingerprint {
                Some(f) => println!("TLS certificate, for callers to pin: {f}"),
                None => println!("links are plain TCP: for a test on one computer only"),
            }
            loop {
                std::thread::park();
            }
        }
        Err(e) => {
            eprintln!("radiopost-test-service: {e}");
            ExitCode::from(2)
        }
    }
}
