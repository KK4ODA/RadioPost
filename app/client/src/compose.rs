// SPDX-License-Identifier: MIT OR Apache-2.0
//! Writing a message (RP-2): its fields from what the user gave, its text normalised as RP-2 §2
//! requires (NFC, LF line endings), and the files attached.
//!
//! A reply, a reply to all or a forward starts from the message it answers ([`answer`], W1).

use radiopost::cbor::Value;
use radiopost::message::{Message, key};
use unicode_normalization::UnicodeNormalization;

/// A file attached (RP-2 §2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    /// Its name.
    pub name: String,
    /// Its Internet media type.
    pub media_type: String,
    /// Its bytes.
    pub data: Vec<u8>,
}

/// What a message says, before it is encoded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    /// The sender's address.
    pub from: String,
    /// Recipients.
    pub to: Vec<String>,
    /// Copy recipients.
    pub cc: Vec<String>,
    /// When it was written, Unix seconds.
    pub date: u64,
    /// The subject.
    pub subject: String,
    /// The text.
    pub body: String,
    /// Files attached.
    pub files: Vec<File>,
    /// 0 routine, 1 priority, 2 emergency.
    pub priority: u64,
    /// The content ID of the message this answers.
    pub reply_to: Option<[u8; 12]>,
}

/// RP-0 §4.3: NFC, and a single LF for every line ending.
#[must_use]
pub fn normalise(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .nfc()
        .collect()
}

/// RP-0 §6.2: an address with its domain in lowercase; the local part is the account's own.
#[must_use]
pub fn address(text: &str) -> String {
    let text = text.trim();
    match text.rsplit_once('@') {
        Some((local, domain)) => format!("{local}@{}", domain.to_ascii_lowercase()),
        None => text.to_owned(),
    }
}

/// The characters a local part may hold besides letters and digits: RFC 5322's `atext`.
const ATEXT: &str = "!#$%&'*+-/=?^_`{|}~";

/// RP-0 §6.2: an Internet mail address (RFC 5322's `addr-spec`, in its dot-atom form), checked as
/// a user typed it, and returned with its domain in lowercase. Checked here, where the user can
/// still correct it: a mistyped address would otherwise go on the air and fail at the post office.
///
/// # Errors
///
/// It is not an address, with the reason in the user's words.
pub fn checked_address(text: &str) -> Result<String, String> {
    let text = text.trim();
    let wrong = |why: &str| {
        Err(format!(
            "\u{201c}{text}\u{201d} is not an email address: {why}"
        ))
    };
    let Some((local, domain)) = text.rsplit_once('@') else {
        return wrong("it has no @");
    };
    if local.is_empty() {
        return wrong("nothing comes before the @");
    }
    if domain.is_empty() {
        return wrong("nothing comes after the @");
    }
    if text.len() > 254 || local.len() > 64 {
        return wrong("it is too long");
    }
    if !local
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ATEXT.contains(ch))
    {
        return wrong("the part before the @ holds a character an address cannot");
    }
    if local.starts_with('.') || local.ends_with('.') || local.contains("..") {
        return wrong("a dot at the start or end of the name, or two in a row");
    }
    if domain.starts_with('[') {
        return wrong("write the domain's name, not its numbers");
    }
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return wrong("the domain needs a dot, as in example.org");
    }
    for label in &labels {
        let fine = !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-');
        if !fine {
            return wrong(
                "the domain holds an empty part, a character a domain cannot, or a - at a part's start or end",
            );
        }
    }
    if labels
        .last()
        .is_some_and(|tld| tld.chars().all(|ch| ch.is_ascii_digit()))
    {
        return wrong("the domain ends in numbers");
    }
    Ok(address(text))
}

/// The addresses in what a user typed in To or Cc: separated by commas, semicolons, new lines or
/// spaces, each a plain address or a name with the address in angle brackets
/// (`Maria <maria@example.com>`, `"Pérez, Ana" <ana@example.org>`), whose name is left out.
/// Each is checked ([`checked_address`]); one given twice is kept once.
///
/// # Errors
///
/// The first entry that is not an address.
pub fn parse_addresses(text: &str) -> Result<Vec<String>, String> {
    Ok(parse_named(text)?.into_iter().map(|(_, a)| a).collect())
}

