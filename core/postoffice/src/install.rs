// SPDX-License-Identifier: MIT OR Apache-2.0
//! `radiopost-po install`: the post office guide's set-up, done by the program on a fresh Debian
//! server, step by step (design D35).
//!
//! Each step says what it does and why, checks whether it is done already, and does only what is
//! left, so running it again is safe and finishes what an earlier run did not. What a program
//! cannot do — the mail server's setup wizard in a browser, the records at the domain's DNS host,
//! the reverse DNS at the provider — it says plainly, with the values to type, and waits.
//!
//! Nothing the operator types reaches a shell: the domain, callsign and address are checked
//! first, and the few commands that need a pipe are fixed strings.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What the operator gives.
#[derive(Clone, Debug)]
pub struct Options {
    /// The post office's domain: `radiopost.org`.
    pub domain: String,
    /// The operator's callsign.
    pub operator: String,
    /// Where people write to the operator: `postmaster@radiopost.org`.
    pub contact: String,
    /// The post office's directory.
    pub dir: PathBuf,
    /// Where the program is installed.
    pub exe: PathBuf,
    /// The program running now, copied to `exe` when it is elsewhere.
    pub from: PathBuf,
}

impl Options {
    /// The mail and web host: `po.<domain>`.
    #[must_use]
    pub fn host(&self) -> String {
        format!("po.{}", self.domain)
    }

    /// The options, checked: a lowercase domain, a callsign, an address.
    ///
    /// # Errors
    ///
    /// One of them is not of its form.
    pub fn checked(self) -> Result<Options, String> {
        let labels: Vec<&str> = self.domain.split('.').collect();
        let domain_ok = labels.len() >= 2
            && labels.iter().all(|l| {
                !l.is_empty()
                    && !l.starts_with('-')
                    && l.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            });
        if !domain_ok {
            return Err(format!("{:?} is not a domain in lowercase", self.domain));
        }
        let call_ok = (3..=10).contains(&self.operator.len())
            && self
                .operator
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
        if !call_ok {
            return Err(format!("{:?} is not a callsign in capitals", self.operator));
        }
        let contact_ok = self.contact.split_once('@').is_some_and(|(l, d)| {
            !l.is_empty()
                && d.contains('.')
                && self
                    .contact
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"@.-_+".contains(&b))
        });
        if !contact_ok {
            return Err(format!("{:?} is not an email address", self.contact));
        }
        Ok(self)
    }
}

/// What the plan reads of the server, so that it can be planned for a test.
pub trait Probe {
    /// Whether the path exists.
    fn exists(&self, path: &str) -> bool;
    /// A file's text.
    fn read(&self, path: &str) -> Option<String>;
    /// Whether a program is on the PATH.
    fn has(&self, program: &str) -> bool;
}

/// The real server.
pub struct System;

impl Probe for System {
    fn exists(&self, path: &str) -> bool {
        Path::new(path).exists()
    }
    fn read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
    fn has(&self, program: &str) -> bool {
        std::env::var_os("PATH")
            .is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
    }
}

/// One thing a step does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// A program and its arguments.
    Run(Vec<String>),
    /// A file written whole, with its permissions.
    Write {
        /// Where.
        path: String,
        /// What.
        text: String,
        /// Its mode.
        mode: u32,
    },
}

/// A step of the set-up.
#[derive(Clone, Debug)]
pub struct Step {
    /// What it is.
    pub title: String,
    /// Why, in a sentence.
    pub why: String,
    /// Why it is skipped, when it is done already.
    pub done: Option<String>,
    /// What it does.
    pub actions: Vec<Action>,
    /// What the operator does by hand afterwards, when anything.
    pub by_hand: Option<String>,
}

fn run(words: &[&str]) -> Action {
    Action::Run(words.iter().map(|w| (*w).to_owned()).collect())
}

fn write(path: &str, text: String, mode: u32) -> Action {
    Action::Write {
        path: path.to_owned(),
        text,
        mode,
    }
}

