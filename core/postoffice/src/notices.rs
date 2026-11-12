// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the post office tells people by email about applications (D35): its operators, that one
//! is waiting; the applicant, how it was decided.
//!
//! The emails leave through the mail server's submission port, as the mail users send by radio
//! does (RP-6 §3), from the post office's contact address. An applicant is written to at the
//! address they gave to be reached at, which no one has checked, so nothing in these emails is
//! a secret but the new mailbox's first password, which only an approved applicant is sent.

use std::fmt::Write as _;

use serde_json::Value as Json;

use crate::mail::{base64_lines, encoded_header, rfc5322_date};
use crate::operator::Application;

/// One email, ready for SMTP: plain UTF-8 text, base64, with RFC 2047 headers where they need it.
#[must_use]
pub fn compose(from: &str, to: &str, subject: &str, text: &str, at: u64, id: &str) -> String {
    let domain = from
        .rsplit_once('@')
        .map_or("radiopost.invalid", |(_, d)| d);
    let body = text.replace("\r\n", "\n").replace('\n', "\r\n");
    format!(
        "From: RadioPost <{from}>\r\nTo: {to}\r\nSubject: {}\r\nDate: {}\r\n\
         Message-ID: <{id}@{domain}>\r\nAuto-Submitted: auto-generated\r\nMIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{}",
        encoded_header(subject),
        rfc5322_date(at),
        base64_lines(body.as_bytes())
    )
}

/// Sends `email` from `from` to `to` through the submission port at `smtp`.
///
/// # Errors
///
/// The URL is not an SMTP URL, or the mail server refused.
pub fn send(smtp: &str, from: &str, to: &str, email: &str) -> Result<(), String> {
    use lettre::Transport as _;
    let transport = lettre::SmtpTransport::from_url(smtp)
        .map_err(|e| format!("the submission port: {e}"))?
        .build();
    let parse = |a: &str| {
        a.parse::<lettre::Address>()
            .map_err(|e| format!("{a}: {e}"))
    };
    let envelope = lettre::address::Envelope::new(Some(parse(from)?), vec![parse(to)?])
        .map_err(|e| e.to_string())?;
    transport
        .send_raw(&envelope, email.as_bytes())
        .map(|_| ())
        .map_err(|e| format!("the mail server: {e}"))
}

/// The operators' notice of a new application, with what the lookups found.
#[must_use]
pub fn new_application(a: &Application) -> (String, String) {
    let mut text = format!(
        "{} ({}) asks for {}.\nTo be written to at {}.\n\n",
        a.name, a.operator, a.address, a.contact
    );
    if let Some(e) = &a.evidence {
        let verdict = match e["verdict"].as_str() {
            Some("good") => "The licence lookups agree with the application.",
            Some("bad") => "The licence lookups found a problem.",
            _ => "Something in the licence lookups needs a look.",
        };
        text.push_str(verdict);
        text.push('\n');
        for line in lines(e, "lines").into_iter().chain(lines(e, "flags")) {
            let _ = writeln!(text, "  {line}");
        }
        text.push('\n');
    }
    text.push_str(
        "Open the Post office button in your RadioPost client to approve it, decline it or \
         send a code by post.\n",
    );
    (
        format!(
            "RadioPost: {} ({}) applied for an account",
            a.name, a.operator
        ),
        text,
    )
}

fn lines(e: &Json, k: &str) -> Vec<String> {
    e[k].as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l.as_str().map(str::to_owned))
        .collect()
}

/// Where the users' mail programs connect, from the info document's `mail` (RP-5 §4.1).
pub struct MailPrograms {
    /// `host:port` of IMAP, with TLS.
    pub imap: Option<String>,
    /// The mail server's host, for SMTP submission on 465.
    pub host: Option<String>,
}

