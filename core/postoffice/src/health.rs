// SPDX-License-Identifier: MIT OR Apache-2.0
//! The post office looking after itself (D35): `check` every hour, with what only the server
//! sees (its disk, mail stuck going out, its backups, its jobs runner); a backup every night;
//! and an email to its contact address when a check turns red, and again when all is well.
//!
//! The operator reads the same lines on the desk, so the commands of the guide are for setting
//! up, not for every day.

use std::fmt::Write as _;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value as Json, json};

use crate::backups;
use crate::check::{self, Line, Verdict};
use crate::jobs;
use crate::lookups::day;
use crate::serve::{Running, now};

/// How often the post office checks itself.
pub const EVERY_S: u64 = 3600;

/// The last check, and what the operators were last told of.
#[derive(Default)]
pub struct Care {
    last: Mutex<Option<Json>>,
    told: Mutex<Vec<String>>,
}

fn line(verdict: Verdict, text: String) -> Line {
    Line { verdict, text }
}

/// What is free on the disk that holds `dir`, from `df -Pk`: kilobytes free and the share used.
fn disk(dir: &std::path::Path) -> Option<(u64, u64)> {
    let out = std::process::Command::new("df")
        .arg("-Pk")
        .arg(dir)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let f: Vec<&str> = text.lines().nth(1)?.split_whitespace().collect();
    let free = f.get(3)?.parse().ok()?;
    let used = f.get(4)?.trim_end_matches('%').parse().ok()?;
    Some((free, used))
}

impl Running {
    pub(crate) fn care(&self) -> &Care {
        &self.care_state
    }

    /// What only the server sees: its disk, mail stuck going out, its backups, its runner.
    fn local_lines(&self) -> Vec<Line> {
        let dir = self.files().dir().to_path_buf();
        let at = now();
        let mut out = Vec::new();
        match disk(&dir) {
            Some((free, used)) if free < 1_048_576 || used >= 95 => out.push(line(
                Verdict::Fail,
                format!("disk: {} MB free ({used}% used): make room", free / 1024),
            )),
            Some((free, used)) => out.push(line(
                Verdict::Ok,
                format!(
                    "disk: {}.{} GB free ({used}% used)",
                    free / 1_048_576,
                    free % 1_048_576 * 10 / 1_048_576
                ),
            )),
            None => out.push(line(Verdict::Note, "disk: `df` did not answer".into())),
        }
        let oldest = self.office().state.outgoing.iter().map(|o| o.queued).min();
        match oldest {
            Some(q) if q > 0 && at.saturating_sub(q) > 7200 => out.push(line(
                Verdict::Fail,
                format!(
                    "email out: a message has waited {} hours: the mail server refuses it or is down",
                    at.saturating_sub(q) / 3600
                ),
            )),
            Some(_) => out.push(line(Verdict::Ok, "email out: moving".into())),
            None => out.push(line(Verdict::Ok, "email out: nothing waiting".into())),
        }
        match backups::list(&dir).first() {
            Some((name, _, written)) if at.saturating_sub(*written) > 2 * 86_400 => out.push(line(
                Verdict::Fail,
                format!(
                    "backups: the last, {name}, is {} days old",
                    at.saturating_sub(*written) / 86_400
                ),
            )),
            Some((name, _, _)) => {
                out.push(line(Verdict::Ok, format!("backups: last night's, {name}")));
            }
            None => out.push(line(
                Verdict::Note,
                "backups: none yet; the first is made tonight".into(),
            )),
        }
        out.extend(self.known_lines(at));
        if !jobs::runner_installed() {
            out.push(line(
                Verdict::Note,
                "jobs: the runner is not installed, so the desk cannot update or restart the \
                 server: `sudo radiopost-po --dir DIR jobs --install`, once"
                    .into(),
            ));
        }
        if let Some(u) = self.update_status()
            && let Some(v) = u["offer"]["version"].as_str()
        {
            out.push(line(
                Verdict::Note,
                format!("RadioPost {v} is out: the desk's Server tab installs it"),
            ));
        }
        out
    }

    /// The other post offices it knows (RP-1 §7.5): one fetched before and not for a day fails,
    /// since its revocations may be missed; one never fetched, known by a key typed for it, is
    /// noted.
    fn known_lines(&self, at: u64) -> Vec<Line> {
        let settings = self.settings();
        let office = self.office();
        let mut out = Vec::new();
        for domain in crate::known::accepted(&settings) {
            if settings.blocked.contains(&domain) {
                continue;
            }
            let fetched = office.state.post_offices.get(&domain);
            let why = fetched
                .and_then(|f| f.error.clone())
                .unwrap_or_else(|| "not tried yet".into());
            match fetched.and_then(|f| f.found.as_ref()) {
                Some(found) if at.saturating_sub(found.at) > 86_400 => out.push(line(
                    Verdict::Fail,
                    format!(
                        "{domain}: last fetched {} days ago, so its revocations may be missed: \
                         {why}",
                        at.saturating_sub(found.at) / 86_400
                    ),
                )),
                Some(_) => {}
                None => out.push(line(
                    Verdict::Note,
                    format!("{domain}: known, never fetched from DNS: {why}"),
                )),
            }
        }
        out
    }