/// As [`parse_addresses`], each address with the name written before it, without its quotes:
/// what the address list keeps.
///
/// # Errors
///
/// The first entry that is not an address.
pub fn parse_named(text: &str) -> Result<Vec<(Option<String>, String)>, String> {
    let mut entries = Vec::new();
    let (mut quoted, mut angled, mut entry) = (false, false, String::new());
    for ch in text.chars() {
        match ch {
            '"' if !angled => quoted = !quoted,
            '<' if !quoted => angled = true,
            '>' if !quoted => angled = false,
            ',' | ';' | '\n' | '\r' if !quoted && !angled => {
                entries.push(std::mem::take(&mut entry));
                continue;
            }
            _ => {}
        }
        entry.push(ch);
    }
    entries.push(entry);
    let mut found: Vec<(Option<String>, String)> = Vec::new();
    for entry in entries.iter().map(|e| e.trim()).filter(|e| !e.is_empty()) {
        let named = match (entry.find('<'), entry.rfind('>')) {
            (Some(open), Some(close)) if open < close && entry[close + 1..].trim().is_empty() => {
                let name = entry[..open].trim().trim_matches('"').trim();
                vec![(
                    Some(name.to_owned()).filter(|n| !n.is_empty()),
                    checked_address(&entry[open + 1..close])?,
                )]
            }
            (None, None) => entry
                .split_whitespace()
                .map(|a| Ok((None, checked_address(a)?)))
                .collect::<Result<_, String>>()?,
            _ => {
                return Err(format!(
                    "\u{201c}{entry}\u{201d} is not an email address: write it as \
                     name@example.org, or Name <name@example.org>"
                ));
            }
        };
        for (name, a) in named {
            if !found.iter().any(|(_, x)| *x == a) {
                found.push((name, a));
            }
        }
    }
    Ok(found)
}

/// A date as people read it, UTC (RP-0 §4.4): `2026-11-09 14:30Z`.
#[must_use]
pub fn utc(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    // civil from days (H. Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60
    )
}

/// How a message is answered or passed on (W1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// To its sender, in its thread.
    Reply,
    /// To its sender, with its other recipients in Cc, in its thread.
    ReplyAll,
    /// To someone else: its text and files in a new message, which answers nothing.
    Forward,
}

impl Answer {
    /// The way of that name: `reply`, `reply-all` or `forward`.
    ///
    /// # Errors
    ///
    /// No way has that name.
    pub fn named(name: &str) -> Result<Answer, String> {
        match name {
            "reply" => Ok(Answer::Reply),
            "reply-all" => Ok(Answer::ReplyAll),
            "forward" => Ok(Answer::Forward),
            _ => Err(format!(
                "{name}: a message is answered by reply, reply-all or forward"
            )),
        }
    }

    /// Its name, as [`Answer::named`] takes it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Answer::Reply => "reply",
            Answer::ReplyAll => "reply-all",
            Answer::Forward => "forward",
        }
    }
}

/// A held attachment (RP-2 §2, key 12), as a forward lists it: not included.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    /// Its name.
    pub name: String,
    /// Its media type.
    pub media_type: String,
    /// Its size in bytes.
    pub size: u64,
}

/// What answering or forwarding a message starts with: the fields a composer fills in, which the
/// user may change, and what a forward cannot carry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Started {
    /// Recipients.
    pub to: Vec<String>,
    /// Copy recipients.
    pub cc: Vec<String>,
    /// The subject.
    pub subject: String,
    /// The text: nothing for a reply, which never quotes; the message forwarded for a forward.
    pub body: String,
    /// The files a forward carries.
    pub files: Vec<File>,
    /// What a reply answers: the original's content ID.
    pub reply_to: Option<[u8; 12]>,
    /// The original's held attachments, which a forward cannot carry: their `ref` means
    /// something only to the recipient's own post office (RP-6 §2.5).
    pub held: Vec<Held>,
}

/// `subject` with `prefix` (`Re:` or `Fwd:`) and a space before it.
fn prefixed(prefix: &str, subject: &str) -> String {
    format!("{prefix} {subject}").trim_end().to_owned()
}

