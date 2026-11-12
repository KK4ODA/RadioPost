// SPDX-License-Identifier: MIT OR Apache-2.0
//! The adapter against real Mercury modems: two `mercury` processes whose audio is joined through
//! named pipes (Mercury's `fifo` sound system, as its own `tests/integration` joins two), each
//! with this adapter in front, pass the conformance tester's A-01 to A-04. A-04 carries
//! session-01 (RP-3 §12) across, byte for byte.
//!
//! It needs a `mercury` of 1.9.23 or later, named by `MERCURY`, built from Rhizomatica's
//! repository (`make HAVE_HAMLIB=0`, which needs `libasound2-dev` and `libpulse-dev`), and takes
//! as long as the calls take on the air (minutes):
//!
//! ```text
//! MERCURY=/path/to/mercury cargo test -p radiopost-mercury --test mercury -- --ignored
//! ```
//!
//! Each Mercury keeps its logs, and what state it keeps, in the test's own directory.

#![cfg(unix)]

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use radiopost_mercury::serve::listen;
use radiopost_tester::adapter::{Plan, run};

struct Modem {
    child: Child,
    log: PathBuf,
}

impl Drop for Modem {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A free port whose next is free too, and a third for broadcasts.
fn ports() -> (u16, u16) {
    for _ in 0..200 {
        let a = TcpListener::bind("127.0.0.1:0").expect("a port");
        let base = a.local_addr().expect("bound").port();
        let Some(next) = base.checked_add(1) else {
            continue;
        };
        let Ok(b) = TcpListener::bind(("127.0.0.1", next)) else {
            continue;
        };
        let c = TcpListener::bind("127.0.0.1:0").expect("a port");
        let broadcast = c.local_addr().expect("bound").port();
        drop((a, b, c));
        return (base, broadcast);
    }
    panic!("no two ports in a row are free");
}

fn fifo(path: &Path) {
    let made = Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("mkfifo runs");
    assert!(made.success(), "mkfifo {}", path.display());
}

impl Modem {
    fn start(binary: &str, dir: &Path, name: &str, rx: &Path, tx: &Path) -> (Modem, u16) {
        let (base, broadcast) = ports();
        let state = dir.join(format!("{name}-state"));
        std::fs::create_dir_all(&state).expect("a state directory");
        let log = dir.join(format!("{name}.log"));
        let child = Command::new(binary)
            .args(["-x", "fifo", "-m", "1"])
            .arg("-i")
            .arg(rx)
            .arg("-o")
            .arg(tx)
            .args(["-p", &base.to_string(), "-b", &broadcast.to_string()])
            // no configuration file: none of the user's is read
            .arg("-C")
            .arg(dir.join(format!("{name}-none.ini")))
            .env("XDG_STATE_HOME", &state)
            .env("XDG_CONFIG_HOME", &state)
            .stdout(Stdio::from(File::create(&log).expect("a log")))
            .stderr(Stdio::from(
                File::create(dir.join(format!("{name}.err"))).expect("a log"),
            ))
            .spawn()
            .expect("mercury starts");
        (Modem { child, log }, base)
    }

    /// Waits for the control port, as long as the modem runs.
    fn ready(&mut self, base: u16) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!("mercury exited {status}: see {}", self.log.display());
            }
            if TcpStream::connect(("127.0.0.1", base)).is_ok() {
                // that connection took the control port and gave it back: let Mercury see it go
                thread::sleep(Duration::from_millis(300));
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!("mercury's control port {base} never opened");
    }
}

/// Carries one station's transmitted audio to the other's receiver, for as long as both run.
fn join(from: PathBuf, to: PathBuf) {
    thread::spawn(move || {
        // each open waits for Mercury's end of the pipe
        let Ok(mut tx) = File::open(&from) else {
            return;
        };
        let Ok(mut rx) = OpenOptions::new().write(true).open(&to) else {
            return;
        };
        let mut buf = [0u8; 4096];
        loop {
            match tx.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    if rx.write_all(&buf[..n]).is_err() {
                        return;
                    }
                }
            }
        }
    });
}

#[test]
#[ignore = "needs a mercury binary: MERCURY=<path>"]
fn two_adapters_on_two_mercury_modems_pass_a01_to_a04() {
    let binary = std::env::var("MERCURY").expect("MERCURY names a mercury binary");
    let dir = std::env::temp_dir().join(format!("radiopost-mercury-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a directory");
    let pipe = |name: &str| {
        let p = dir.join(name);
        let _ = std::fs::remove_file(&p);
        fifo(&p);
        p
    };
    let (a_rx, a_tx, b_rx, b_tx) = (pipe("a_rx"), pipe("a_tx"), pipe("b_rx"), pipe("b_tx"));
    let (mut a, a_base) = Modem::start(&binary, &dir, "a", &a_rx, &a_tx);
    let (mut b, b_base) = Modem::start(&binary, &dir, "b", &b_rx, &b_tx);
    join(a_tx.clone(), b_rx.clone());
    join(b_tx.clone(), a_rx.clone());
    a.ready(a_base);
    b.ready(b_base);

    let near = listen("127.0.0.1:0", &format!("127.0.0.1:{a_base}")).expect("a port");
    let far = listen("127.0.0.1:0", &format!("127.0.0.1:{b_base}")).expect("a port");
    let plan = Plan {
        at: near.to_string(),
        far: Some(far.to_string()),
        reach: "N1CALL-10".into(),
        only: Vec::new(),
    };
    let verdicts = run(&plan, |v| {
        eprintln!(
            "{} {} ({:.0} s)",
            v.id,
            if v.passed == Some(true) {
                "passed"
            } else {
                "FAILED"
            },
            v.seconds
        );
    })
    .expect("the tester runs");
    let failed: Vec<String> = verdicts
        .iter()
        .filter(|v| v.passed != Some(true))
        .map(|v| format!("{}: {}\n  {}", v.id, v.why, v.transcript.join("\n  ")))
        .collect();
    assert_eq!(
        failed,
        Vec::<String>::new(),
        "Mercury's logs are in {}",
        dir.display()
    );
    assert_eq!(verdicts.len(), 4);
    drop((a, b));
    let _ = std::fs::remove_dir_all(&dir);
}
