// SPDX-License-Identifier: MIT OR Apache-2.0
//! A terminal on the post office's server, for its operators (design D35): the system's own
//! terminal running the system's own `ssh`, so the key stays in the system's own agent and the
//! client holds no password and no key of the server's.
//!
//! The target is checked to be `user@host` (or `user@host:port`) before anything runs, so nothing
//! typed there can become an option of `ssh` or a command of a shell.

use std::process::Command;

/// `user@host` or `user@host:port`, plain: what `ssh` is given, and nothing else.
fn target(text: &str) -> Option<(String, Option<u16>)> {
    let (user, rest) = text.split_once('@')?;
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) => (h, Some(p.parse::<u16>().ok()?)),
        None => (rest, None),
    };
    let word = |s: &str, extra: &[u8]| {
        !s.is_empty()
            && !s.starts_with('-')
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
    };
    (word(user, b"._-") && word(host, b".-")).then(|| (format!("{user}@{host}"), port))
}

/// The `ssh` command for the target.
fn ssh(target: &str, port: Option<u16>) -> Vec<String> {
    let mut words = vec!["ssh".to_owned()];
    if let Some(p) = port {
        words.extend(["-p".to_owned(), p.to_string()]);
    }
    words.push(target.to_owned());
    words
}

/// Opens a terminal running `ssh` to `text` (`user@host[:port]`). Returns the terminal opened.
///
/// # Errors
///
/// The target is not `user@host`, or no terminal could be opened.
pub fn open(text: &str) -> Result<String, String> {
    let (to, port) =
        target(text).ok_or("the server as user@host, such as root@po.radiopost.org")?;
    let words = ssh(&to, port);
    let tries: Vec<(&str, Vec<String>)> = if cfg!(windows) {
        let mut wt = vec!["-w".to_owned(), "new".to_owned()];
        wt.extend(words.clone());
        let mut start = vec![
            "/c".to_owned(),
            "start".to_owned(),
            "RadioPost server".to_owned(),
        ];
        start.extend(words);
        vec![("wt.exe", wt), ("cmd", start)]
    } else if cfg!(target_os = "macos") {
        let script = format!(
            "tell application \"Terminal\" to do script \"{}\"",
            words.join(" ")
        );
        vec![("osascript", vec!["-e".to_owned(), script])]
    } else {
        let mut dash_e = vec!["-e".to_owned()];
        dash_e.extend(words.clone());
        let mut double = vec!["--".to_owned()];
        double.extend(words);
        vec![
            ("x-terminal-emulator", dash_e.clone()),
            ("gnome-terminal", double),
            ("konsole", dash_e.clone()),
            ("xterm", dash_e),
        ]
    };
    for (program, args) in tries {
        if Command::new(program).args(&args).spawn().is_ok() {
            return Ok(format!("{program}: {}", ssh(&to, port).join(" ")));
        }
    }
    Err("no terminal could be opened here: run `ssh` in one yourself".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_user_at_host_is_taken() {
        assert_eq!(
            target("root@po.radiopost.org"),
            Some(("root@po.radiopost.org".into(), None))
        );
        assert_eq!(
            target("kk4oda@203.0.113.7:2222"),
            Some(("kk4oda@203.0.113.7".into(), Some(2222)))
        );
        for bad in [
            "po.radiopost.org",
            "-oProxyCommand=x@h",
            "root@-oProxyCommand=x",
            "root@host; rm -rf /",
            "root@host name",
            "root@host:99999",
            "@host",
            "root@",
        ] {
            assert_eq!(target(bad), None, "{bad}");
        }
        assert_eq!(ssh("root@h", Some(22)), ["ssh", "-p", "22", "root@h"]);
    }
}
