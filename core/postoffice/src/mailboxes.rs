// SPDX-License-Identifier: MIT OR Apache-2.0
//! The mail server's mailbox for each account the operators approve (D35).
//!
//! The mail server refuses a recipient it has no mailbox for, before the door is asked (RP-6
//! §2.1), so an account with no mailbox receives no email at all. Making it was a page of the
//! mail server's administration per user; now the post office makes it when an application is
//! approved, one of two ways:
//!
//! * `stalwart`: Stalwart's own JMAP methods (`x:Domain/get`, then `x:Account/set` creating a
//!   `User` with a password), with an API key or an administrator's login;
//! * `command`: a program the operator names, run with the address and display name as
//!   arguments and the password in `RADIOPOST_PASSWORD`, for any other mail server, or a
//!   Stalwart whose interface differs.
//!
//! The password goes to the new user in the welcome email, for their mail programs. A mailbox
//! that could not be made is said on the desk, and never undoes the approval.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

/// How the post office makes mailboxes, as `config.json`'s `mailboxes` holds it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MailboxSettings {
    /// `stalwart`, `command`, or empty for none.
    pub kind: String,
    /// Stalwart's web listener, as the post office reaches it (`http://127.0.0.1:8080`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// A Stalwart API key with the right to create accounts, sent as a bearer token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Or an administrator's login, `user:password`, sent as basic authentication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login: Option<String>,
    /// The program and its first arguments, for `command`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    /// Each mailbox's size, in megabytes; none, the mail server's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_mb: Option<u64>,
}

impl MailboxSettings {
    /// Nothing set: `config.json` leaves the member out.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == MailboxSettings::default()
    }

    /// What the desk shows: never a key or a login.
    #[must_use]
    pub fn shown(&self) -> Json {
        json!({"kind": self.kind, "url": self.url, "command": self.command,
               "key": self.api_key.is_some() || self.login.is_some(),
               "quota_mb": self.quota_mb})
    }
}

/// A password for a new mailbox: 20 letters and digits, about 115 bits.
///
/// # Panics
///
/// The system has no random numbers to give.
#[must_use]
pub fn new_password() -> String {
    const ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut raw = [0u8; 20];
    getrandom::fill(&mut raw).expect("the system's random numbers");
    raw.iter()
        .map(|b| char::from(ALPHABET[usize::from(*b) % ALPHABET.len()]))
        .collect()
}

/// Makes the mailbox for `address`, with `name` and `password`. Returns what to tell the
/// operator; `Ok(None)` when no way is set.
///
/// # Errors
///
/// The mail server refused, could not be reached, or the command failed: what it said.
pub fn make(
    settings: &MailboxSettings,
    address: &str,
    name: &str,
    password: &str,
) -> Result<Option<String>, String> {
    match settings.kind.as_str() {
        "stalwart" => stalwart(settings, address, name, password).map(Some),
        "command" => command(&settings.command, address, name, password).map(Some),
        "" => Ok(None),
        other => Err(format!("no way to make mailboxes called {other:?}")),
    }
}

/// One JMAP request to Stalwart: its method responses.
fn jmap(settings: &MailboxSettings, calls: &Json) -> Result<Vec<Json>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into();
    let url = format!("{}/jmap/", settings.url.trim_end_matches('/'));
    let mut request = agent.post(&url).header("Content-Type", "application/json");
    if let Some(key) = &settings.api_key {
        request = request.header("Authorization", &format!("Bearer {key}"));
    } else if let Some(login) = &settings.login {
        let basic = crate::mail::base64_lines(login.as_bytes()).replace("\r\n", "");
        request = request.header("Authorization", &format!("Basic {basic}"));
    }
    let body = json!({"using": ["urn:ietf:params:jmap:core", "urn:stalwart:jmap"],
                      "methodCalls": calls});
    let mut response = request
        .send(body.to_string())
        .map_err(|e| format!("Stalwart at {url}: {e}"))?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string().unwrap_or_default();
    if status == 401 || status == 403 {
        return Err(format!(
            "Stalwart refused the post office's key or login ({status}): give it one with the \
             right to create accounts"
        ));
    }
    let answer: Json = serde_json::from_str(&text).map_err(|_| {
        format!(
            "Stalwart answered {status}: {}",
            text.chars().take(200).collect::<String>()
        )
    })?;
    let responses = answer["methodResponses"]
        .as_array()
        .cloned()
        .ok_or_else(|| format!("Stalwart answered {status} with no methodResponses"))?;
    for r in &responses {
        if r[0] == "error" {
            return Err(format!(
                "Stalwart: {} {}",
                r[1]["type"].as_str().unwrap_or("error"),
                r[1]["description"].as_str().unwrap_or("")
            ));
        }
    }
    Ok(responses)
}