/// The subject of a reply: `Re: ` and the original's, unless it starts with `Re:` already, in any
/// case.
#[must_use]
pub fn reply_subject(subject: &str) -> String {
    let already = subject
        .trim_start()
        .get(..3)
        .is_some_and(|p| p.eq_ignore_ascii_case("re:"));
    if already {
        subject.to_owned()
    } else {
        prefixed("Re:", subject)
    }
}

/// The text a forward carries: a short header (`Forwarded message`, From, Date, Subject), a blank
/// line, and the original's text. Short, because every byte goes on the air.
#[must_use]
pub fn forwarded(original: &Message) -> String {
    let map = original.map();
    let text = |k| map.get(k).and_then(Value::as_text).unwrap_or("");
    let date = map.get(key::DATE).and_then(Value::as_u64).unwrap_or(0);
    format!(
        "Forwarded message\nFrom: {}\nDate: {}\nSubject: {}\n\n{}",
        text(key::FROM),
        utc(date),
        text(key::SUBJECT),
        text(key::BODY)
    )
}

/// A forward's text with the sender's own note above the message forwarded, a blank line between.
#[must_use]
pub fn with_note(note: &str, forwarded: &str) -> String {
    let note = note.trim_end();
    if note.trim().is_empty() {
        forwarded.to_owned()
    } else {
        format!("{note}\n\n{forwarded}")
    }
}

/// The files a message carries (RP-2 §2, key 8).
#[must_use]
pub fn files_of(message: &Message) -> Vec<File> {
    message
        .map()
        .get(key::FILES)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|f| {
            Some(File {
                name: f.get(1)?.as_text()?.to_owned(),
                media_type: f
                    .get(2)
                    .and_then(Value::as_text)
                    .unwrap_or("application/octet-stream")
                    .to_owned(),
                data: f.get(3)?.as_bytes()?.to_vec(),
            })
        })
        .collect()
}

/// The attachments a message lists as held at the post office (RP-2 §2, key 12).
#[must_use]
pub fn held_of(message: &Message) -> Vec<Held> {
    message
        .map()
        .get(key::HELD)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|f| {
            Some(Held {
                name: f.get(1)?.as_text()?.to_owned(),
                media_type: f.get(2).and_then(Value::as_text).unwrap_or("").to_owned(),
                size: f.get(3).and_then(Value::as_u64).unwrap_or(0),
            })
        })
        .collect()
}

/// What answering `original` the way `how` starts with, for the account whose address is `me`.
///
/// * A reply is to the original's sender, its subject `Re: ` and the original's, its `reply_to`
///   the original's content ID (RP-2 §2, key 10), from which the post office restores
///   `In-Reply-To` and `References` (RP-6 §3.1). It never quotes the original: every byte goes on
///   the air, and the recipient's mail program shows the thread.
/// * A reply to all adds the original's To and Cc as Cc, less `me` and anything already in To.
/// * A forward answers nothing: its subject is `Fwd: ` and the original's, its text
///   [`forwarded`], and it carries the original's files; the held ones are listed, not carried.
///
/// The priority is the user's to choose, and starts routine whatever the original's.
#[must_use]
pub fn answer(original: &Message, me: &str, how: Answer) -> Started {
    let map = original.map();
    let list = |k| -> Vec<String> {
        map.get(k)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_text)
            .map(address)
            .collect()
    };
    let subject = map.get(key::SUBJECT).and_then(Value::as_text).unwrap_or("");
    let from = address(original.from().unwrap_or(""));
    let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    match how {
        Answer::Reply | Answer::ReplyAll => {
            let to = vec![from];
            let mut cc: Vec<String> = Vec::new();
            if how == Answer::ReplyAll {
                for a in list(key::TO).into_iter().chain(list(key::CC)) {
                    let taken = same(&a, me)
                        || to.iter().any(|t| same(t, &a))
                        || cc.iter().any(|c| same(c, &a));
                    if !taken {
                        cc.push(a);
                    }
                }
            }
            Started {
                to,
                cc,
                subject: reply_subject(subject),
                reply_to: Some(original.content_id().0),
                ..Started::default()
            }
        }
        Answer::Forward => Started {
            subject: prefixed("Fwd:", subject),
            body: forwarded(original),
            files: files_of(original),
            held: held_of(original),
            ..Started::default()
        },
    }
}

