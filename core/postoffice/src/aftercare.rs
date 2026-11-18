// SPDX-License-Identifier: MIT OR Apache-2.0
//! What follows an application, outside the API (D35): the operators told of a new one, and,
//! once one is decided, the mailbox made for an approved account and the applicant told how it
//! went. Each runs on a thread of its own and says how it went in the application's own members
//! (`mailbox`, `told`), which the desk shows, and in the log.
//!
//! Every account needs its mailbox at the mail server, or email from outside is refused before
//! the door is asked (RP-6 §2.1). So the mailbox is made wherever an account begins: at an
//! approval (its password in the welcome), with a code for an address that is no account yet
//! (its password shown to the operator beside the code), and when an account approved by
//! postcard enrolls with the card's code (its password in the welcome then, once the licence is
//! proved). An account left without one gets it from the desk's Accounts tab or
//! `radiopost-po mailbox`. A password never goes on the air, and the post office keeps none.

use serde_json::{Value as Json, json};

use crate::mailboxes::{self, new_password};
use crate::notices::{self, MailPrograms};
use crate::operator::Application;
use crate::serve::{Running, now};

/// What a mailbox not made for want of a way says.
const NO_WAY: &str = "no way to make mailboxes is set: make it in the mail server, or set one in \
                      Settings";

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
    pub(crate) fn send_notice(&self, to: &str, subject: &str, text: &str) -> Result<bool, String> {
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
        match self.send_notice(&to, &subject, &text) {
            Ok(true) => eprintln!("radiopost-po: told {to} of {}'s application", a.operator),
            Ok(false) => {}
            Err(e) => eprintln!(
                "radiopost-po: could not tell {to} of {}'s application: {e}",
                a.operator
            ),
        }
    }

    /// Makes the mail server's mailbox for `address`, named `name`: what to record (`made` or
    /// `error`, and `at`), and the new password when one was made. The password is never kept.
    pub(crate) fn make_mailbox(&self, address: &str, name: &str) -> (Json, Option<String>) {
        let password = new_password();
        match mailboxes::make(&self.settings().mailboxes, address, name, &password) {
            Ok(Some(said)) => {
                eprintln!("radiopost-po: {said}");
                (json!({"made": said, "at": now()}), Some(password))
            }
            Ok(None) => (json!({"error": NO_WAY, "at": now()}), None),
            Err(e) => {
                eprintln!("radiopost-po: the mailbox for {address}: {e}");
                (json!({"error": e, "at": now()}), None)
            }
        }
    }

    /// The mailbox of an account, made now for the operator, who gives the password on: what was
    /// made (`address`, `made` and `password`, or `error`), or `None` when `address` is no
    /// account. Named as the account's email is, or by its address when it has no name.
    pub(crate) fn mailbox_for(&self, address: &str) -> Option<Json> {
        let address = address.trim().to_lowercase();
        let name = {
            let office = self.office();
            if !office.state.accounts.contains(&address) {
                return None;
            }
            office
                .state
                .names
                .iter()
                .find(|(a, _)| a.eq_ignore_ascii_case(&address))
                .map_or_else(|| address.clone(), |(_, n)| n.clone())
        };
        let (mut record, password) = self.make_mailbox(&address, &name);
        record["address"] = json!(address);
        if let Some(pw) = password {
            record["password"] = json!(pw);
        }
        Some(record)
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
            let (record, made) = self.make_mailbox(&a.address, &a.name);
            password = made;
            self.note(id, |x| x.mailbox = Some(record));
        }
        let Some((subject, text)) = notices::decided(
            &a,
            &settings.info.contact,
            password.as_deref(),
            &programs(&settings.info.mail),
        ) else {
            return;
        };
        self.tell(id, &a, &subject, &text);
    }

    /// After an enrollment as `address` (RP-5 §6.1): an account approved by postcard has proved
    /// its licence now, by the card's code, so its mailbox is made and the welcome goes to the
    /// applicant with the mailbox's password, as at an approval. Once only: the application
    /// records the mailbox.
    pub(crate) fn after_enrollment(&self, address: &str) {
        let id = self
            .office()
            .state
            .applications
            .iter()
            .find(|a| {
                a.status == "postcard"
                    && a.mailbox.is_none()
                    && a.address.eq_ignore_ascii_case(address)
            })
            .map(|a| a.application.clone());
        let Some(id) = id else {
            return;
        };
        let Some(me) = self.lookups().me() else {
            self.follow_postcard(&id);
            return;
        };
        std::thread::spawn(move || me.follow_postcard(&id));
    }

    fn follow_postcard(&self, id: &str) {
        let Some(a) = self.application_of(id) else {
            return;
        };
        let (record, password) = self.make_mailbox(&a.address, &a.name);
        self.note(id, |x| x.mailbox = Some(record));
        let settings = self.settings();
        let (subject, text) = notices::welcome(
            &a,
            &settings.info.contact,
            password.as_deref(),
            &programs(&settings.info.mail),
        );
        self.tell(id, &a, &subject, &text);
    }

    /// Emails the applicant of `id`, and records how it went in `told`.
    fn tell(&self, id: &str, a: &Application, subject: &str, text: &str) {
        let told = match self.send_notice(&a.contact, subject, text) {
            Ok(true) => {
                eprintln!("radiopost-po: told {} of {}", a.contact, a.address);
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
