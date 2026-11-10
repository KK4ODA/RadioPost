// SPDX-License-Identifier: MIT OR Apache-2.0
//! Email in and out at a running post office (RP-6): the door, as the mail server's MTA hook, and
//! the sender, which hands the mail users sent to the mail server by SMTP submission.
//!
//! **The door.** The mail server calls it at `RCPT TO` and at `DATA` with a JSON body, and it
//! answers whether to take the recipient or the message (RP-6 §2.1, §2.3). Two shapes of request
//! are read: Stalwart's MTA hooks (`context.stage`, `message.headers` and `contents`), and the
//! MTA Hooks Internet-Draft's (`stage`, `rawMessage` in base64), which Stalwart's authors wrote;
//! the answer takes the shape of the request (Stalwart's reply in its 1.1 form, with
//! `enhanced_status`). Only the post office's accounts are judged; any other address is the mail
//! server's. The DMARC and DKIM verdicts are read from the mail server's own fields (the draft's
//! `senderAuth`, Stalwart's `serverHeaders`), never from the message's headers; Stalwart 1.1
//! gives none, and the door then makes the checks itself (`auth`). A message taken at `DATA` is
//! made into its RadioPost message there (§2.4, §3.3, §4) and put in the recipient's mailbox;
//! held attachments are written to `held/` in the post office's directory.
//!
//! **The sender.** Mail a user sent to addresses this post office does not serve waits in
//! `outgoing`. Each is made into an email (§3.1) and submitted, one SMTP transaction for the
//! recipients at RadioPost post offices it knows (with the message part, §4) and one for the
//! rest. A recipient the mail server refuses for good, or one still not reached after three
//! days, gets the sender a `Not delivered` message (§3.3).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::net::IpAddr;

use radiopost::base64url;
use radiopost::message::Message;
use serde_json::{Value as Json, json};

use crate::mail::{self, Arrival, Bounce, Carried, Door, Leaving, Verdicts};
use crate::office::Outgoing;
use crate::serve::{Running, now};

/// How mail leaves: the envelope sender, the recipients and the email; an error says whether it
/// is for good (`true`) and why.
pub type Submit<'a> = dyn Fn(&str, &[String], &[u8]) -> Result<(), (bool, String)> + 'a;

/// How long a message waits for a recipient the mail server cannot reach yet.
pub const GIVE_UP_S: u64 = 3 * 86_400;

/// Standard base64 (RFC 4648 §4), padding and line breaks allowed.
fn standard_base64(text: &str) -> Option<Vec<u8>> {
    let url: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '=')
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            c => c,
        })
        .collect();
    base64url::decode(&url)
}

/// One call of the hook, read.
#[derive(Debug, Default)]
pub struct Hook {
    /// `rcpt`, `data`, or a stage the door does not judge.
    pub stage: String,
    /// The envelope sender; empty for the null sender.
    pub sender: String,
    /// The envelope recipients, the last one added last.
    pub recipients: Vec<String>,
    /// The whole message, at `data`.
    pub raw: Option<Vec<u8>>,
    /// The DMARC result: the mail server's, or the door's own (`auth`).
    pub dmarc: String,
    /// The domains a DKIM signature passed for.
    pub dkim_pass: Vec<String>,
    /// Whether the mail server gave its verdicts (the draft's `senderAuth`, or the
    /// `Authentication-Results` among Stalwart's `serverHeaders`); without them the door makes
    /// the checks itself.
    pub given: bool,
    /// The connecting client: its address and the name it gave in EHLO.
    pub client: Option<(IpAddr, String)>,
    /// The Internet-Draft's shape, not Stalwart's.
    pub draft: bool,
}