/// The secrets the installer made, kept so that a second run uses the same.
#[derive(Clone, Debug)]
pub struct Secrets {
    /// The token Stalwart's MTA hook sends to the door.
    pub hook_token: String,
    /// The password of the mail server's `radiopost` account.
    pub smtp_password: String,
}

/// The Caddyfile for the post office's host.
#[must_use]
pub fn caddyfile(host: &str) -> String {
    format!(
        "{host} {{\n    tls {{\n        issuer acme\n    }}\n    handle /rp/v1/* {{\n        \
         reverse_proxy 127.0.0.1:8770\n    }}\n    handle {{\n        reverse_proxy \
         127.0.0.1:8080\n    }}\n}}\n"
    )
}

/// The script that copies Caddy's certificate to Stalwart when it changed.
#[must_use]
pub fn certs_script(host: &str) -> String {
    format!(
        "#!/bin/sh\nset -e\nsrc=/var/lib/caddy/.local/share/caddy/certificates/acme-v02.api.letsencrypt.org-directory/{host}\n\
         dst=/etc/stalwart/tls\nmkdir -p \"$dst\"\ncmp -s \"$src/{host}.crt\" \"$dst/{host}.crt\" && exit 0\n\
         install -o stalwart -g stalwart -m 0640 \"$src/{host}.crt\" \"$dst/\"\n\
         install -o stalwart -g stalwart -m 0600 \"$src/{host}.key\" \"$dst/\"\n\
         systemctl try-restart stalwart\n"
    )
}

/// The post office's systemd unit.
#[must_use]
pub fn unit(o: &Options) -> String {
    format!(
        "[Unit]\nDescription=RadioPost post office\nAfter=network-online.target stalwart.service\n\n\
         [Service]\nExecStart={} --dir {} serve\nUser=radiopost\nRestart=on-failure\n\n\
         [Install]\nWantedBy=multi-user.target\n",
        o.exe.display(),
        o.dir.display()
    )
}

/// The Stalwart wizard's screens and settings (the guide's §5), with this post office's values.
#[must_use]
pub fn stalwart_by_hand(o: &Options, s: &Secrets) -> String {
    let host = o.host();
    let d = &o.domain;
    format!(
        "Stalwart is installed. Its setup is a web page, which the firewall keeps closed: on your\n\
         own computer run   ssh -L 8080:127.0.0.1:8080 root@{host}\n\
         and open http://localhost:8080/admin with the admin password the install printed\n\
         (journalctl -u stalwart -n 200 | grep -A8 'bootstrap mode' shows it again).\n\n\
         The wizard: hostname {host}; default domain {d}; UNtick 'Automatically obtain TLS\n\
         certificate'; keep 'Generate email signing keys'; storage, directory and logging as\n\
         offered; DNS: manual. Keep the admin@{d} password it shows.\n\n\
         Then, in the administration (the search box finds each page):\n\
         1. Certificates: a new one, of type File: /etc/stalwart/tls/{host}.crt and .key.\n\
         2. Listeners: delete 'https'; set 'http' to bind 127.0.0.1:8080.\n\
         3. HTTP, General: client IP source X-Forwarded-For, 1 hop.\n\
         4. Sender Authentication: DMARC verify 'strict' on port 25.\n\
         5. Rates & Quotas, Outbound Rate Limits (only if your host limits mail): 4 per 60000 ms.\n\
         6. Directory, Accounts: an account 'radiopost', password {pw}\n\
         7. AUTH Stage, Must Match Sender: false if authenticated_as == 'radiopost' ||\n\
            authenticated_as == 'radiopost@{d}', otherwise true.\n\
         8. MTA Hooks: URL http://127.0.0.1:8772/, stages rcpt and data, Bearer {token},\n\
            timeout 30000 ms, temporary failure on error, enabled if local_port == 25.\n\
         9. An API key (Directory, API keys) with the right to create accounts: give it to the\n\
            desk's Settings, Mailboxes, so approving an application makes its mailbox.\n\
         Then: systemctl restart stalwart",
        pw = s.smtp_password,
        token = s.hook_token
    )
}

