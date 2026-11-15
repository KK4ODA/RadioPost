// SPDX-License-Identifier: MIT OR Apache-2.0
//! One mailbox, the rest of it (RP-6 §2.6, wishlist W36): what the radio did, shown in the mail
//! program. A message a user sends by radio goes into their Sent folder, as the email RP-6 §3.1
//! makes of it, marked read; a message they pick up by radio is marked read in their mailbox.
//!
//! The post office queues each as a [`MailboxTask`] where it happens (`send_on`, `picked_up`),
//! and the service carries them out in the background every minute, over JMAP (RFC 8620, RFC
//! 8621) with the mailboxes key, as `radio_skips_read` reads (`crate::seen`): never while a
//! gateway waits on a session. A message is found in the mail server by its `Message-ID`. One
//! sent by radio to a user here reaches their mailbox by the submission port, maybe after they
//! picked it up by radio, so a task waits for its message, and is given up after two days.
//!
//! The Sent copy is uploaded (RFC 8620 §6.1) and imported (RFC 8621 §4.8) into the mailbox with the
//! role `sent`; a copy already there is not imported again. The upload goes to the address the
//! service reaches the mail server at, whatever host the mail server's session names.

use std::time::Duration;

use radiopost::base64url;
use radiopost::message::Message;
use serde_json::{Value as Json, json};

use crate::mail::{self, Carried, Leaving};
use crate::mailboxes::{self, MailboxSettings};
use crate::office::MailboxTask;
use crate::serve::{Running, now};

/// The capabilities a JMAP request for mail names.
const MAIL: &[&str] = &["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"];

/// What one look did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mirrored {
    /// Messages put in Sent folders.
    pub sent: usize,
    /// Messages marked read.
    pub read: usize,
    /// Tasks given up after two days.
    pub given_up: usize,
}

/// The emails of an account whose `Message-ID` is `message_id` (angle brackets or none): each
/// one's JMAP id, whether it is read, and its mailboxes.
fn found(
    settings: &MailboxSettings,
    account: &str,
    message_id: &str,
) -> Result<Vec<(String, bool, Json)>, String> {
    let bare = message_id.trim_start_matches('<').trim_end_matches('>');
    let responses = mailboxes::jmap_using(
        settings,
        MAIL,
        &json!([
            ["Email/query", {"accountId": account, "filter": {"header": ["Message-ID", bare]},
                             "limit": 20}, "q"],
            ["Email/get", {"accountId": account,
                           "properties": ["messageId", "keywords", "mailboxIds"],
                           "#ids": {"resultOf": "q", "name": "Email/query", "path": "/ids"}}, "g"]
        ]),
    )?;
    Ok(responses
        .iter()
        .filter(|r| r[0] == "Email/get")
        .flat_map(|r| r[1]["list"].as_array().cloned().unwrap_or_default())
        // the filter may match more loosely than the header: the identifier itself decides
        .filter(|e| {
            e["messageId"]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|m| m.as_str() == Some(bare)))
        })
        .filter_map(|e| {
            Some((
                e["id"].as_str()?.to_owned(),
                e["keywords"]["$seen"].as_bool() == Some(true),
                e["mailboxIds"].clone(),
            ))
        })
        .collect())
}

/// Marks the message read: `true` once it is, `false` while the mail server has no such message.
fn mark_read(settings: &MailboxSettings, account: &str, message_id: &str) -> Result<bool, String> {
    let emails = found(settings, account, message_id)?;
    if emails.is_empty() {
        return Ok(false);
    }
    let update: serde_json::Map<String, Json> = emails
        .iter()
        .filter(|(_, read, _)| !read)
        .map(|(id, _, _)| (id.clone(), json!({"keywords/$seen": true})))
        .collect();
    if update.is_empty() {
        return Ok(true);
    }
    let responses = mailboxes::jmap_using(
        settings,
        MAIL,
        &json!([["Email/set", {"accountId": account, "update": update}, "s"]]),
    )?;
    refused_in(&responses, "Email/set", "notUpdated")?;
    Ok(true)
}