/// Stalwart's ID of `domain`.
///
/// # Errors
///
/// Stalwart could not be asked, or does not have the domain.
pub fn stalwart_domain(settings: &MailboxSettings, domain: &str) -> Result<String, String> {
    let responses = jmap(settings, &json!([["x:Domain/get", {"ids": null}, "d"]]))?;
    responses
        .iter()
        .filter(|r| r[0] == "x:Domain/get")
        .flat_map(|r| r[1]["list"].as_array().cloned().unwrap_or_default())
        .find(|d| {
            d["name"]
                .as_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(domain))
        })
        .and_then(|d| d["id"].as_str().map(str::to_owned))
        .ok_or_else(|| format!("Stalwart has no domain {domain}: add it there first"))
}

fn stalwart(
    settings: &MailboxSettings,
    address: &str,
    name: &str,
    password: &str,
) -> Result<String, String> {
    let (local, domain) = address
        .split_once('@')
        .ok_or_else(|| format!("{address} is not an address"))?;
    let domain_id = stalwart_domain(settings, domain)?;
    let mut user = json!({
        "@type": "User",
        "name": local,
        "domainId": domain_id,
        "description": name,
        "credentials": [{"@type": "Password", "secret": password}],
        "roles": {"@type": "User"},
    });
    if let Some(mb) = settings.quota_mb {
        user["quotas"] = json!({"maxDiskQuota": mb * 1024 * 1024});
    }
    let responses = jmap(
        settings,
        &json!([["x:Account/set", {"create": {"new": user}}, "a"]]),
    )?;
    let set = responses
        .iter()
        .find(|r| r[0] == "x:Account/set")
        .ok_or("Stalwart did not answer x:Account/set")?;
    if let Some(why) = set[1]["notCreated"].get("new") {
        let kind = why["type"].as_str().unwrap_or("refused");
        let text = why["description"].as_str().unwrap_or("");
        return Err(format!("Stalwart did not make {address}: {kind} {text}"));
    }
    let id = set[1]["created"]["new"]["id"].as_str().unwrap_or("?");
    Ok(format!("mailbox {address} made in Stalwart (account {id})"))
}

fn command(cmd: &[String], address: &str, name: &str, password: &str) -> Result<String, String> {
    let (program, args) = cmd
        .split_first()
        .ok_or("no command set to make mailboxes")?;
    let out = std::process::Command::new(program)
        .args(args)
        .arg(address)
        .arg(name)
        .env("RADIOPOST_PASSWORD", password)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    let said = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if out.status.success() {
        Ok(if said.is_empty() {
            format!("mailbox {address} made by {program}")
        } else {
            said
        })
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        Err(format!("{program} failed ({}): {err}{said}", out.status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_are_new_each_time() {
        let (a, b) = (new_password(), new_password());
        assert_eq!(a.len(), 20);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn a_command_is_given_the_address_name_and_password() {
        let dir = std::env::temp_dir().join(format!("radiopost-mbox-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        let out = dir.join("made");
        let cmd = vec![
            "sh".to_owned(),
            "-c".to_owned(),
            format!(
                "printf '%s %s %s' \"$1\" \"$2\" \"$RADIOPOST_PASSWORD\" > {}",
                out.display()
            ),
            "make".to_owned(),
        ];
        let said = command(&cmd, "jose@radiopost.org", "José", "s3cret").expect("made");
        assert_eq!(said, "mailbox jose@radiopost.org made by sh");
        assert_eq!(
            std::fs::read_to_string(&out).expect("written"),
            "jose@radiopost.org José s3cret"
        );
        let failing = vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "echo no >&2; exit 3".to_owned(),
        ];
        assert!(
            command(&failing, "a@b.c", "A", "p")
                .expect_err("failed")
                .contains("no")
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