/// DMARC and DKIM from `Authentication-Results` headers (RFC 8601).
fn authentication_results(values: &[String], hook: &mut Hook) {
    for value in values {
        for segment in value.split(';').map(str::trim) {
            let lower = segment.to_lowercase();
            if let Some(result) = lower.strip_prefix("dmarc=") {
                if hook.dmarc.is_empty() || result.starts_with("fail") {
                    result
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .clone_into(&mut hook.dmarc);
                }
            } else if lower.starts_with("dkim=pass") {
                let domain = lower.split_whitespace().find_map(|t| {
                    t.strip_prefix("header.d=")
                        .or_else(|| t.strip_prefix("header.i=@"))
                        .map(str::to_owned)
                });
                if let Some(d) = domain
                    && !hook.dkim_pass.contains(&d)
                {
                    hook.dkim_pass.push(d);
                }
            }
        }
    }
}

/// The client at `ip` that gave `helo`, if both are there.
fn client_of(ip: Option<&Json>, helo: Option<&Json>) -> Option<(IpAddr, String)> {
    let ip = ip?.as_str()?.parse().ok()?;
    let helo = helo.and_then(Json::as_str).unwrap_or("").to_owned();
    Some((ip, helo))
}

/// Reads a hook request in either shape. Verdicts are taken only from where the mail server
/// writes them, never from the message's own headers, which its sender wrote.
#[must_use]
pub fn read_hook(body: &Json) -> Hook {
    let address = |v: &Json| {
        v.get("address")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_owned()
    };
    let envelope = &body["envelope"];
    let mut hook = Hook {
        sender: address(&envelope["from"]),
        recipients: envelope["to"]
            .as_array()
            .map(|a| a.iter().map(address).collect())
            .unwrap_or_default(),
        ..Hook::default()
    };
    if let Some(stage) = body.get("stage").and_then(Json::as_str) {
        hook.draft = true;
        hook.stage = stage.to_lowercase();
        hook.raw = body
            .get("rawMessage")
            .and_then(Json::as_str)
            .and_then(standard_base64);
        if let Some(auth) = body.get("senderAuth").filter(|a| a.is_object()) {
            hook.given = true;
            if let Some(d) = auth.get("dmarc").and_then(Json::as_str) {
                hook.dmarc = d.to_lowercase();
            }
        }
        hook.client = client_of(body.pointer("/client/ip"), body.pointer("/client/ehlo"));
    } else {
        hook.stage = body
            .pointer("/context/stage")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_lowercase();
        let pairs = |name: &str| -> Vec<(String, String)> {
            body["message"][name]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|h| {
                    let a = h.as_array()?;
                    Some((
                        a.first()?.as_str()?.to_owned(),
                        a.get(1)?.as_str()?.to_owned(),
                    ))
                })
                .collect()
        };
        let headers = pairs("headers");
        if let Some(contents) = body.pointer("/message/contents").and_then(Json::as_str) {
            let mut raw = String::new();
            // Stalwart 1.1 gives each value with its leading space and its line ending; older
            // versions gave neither.
            for (n, v) in &headers {
                let space = if v.starts_with([' ', '\t']) { "" } else { " " };
                let end = if v.ends_with('\n') { "" } else { "\r\n" };
                let _ = write!(raw, "{n}:{space}{v}{end}");
            }
            raw.push_str("\r\n");
            raw.push_str(contents);
            hook.raw = Some(raw.into_bytes());
        }
        let results: Vec<String> = pairs("serverHeaders")
            .into_iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case("Authentication-Results"))
            .map(|(_, v)| v)
            .collect();
        hook.given = !results.is_empty();
        authentication_results(&results, &mut hook);
        hook.client = client_of(
            body.pointer("/context/client/ip"),
            body.pointer("/context/client/helo"),
        );
    }
    if hook.given && hook.dmarc.is_empty() {
        hook.dmarc = "none".into();
    }
    hook
}

