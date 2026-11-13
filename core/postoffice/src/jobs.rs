// SPDX-License-Identifier: MIT OR Apache-2.0
//! Jobs the operator asks for from the desk that need the server's administrator (D35): an
//! update, a restart, a log to read. Nothing here runs a command a request names.
//!
//! The post office runs as an ordinary user and cannot do them. It writes the job's name into
//! `requests/new/`; a root-owned systemd path unit (`radiopost-po-jobs.path`) sees the folder
//! become non-empty and starts `radiopost-po jobs` as root, which runs the job from the fixed
//! table below — programs and arguments, no shell — and writes how it went, with what it said,
//! beside the request, where the desk reads it. `radiopost-po jobs --install` puts the two units
//! in place once.

use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value as Json, json};

/// A job: its name, what it does in the operator's words, and its commands, run in order until
/// one fails.
#[derive(Clone, Debug)]
pub struct Job {
    /// Its name, as asked for.
    pub name: &'static str,
    /// What it does.
    pub what: &'static str,
    /// Each command: the program and its arguments.
    pub commands: Vec<Vec<String>>,
}

fn cmd(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| (*w).to_owned()).collect()
}

/// The jobs, for the post office at `dir` and the program at `exe`.
#[must_use]
#[allow(clippy::too_many_lines, reason = "the fixed table, a job each")]
pub fn table(dir: &Path, exe: &Path) -> Vec<Job> {
    let exe = exe.to_string_lossy().into_owned();
    let dir = dir.to_string_lossy().into_owned();
    vec![
        Job {
            name: "update",
            what: "Install the newest RadioPost release, its signature checked, and restart the post office",
            commands: vec![
                vec![exe.clone(), "update".into()],
                cmd(&["systemctl", "restart", "radiopost-po"]),
            ],
        },
        Job {
            name: "restart",
            what: "Restart the post office",
            commands: vec![cmd(&["systemctl", "restart", "radiopost-po"])],
        },
        Job {
            name: "restart-mail",
            what: "Restart the mail server (Stalwart)",
            commands: vec![cmd(&["systemctl", "restart", "stalwart"])],
        },
        Job {
            name: "certificate",
            what: "Copy the web server's certificate to the mail server now",
            commands: vec![cmd(&["systemctl", "start", "stalwart-certs"])],
        },
        Job {
            name: "services",
            what: "Whether the post office, the mail server and the web server run",
            commands: vec![cmd(&[
                "systemctl",
                "--no-pager",
                "--lines=0",
                "status",
                "radiopost-po",
                "stalwart",
                "caddy",
            ])],
        },
        Job {
            name: "log",
            what: "The post office's log, its last 300 lines",
            commands: vec![cmd(&[
                "journalctl",
                "--no-pager",
                "-u",
                "radiopost-po",
                "-n",
                "300",
            ])],
        },
        Job {
            name: "mail-log",
            what: "The mail server's log, its last 200 lines",
            commands: vec![cmd(&[
                "journalctl",
                "--no-pager",
                "-u",
                "stalwart",
                "-n",
                "200",
            ])],
        },
        Job {
            name: "disk",
            what: "Disk space",
            commands: vec![cmd(&["df", "-h", "/", &dir])],
        },
        Job {
            name: "firewall",
            what: "The firewall's rules",
            commands: vec![cmd(&["ufw", "status", "verbose"])],
        },
        Job {
            name: "ports",
            what: "What listens on which port",
            commands: vec![cmd(&["ss", "-tlnp"])],
        },
        Job {
            name: "logins",
            what: "Who logged in to the server lately",
            commands: vec![cmd(&["last", "-n", "20", "-a"])],
        },
        Job {
            name: "system-updates",
            what: "Debian's security updates, now rather than tonight",
            commands: vec![cmd(&["unattended-upgrade", "-v"])],
        },
        Job {
            name: "mail-backup",
            what: "Back up the mail server's mailboxes now: Stalwart stopped for a moment, its data archived, started again; the last three kept (done each night)",
            commands: vec![vec![
                exe.clone(),
                "--dir".into(),
                dir.clone(),
                "mail-backup".into(),
            ]],
        },
        Job {
            name: "check",
            what: "The post office checked from outside, as `radiopost-po check` does",
            commands: vec![vec![exe, "--dir".into(), dir, "check".into()]],
        },
    ]
}

