// SPDX-License-Identifier: MIT OR Apache-2.0
//! What follows an application, outside the API (D35): the operators told of a new one, and,
//! once one is decided, the mailbox made for an approved account and the applicant told how it
//! went. Each runs on a thread of its own and says how it went in the application's own members
//! (`mailbox`, `told`), which the desk shows, and in the log.

use serde_json::{Value as Json, json};

use crate::mailboxes::{self, new_password};
use crate::notices::{self, MailPrograms};
use crate::operator::Application;
use crate::serve::{Running, now};

impl Running {
    fn application_of(&self, id: &str) -> Option<Application> {
        self.office()
            .state
            .applications
            .iter()
            .find(|a| a.application == id)
            .cloned()
    }

    fn note(&self, id: &str, change: impl FnOnce(&mut Application)) {
        let mut office = self.office();
        if let Some(a) = office
            .state
            .applications
            .iter_mut()
            .find(|a| a.application == id)
        {
            change(a);
        }
        self.save(&office);
    }

    /// Sends one email from the post office's contact address; `Ok(false)` when it has no
    /// submission port to send through.
    fn email(&self, to: &str, subject: &str, text: &str) -> Result<bool, String> {
        let settings = self.settings();
        let Some(smtp) = settings.smtp.as_deref() else {
            return Ok(false);
        };
        let from = settings.info.contact.clone();
        let id = format!("notice-{}-{}", now(), new_password().to_lowercase());
        let email = notices::compose(&from, to, subject, text, now(), &id);
        notices::send(smtp, &from, to, &email).map(|()| true)
    }

    /// Tells the operators, at the post office's contact address, that `id` waits for them.
    pub(crate) fn announce(&self, id: &str) {
        let Some(a) = self.application_of(id) else {
            return;
        };
        let (subject, text) = notices::new_application(&a);
        let to = self.settings().info.contact;
        match self.email(&to, &subject, &text) {
            Ok(true) => eprintln!("radiopost-po: told {to} of {}'s application", a.operator),
            Ok(false) => {}
            Err(e) => eprintln!(
                "radiopost-po: could not tell {to} of {}'s application: {e}",
                a.operator
            ),
        }
    }

    /// After a decision: the mailbox for an approved account, then the email to the applicant.
    pub(crate) fn after_decision(&self, id: &str) {
        let Some(me) = self.lookups().me() else {
            self.follow_decision(id);
            return;
        };
        let id = id.to_owned();
        std::thread::spawn(move || me.follow_decision(&id));
    }

    fn follow_decision(&self, id: &str) {
        let Some(a) = self.application_of(id) else {
            return;
        };
        let settings = self.settings();
        let mut password = None;
        if a.status == "approved" {
            let pw = new_password();
            let made = mailboxes::make(&settings.mailboxes, &a.address, &a.name, &pw);
            let record = match &made {
                Ok(Some(said)) => {
                    eprintln!("radiopost-po: {said}");
                    password = Some(pw);
                    json!({"made": said, "at": now()})
                }
                Ok(None) => json!({"error": "no way to make mailboxes is set: make it in the \
                                             mail server, or set one in Settings", "at": now()}),
                Err(e) => {
                    eprintln!("radiopost-po: the mailbox for {}: {e}", a.address);
                    json!({"error": e, "at": now()})
                }
            };
            self.note(id, |x| x.mailbox = Some(record));
        }
        let programs = programs(&settings.info.mail);
        let Some((subject, text)) =
            notices::decided(&a, &settings.info.contact, password.as_deref(), &programs)
        else {
            return;
        };
        let told = match self.email(&a.contact, &subject, &text) {
            Ok(true) => {
                eprintln!(
                    "radiopost-po: told {} of the decision on {}",
                    a.contact, a.address
                );
                json!({"to": a.contact, "at": now()})
            }
            Ok(false) => {
                json!({"to": a.contact, "error": "no submission port (`smtp`) to send it through"})
            }
            Err(e) => {
                eprintln!("radiopost-po: could not tell {}: {e}", a.contact);
                json!({"to": a.contact, "error": e})
            }
        };
        self.note(id, |x| x.told = Some(told));
    }
}

/// Where mail programs connect, from the info document's `mail` (`imap` as `host:port`).
fn programs(mail: &Json) -> MailPrograms {
    let imap = mail["imap"].as_str().map(str::to_owned);
    let host = imap.as_deref().and_then(|i| {
        i.rsplit_once(':')
            .map(|(h, _)| h.to_owned())
            .or(Some(i.to_owned()))
    });
    MailPrograms { imap, host }
}