/// The door's answer in the shape of the request: an SMTP reply and its text.
#[must_use]
pub fn hook_answer(draft: bool, reply: &str, text: &str) -> (u16, String) {
    let (code, enhanced) = reply.split_once(' ').unwrap_or((reply, ""));
    let code: u16 = code.parse().unwrap_or(451);
    if code < 300 {
        return if draft {
            (204, String::new())
        } else {
            (200, json!({"action": "accept"}).to_string())
        };
    }
    let answer = if draft {
        json!({"set": [
            {"path": "/action", "value": "reject"},
            {"path": "/response", "value": {"code": code, "enhancedCode": enhanced, "message": text}},
        ]})
    } else {
        json!({"action": "reject",
               "response": {"status": code, "enhanced_status": enhanced, "message": text}})
    };
    (200, answer.to_string())
}

/// RP-6 §2.1: the sender could not be checked for now (a DNS lookup failed), so the sending
/// server tries again later.
pub const TRY_LATER: &str = "451 4.7.1";

/// RP-6 §2.3: what a refusal says, in plain words.
fn refusal_text(reply: &str, recipient: &str, max_email_bytes: u64) -> String {
    let domain = mail::domain_of(recipient);
    match reply {
        TRY_LATER => format!(
            "The sender of mail to {domain} could not be checked just now. Please try again later."
        ),
        mail::NOT_LISTED => format!(
            "<{recipient}> reads mail by amateur radio and takes it only from senders on their \
             list. Ask them to add you. Amateur radio carries no business content."
        ),
        mail::TOO_LARGE => format!(
            "Mail to {domain} travels by amateur radio and must be under {max_email_bytes} bytes."
        ),
        mail::NOT_ALLOWED => format!(
            "Mail to {domain} travels by amateur radio, where encryption is not allowed and \
             programs are not carried. Please send it unencrypted, without programs."
        ),
        _ => "Not taken.".to_owned(),
    }
}

impl Running {
    fn ours(&self, address: &str) -> bool {
        let domain = mail::domain_of(address);
        self.settings()
            .domains
            .iter()
            .any(|d| d.to_lowercase() == domain)
    }

    /// Answers one call of the mail server's MTA hook (RP-6 §2.1): a status and a body.
    #[must_use]
    pub fn hook(&self, body: &[u8]) -> (u16, String) {
        let Ok(json) = serde_json::from_slice::<Json>(body) else {
            return (400, json!({"error": "not JSON"}).to_string());
        };
        let mut hook = read_hook(&json);
        let settings = self.settings();
        let max = settings.info.max_email_bytes;
        let accounts = {
            let mut office = self.office();
            office.at(now());
            office.door_accounts()
        };
        // The door judges the post office's accounts; any other address at its domains
        // (postmaster@, abuse@) is the mail server's to deliver or refuse.
        let account = |r: &String| accounts.contains_key(&r.to_lowercase());
        let ours: Vec<String> = hook
            .recipients
            .iter()
            .filter(|r| account(r))
            .cloned()
            .collect();
        if hook.stage == "data" && !ours.is_empty() {
            if !hook.given {
                self.check_sender(&mut hook);
            }
            if hook.dmarc == "temperror" {
                return hook_answer(
                    hook.draft,
                    TRY_LATER,
                    &refusal_text(TRY_LATER, &ours[0], max),
                );
            }
        }
        let mut office = self.office();
        office.at(now());
        let known = office.known_post_offices();
        let sent = |id: &str| office.sent_content_id(id).is_some();
        let door = Door {
            accounts: &accounts,
            known_post_offices: &known,
            max_email_bytes: max,
            sent: &sent,
        };
        let verdicts = Verdicts {
            dmarc: &hook.dmarc,
            dkim_pass: &hook.dkim_pass,
        };
        let refused = match hook.stage.as_str() {
            "rcpt" => hook
                .recipients
                .last()
                .filter(|r| account(r))
                .map(|r| (mail::rcpt(&hook.sender, r, &door), r.clone()))
                .filter(|(reply, _)| *reply != mail::OK_RCPT),
            "data" => {
                let raw = hook.raw.as_deref().unwrap_or_default();
                ours.iter().find_map(|r| {
                    let (rcpt, data) = mail::door(&hook.sender, r, raw, &verdicts, &door);
                    let reply = data.unwrap_or(rcpt);
                    (reply != mail::OK_DATA).then(|| (reply, r.clone()))
                })
            }
            _ => None,
        };
        drop(office);
        if let Some((reply, recipient)) = refused {
            return hook_answer(hook.draft, reply, &refusal_text(reply, &recipient, max));
        }
        if hook.stage == "data"
            && !ours.is_empty()
            && let Some(raw) = &hook.raw
        {
            if let Err(e) = self.email_in(&hook.sender, &ours, raw) {
                eprintln!("radiopost-po: an email taken but not made a message: {e}");
            }
        }
        let ok = if hook.stage == "rcpt" {
            mail::OK_RCPT
        } else {
            mail::OK_DATA
        };
        hook_answer(hook.draft, ok, "")
    }

