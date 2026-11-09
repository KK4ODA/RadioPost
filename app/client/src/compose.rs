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
    /// What RP-2 refuses: no recipient, or a priority above 2.
    pub fn message(&self) -> Result<Message, String> {
        if self.to.is_empty() && self.cc.is_empty() {
            return Err("a message needs a recipient (RP-2 §2)".into());
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
    use super::{Draft, File, normalise};

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