fn requests(dir: &Path) -> PathBuf {
    dir.join("requests")
}

/// Asks for `name`, by `who`. Returns whether it was taken: a job not in the table is not.
///
/// # Errors
///
/// The request could not be written.
pub fn request(dir: &Path, jobs: &[Job], name: &str, who: &str, at: u64) -> Result<bool, String> {
    if !jobs.iter().any(|j| j.name == name) {
        return Ok(false);
    }
    let new = requests(dir).join("new");
    std::fs::create_dir_all(&new).map_err(|e| e.to_string())?;
    let body = json!({"job": name, "by": who, "at": at});
    std::fs::write(new.join(name), body.to_string()).map_err(|e| e.to_string())?;
    Ok(true)
}

/// Where each job stands: asked for, running, and how it last went.
#[must_use]
pub fn status(dir: &Path, jobs: &[Job]) -> Json {
    let r = requests(dir);
    let list: Vec<Json> = jobs
        .iter()
        .map(|j| {
            let last = std::fs::read_to_string(r.join(format!("{}.done", j.name)))
                .ok()
                .and_then(|t| serde_json::from_str::<Json>(&t).ok());
            json!({"name": j.name, "what": j.what,
                   "asked": r.join("new").join(j.name).exists(),
                   "running": r.join(format!("{}.running", j.name)).exists(),
                   "last": last})
        })
        .collect();
    json!({"jobs": list, "runner": runner_installed()})
}

/// Whether the root-owned units that run the jobs are in place.
#[must_use]
pub fn runner_installed() -> bool {
    Path::new("/etc/systemd/system/radiopost-po-jobs.path").exists()
}

/// Runs one command, at most `limit`, its output (both streams) cut at 64 kB.
fn run(command: &[String], limit: Duration) -> (bool, String) {
    let Some((program, args)) = command.split_first() else {
        return (false, "no command".into());
    };
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return (false, format!("{program}: {e}\n")),
    };
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => break None,
        }
    };
    let mut out = String::new();
    if let Some(mut o) = child.stdout.take() {
        let _ = o.read_to_string(&mut out);
    }
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut out);
    }
    if out.len() > 65_536 {
        let cut = out.len() - 65_536;
        let at = (cut..out.len())
            .find(|i| out.is_char_boundary(*i))
            .unwrap_or(out.len());
        out = format!("…\n{}", &out[at..]);
    }
    match status {
        Some(s) => (s.success(), out),
        None => (
            false,
            format!("{out}\nstopped after {} minutes\n", limit.as_secs() / 60),
        ),
    }
}

/// The runner: every job asked for, in the order asked, each written down when it ends. Returns
/// a line per job, for the journal.
#[must_use]
pub fn run_pending(dir: &Path, jobs: &[Job], now: impl Fn() -> u64) -> Vec<String> {
    let r = requests(dir);
    let mut asked: Vec<(u64, PathBuf)> = std::fs::read_dir(r.join("new"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let at = e.metadata().ok()?.modified().ok()?;
            let at = at.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
            Some((at, e.path()))
        })
        .collect();
    asked.sort();
    let mut said = Vec::new();
    for (_, path) in asked {
        let request: Json = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Json::Null);
        let _ = std::fs::remove_file(&path);
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let Some(job) = jobs.iter().find(|j| j.name == name) else {
            said.push(format!("no job called {name:?}: left alone"));
            continue;
        };
        let running = r.join(format!("{name}.running"));
        let started = now();
        let _ = std::fs::write(&running, started.to_string());
        let mut output = String::new();
        let mut ok = true;
        for c in &job.commands {
            let _ = writeln!(output, "$ {}", c.join(" "));
            let (good, out) = run(c, Duration::from_secs(900));
            output.push_str(&out);
            if !good {
                ok = false;
                break;
            }
        }
        let done = json!({"job": name, "by": request["by"], "asked": request["at"],
                          "started": started, "finished": now(), "ok": ok, "output": output});
        let _ = std::fs::write(r.join(format!("{name}.done")), done.to_string());
        let _ = std::fs::remove_file(&running);
        said.push(format!("{name}: {}", if ok { "done" } else { "failed" }));
    }
    said
}