    /// The door's own sender checks (`auth`), when the mail server gave no verdicts. Without
    /// the client to check, or with no resolver to check it, the message waits: `temperror`.
    fn check_sender(&self, hook: &mut Hook) {
        let (Some((ip, helo)), Some(raw)) = (&hook.client, &hook.raw) else {
            eprintln!(
                "radiopost-po: the mail server gave the door neither its verdicts nor the client"
            );
            "temperror".clone_into(&mut hook.dmarc);
            return;
        };
        match self.checker() {
            Ok(checker) => {
                let found = checker.check(*ip, helo, &hook.sender, raw);
                hook.dmarc = found.dmarc;
                hook.dkim_pass = found.dkim_pass;
            }
            Err(e) => {
                eprintln!("radiopost-po: {e}");
                "temperror".clone_into(&mut hook.dmarc);
            }
        }
    }

    /// An email taken at the door, made into the RadioPost message of each recipient here and
    /// put in their mailboxes (RP-6 §2.4, §3.3, §4).
    ///
    /// # Errors
    ///
    /// It could not be read as an email.
    ///
    /// # Panics
    ///
    /// When the system has no random numbers for a held attachment's ref.
    pub fn email_in(&self, sender: &str, recipients: &[String], raw: &[u8]) -> Result<(), String> {
        let settings = self.settings();
        let mut office = self.office();
        let at = now();
        office.at(at);
        let message_id = mail::parse(raw)
            .and_then(|m| m.message_id().map(|id| format!("<{id}>")))
            .filter(|id| id.len() > 2);
        for recipient in recipients {
            let address = recipient.to_lowercase();
            let sent = |id: &str| office.sent_content_id(id);
            if sender.is_empty() {
                let domain = mail::domain_of(&address);
                let m = mail::bounce(
                    raw,
                    &Bounce {
                        domain: &domain,
                        sender: &address,
                        arrival: at,
                        sent: &sent,
                    },
                )?;
                office.email_arrived(&address, &m, true, None);
                continue;
            }
            let trust = office.trust();
            if let Some(m) =
                mail::find_part(raw).and_then(|p| mail::check_part(&p, &address, &trust).ok())
            {
                office.email_arrived(&address, &m, false, message_id.as_deref());
                continue;
            }
            let found = mail::parse(raw).map_or(0, |m| m.attachment_count());
            let held_refs: Vec<[u8; 8]> = (0..found)
                .map(|_| {
                    let mut r = [0u8; 8];
                    getrandom::fill(&mut r).expect("the system's random numbers");
                    r
                })
                .collect();
            let attach = office.attach_bytes(&address, settings.info.max_air_bytes);
            let made = mail::to_message(
                raw,
                &Arrival {
                    arrival: at,
                    attach_bytes: attach,
                    max_air_bytes: settings.info.max_air_bytes,
                    sent: &sent,
                    held_refs,
                },
            )?;
            for (r, name, kind, data) in &made.held {
                let meta = json!({"address": address, "name": name, "type": kind,
                                  "size": data.len()});
                let at = format!("held/{}", radiopost::cbor::hex(r));
                if let Err(e) = self
                    .files()
                    .write_bytes(&at, data)
                    .and_then(|()| self.files().write(&format!("{at}.json"), &meta.to_string()))
                {
                    eprintln!("radiopost-po: cannot keep a held attachment: {e}");
                }
            }
            office.email_arrived(&address, &made.message, true, message_id.as_deref());
        }
        self.save(&office);
        Ok(())
    }

