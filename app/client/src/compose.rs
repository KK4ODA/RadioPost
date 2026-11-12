// SPDX-License-Identifier: MIT OR Apache-2.0
//! Writing a message (RP-2): its fields from what the user gave, its text normalised as RP-2 §2
//! requires (NFC, LF line endings), and the files attached.

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
    use super::{Draft, File, checked_address, normalise, parse_addresses};

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
