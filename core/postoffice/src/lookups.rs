// SPDX-License-Identifier: MIT OR Apache-2.0
//! The service's lookups for its operators (D35): each new application's callsign looked up, on
//! a thread of its own, in the post office's copy of the FCC's licence database and in QRZ.com's
//! callbook when the operator gave an account; the copy fetched again each week.
//!
//! The answers go into the application's `evidence` (radiopost-po's own member of the entry,
//! RP-5 §7.3), which the operator's desk shows beside the decision.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::evidence::{self, Found};
use crate::licences::{self, FCC_EVERY_S, FCC_URL, Fcc};
use crate::qrz::{QRZ_URL, Qrz};
use crate::serve::{Running, now};

/// What the operator turned on, as `config.json`'s `lookups` holds it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct LookupSettings {
    /// Keep a copy of the FCC's licence database, fetched each week (about 200 MB to fetch,
    /// 150 MB kept).
    pub fcc: bool,
    /// The operator's QRZ.com username, for its XML interface.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qrz_username: Option<String>,
    /// Its password.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qrz_password: Option<String>,
}

impl LookupSettings {
    /// Nothing turned on: `config.json` leaves the member out.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == LookupSettings::default()
    }
}

/// The service's side of the lookups: the worker's queue, the QRZ session, whether the copy is
/// being fetched.
#[derive(Default)]
pub struct Lookups {
    queue: Mutex<Option<Sender<(String, bool)>>>,
    qrz: Mutex<Option<(String, Arc<Qrz>)>>,
    fetching: AtomicBool,
    fetched: Mutex<Option<String>>,
    me: OnceLock<Weak<Running>>,
}

impl Lookups {
    /// The service, for a thread of its own; none before [`start`].
    pub(crate) fn me(&self) -> Option<Arc<Running>> {
        self.me.get().and_then(Weak::upgrade)
    }
}

impl Running {
    fn fcc_path(&self) -> PathBuf {
        self.files().dir().join("licences").join("fcc.tsv")
    }

    /// The FCC copy, when it is turned on and there.
    fn fcc(&self) -> Option<Fcc> {
        if !self.settings().lookups.fcc {
            return None;
        }
        Fcc::open(&self.fcc_path())
    }

    /// QRZ, when the operator gave an account: one session kept while the account stays.
    fn qrz(&self) -> Option<Arc<Qrz>> {
        let s = self.settings().lookups;
        let (user, pass) = (s.qrz_username?, s.qrz_password?);
        let mut held = self
            .lookups()
            .qrz
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let id = format!("{user}\n{pass}");
        if held.as_ref().is_none_or(|(k, _)| *k != id) {
            *held = Some((id.clone(), Arc::new(Qrz::new(QRZ_URL, &user, &pass))));
        }
        held.as_ref().map(|(_, q)| Arc::clone(q))
    }

    /// The evidence for `call` given as `name`, with the post office's own history of the
    /// callsign (applications other than `skip`, and accounts).
    #[must_use]
    pub fn evidence_for(&self, call: &str, name: &str, skip: Option<&str>) -> Json {
        let call = licences::base_of(call);
        let found = Found {
            fcc: self.fcc().map(|f| f.find(&call)),
            qrz: self.qrz().map(|q| q.find(&call)),
        };
        let mut history = Vec::new();
        {
            let office = self.office();
            for a in &office.state.applications {
                if licences::base_of(&a.operator) == call && Some(a.application.as_str()) != skip {
                    let when = a.decided.unwrap_or(a.received);
                    history.push(format!(
                        "Applied before as {} for {}: {} ({})",
                        a.operator,
                        a.address,
                        a.status,
                        day(when)
                    ));
                }
            }
            let mut held: Vec<&str> = office
                .state
                .credentials
                .values()
                .filter(|c| licences::base_of(&c.operator) == call)
                .map(|c| c.address.as_str())
                .collect();
            held.sort_unstable();
            held.dedup();
            if !held.is_empty() {
                history.push(format!("{call} already holds {}", held.join(", ")));
            }
        }
        let mut e = evidence::judge(name, &call, &found, &history, &day(now()));
        e["checked"] = json!(now());
        e
    }