/// A method's per-object refusals (`notCreated`, `notUpdated`) as an error.
fn refused_in(responses: &[Json], method: &str, member: &str) -> Result<(), String> {
    let refused = responses
        .iter()
        .filter(|r| r[0] == method)
        .filter_map(|r| r[1][member].as_object())
        .flat_map(|m| m.values())
        .find(|e| e["type"] != "alreadyExists");
    match refused {
        Some(e) => Err(format!(
            "Stalwart refused {method}: {} {}",
            e["type"].as_str().unwrap_or("error"),
            e["description"].as_str().unwrap_or("")
        )),
        None => Ok(()),
    }
}

/// The account's mailbox with the role `sent` (RFC 8621 §2).
fn sent_mailbox(settings: &MailboxSettings, account: &str) -> Result<Option<String>, String> {
    let responses = mailboxes::jmap_using(
        settings,
        MAIL,
        &json!([["Mailbox/query", {"accountId": account, "filter": {"role": "sent"}}, "m"]]),
    )?;
    Ok(responses
        .iter()
        .filter(|r| r[0] == "Mailbox/query")
        .find_map(|r| r[1]["ids"][0].as_str().map(str::to_owned)))
}

/// Uploads `data` to the account (RFC 8620 §6.1): its blob ID. The path comes from the mail
/// server's session; the host is the one the service reaches it at.
fn upload(settings: &MailboxSettings, account: &str, data: &[u8]) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .build()
        .into();
    let base = settings.url.trim_end_matches('/');
    let auth = mailboxes::authorization(settings);
    let mut ask = agent.get(&format!("{base}/jmap/session"));
    if let Some(a) = &auth {
        ask = ask.header("Authorization", a);
    }
    let template = ask
        .call()
        .ok()
        .and_then(|mut r| r.body_mut().read_to_string().ok())
        .and_then(|t| serde_json::from_str::<Json>(&t).ok())
        .and_then(|s| s["uploadUrl"].as_str().map(str::to_owned))
        .unwrap_or_default();
    let path = upload_path(&template, account);
    let url = format!("{base}{path}");
    let mut request = agent.post(&url).header("Content-Type", "message/rfc822");
    if let Some(a) = &auth {
        request = request.header("Authorization", a);
    }
    let mut response = request
        .send(data)
        .map_err(|e| format!("Stalwart at {url}: {e}"))?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string().unwrap_or_default();
    let answer: Json = serde_json::from_str(&text).unwrap_or(Json::Null);
    answer["blobId"].as_str().map(str::to_owned).ok_or_else(|| {
        format!(
            "Stalwart did not take the upload ({status}): {}",
            text.chars().take(200).collect::<String>()
        )
    })
}

/// The upload path for `account` from the session's `uploadUrl` template, its host left out:
/// Stalwart names its public host there, and the service may reach it on another.
#[must_use]
pub fn upload_path(template: &str, account: &str) -> String {
    let path = template
        .find("/jmap/upload")
        .map_or("/jmap/upload/{accountId}/", |i| &template[i..]);
    path.replace("{accountId}", account)
}

/// Puts the email in the account's Sent folder, read: `true` once it is there.
fn file_sent(
    settings: &MailboxSettings,
    account: &str,
    message_id: &str,
    email: &[u8],
    received: u64,
) -> Result<bool, String> {
    let sent = sent_mailbox(settings, account)?
        .ok_or("the account has no mailbox with the role sent (a Sent folder)")?;
    // filed already: by an earlier try, or the user's own copy
    let there = found(settings, account, message_id)?
        .iter()
        .any(|(_, _, boxes)| boxes[&sent] == true);
    if there {
        return Ok(true);
    }
    let blob = upload(settings, account, email)?;
    let responses = mailboxes::jmap_using(
        settings,
        MAIL,
        &json!([["Email/import", {"accountId": account, "emails": {"e": {
            "blobId": blob, "mailboxIds": {sent: true}, "keywords": {"$seen": true},
            "receivedAt": mail::iso(received)}}}, "i"]]),
    )?;
    refused_in(&responses, "Email/import", "notCreated")?;
    Ok(true)
}

