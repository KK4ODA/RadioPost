// SPDX-License-Identifier: MIT OR Apache-2.0
//! The gateway answering on a thread of its own, started and stopped by its operator, with its
//! post offices checked beside it: what the gateway app and `radiopost-gateway panel` run.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::monitor::Monitor;
use crate::station::{self, Station};

/// How often the post offices are checked while the gateway answers.
pub const CHECK_EVERY: Duration = Duration::from_secs(600);

/// A gateway's answering loop and its checks, for the gateway in one directory.
pub struct Runner {
    dir: PathBuf,
    monitor: Arc<Monitor>,
    answering: Mutex<Option<JoinHandle<()>>>,
    /// Each start's number: a check thread ends when another start, or a stop, follows its own.
    generation: Arc<AtomicU64>,
    /// What the gateway says goes to standard output too.
    echo: bool,
}

impl Runner {
    /// A runner for the gateway in `dir`, keeping its sessions there.
    #[must_use]
    pub fn new(dir: &Path) -> Runner {
        Runner {
            dir: dir.to_owned(),
            monitor: Arc::new(Monitor::keeping(dir)),
            answering: Mutex::new(None),
            generation: Arc::new(AtomicU64::new(0)),
            echo: false,
        }
    }

    /// The same runner, saying what the gateway does on standard output too, for a gateway run
    /// as a service, whose output goes to its log.
    #[must_use]
    pub fn echoing(mut self) -> Runner {
        self.echo = true;
        self
    }

    /// The gateway's directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// What the gateway is doing.
    #[must_use]
    pub fn monitor(&self) -> &Arc<Monitor> {
        &self.monitor
    }

    fn answering(&self) -> MutexGuard<'_, Option<JoinHandle<()>>> {
        self.answering
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the gateway is answering, or waiting for its modem to answer.
    #[must_use]
    pub fn running(&self) -> bool {
        self.answering().as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Starts answering calls, and checking the post offices every ten minutes.
    ///
    /// # Errors
    ///
    /// It already runs, or the gateway is not set up and registered.
    pub fn start(&self) -> Result<(), String> {
        let mut answering = self.answering();
        if answering.as_ref().is_some_and(|t| !t.is_finished()) {
            return Err("the gateway is already answering".into());
        }
        Station::load(&self.dir)?;
        self.monitor.clear_stop();
        self.monitor.set_state("opening", None);
        let (dir, monitor, echo) = (self.dir.clone(), self.monitor.clone(), self.echo);
        *answering = Some(thread::spawn(move || {
            let mut say = |line: &str| {
                if echo {
                    println!("{line}");
                }
            };
            if let Err(e) = station::run(&dir, &monitor, &mut say) {
                say(&format!("stopped: {e}"));
                monitor.log(&format!("stopped: {e}"));
                monitor.set_state("stopped", Some(e));
            }
        }));
        // the checks run apart: a post office that does not answer holds up nothing
        let mine = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let (dir, monitor, generation) = (
            self.dir.clone(),
            self.monitor.clone(),
            self.generation.clone(),
        );
        thread::spawn(move || {
            let current = || generation.load(Ordering::Relaxed) == mine && !monitor.stopping();
            while current() {
                if let Ok(station) = Station::load(&dir) {
                    let checks = station.check();
                    if current() {
                        monitor.set_checks(checks);
                    }
                }
                let mut waited = Duration::ZERO;
                while waited < CHECK_EVERY && current() {
                    thread::sleep(Duration::from_millis(200));
                    waited += Duration::from_millis(200);
                }
            }
        });
        Ok(())
    }

    /// Stops answering: a session under way ends, and the modem is let go. Returns once the loop
    /// has stopped.
    pub fn stop(&self) {
        self.monitor.stop();
        self.generation.fetch_add(1, Ordering::Relaxed);
        let answering = self.answering().take();
        if let Some(t) = answering {
            let _ = t.join();
        }
    }

    /// Checks the post offices now, and says how they answered.
    ///
    /// # Errors
    ///
    /// The gateway is not set up and registered.
    pub fn check(&self) -> Result<(), String> {
        let station = Station::load(&self.dir)?;
        self.monitor.set_checks(station.check());
        Ok(())
    }
}