/// One step's planner.
type Planner = fn(&Options, &Secrets, &dyn Probe) -> Step;

/// The steps, as they stand on the server `p`.
#[must_use]
pub fn plan(o: &Options, s: &Secrets, p: &dyn Probe) -> Vec<Step> {
    let steps: [Planner; 8] = [
        step_firewall,
        step_server_name,
        step_ssh,
        step_caddy,
        step_stalwart,
        step_post_office,
        step_dns,
        step_the_check,
    ];
    steps.iter().map(|step| step(o, s, p)).collect()
}

fn step_firewall(_o: &Options, _s: &Secrets, p: &dyn Probe) -> Step {
    let ufw_on = p
        .read("/etc/ufw/ufw.conf")
        .is_some_and(|t| t.contains("ENABLED=yes"));
    Step {
        title: "Updates and the firewall".into(),
        why: "Debian's security updates each night, and only the ports the post office needs \
              open: SSH, mail (25, 465, 993) and the web (80, 443)."
            .into(),
        done: ufw_on.then(|| "already done: the firewall is on".into()),
        actions: vec![
            run(&["apt-get", "update"]),
            run(&[
                "apt-get",
                "install",
                "-y",
                "ufw",
                "unattended-upgrades",
                "curl",
                "gnupg",
                "unzip",
                "debian-keyring",
                "debian-archive-keyring",
                "apt-transport-https",
                "swaks",
            ]),
            run(&["ufw", "default", "deny", "incoming"]),
            run(&["ufw", "default", "allow", "outgoing"]),
            run(&["ufw", "allow", "22/tcp"]),
            run(&["ufw", "allow", "25/tcp"]),
            run(&["ufw", "allow", "80/tcp"]),
            run(&["ufw", "allow", "443/tcp"]),
            run(&["ufw", "allow", "465/tcp"]),
            run(&["ufw", "allow", "993/tcp"]),
            run(&["ufw", "--force", "enable"]),
        ],
        by_hand: None,
    }
}

fn step_server_name(o: &Options, _s: &Secrets, p: &dyn Probe) -> Step {
    let host = o.host();
    let named = p.read("/etc/hostname").is_some_and(|h| h.trim() == host);
    let mut actions = vec![
        run(&["hostnamectl", "set-hostname", &host]),
        run(&[
            "sed",
            "-i",
            &format!("s/^127\\.0\\.1\\.1.*/127.0.1.1\\t{host} po/"),
            "/etc/hosts",
        ]),
    ];
    if p.exists("/etc/cloud/cloud.cfg.d") {
        actions.push(write(
            "/etc/cloud/cloud.cfg.d/99-radiopost.cfg",
            "preserve_hostname: true\nmanage_etc_hosts: false\n".into(),
            0o644,
        ));
    }
    Step {
        title: format!("The server's name, {host}"),
        why: "Mail servers check that a server's name and address agree.".into(),
        done: named.then(|| format!("already done: it is {host}")),
        actions,
        by_hand: Some(format!(
            "At your domain's DNS host, point {host} at this server now (an A record to the\n\
             address `hostname -I` shows first; AAAA too if it has IPv6), so that Caddy can get\n\
             its certificate in step 4. At your server's provider, set the reverse DNS (PTR) of\n\
             each address to {host} (Hostinger: VPS, Settings, IP address, Set PTR record)."
        )),
    }
}