    /// Checks the post office now, inside and out; keeps the result; tells the operators when a
    /// check turned red, or when all is well again. Returns the result.
    pub fn health_now(&self) -> Json {
        let settings = self.settings();
        let public = self
            .files()
            .key()
            .map_or([0; 32], |k| k.verifying_key().to_bytes());
        let mut lines = check::run(
            &settings,
            &public,
            radiopost_api::RESOLVER,
            &settings.health.dkim,
        );
        lines.extend(self.local_lines());
        let fails: Vec<String> = lines
            .iter()
            .filter(|l| l.verdict == Verdict::Fail)
            .map(|l| l.text.clone())
            .collect();
        let result = json!({
            "at": now(),
            "fails": fails.len(),
            "lines": lines.iter().map(|l| json!({
                "verdict": match l.verdict { Verdict::Ok => "ok", Verdict::Fail => "fail", Verdict::Note => "note" },
                "text": l.text,
            })).collect::<Vec<_>>(),
        });
        let _ = self.files().write("health.json", &result.to_string());
        *self
            .care()
            .last
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(result.clone());
        self.alert(&fails);
        result
    }

    /// The last check, from memory or `health.json`.
    #[must_use]
    pub fn health(&self) -> Option<Json> {
        let held = self
            .care()
            .last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        held.or_else(|| {
            std::fs::read_to_string(self.files().dir().join("health.json"))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
        })
    }

    /// An email to the contact address when the failing checks change: new ones, or none left.
    fn alert(&self, fails: &[String]) {
        let mut told = self
            .care()
            .told
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let new: Vec<&String> = fails.iter().filter(|f| !told.contains(f)).collect();
        let cleared = fails.is_empty() && !told.is_empty();
        if new.is_empty() && !cleared {
            *told = fails.to_vec();
            return;
        }
        let domain = self.settings().domains.first().cloned().unwrap_or_default();
        let (subject, text) = if cleared {
            (
                format!("RadioPost {domain}: all checks pass again"),
                "Every check of the post office passes again.\n".to_owned(),
            )
        } else {
            let mut text = format!(
                "{} check{} of the post office {domain} failed:\n\n",
                fails.len(),
                if fails.len() == 1 { "" } else { "s" }
            );
            for f in fails {
                let _ = writeln!(text, "  {f}");
            }
            text.push_str(
                "\nThe Health tab of the Post office in your RadioPost client shows every check, \
                 and checks again; its Server tab has the jobs that restart or update the server.\n",
            );
            (
                format!(
                    "RadioPost {domain}: {} check{} failed",
                    fails.len(),
                    if fails.len() == 1 { "" } else { "s" }
                ),
                text,
            )
        };
        let to = self.settings().info.contact;
        drop(told);
        match self.send_notice(&to, &subject, &text) {
            Ok(true) => eprintln!("radiopost-po: told {to}: {subject}"),
            Ok(false) => eprintln!("radiopost-po: {subject} (no `smtp` to tell {to})"),
            Err(e) => eprintln!("radiopost-po: could not tell {to} that {subject}: {e}"),
        }
        *self
            .care()
            .told
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = fails.to_vec();
    }

    /// Tonight's backup, said in the log.
    ///
    /// # Errors
    ///
    /// The backup could not be made or written.
    pub fn back_up(&self) -> Result<String, String> {
        let path = backups::nightly(self.files().dir(), &day(now()))?;
        Ok(path.display().to_string())
    }
}

/// The hourly check and the nightly backup, each on a thread of its own.
pub fn start(running: &Arc<Running>) {
    let checker = Arc::clone(running);
    std::thread::spawn(move || {
        // the first a minute after starting: the service and its neighbours are up by then
        std::thread::sleep(Duration::from_secs(60));
        loop {
            let h = checker.health_now();
            if h["fails"].as_u64().unwrap_or(0) > 0 {
                eprintln!("radiopost-po: health: {} checks failed", h["fails"]);
            }
            std::thread::sleep(Duration::from_secs(EVERY_S));
        }
    });
    let keeper = Arc::clone(running);
    std::thread::spawn(move || {
        loop {
            let today = day(now());
            let done = backups::list(keeper.files().dir())
                .first()
                .is_some_and(|(name, _, _)| name.contains(&today));
            if !done {
                match keeper.back_up() {
                    Ok(path) => eprintln!("radiopost-po: backup written: {path}"),
                    Err(e) => eprintln!("radiopost-po: the backup failed: {e}"),
                }
            }
            std::thread::sleep(Duration::from_secs(3600));
        }
    });
}