    /// Sends what waits in `outgoing` (RP-6 §3) through `send` (envelope sender, recipients,
    /// the email), and says what happened, one line each.
    pub fn send_waiting(&self, send: &Submit<'_>) -> Vec<String> {
        let waiting: Vec<Outgoing> = self.office().state.outgoing.clone();
        let mut said = Vec::new();
        for item in waiting {
            let (left, lines) = self.send_one(&item, send);
            said.extend(lines);
            let mut office = self.office();
            office.at(now());
            let entry = office
                .state
                .outgoing
                .iter()
                .position(|o| o.id == item.id && o.to == item.to);
            if let Some(i) = entry {
                if left.is_empty() {
                    office.state.outgoing.remove(i);
                } else {
                    office.state.outgoing[i].to = left;
                }
            }
            self.save(&office);
        }
        said
    }

    /// One message: the recipients still to try, and what was said.
    fn send_one(&self, item: &Outgoing, send: &Submit<'_>) -> (Vec<String>, Vec<String>) {
        let Some(message) = base64url::decode(&item.air).and_then(|a| Message::from_air(&a).ok())
        else {
            return (
                Vec::new(),
                vec![format!("{}: not a message, dropped", item.id)],
            );
        };
        let from = message.from().unwrap_or("").to_owned();
        let (name, known, ids) = {
            let office = self.office();
            let ids: BTreeMap<String, String> = office.state.email_ids.clone();
            (
                office.state.names.get(&from).cloned().unwrap_or_default(),
                office.known_post_offices(),
                ids,
            )
        };
        let carried = item
            .route
            .as_ref()
            .map_or_else(Carried::default, |r| Carried {
                operator: r.operator.clone(),
                gateways: r.gateways.clone(),
                started: r.started,
                session: r.session.clone(),
                proof: match (
                    base64url::decode(&r.credential),
                    base64url::decode(&r.statement),
                    base64url::decode(&r.signature),
                ) {
                    (Some(c), Some(s), Some(g)) if !c.is_empty() => Some((c, s, g)),
                    _ => None,
                },
            });
        let message_id = |cid: &[u8]| ids.get(&radiopost::cbor::hex(cid)).cloned();
        let (radiopost, others): (Vec<String>, Vec<String>) = item
            .to
            .iter()
            .cloned()
            .partition(|r| known.contains(&mail::domain_of(r)));
        let mut left = Vec::new();
        let mut failed = Vec::new();
        let mut said = Vec::new();
        for (group, with_part) in [(radiopost, true), (others, false)] {
            if group.is_empty() {
                continue;
            }
            let (_, email) = mail::from_message(
                &message,
                &Leaving {
                    display_name: &name,
                    path: &carried,
                    message_id: &message_id,
                    with_part: with_part && carried.proof.is_some(),
                },
            );
            let mut tries = vec![group.clone()];
            while let Some(to) = tries.pop() {
                match send(&from, &to, &email) {
                    Ok(()) => {
                        let cid = message.content_id();
                        let mid = format!("<{}@{}>", cid.text(), mail::domain_of(&from));
                        self.office().email_sent(&mid, &cid);
                        said.push(format!("{}: sent to {}", item.id, to.join(", ")));
                    }
                    // one recipient refused fails the whole transaction: each on its own, then
                    Err((true, _)) if to.len() > 1 => {
                        tries.extend(to.iter().rev().map(|r| vec![r.clone()]));
                    }
                    Err((true, why)) => {
                        failed.extend(to.iter().map(|r| format!("{r}: {why}")));
                        said.push(format!("{}: refused for {}: {why}", item.id, to.join(", ")));
                    }
                    Err((false, why)) if now().saturating_sub(item.queued) > GIVE_UP_S => {
                        failed.extend(
                            to.iter()
                                .map(|r| format!("{r}: not reached in three days ({why})")),
                        );
                        said.push(format!(
                            "{}: given up for {}: {why}",
                            item.id,
                            to.join(", ")
                        ));
                    }
                    Err((false, why)) => {
                        left.extend(to.iter().cloned());
                        said.push(format!(
                            "{}: will try again for {}: {why}",
                            item.id,
                            to.join(", ")
                        ));
                    }
                }
            }
        }
        if !failed.is_empty() {
            self.tell_not_delivered(&message, &from, &failed);
        }
        (left, said)
    }