fn step_ssh(_o: &Options, _s: &Secrets, p: &dyn Probe) -> Step {
    let keyed = p.read("/root/.ssh/authorized_keys").is_some_and(|k| {
        k.lines()
            .any(|l| l.starts_with("ssh-") || l.starts_with("ecdsa-"))
    });
    let hardened = p.exists("/etc/ssh/sshd_config.d/00-radiopost.conf");
    Step {
        title: "SSH by key only".into(),
        why: "Passwords can be guessed; a key cannot.".into(),
        done: if hardened {
            Some("already done".into())
        } else if !keyed {
            Some(
                "not now: root has no SSH key yet, and turning passwords off would lock you \
                 out. Add your key to /root/.ssh/authorized_keys and run this again"
                    .into(),
            )
        } else {
            None
        },
        actions: vec![
            write(
                "/etc/ssh/sshd_config.d/00-radiopost.conf",
                "PasswordAuthentication no\nPermitRootLogin prohibit-password\n".into(),
                0o644,
            ),
            run(&["sshd", "-t"]),
            run(&["systemctl", "reload", "ssh"]),
        ],
        by_hand: None,
    }
}

fn step_caddy(o: &Options, _s: &Secrets, p: &dyn Probe) -> Step {
    let host = o.host();
    let caddy_ok = p
        .read("/etc/caddy/Caddyfile")
        .is_some_and(|c| c.contains(&host) && c.contains("127.0.0.1:8770"));
    let mut actions = Vec::new();
    if !p.has("caddy") {
        actions.extend([
            run(&[
                "sh",
                "-c",
                "curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | \
                 gpg --dearmor --yes -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg",
            ]),
            run(&[
                "sh",
                "-c",
                "curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' \
                 > /etc/apt/sources.list.d/caddy-stable.list",
            ]),
            run(&[
                "chmod",
                "o+r",
                "/usr/share/keyrings/caddy-stable-archive-keyring.gpg",
                "/etc/apt/sources.list.d/caddy-stable.list",
            ]),
            run(&["apt-get", "update"]),
            run(&["apt-get", "install", "-y", "caddy"]),
        ]);
    }
    actions.extend([
        write("/etc/caddy/Caddyfile", caddyfile(&host), 0o644),
        run(&["systemctl", "reload-or-restart", "caddy"]),
    ]);
    Step {
        title: "The web server and its certificate (Caddy)".into(),
        why: format!(
            "Caddy gets and renews the certificate for {host} and sends the post office's API to \
             it, everything else to the mail server's web page."
        ),
        done: caddy_ok.then(|| "already done: Caddy serves the post office".into()),
        actions,
        by_hand: None,
    }
}

fn step_stalwart(o: &Options, s: &Secrets, p: &dyn Probe) -> Step {
    let host = o.host();
    let stalwart = p.exists("/etc/systemd/system/stalwart.service") || p.exists("/opt/stalwart");
    let certs = p.exists("/etc/systemd/system/stalwart-certs.timer");
    let mut actions = Vec::new();
    if !stalwart {
        actions.push(run(&["sh", "-c", "curl --proto '=https' --tlsv1.2 -sSf \
             https://get.stalw.art/install.sh -o /root/stalwart-install.sh && sh /root/stalwart-install.sh"]));
    }
    actions.extend([
        write("/usr/local/sbin/stalwart-certs", certs_script(&host), 0o755),
        write(
            "/etc/systemd/system/stalwart-certs.service",
            "[Unit]\nDescription=Copy Caddy's certificate to Stalwart\n\n[Service]\nType=oneshot\n\
             ExecStart=/usr/local/sbin/stalwart-certs\n"
                .into(),
            0o644,
        ),
        write(
            "/etc/systemd/system/stalwart-certs.timer",
            "[Unit]\nDescription=Copy Caddy's certificate to Stalwart each day\n\n[Timer]\n\
             OnCalendar=daily\nRandomizedDelaySec=1h\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n"
                .into(),
            0o644,
        ),
        run(&["systemctl", "daemon-reload"]),
        run(&["systemctl", "enable", "--now", "stalwart-certs.timer"]),
        run(&["sh", "-c", "systemctl start stalwart-certs || echo 'no certificate yet: Caddy \
             gets it once DNS points here, and the timer copies it each day'"]),
        run(&["sh", "-c", &format!(
            "grep -q STALWART_PUBLIC_URL /etc/stalwart/stalwart.env 2>/dev/null || \
             echo 'STALWART_PUBLIC_URL=https://{host}' >> /etc/stalwart/stalwart.env")]),
    ]);
    Step {
        title: "The mail server (Stalwart)".into(),
        why: "It receives and sends the email, keeps the mailboxes users read in mail programs, \
              and signs outgoing mail."
            .into(),
        done: (stalwart && certs)
            .then(|| "already done: Stalwart is installed, and gets the certificate".into()),
        actions,
        by_hand: Some(stalwart_by_hand(o, s)),
    }
}