/// The two systemd units that run the jobs as root, for the post office at `dir` and the program
/// at `exe`.
#[must_use]
pub fn units(dir: &Path, exe: &Path) -> (String, String) {
    let dir = dir.display();
    let exe = exe.display();
    (
        format!(
            "[Unit]\nDescription=Jobs the RadioPost post office's operators ask for\n\n\
             [Path]\nDirectoryNotEmpty={dir}/requests/new\nMakeDirectory=yes\n\n\
             [Install]\nWantedBy=multi-user.target\n"
        ),
        format!(
            "[Unit]\nDescription=Run the jobs the RadioPost post office's operators asked for\n\n\
             [Service]\nType=oneshot\nExecStart={exe} --dir {dir} jobs\n"
        ),
    )
}

/// Puts the units in place and starts watching (run as root once).
///
/// # Errors
///
/// Not root, or systemd refused.
pub fn install(dir: &Path, exe: &Path) -> Result<String, String> {
    let (path_unit, service_unit) = units(dir, exe);
    let at = Path::new("/etc/systemd/system");
    std::fs::write(at.join("radiopost-po-jobs.path"), path_unit)
        .map_err(|e| format!("{e}: run it with sudo"))?;
    std::fs::write(at.join("radiopost-po-jobs.service"), service_unit)
        .map_err(|e| format!("{e}: run it with sudo"))?;
    let new = requests(dir).join("new");
    std::fs::create_dir_all(&new).map_err(|e| e.to_string())?;
    let _ = Command::new("chown")
        .args(["-R", "radiopost:radiopost"])
        .arg(requests(dir))
        .status();
    for args in [
        vec!["daemon-reload"],
        vec!["enable", "--now", "radiopost-po-jobs.path"],
    ] {
        let ok = Command::new("systemctl")
            .args(&args)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return Err(format!("systemctl {} failed", args.join(" ")));
        }
    }
    Ok("the jobs runner watches for the desk's requests".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_asked_for_runs_once_and_says_how_it_went() {
        let dir = std::env::temp_dir().join(format!("radiopost-jobs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory");
        let jobs = vec![
            Job {
                name: "hello",
                what: "says hello",
                commands: vec![cmd(&["echo", "hello"])],
            },
            Job {
                name: "fails",
                what: "fails at the first",
                commands: vec![cmd(&["false"]), cmd(&["echo", "never"])],
            },
        ];
        assert_eq!(request(&dir, &jobs, "rm -rf /", "KK4ODA", 1), Ok(false));
        assert_eq!(request(&dir, &jobs, "hello", "KK4ODA", 1), Ok(true));
        assert_eq!(request(&dir, &jobs, "fails", "KK4ODA", 2), Ok(true));
        assert_eq!(status(&dir, &jobs)["jobs"][0]["asked"], json!(true));
        let said = run_pending(&dir, &jobs, || 10);
        assert_eq!(said.len(), 2, "{said:?}");
        let s = status(&dir, &jobs);
        let hello = &s["jobs"][0]["last"];
        assert_eq!(
            (hello["ok"].as_bool(), hello["by"].as_str()),
            (Some(true), Some("KK4ODA"))
        );
        assert!(
            hello["output"]
                .as_str()
                .is_some_and(|o| o.contains("$ echo hello\nhello"))
        );
        let fails = &s["jobs"][1]["last"];
        assert_eq!(fails["ok"], json!(false));
        assert!(!fails["output"].as_str().unwrap_or("").contains("never"));
        assert_eq!(s["jobs"][0]["asked"], json!(false));
        assert!(run_pending(&dir, &jobs, || 11).is_empty(), "each runs once");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_mailboxes_are_backed_up_by_the_program_as_root() {
        let table = table(
            Path::new("/var/lib/radiopost"),
            Path::new("/usr/local/bin/radiopost-po"),
        );
        let job = table
            .iter()
            .find(|j| j.name == "mail-backup")
            .expect("the job");
        assert_eq!(
            job.commands,
            [cmd(&[
                "/usr/local/bin/radiopost-po",
                "--dir",
                "/var/lib/radiopost",
                "mail-backup"
            ])]
        );
    }

    #[test]
    fn the_units_watch_the_requests_and_run_the_program() {
        let (path, service) = units(
            Path::new("/var/lib/radiopost"),
            Path::new("/usr/local/bin/radiopost-po"),
        );
        assert!(path.contains("DirectoryNotEmpty=/var/lib/radiopost/requests/new"));
        assert!(
            service.contains("ExecStart=/usr/local/bin/radiopost-po --dir /var/lib/radiopost jobs")
        );
    }
}