    /// RP-6 §3.3: the sender is told, in their mailbox, which recipients failed and why.
    fn tell_not_delivered(&self, message: &Message, from: &str, lines: &[String]) {
        if !self.ours(from) {
            return;
        }
        let mut office = self.office();
        let at = now();
        office.at(at);
        let subject = message
            .map()
            .get(radiopost::message::key::SUBJECT)
            .and_then(radiopost::cbor::Value::as_text)
            .unwrap_or("")
            .to_owned();
        let domain = mail::domain_of(from);
        let none = |_: &str| None;
        let message_left = mail::not_delivered(
            &Bounce {
                domain: &domain,
                sender: from,
                arrival: at,
                sent: &none,
            },
            &subject,
            lines,
            Some(message.content_id().0),
        );
        if let Ok(m) = message_left {
            office.email_arrived(from, &m, true, None);
        }
        self.save(&office);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{hook_answer, read_hook};

    #[test]
    fn both_shapes_of_hook_are_read() {
        let stalwart = json!({
            "context": {"stage": "data"},
            "envelope": {"from": {"address": "maria@example.com"},
                         "to": [{"address": "facundo@radiopost.org"}]},
            "message": {
                "headers": [["From", " Maria <maria@example.com>"], ["Subject", " Hola"]],
                "serverHeaders": [["Authentication-Results",
                    " mx.radiopost.org; dkim=pass header.d=example.com; dmarc=pass header.from=example.com"]],
                "contents": "Hello.\r\n"}
        });
        let h = read_hook(&stalwart);
        assert!(!h.draft);
        assert_eq!(
            (h.stage.as_str(), h.sender.as_str()),
            ("data", "maria@example.com")
        );
        assert_eq!(h.recipients, ["facundo@radiopost.org"]);
        assert_eq!(h.dmarc, "pass");
        assert_eq!(h.dkim_pass, ["example.com"]);
        let raw = String::from_utf8(h.raw.expect("the message")).expect("text");
        assert!(raw.starts_with("From: Maria <maria@example.com>\r\nSubject: Hola\r\n\r\nHello."));

        let draft = json!({
            "stage": "data",
            "envelope": {"from": {"address": null}, "to": [{"address": "facundo@radiopost.org"}]},
            "rawMessage": "RnJvbTogeEB5LnoNCg0KaGk=",
            "senderAuth": {"dmarc": "fail"}
        });
        let h = read_hook(&draft);
        assert!(h.draft);
        assert_eq!(h.sender, "");
        assert_eq!(h.dmarc, "fail");
        assert_eq!(h.raw.as_deref(), Some(&b"From: x@y.z\r\n\r\nhi"[..]));
    }

    /// Stalwart 1.1's request, as its documentation gives it: each header value keeps its
    /// leading space and its line ending, and no `Authentication-Results` comes with it.
    #[test]
    fn stalwart_1_1_headers_keep_their_line_endings() {
        let request = json!({
            "context": {
                "stage": "data",
                "client": {"ip": "192.0.2.1", "port": 34567, "ptr": "mail.example.com",
                           "helo": "mail.example.com", "activeConnections": 1},
                "server": {"name": "Stalwart v1.1.0", "port": 25, "ip": "192.0.2.2"},
                "queue": {"id": "1d6f4a2b3c"},
                "protocol": {"version": 1}
            },
            "envelope": {"from": {"address": "john@example.com"},
                         "to": [{"address": "facundo@radiopost.org"}]},
            "message": {
                "headers": [
                    ["From", " John Doe <john@example.com>\r\n"],
                    ["To", " Facundo <facundo@radiopost.org>\r\n"],
                    ["Subject", " Hello\r\n"]
                ],
                "contents": "Hello, world.\r\n",
                "size": 158
            }
        });
        let h = read_hook(&request);
        assert_eq!(h.stage, "data");
        assert!(!h.given, "1.1 gives the door no verdicts");
        assert_eq!(
            h.client,
            Some((
                "192.0.2.1".parse().expect("an address"),
                "mail.example.com".to_owned()
            ))
        );
        let raw = String::from_utf8(h.raw.expect("the message")).expect("text");
        assert_eq!(
            raw,
            "From: John Doe <john@example.com>\r\nTo: Facundo <facundo@radiopost.org>\r\n\
             Subject: Hello\r\n\r\nHello, world.\r\n"
        );
    }

    /// Whoever sends a message can write `Authentication-Results` into it: the door reads
    /// verdicts only from the mail server's own fields.
    #[test]
    fn the_messages_own_authentication_results_are_not_read() {
        let forged = " po.radiopost.org; dkim=pass header.d=hampost.org; dmarc=pass\r\n";
        let stalwart = json!({
            "context": {"stage": "data",
                        "client": {"ip": "203.0.113.9", "helo": "forger.example.net"}},
            "envelope": {"from": {"address": "eve@hampost.org"},
                         "to": [{"address": "facundo@radiopost.org"}]},
            "message": {
                "headers": [["Authentication-Results", forged],
                            ["From", " Eve <eve@hampost.org>\r\n"]],
                "contents": "Hello.\r\n"}
        });
        let h = read_hook(&stalwart);
        assert!(!h.given);
        assert!(h.dkim_pass.is_empty(), "{:?}", h.dkim_pass);
        assert_eq!(h.dmarc, "");

        let draft = json!({
            "stage": "data",
            "envelope": {"from": {"address": "eve@hampost.org"},
                         "to": [{"address": "facundo@radiopost.org"}]},
            "rawMessage": standard(&format!("Authentication-Results:{forged}From: eve@hampost.org\r\n\r\nhi")),
            "senderAuth": {"dmarc": "none"}
        });
        let h = read_hook(&draft);
        assert!(h.given);
        assert!(h.dkim_pass.is_empty(), "{:?}", h.dkim_pass);
        assert_eq!(h.dmarc, "none");
    }

    fn standard(text: &str) -> String {
        let b64 = radiopost::base64url::encode(text.as_bytes())
            .replace('-', "+")
            .replace('_', "/");
        let pad = "=".repeat((4 - b64.len() % 4) % 4);
        format!("{b64}{pad}")
    }

    #[test]
    fn answers_take_the_shape_of_the_request() {
        assert_eq!(hook_answer(true, "250 2.0.0", "").0, 204);
        assert_eq!(
            hook_answer(false, "250 2.1.5", "").1,
            r#"{"action":"accept"}"#
        );
        let (_, body) = hook_answer(false, "550 5.7.1", "Ask them to add you.");
        let j: serde_json::Value = serde_json::from_str(&body).expect("JSON");
        assert_eq!(j["action"], "reject");
        assert_eq!(j["response"]["status"], 550);
        assert_eq!(j["response"]["enhanced_status"], "5.7.1");
        assert_eq!(j["response"]["message"], "Ask them to add you.");
        let (_, body) = hook_answer(true, "554 5.7.1", "No.");
        let j: serde_json::Value = serde_json::from_str(&body).expect("JSON");
        assert_eq!(j["set"][0]["value"], "reject");
        assert_eq!(j["set"][1]["value"]["code"], 554);
    }
}