fn step_post_office(o: &Options, s: &Secrets, p: &dyn Probe) -> Step {
    let host = o.host();
    let dir = o.dir.display().to_string();
    let exe = o.exe.display().to_string();
    let config = format!("{dir}/config.json");
    let installed = p.exists(&config);
    let mut actions = Vec::new();
    if o.from != o.exe {
        let from = o.from.display().to_string();
        actions.push(run(&["install", "-m", "0755", &from, &exe]));
    }
    if !p
        .read("/etc/passwd")
        .is_some_and(|t| t.lines().any(|l| l.starts_with("radiopost:")))
    {
        actions.push(run(&[
            "useradd",
            "--system",
            "--home-dir",
            &dir,
            "--create-home",
            "--shell",
            "/usr/sbin/nologin",
            "radiopost",
        ]));
    }
    if !installed {
        let smtp = format!("smtps://radiopost:{}@{host}:465", s.smtp_password);
        actions.push(run(&[
            "runuser",
            "-u",
            "radiopost",
            "--",
            &exe,
            "--dir",
            &dir,
            "init",
            "--domain",
            &o.domain,
            "--base-url",
            &format!("https://{host}/rp/v1"),
            "--operator",
            &o.operator,
            "--contact",
            &o.contact,
            "--hook-token",
            &s.hook_token,
            "--smtp",
            &smtp,
        ]));
    }
    actions.extend([
        write("/etc/systemd/system/radiopost-po.service", unit(o), 0o644),
        run(&["systemctl", "daemon-reload"]),
        run(&["systemctl", "enable", "--now", "radiopost-po"]),
        run(&[&exe, "--dir", &dir, "jobs", "--install"]),
    ]);
    Step {
        title: "The post office (radiopost-po)".into(),
        why: "Accounts, credentials, sessions from gateways, the door, email in and out; and the \
              runner for the jobs the desk asks for."
            .into(),
        done: None,
        actions,
        by_hand: None,
    }
}

fn step_dns(o: &Options, _s: &Secrets, _p: &dyn Probe) -> Step {
    let host = o.host();
    let dir = o.dir.display().to_string();
    let exe = o.exe.display().to_string();
    Step {
        title: "DNS".into(),
        why: "The world finds the post office, and trusts its mail, by its DNS records.".into(),
        done: None,
        actions: vec![run(&[&exe, "--dir", &dir, "dns"])],
        by_hand: Some(format!(
            "At your domain's DNS host, publish:\n\
             {host}.  A     (this server's address: hostname -I shows it)\n\
             {d}.  MX    10 {host}.\n\
             {d}.  TXT   \"v=spf1 mx -all\"\n\
             {host}.  TXT   \"v=spf1 a -all\"\n\
             _dmarc.{d}.  TXT   \"v=DMARC1; p=quarantine; rua=mailto:{contact}\"\n\
             the _radiopost records printed above, and the two DKIM records from Stalwart's\n\
             Management, Domains, View Zone File.\n\
             At your server's provider: the reverse DNS (PTR) of each address, {host}.\n\
             Then give the DKIM selectors to the desk's Health tab, or: {exe} --dir {dir} check --dkim SELECTORS",
            d = o.domain,
            contact = o.contact
        )),
    }
}

fn step_the_check(o: &Options, _s: &Secrets, _p: &dyn Probe) -> Step {
    let dir = o.dir.display().to_string();
    let exe = o.exe.display().to_string();
    Step {
        title: "The check".into(),
        why: "Everything above, checked from outside as the world sees it.".into(),
        done: None,
        actions: vec![run(&[&exe, "--dir", &dir, "check"])],
        by_hand: None,
    }
}