impl Running {
    /// The email a message sent by radio becomes for its sender's Sent folder: RP-6 §3.1's,
    /// without §4's part, as the sender's own mail program would have kept it.
    fn sent_email(&self, task: &MailboxTask) -> Option<Vec<u8>> {
        let message = base64url::decode(&task.air).and_then(|a| Message::from_air(&a).ok())?;
        let office = self.office();
        let name = office
            .state
            .names
            .get(&task.account)
            .cloned()
            .unwrap_or_default();
        let ids = office.state.email_ids.clone();
        drop(office);
        let message_id = |cid: &[u8]| ids.get(&radiopost::cbor::hex(cid)).cloned();
        let (_, email) = mail::from_message(
            &message,
            &Leaving {
                display_name: &name,
                path: &Carried::default(),
                message_id: &message_id,
                with_part: false,
                compress: false,
            },
        );
        Some(email)
    }

    /// One look at what the mail server is still to be told (RP-6 §2.6): each message sent by
    /// radio put in its sender's Sent folder, each picked up by radio marked read. Tasks whose
    /// message the mail server does not have yet wait; after two days they are given up.
    ///
    /// # Errors
    ///
    /// The mail server could not be reached, or refused: what was done is kept, the rest waits.
    pub fn mirror(&self) -> Result<Mirrored, String> {
        let tasks = self.office().state.to_mailboxes.clone();
        let mut done_here = Mirrored::default();
        if tasks.is_empty() {
            return Ok(done_here);
        }
        let settings = self.settings().mailboxes;
        let mut done = Vec::new();
        let mut failure = None;
        if settings.kind == "stalwart" && !settings.url.is_empty() {
            let ids = crate::seen::account_ids(&settings)?;
            for task in &tasks {
                let Some(account) = ids.get(&task.account.to_lowercase()) else {
                    continue;
                };
                let result = match task.kind.as_str() {
                    "read" => mark_read(&settings, account, &task.message_id),
                    "sent" => match self.sent_email(task) {
                        Some(email) => {
                            file_sent(&settings, account, &task.message_id, &email, task.queued)
                        }
                        None => Ok(true), // not a message: nothing to file
                    },
                    _ => Ok(true),
                };
                match result {
                    Ok(true) => {
                        match task.kind.as_str() {
                            "read" => done_here.read += 1,
                            _ => done_here.sent += 1,
                        }
                        done.push(task.clone());
                    }
                    Ok(false) => {}
                    Err(e) => {
                        failure = Some(e);
                        break;
                    }
                }
            }
        } else {
            failure = Some(
                "mail sent and picked up by radio is not shown in mail programs: set mailboxes \
                 to Stalwart with its key (post office guide §8)"
                    .to_owned(),
            );
        }
        let mut office = self.office();
        office.at(now());
        let before = office.state.to_mailboxes.len();
        done_here.given_up = office.mailbox_tasks_done(&done);
        if office.state.to_mailboxes.len() != before {
            self.save(&office);
        }
        drop(office);
        match failure {
            Some(e) => Err(e),
            None => Ok(done_here),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::upload_path;

    /// The session's host is left out, its path kept; with no session, JMAP's usual path.
    #[test]
    fn the_upload_goes_to_the_path_the_session_names() {
        assert_eq!(
            upload_path("https://mail.example.org/jmap/upload/{accountId}/", "b"),
            "/jmap/upload/b/"
        );
        assert_eq!(upload_path("", "c"), "/jmap/upload/c/");
    }
}
