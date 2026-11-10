// SPDX-License-Identifier: MIT OR Apache-2.0
//! `rpmi-loopback`: the loopback adapter, a simulated channel whose stations hosts reach over TCP,
//! for running RadioPost programs together with no radio.
//!
//! ```text
//! rpmi-loopback --station KK4ODA@127.0.0.1:8600 --station N0CALL@127.0.0.1:8601
//! ```

use std::process::ExitCode;
use std::time::Duration;

use rpmi::channel::{Channel, StationConfig};
use rpmi::tcp::Server;
use rpmi::wire::Caps;

const USAGE: &str = "usage: rpmi-loopback --station NAME@HOST:PORT [--station NAME@HOST:PORT]...";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut stations = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let ("--station", Some((name, addr))) =
            (a.as_str(), it.next().and_then(|v| v.split_once('@')))
        {
            stations.push((name.to_owned(), addr.to_owned()));
        } else {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    }
    if stations.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let mut channel = Channel::new();
    let numbers: Vec<usize> = stations
        .iter()
        .enumerate()
        .map(|(i, (name, _))| {
            channel.add_station(StationConfig {
                name: name.clone(),
                modem: "rpmi loopback".into(),
                caps: Caps {
                    bandwidths: vec![500, 2300],
                    max_links: 1,
                    delivery: true,
                    broadcast: true,
                    quality: false,
                    busy: true,
                    regulatory: false,
                    max_queue: 65536,
                    shared: true,
                },
                // link IDs: a letter for the station and a count, as in the vectors
                prefix: char::from(b'a' + u8::try_from(i % 26).unwrap_or(0)).to_string(),
                frame_bytes: 128,
            })
        })
        .collect();
    let server = Server::new(channel, Duration::from_secs(30));
    for ((name, addr), n) in stations.iter().zip(numbers) {
        match server.listen(n, addr.as_str()) {
            Ok(at) => eprintln!("rpmi-loopback: {name} on {at}"),
            Err(e) => {
                eprintln!("rpmi-loopback: cannot listen on {addr}: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    loop {
        std::thread::park();
    }
}