/// Runs the steps: each said, skipped when done, its actions run (or only said, `dry_run`), its
/// part by hand said and waited for (`wait` returns when the operator is ready).
///
/// # Errors
///
/// An action failed: which, and what to do.
pub fn execute(
    steps: &[Step],
    dry_run: bool,
    out: &mut impl std::io::Write,
    mut wait: impl FnMut() -> bool,
) -> Result<(), String> {
    let total = steps.len();
    for (i, step) in steps.iter().enumerate() {
        let _ = writeln!(
            out,
            "\n[{}/{total}] {}\n    {}",
            i + 1,
            step.title,
            step.why
        );
        if let Some(why) = &step.done {
            let _ = writeln!(out, "    {why}");
            continue;
        }
        for action in &step.actions {
            match action {
                Action::Run(words) => {
                    let _ = writeln!(out, "    $ {}", shown(words));
                    if dry_run {
                        continue;
                    }
                    let ok = Command::new(&words[0])
                        .args(&words[1..])
                        .status()
                        .is_ok_and(|s| s.success());
                    if !ok {
                        return Err(format!(
                            "step {} ({}) stopped at `{}`: put it right and run the installer \
                             again; what is done stays done",
                            i + 1,
                            step.title,
                            shown(words)
                        ));
                    }
                }
                Action::Write { path, text, mode } => {
                    let _ = writeln!(out, "    write {path}");
                    if dry_run {
                        continue;
                    }
                    let target = Path::new(path);
                    if let Some(old) = std::fs::read_to_string(target).ok().filter(|o| o != text) {
                        let _ = std::fs::write(format!("{path}.before-radiopost"), old);
                    }
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| format!("{path}: {e}"))?;
                    }
                    std::fs::write(target, text).map_err(|e| format!("{path}: {e}"))?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt as _;
                        let _ = std::fs::set_permissions(
                            target,
                            std::fs::Permissions::from_mode(*mode),
                        );
                    }
                    #[cfg(not(unix))]
                    let _ = mode;
                }
            }
        }
        if let Some(text) = &step.by_hand {
            let _ = writeln!(out, "\n    By hand:\n");
            for line in text.lines() {
                let _ = writeln!(out, "      {line}");
            }
            let _ = out.flush();
            if !dry_run && !wait() {
                return Err(format!(
                    "stopped at step {}: run the installer again when it is done",
                    i + 1
                ));
            }
        }
    }
    Ok(())
}