/// A media type for a file's name, for the common kinds; anything else is octets.
#[must_use]
pub fn media_type(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "txt" => "text/plain",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "csv" => "text/csv",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

impl Draft {
    /// The message, encoded deterministically.
    ///
    /// # Errors
    ///
    /// What RP-2 refuses: no recipient, a recipient that is not an address (RP-0 §6.2), or a
    /// priority above 2.
    pub fn message(&self) -> Result<Message, String> {
        if self.to.is_empty() && self.cc.is_empty() {
            return Err("a message needs a recipient (RP-2 §2)".into());
        }
        for recipient in self.to.iter().chain(&self.cc) {
            checked_address(recipient)?;
        }
        if self.priority > 2 {
            return Err("the priority is 0, 1 or 2 (RP-2 §2)".into());
        }
        let addresses = |list: &[String]| {
            Value::Array(
                list.iter()
                    .map(|a| Value::text(address(a).as_str()))
                    .collect(),
            )
        };
        let mut entries = vec![
            (key::V, Value::Uint(1)),
            (key::FROM, Value::text(address(&self.from).as_str())),
            (key::TO, addresses(&self.to)),
            (key::DATE, Value::Uint(self.date)),
            (key::SUBJECT, Value::text(normalise(&self.subject).as_str())),
            (key::BODY, Value::text(normalise(&self.body).as_str())),
        ];
        if !self.cc.is_empty() {
            entries.push((key::CC, addresses(&self.cc)));
        }
        if !self.files.is_empty() {
            let files = self
                .files
                .iter()
                .map(|f| {
                    Value::int_map(vec![
                        (1, Value::text(normalise(&f.name).as_str())),
                        (2, Value::text(f.media_type.as_str())),
                        (3, Value::bytes(&f.data)),
                    ])
                })
                .collect();
            entries.push((key::FILES, Value::Array(files)));
        }
        if self.priority > 0 {
            entries.push((key::PRIORITY, Value::Uint(self.priority)));
        }
        if let Some(id) = self.reply_to {
            entries.push((key::REPLY_TO, Value::bytes(&id)));
        }
        Message::from_map(&Value::int_map(entries)).map_err(|e| format!("not a message: {}", e.0))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Draft, File, checked_address, normalise, parse_addresses, reply_subject, utc, with_note,
    };

    #[test]
    fn a_reply_says_re_once() {
        for (original, reply) in [
            ("Are you safe?", "Re: Are you safe?"),
            ("Re: Are you safe?", "Re: Are you safe?"),
            ("RE: roster", "RE: roster"),
            ("re:roster", "re:roster"),
            ("Fwd: roster", "Re: Fwd: roster"),
            ("Regarding the net", "Re: Regarding the net"),
            ("", "Re:"),
            ("Ré", "Re: Ré"),
        ] {
            assert_eq!(reply_subject(original), reply, "{original}");
        }
    }

    #[test]
    fn a_forward_puts_the_note_above() {
        let fwd = "Forwarded message\nFrom: a@b.org\n\ntext\n";
        assert_eq!(with_note("", fwd), fwd);
        assert_eq!(with_note(" \n\n", fwd), fwd);
        assert_eq!(
            with_note("See below.\n\n\n", fwd),
            format!("See below.\n\n{fwd}")
        );
        assert_eq!(utc(1_794_224_400), "2026-11-09 11:40Z");
        assert_eq!(utc(0), "1970-01-01 00:00Z");
    }

    #[test]
    fn an_address_is_checked_and_its_domain_lowercased() {
        for (typed, kept) in [
            ("maria@example.com", "maria@example.com"),
            (
                "  Maria.Perez+radio@Example.COM ",
                "Maria.Perez+radio@example.com",
            ),
            ("kk4oda@radio-post.org", "kk4oda@radio-post.org"),
            ("a_b-c@mail.example.co.uk", "a_b-c@mail.example.co.uk"),
            ("o'brien@example.ie", "o'brien@example.ie"),
        ] {
            assert_eq!(checked_address(typed).as_deref(), Ok(kept), "{typed}");
        }
        for (typed, why) in [
            ("maria", "it has no @"),
            ("@example.com", "nothing comes before the @"),
            ("maria@", "nothing comes after the @"),
            ("maria@example", "the domain needs a dot"),
            ("maria perez@example.com", "a character an address cannot"),
            ("Maria <maria@example.com>", "a character an address cannot"),
            ("maria..p@example.com", "two in a row"),
            (".maria@example.com", "start or end"),
            ("maria@-example.com", "a - at a part's start or end"),
            ("maria@example..com", "an empty part"),
            ("maria@exa_mple.com", "a character a domain cannot"),
            ("maria@[192.0.2.1]", "not its numbers"),
            ("maria@192.0.2.1", "ends in numbers"),
            ("maría@example.com", "a character an address cannot"),
        ] {
            let refused = checked_address(typed).expect_err(typed);
            assert!(refused.contains(why), "{typed}: {refused}");
            assert!(refused.contains(typed.trim()), "{typed}: {refused}");
        }
        let long = format!("{}@example.com", "a".repeat(65));
        assert!(
            checked_address(&long)
                .expect_err("long")
                .contains("too long")
        );
    }

    #[test]
    fn what_a_user_types_in_to_is_read_as_addresses() {
        let read = |typed: &str| parse_addresses(typed);
        assert_eq!(
            read("ana@hampost.org, maria@example.com"),
            Ok(vec!["ana@hampost.org".into(), "maria@example.com".into()])
        );
        assert_eq!(
            read("ana@hampost.org; maria@example.com\nbob@example.net  eve@example.org"),
            Ok(vec![
                "ana@hampost.org".into(),
                "maria@example.com".into(),
                "bob@example.net".into(),
                "eve@example.org".into()
            ])
        );
        assert_eq!(
            read(r#"Maria Perez <maria@Example.com>, "Lopez, Ana" <ana@hampost.org>"#),
            Ok(vec!["maria@example.com".into(), "ana@hampost.org".into()])
        );
        assert_eq!(
            read("ana@hampost.org, Ana <ana@hampost.org>,,"),
            Ok(vec!["ana@hampost.org".into()]),
            "once, and empty entries ignored"
        );
        assert_eq!(read("  "), Ok(Vec::new()));
        for typed in [
            "Maria <maria@example.com",
            "maria@example.com>",
            "<maria@example.com> Maria",
            "ana@hampost.org, maria@example",
        ] {
            let refused = read(typed).expect_err(typed);
            assert!(
                refused.contains("is not an email address"),
                "{typed}: {refused}"
            );
        }
    }

    #[test]
    fn a_draft_with_a_recipient_that_is_not_an_address_is_refused() {
        let draft = Draft {
            from: "kk4oda@radiopost.org".into(),
            to: vec!["ana@hampost.org".into()],
            cc: vec!["maria@example".into()],
            ..Draft::default()
        };
        let refused = draft.message().expect_err("refused");
        assert!(refused.contains("maria@example"), "{refused}");
    }

    #[test]
    fn text_is_nfc_with_lf() {
        assert_eq!(
            normalise("Cafe\u{301}\r\nline\rend"),
            "Caf\u{e9}\nline\nend"
        );
    }

    #[test]
    fn a_draft_becomes_a_message_rp2_reads() {
        let draft = Draft {
            from: "facundo@RadioPost.ORG".into(),
            to: vec!["ana@hampost.org".into()],
            date: 1_794_249_000,
            subject: "Hola".into(),
            body: "All well.\r\n".into(),
            files: vec![File {
                name: "map.txt".into(),
                media_type: "text/plain".into(),
                data: b"N".to_vec(),
            }],
            priority: 1,
            ..Draft::default()
        };
        let m = draft.message().expect("a message");
        assert_eq!(m.from(), Some("facundo@radiopost.org"));
        assert_eq!(m.recipients(), ["ana@hampost.org"]);
        // the same draft is the same message, so the same content ID
        assert_eq!(draft.message().expect("again").content_id(), m.content_id());
        assert!(Draft::default().message().is_err(), "no recipient");
    }
}