    /// Looks the application `id` up again and keeps what was found in it. Returns its entry.
    pub fn look_up(&self, id: &str) -> Option<Json> {
        let (call, name) = {
            let office = self.office();
            let a = office
                .state
                .applications
                .iter()
                .find(|a| a.application == id)?;
            (a.operator.clone(), a.name.clone())
        };
        let found = self.evidence_for(&call, &name, Some(id));
        let mut office = self.office();
        let a = office
            .state
            .applications
            .iter_mut()
            .find(|a| a.application == id)?;
        a.evidence = Some(found);
        let entry = a.entry();
        self.save(&office);
        Some(entry)
    }

    /// Hands a new application to the worker, or looks it up now when none runs.
    /// With `announce`, the operators are told of it once it is looked up.
    pub(crate) fn queue_lookup(&self, id: &str, announce: bool) {
        let queue = self
            .lookups()
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(q) = queue
            && q.send((id.to_owned(), announce)).is_ok()
        {
            return;
        }
        let _ = self.look_up(id);
        if announce {
            self.announce(id);
        }
    }

    /// Whether there is no copy, or one older than a week.
    pub(crate) fn fcc_stale(&self) -> bool {
        let made = Fcc::open(&self.fcc_path()).map_or(0, |f| f.about().1);
        now().saturating_sub(made) > FCC_EVERY_S
    }

    /// Whether the copy is being fetched now, and how the last fetch went.
    #[must_use]
    pub fn fcc_status(&self) -> Json {
        let (count, made) = Fcc::open(&self.fcc_path()).map_or((0, 0), |f| f.about());
        json!({
            "on": self.settings().lookups.fcc,
            "count": count,
            "made": made,
            "fetching": self.lookups().fetching.load(Ordering::Relaxed),
            "last": *self.lookups().fetched.lock().unwrap_or_else(PoisonError::into_inner),
        })
    }

    /// Fetches the FCC's weekly file and builds the copy, on a thread of its own, unless a
    /// fetch is under way. Says how it went in the log and in [`Running::fcc_status`].
    pub fn fetch_fcc(&self) -> bool {
        let Some(me) = self.lookups().me.get().and_then(Weak::upgrade) else {
            return false;
        };
        if self.lookups().fetching.swap(true, Ordering::SeqCst) {
            return false;
        }
        std::thread::spawn(move || {
            let dir = me.files().dir().join("licences");
            eprintln!("radiopost-po: fetching the FCC's licence database ({FCC_URL})");
            let said = match licences::update(&dir, FCC_URL) {
                Ok(n) => format!("{n} callsigns, fetched {}", day(now())),
                Err(e) => format!("the fetch failed {}: {e}", day(now())),
            };
            eprintln!("radiopost-po: FCC licence copy: {said}");
            *me.lookups()
                .fetched
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(said);
            me.lookups().fetching.store(false, Ordering::SeqCst);
        });
        true
    }
}

/// `YYYY-MM-DD` for a Unix time.
#[must_use]
pub fn day(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    // Howard Hinnant's civil_from_days
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let doe = shifted.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let mday = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{mday:02}")
}

/// Starts the worker that looks new applications up, and the weekly fetch of the FCC copy.
pub fn start(running: &Arc<Running>) {
    let _ = running.lookups().me.set(Arc::downgrade(running));
    let (tx, rx) = channel::<(String, bool)>();
    *running
        .lookups()
        .queue
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(tx);
    let worker = Arc::clone(running);
    std::thread::spawn(move || {
        for (id, announce) in rx {
            let _ = worker.look_up(&id);
            if announce {
                worker.announce(&id);
            }
        }
    });
    let weekly = Arc::clone(running);
    std::thread::spawn(move || {
        loop {
            if weekly.settings().lookups.fcc && weekly.fcc_stale() {
                weekly.fetch_fcc();
            }
            std::thread::sleep(Duration::from_secs(3600));
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn days_are_civil_dates() {
        assert_eq!(super::day(0), "1970-01-01");
        assert_eq!(super::day(1_794_494_400), "2026-11-12");
        assert_eq!(super::day(951_782_400), "2000-02-29");
    }
}