/// The applicant's email about the decision: approved (with the mailbox's first password when
/// one was made), declined with the reason, or a code coming by post.
#[must_use]
pub fn decided(
    a: &Application,
    contact: &str,
    password: Option<&str>,
    programs: &MailPrograms,
) -> Option<(String, String)> {
    let hello = format!("{},\n\n", a.name);
    let questions = format!("\nQuestions: {contact}\n\n73\n");
    match a.status.as_str() {
        "approved" => {
            let mut text = format!(
                "{hello}Your RadioPost account {} is ready, for {}. Your RadioPost client took its \
                 credential when it last asked: it is enrolled, and calls gateways now.\n\n\
                 Email reaches you only from the addresses on your accept list, and writing to \
                 someone adds them. Add others in the client's Setup.\n",
                a.address, a.operator
            );
            if let Some(pw) = password {
                let _ = write!(
                    text,
                    "\nYour mailbox works in an ordinary mail program too, when you have the \
                     Internet:\n  user name  {}\n  password   {pw}\n",
                    a.address
                );
                if let Some(imap) = &programs.imap {
                    let _ = writeln!(text, "  IMAP       {imap} (TLS)");
                }
                if let Some(host) = &programs.host {
                    let _ = writeln!(text, "  SMTP       {host}:465 (TLS)");
                }
                text.push_str(
                    "Change the password in the mail server's web page when you first sign in.\n",
                );
            }
            text.push_str(&questions);
            Some((format!("Welcome to RadioPost: {}", a.address), text))
        }
        "declined" => Some((
            "Your RadioPost application".into(),
            format!(
                "{hello}Your application for {} was not approved:\n\n  {}\n{questions}",
                a.address,
                a.text.as_deref().unwrap_or("no reason was given")
            ),
        )),
        "postcard" => Some((
            "Your RadioPost application: a code by post".into(),
            format!(
                "{hello}To be sure the licence for {} is yours, a postcard with a one-time code \
                 goes to the address on your licence. When it comes, choose \"I have a code\" in \
                 the RadioPost client's Setup, and enroll with your address, your callsign and \
                 the code. It is good for 30 days.\n{questions}",
                a.operator
            ),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_email_is_plain_text_in_base64_with_its_subject_encoded() {
        let e = compose(
            "postmaster@radiopost.org",
            "jose@example.net",
            "Bienvenido, José",
            "Hola\nJosé\n",
            1_794_494_400,
            "w1",
        );
        assert!(e.contains("Subject: =?"), "{e}");
        assert!(e.contains("Date: Thu, 12 Nov 2026 14:40:00 +0000"), "{e}");
        assert!(e.contains("Message-ID: <w1@radiopost.org>"), "{e}");
        let body = e
            .split("\r\n\r\n")
            .nth(1)
            .expect("a body")
            .replace("\r\n", "");
        let decoded = radiopost_tester::mailserver::base64_decode(&body).expect("base64");
        assert_eq!(
            String::from_utf8(decoded).expect("UTF-8"),
            "Hola\r\nJosé\r\n"
        );
    }

    #[test]
    fn each_decision_says_what_comes_next() {
        let mut a = Application {
            name: "José Pérez".into(),
            operator: "N5CALL".into(),
            address: "jose@radiopost.org".into(),
            status: "approved".into(),
            ..Application::default()
        };
        let programs = MailPrograms {
            imap: Some("po.radiopost.org:993".into()),
            host: Some("po.radiopost.org".into()),
        };
        let (subject, text) =
            decided(&a, "postmaster@radiopost.org", Some("pw"), &programs).expect("said");
        assert_eq!(subject, "Welcome to RadioPost: jose@radiopost.org");
        assert!(
            text.contains("password   pw") && text.contains("po.radiopost.org:993"),
            "{text}"
        );
        a.status = "declined".into();
        a.text = Some("N5CALL is not licensed to José Pérez.".into());
        let (_, text) = decided(&a, "postmaster@radiopost.org", None, &programs).expect("said");
        assert!(text.contains("not licensed"), "{text}");
        a.status = "pending".into();
        assert!(decided(&a, "x@y.z", None, &programs).is_none());
    }
}