/// A command as the operator would type it, the secrets in it hidden.
fn shown(words: &[String]) -> String {
    words
        .iter()
        .map(|w| {
            if w.starts_with("smtps://") {
                "smtps://radiopost:••••@…".to_owned()
            } else if w.contains('\'') {
                format!("\"{w}\"")
            } else if w.contains(' ') {
                format!("'{w}'")
            } else {
                w.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Fake(HashMap<&'static str, &'static str>, Vec<&'static str>);

    impl Probe for Fake {
        fn exists(&self, path: &str) -> bool {
            self.0.contains_key(path)
        }
        fn read(&self, path: &str) -> Option<String> {
            self.0.get(path).map(|t| (*t).to_owned())
        }
        fn has(&self, program: &str) -> bool {
            self.1.contains(&program)
        }
    }

    fn options() -> Options {
        Options {
            domain: "radiopost.test".into(),
            operator: "KK4ODA".into(),
            contact: "postmaster@radiopost.test".into(),
            dir: "/var/lib/radiopost".into(),
            exe: "/usr/local/bin/radiopost-po".into(),
            from: "/root/radiopost-tools-linux-x64/radiopost-po".into(),
        }
        .checked()
        .expect("checked")
    }

    fn secrets() -> Secrets {
        Secrets {
            hook_token: "hook".into(),
            smtp_password: "pw".into(),
        }
    }

    #[test]
    fn a_fresh_server_gets_every_step_and_a_set_up_one_skips_them() {
        let fresh = plan(&options(), &secrets(), &Fake(HashMap::new(), vec![]));
        assert_eq!(fresh.len(), 8);
        assert!(fresh[0].done.is_none());
        assert!(
            fresh[2]
                .done
                .as_deref()
                .is_some_and(|d| d.contains("no SSH key")),
            "no key: no lockout"
        );
        assert!(
            fresh[1]
                .by_hand
                .as_deref()
                .is_some_and(|h| h.contains("A record")),
            "DNS before Caddy"
        );
        assert!(
            fresh[3]
                .actions
                .iter()
                .any(|a| matches!(a, Action::Run(w) if w.contains(&"caddy".to_owned())))
        );
        assert!(
            fresh[5]
                .actions
                .iter()
                .any(|a| matches!(a, Action::Run(w) if w.contains(&"init".to_owned())))
        );
        assert!(
            fresh[5]
                .actions
                .iter()
                .any(|a| matches!(a, Action::Run(w) if w[0] == "install")),
            "the program is copied to /usr/local/bin"
        );
        let caddy = "po.radiopost.test { reverse_proxy 127.0.0.1:8770 }";
        let set_up = Fake(
            HashMap::from([
                ("/etc/ufw/ufw.conf", "ENABLED=yes\n"),
                ("/etc/hostname", "po.radiopost.test\n"),
                ("/root/.ssh/authorized_keys", "ssh-ed25519 AAAA kk4oda\n"),
                ("/etc/ssh/sshd_config.d/00-radiopost.conf", ""),
                ("/etc/caddy/Caddyfile", caddy),
                ("/etc/systemd/system/stalwart.service", ""),
                ("/etc/systemd/system/stalwart-certs.timer", ""),
                ("/var/lib/radiopost/config.json", "{}"),
            ]),
            vec!["caddy"],
        );
        let again = plan(&options(), &secrets(), &set_up);
        assert!(
            again[..5].iter().all(|s| s.done.is_some()),
            "{:?}",
            again.iter().map(|s| &s.done).collect::<Vec<_>>()
        );
        assert!(
            !again[5]
                .actions
                .iter()
                .any(|a| matches!(a, Action::Run(w) if w.contains(&"init".to_owned()))),
            "a post office there is not made again"
        );
    }

    #[test]
    fn a_dry_run_says_everything_and_runs_nothing_and_hides_the_secrets() {
        let steps = plan(&options(), &secrets(), &Fake(HashMap::new(), vec![]));
        let mut out = Vec::new();
        execute(&steps, true, &mut out, || {
            panic!("a dry run waits for no one")
        })
        .expect("said");
        let text = String::from_utf8(out).expect("UTF-8");
        assert!(text.contains("[1/8] Updates and the firewall"), "{text}");
        assert!(text.contains("$ ufw --force enable"));
        assert!(text.contains("write /etc/caddy/Caddyfile"));
        assert!(text.contains("By hand:") && text.contains("Bearer hook"));
        assert!(
            !text.contains("radiopost:pw@"),
            "the password stays out of the commands shown"
        );
    }

    #[test]
    fn what_the_operator_types_is_checked() {
        let bad = |d: &str, c: &str, e: &str| {
            Options {
                domain: d.into(),
                operator: c.into(),
                contact: e.into(),
                dir: "/x".into(),
                exe: "/y".into(),
                from: "/y".into(),
            }
            .checked()
            .is_err()
        };
        assert!(bad("RadioPost.org", "KK4ODA", "a@b.org"));
        assert!(bad("radiopost.org; rm -rf /", "KK4ODA", "a@b.org"));
        assert!(bad("radiopost.org", "kk4oda", "a@b.org"));
        assert!(bad("radiopost.org", "KK4ODA", "a'b@c.org"));
        assert!(!bad("radiopost.org", "KK4ODA", "postmaster@radiopost.org"));
    }
}
