// SPDX-License-Identifier: MIT OR Apache-2.0
//! Email interoperation (RP-6): the door that answers the mail server during the SMTP
//! conversation (§2.1), an email made into a RadioPost message (§2.4), a message made into an
//! email (§3), the message part between post offices (§4), and bounces (§3.3).
//!
//! DMARC and DKIM verdicts are inputs, as the mail server hands them over: checking them needs DNS
//! and RSA, which are the mail server's business. Parsing is `mail-parser`'s.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use mail_parser::{Message as Email, MessageParser, MessagePart, MimeHeaders, PartType};
use radiopost::cbor::{self, Value};
use radiopost::credential::{self, Trust, flags};
use radiopost::crypto::{self, ContentId, context};
use radiopost::message::{Message, key};
use radiopost::statement::{Form, Statement, TIME_WINDOW_S};
use unicode_normalization::UnicodeNormalization as _;

/// `RCPT TO` taken.
pub const OK_RCPT: &str = "250 2.1.5";
/// `DATA` taken.
pub const OK_DATA: &str = "250 2.0.0";
/// No such mailbox.
/// Not on the accept list, not signed by a post office, or failed DMARC.
pub const NOT_LISTED: &str = "550 5.7.1";
/// Over `max_email_bytes`.
pub const TOO_LARGE: &str = "552 5.3.4";
/// Encrypted, or carries an executable.
pub const NOT_ALLOWED: &str = "554 5.7.1";

const EXECUTABLE_EXTENSIONS: [&str; 16] = [
    ".exe", ".com", ".scr", ".pif", ".bat", ".cmd", ".msi", ".js", ".jse", ".vbs", ".vbe", ".wsf",
    ".hta", ".jar", ".ps1", ".lnk",
];
const EXECUTABLE_TYPES: [&str; 3] = [
    "application/x-msdownload",
    "application/x-dosexec",
    "application/x-msi",
];
const PGP_MESSAGE: &str = "-----BEGIN PGP MESSAGE-----";

/// What the door needs to know of the post office (§2.1).
pub struct Door<'a> {
    /// Its accounts' accept lists, by address in lowercase.
    pub accounts: &'a BTreeMap<String, Vec<String>>,
    /// The RadioPost post offices it knows (RP-1 §7.3), by domain.
    pub known_post_offices: &'a [String],
    /// Its `max_email_bytes` (RP-5 §4.1).
    pub max_email_bytes: u64,
    /// The `Message-ID`s of the emails it sent from RadioPost (§3.1), with their angle brackets.
    pub sent: &'a dyn Fn(&str) -> bool,
}

/// The mail server's verdicts on an email.
pub struct Verdicts<'a> {
    /// The DMARC result for the `From:` domain: `pass`, `fail`, `none`, …
    pub dmarc: &'a str,
    /// The domains a DKIM signature passes for.
    pub dkim_pass: &'a [String],
}

/// Parses an email.
#[must_use]
pub fn parse(raw: &[u8]) -> Option<Email<'_>> {
    MessageParser::default().parse(raw)
}

/// The domain of an address, in lowercase.
#[must_use]
pub fn domain_of(address: &str) -> String {
    address
        .rsplit_once('@')
        .map_or(address, |(_, d)| d)
        .to_lowercase()
}

/// §2.2: an address entry matches ignoring case; `@domain` matches that domain exactly.
#[must_use]
pub fn on_list(address: &str, accept: &[String]) -> bool {
    let address = address.to_lowercase();
    let domain = domain_of(&address);
    accept.iter().any(|item| {
        let entry = item.to_lowercase();
        entry == address || entry.strip_prefix('@') == Some(domain.as_str())
    })
}

/// The type of a part, `type/subtype` in lowercase (`text/plain` when it says none).
fn kind(part: &MessagePart<'_>) -> String {
    part.content_type().map_or_else(
        || "text/plain".to_owned(),
        |t| {
            format!(
                "{}/{}",
                t.ctype(),
                t.subtype()
                    .unwrap_or(if t.ctype() == "text" { "plain" } else { "" })
            )
            .to_lowercase()
        },
    )
}

/// A parameter of a part's `Content-Type`.
fn param<'p>(part: &'p MessagePart<'_>, name: &str) -> Option<&'p str> {
    part.content_type().and_then(|t| t.attribute(name))
}

/// Every part, descending into attached messages, as Python's `walk` does.
fn walk<'m, 'x>(email: &'m Email<'x>, out: &mut Vec<&'m MessagePart<'x>>) {
    for part in &email.parts {
        out.push(part);
        if let PartType::Message(inner) = &part.body {
            walk(inner, out);
        }
    }
}

fn parts<'m, 'x>(email: &'m Email<'x>) -> Vec<&'m MessagePart<'x>> {
    let mut out = Vec::new();
    walk(email, &mut out);
    out
}

/// A text part's decoded text.
fn text_of(part: &MessagePart<'_>) -> Option<String> {
    match &part.body {
        PartType::Text(t) | PartType::Html(t) => Some(t.to_string()),
        PartType::Binary(b) | PartType::InlineBinary(b) => {
            Some(String::from_utf8_lossy(b).into_owned())
        }
        _ => None,
    }
}

/// §2.1: encrypted, as opposed to signed.
#[must_use]
pub fn encrypted(email: &Email<'_>) -> bool {
    parts(email).into_iter().any(|part| {
        let k = kind(part);
        if k == "multipart/encrypted" {
            return true;
        }
        let enveloped =
            param(part, "smime-type").map_or(String::new(), str::to_lowercase) != "signed-data";
        if (k == "application/pkcs7-mime" || k == "application/x-pkcs7-mime") && enveloped {
            return true;
        }
        k.starts_with("text/") && text_of(part).is_some_and(|t| t.contains(PGP_MESSAGE))
    })
}

/// §2.1: carries an executable.
#[must_use]
pub fn executable(email: &Email<'_>) -> bool {
    parts(email).into_iter().any(|part| {
        let name = part.attachment_name().unwrap_or("").to_lowercase();
        EXECUTABLE_TYPES.contains(&kind(part).as_str())
            || EXECUTABLE_EXTENSIONS.iter().any(|e| name.ends_with(e))
    })
}

/// A `Message-ID` as written, with its angle brackets.
fn bracketed(id: &str) -> String {
    let id = id.trim().trim_start_matches('<').trim_end_matches('>');
    format!("<{id}>")
}

/// The headers of a `text/rfc822-headers` part, or a `message/rfc822` part's message.
fn original_of<'a>(part: &'a MessagePart<'_>) -> Option<Email<'a>> {
    match &part.body {
        PartType::Message(m) => Some(m.clone()),
        // headers alone: the blank line that would end them is added
        PartType::Text(t) => parse(format!("{t}\r\n\r\n").as_bytes()).map(Email::into_owned),
        PartType::Binary(b) | PartType::InlineBinary(b) => {
            let mut b = b.to_vec();
            b.extend_from_slice(b"\r\n\r\n");
            parse(&b).map(Email::into_owned)
        }
        _ => None,
    }
}

/// The `Message-ID` of the message a delivery status report (RFC 3464) is about.
#[must_use]
pub fn reported_message_id(email: &Email<'_>) -> Option<String> {
    let root = email.root_part();
    if kind(root) != "multipart/report"
        || param(root, "report-type").map(str::to_lowercase).as_deref() != Some("delivery-status")
    {
        return None;
    }
    parts(email).into_iter().find_map(|part| {
        let k = kind(part);
        (k == "text/rfc822-headers" || k == "message/rfc822")
            .then(|| original_of(part))
            .flatten()
            .and_then(|o| o.message_id().map(bracketed))
    })
}

/// §2.1 at `RCPT TO`: the reply to a recipient, given the envelope sender (empty for null). An
/// address that is no account of the post office is not the door's to judge: the mail server
/// delivers it to a mailbox of its own (`postmaster@`, RFC 5321 §4.5.1) or refuses it.
#[must_use]
pub fn rcpt(sender: &str, recipient: &str, door: &Door<'_>) -> &'static str {
    rcpt_why(sender, recipient, door).0
}

/// `rcpt`, and why, in a few words for the operator's log.
#[must_use]
pub fn rcpt_why(sender: &str, recipient: &str, door: &Door<'_>) -> (&'static str, &'static str) {
    let Some(accept) = door.accounts.get(&recipient.to_lowercase()) else {
        return (OK_RCPT, "no account here: the mail server's");
    };
    let known = door
        .known_post_offices
        .iter()
        .any(|d| d.to_lowercase() == domain_of(sender));
    if sender.is_empty() {
        (OK_RCPT, "the null sender")
    } else if on_list(sender, accept) {
        (OK_RCPT, "the envelope sender is on the accept list")
    } else if known {
        (OK_RCPT, "a RadioPost post office's user")
    } else {
        (NOT_LISTED, "the envelope sender is not on the accept list")
    }
}

/// §2.1 at `DATA`, for one recipient taken at `RCPT TO`: the reply to the message.
#[must_use]
pub fn data(
    sender: &str,
    recipient: &str,
    raw: &[u8],
    verdicts: &Verdicts<'_>,
    door: &Door<'_>,
) -> &'static str {
    data_why(sender, recipient, raw, verdicts, door).0
}

/// `data`, and why, in a few words for the operator's log.
#[must_use]
pub fn data_why(
    sender: &str,
    recipient: &str,
    raw: &[u8],
    verdicts: &Verdicts<'_>,
    door: &Door<'_>,
) -> (&'static str, &'static str) {
    let Some(accept) = door.accounts.get(&recipient.to_lowercase()) else {
        return (OK_DATA, "no account here: the mail server's");
    };
    let Some(email) = parse(raw) else {
        return (NOT_LISTED, "not readable as an email");
    };
    if verdicts.dmarc.eq_ignore_ascii_case("fail") {
        return (NOT_LISTED, "DMARC fails for the From: domain");
    }
    if sender.is_empty() {
        if !reported_message_id(&email).is_some_and(|id| (door.sent)(&id)) {
            return (
                NOT_LISTED,
                "a null sender, but no report about mail sent from here",
            );
        }
    } else {
        let header_from = first_address(&email).unwrap_or_default();
        let domain = domain_of(&header_from);
        let signed = door
            .known_post_offices
            .iter()
            .any(|d| d.to_lowercase() == domain)
            && verdicts
                .dkim_pass
                .iter()
                .any(|d| d.to_lowercase() == domain);
        if !(on_list(&header_from, accept) || signed) {
            return (NOT_LISTED, "the From: address is not on the accept list");
        }
    }
    if raw.len() as u64 > door.max_email_bytes {
        return (TOO_LARGE, "larger than max_email_bytes");
    }
    if encrypted(&email) {
        return (NOT_ALLOWED, "encrypted");
    }
    if executable(&email) {
        return (NOT_ALLOWED, "an executable attachment");
    }
    (OK_DATA, "taken")
}

/// The door's two answers, `RCPT TO` and then `DATA` (`None` when the recipient was refused).
#[must_use]
pub fn door(
    sender: &str,
    recipient: &str,
    raw: &[u8],
    verdicts: &Verdicts<'_>,
    door: &Door<'_>,
) -> (&'static str, Option<&'static str>) {
    let r = rcpt(sender, recipient, door);
    if r != OK_RCPT {
        return (r, None);
    }
    (r, Some(data(sender, recipient, raw, verdicts, door)))
}

// -- §2.4: email to message ----------------------------------------------------------------------

/// RP-0 §6.2: the domain in lowercase, the local part as written.
#[must_use]
pub fn address(addr: &str) -> String {
    match addr.rsplit_once('@') {
        Some((local, domain)) => format!("{local}@{}", domain.to_lowercase()),
        None => addr.to_owned(),
    }
}

/// RP-0 §4.3: NFC, LF line endings.
#[must_use]
pub fn text(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .nfc()
        .collect()
}

/// §2.4 step 3: no trailing spaces, no trailing blank lines, and one LF at the end (none if
/// empty).
#[must_use]
pub fn tidy(body: &str) -> String {
    let lines: Vec<&str> = body
        .split('\n')
        .map(|l| l.trim_end_matches([' ', '\t']))
        .collect();
    let joined = lines.join("\n");
    let joined = joined.trim_end_matches('\n');
    if joined.is_empty() {
        String::new()
    } else {
        format!("{joined}\n")
    }
}

/// `On … wrote:`, the whole line.
fn intro(s: &str) -> bool {
    s.len() >= 10 && s.starts_with("On ") && s.ends_with(" wrote:")
}

/// Outlook's rule above its `From:` block.
fn outlook_rule(s: &str) -> bool {
    (s.len() >= 10 && s.chars().all(|c| c == '_')) || s == "-----Original Message-----"
}

/// §2.4 step 4: cut a reply's quoted history at the first line that matches; whether it did.
#[must_use]
pub fn trim(body: &str) -> (String, bool) {
    let lines: Vec<&str> = body.split('\n').collect();
    let mut cut = None;
    for (i, line) in lines.iter().enumerate() {
        let s = line.trim();
        let after = &lines[i + 1..];
        if s == "-----Original Message-----" {
            cut = Some(i);
        } else if line.starts_with("From:") && after.iter().take(3).any(|n| n.starts_with("Sent:"))
        {
            cut = Some(if i > 0 && outlook_rule(lines[i - 1].trim()) {
                i - 1
            } else {
                i
            });
        } else if intro(s)
            || (s.starts_with("On ")
                && !after.is_empty()
                && intro(&format!("{s} {}", after[0].trim())))
        {
            let rest = if intro(s) { after } else { &after[1..] };
            if rest
                .iter()
                .all(|n| n.trim().is_empty() || n.starts_with('>'))
            {
                cut = Some(i);
            }
        }
        if cut.is_some() {
            break;
        }
    }
    if cut.is_none() {
        let mut j = lines.len();
        while j > 0 && (lines[j - 1].trim().is_empty() || lines[j - 1].starts_with('>')) {
            j -= 1;
        }
        if lines[j..].iter().any(|n| n.starts_with('>')) {
            cut = Some(j);
        }
    }
    match cut {
        None => (body.to_owned(), false),
        Some(c) => (tidy(&lines[..c].join("\n")), true),
    }
}

/// Decodes HTML character references (`&amp;`, `&#233;`, `&#xE9;`).
fn entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[1..]
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '#')
            .map_or(rest.len(), |e| e + 1);
        let name = &rest[1..end];
        let decoded = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16)
                .ok()
                .and_then(char::from_u32),
            n if n.starts_with('#') => n[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        if let Some(c) = decoded {
            out.push(c);
            rest = rest[end..].strip_prefix(';').unwrap_or(&rest[end..]);
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

/// The value of one attribute in a start tag's text.
fn attribute(tag: &str, name: &str) -> Option<String> {
    // ASCII case only, so byte positions in `lower` are positions in `tag`
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        let before = lower[..at].chars().last();
        let rest = lower[at + name.len()..].trim_start();
        if before.is_some_and(char::is_whitespace) && rest.starts_with('=') {
            let start = tag.len() - rest.len() + 1;
            let value = tag[start..].trim_start();
            let v = match value.chars().next() {
                Some(q @ ('"' | '\'')) => value[1..].split(q).next().unwrap_or(""),
                _ => value
                    .split(|c: char| c.is_whitespace() || c == '>')
                    .next()
                    .unwrap_or(""),
            };
            return Some(entities(v));
        }
        from = at + name.len();
    }
    None
}

/// §2.4 step 3: one acceptable rendering of HTML as text — paragraphs and line breaks as new
/// lines, entities decoded, and each link followed by `[n]`, listed at the end.
#[must_use]
pub fn html_to_text(markup: &str) -> String {
    const BLOCK: [&str; 14] = [
        "p", "div", "br", "li", "tr", "table", "ul", "ol", "h1", "h2", "h3", "h4", "h5", "h6",
    ];
    let mut out = String::new();
    let mut links: Vec<String> = Vec::new();
    let mut href: Option<String> = None;
    let mut skip = 0usize;
    let mut rest = markup;
    let data = |text: &str, out: &mut String, skip: usize| {
        if skip == 0 {
            let decoded = entities(text);
            let mut last_space = false;
            for c in decoded.chars() {
                if c.is_whitespace() && c != '\u{a0}' {
                    if !last_space {
                        out.push(' ');
                    }
                    last_space = true;
                } else {
                    out.push(c);
                    last_space = false;
                }
            }
        }
    };
    while let Some(i) = rest.find('<') {
        data(&rest[..i], &mut out, skip);
        rest = &rest[i..];
        if let Some(comment) = rest.strip_prefix("<!--") {
            rest = comment.find("-->").map_or("", |e| &comment[e + 3..]);
            continue;
        }
        let Some(end) = rest.find('>') else {
            data(rest, &mut out, skip);
            rest = "";
            break;
        };
        let tag = &rest[1..end];
        rest = &rest[end + 1..];
        if tag.starts_with('!') || tag.starts_with('?') {
            continue;
        }
        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_lowercase();
        if closing {
            if ["script", "style", "head"].contains(&name.as_str()) {
                skip = skip.saturating_sub(1);
            }
            if BLOCK.contains(&name.as_str()) && name != "br" || name.starts_with('h') {
                out.push('\n');
            }
            if name == "a"
                && let Some(h) = href.take()
            {
                links.push(h);
                let _ = write!(out, " [{}]", links.len());
            }
        } else {
            if ["script", "style", "head"].contains(&name.as_str()) {
                skip += 1;
            }
            if BLOCK.contains(&name.as_str()) {
                out.push('\n');
            }
            if name == "a" {
                href = attribute(&format!(" {tag}"), "href");
            }
        }
    }
    data(rest, &mut out, skip);
    let trimmed: Vec<&str> = out.split('\n').map(str::trim).collect();
    let mut joined = trimmed.join("\n");
    while joined.contains("\n\n\n") {
        joined = joined.replace("\n\n\n", "\n\n");
    }
    let mut joined = joined.trim_matches('\n').to_owned();
    if !links.is_empty() {
        joined.push_str("\n\n");
        let listed: Vec<String> = links
            .iter()
            .enumerate()
            .map(|(n, u)| format!("[{}] {u}", n + 1))
            .collect();
        joined.push_str(&listed.join("\n"));
    }
    joined.push('\n');
    joined
}

/// The first address of the `From:` header.
fn first_address(email: &Email<'_>) -> Option<String> {
    email
        .from()?
        .iter()
        .find_map(|a| a.address.as_ref().map(ToString::to_string))
}

fn addresses(list: Option<&mail_parser::Address<'_>>) -> Vec<String> {
    list.map(|l| {
        l.iter()
            .filter_map(|a| a.address.as_deref().map(address))
            .collect()
    })
    .unwrap_or_default()
}

/// A file carried or held: name, type, bytes.
struct Attached {
    name: String,
    kind: String,
    data: Vec<u8>,
}

fn attachments(email: &Email<'_>) -> Vec<Attached> {
    email
        .attachments()
        .map(|part| Attached {
            name: text(part.attachment_name().unwrap_or("attachment")),
            kind: kind(part),
            data: part.contents().to_vec(),
        })
        .collect()
}

/// What the post office knows when an email arrives (§2.4).
pub struct Arrival<'a> {
    /// When it arrived, for an email whose `Date:` cannot be read.
    pub arrival: u64,
    /// The recipient's `attach_bytes` (RP-5 §6.3).
    pub attach_bytes: u64,
    /// The post office's `max_air_bytes` (RP-5 §4.1).
    pub max_air_bytes: u64,
    /// The content ID of a message this post office sent, by its `Message-ID` (§3.1, step 7).
    pub sent: &'a dyn Fn(&str) -> Option<[u8; 12]>,
    /// The refs to give held attachments, in order (§2.4 step 5).
    pub held_refs: Vec<[u8; 8]>,
}

/// An email made into a message (§2.4).
pub struct Converted {
    /// The message.
    pub message: Message,
    /// Its text came from HTML, so another rendering is as good.
    pub from_html: bool,
    /// The attachments held at the post office: (ref, name, type, bytes).
    pub held: Vec<([u8; 8], String, String, Vec<u8>)>,
}

/// §2.4: the RadioPost message an email becomes.
///
/// # Errors
///
/// It is not an email with a `From:` address.
#[allow(clippy::too_many_lines, reason = "§2.4's steps, in order")]
pub fn to_message(raw: &[u8], ctx: &Arrival<'_>) -> Result<Converted, String> {
    let email = parse(raw).ok_or("not an email")?;
    let from = first_address(&email).ok_or("no From: address")?;
    let mut entries = vec![
        (key::V, Value::Uint(1)),
        (key::FROM, Value::text(&address(&from))),
        (
            key::TO,
            Value::Array(
                addresses(email.to())
                    .iter()
                    .map(|a| Value::text(a))
                    .collect(),
            ),
        ),
    ];
    let cc = addresses(email.cc());
    if !cc.is_empty() {
        entries.push((
            key::CC,
            Value::Array(cc.iter().map(|a| Value::text(a)).collect()),
        ));
    }
    let date = email
        .date()
        .filter(|d| d.is_valid())
        .and_then(|d| u64::try_from(d.to_timestamp()).ok())
        .unwrap_or(ctx.arrival);
    entries.push((key::DATE, Value::Uint(date)));
    entries.push((
        key::SUBJECT,
        Value::text(&text(email.subject().unwrap_or(""))),
    ));
    let plain = email
        .text_bodies()
        .find(|p| matches!(p.body, PartType::Text(_)));
    let from_html = plain.is_none();
    let body = match plain {
        Some(p) => text(&text_of(p).unwrap_or_default()),
        None => email
            .html_bodies()
            .find(|p| matches!(p.body, PartType::Html(_)))
            .and_then(text_of)
            .map(|h| text(&html_to_text(&h)))
            .unwrap_or_default(),
    };
    let (body, trimmed) = trim(&tidy(&body));
    entries.push((key::BODY, Value::text(&body)));
    let found = attachments(&email);
    let limit = ctx.attach_bytes.min(ctx.max_air_bytes);
    let files: Vec<Value> = found
        .iter()
        .map(|f| {
            Value::int_map(vec![
                (1, Value::text(&f.name)),
                (2, Value::text(&f.kind)),
                (3, Value::bytes(&f.data)),
            ])
        })
        .collect();
    let mut with_files = entries.clone();
    with_files.push((key::FILES, Value::Array(files.clone())));
    let air_size = Value::int_map(with_files)
        .encode()
        .map_or(usize::MAX, |c| c.len() + 1);
    let carried = !found.is_empty() && air_size as u64 <= limit;
    let mut held = Vec::new();
    if carried {
        entries.push((key::FILES, Value::Array(files)));
    } else {
        for (f, r) in found.iter().zip(&ctx.held_refs) {
            held.push((*r, f.name.clone(), f.kind.clone(), f.data.clone()));
        }
    }
    let replied = email.in_reply_to().as_text().map(bracketed);
    if let Some(original) = replied.and_then(|r| (ctx.sent)(&r)) {
        entries.push((key::REPLY_TO, Value::bytes(&original)));
    }
    let message_flags = u64::from(trimmed) | if held.is_empty() { 0 } else { 2 };
    if message_flags != 0 {
        entries.push((key::FLAGS, Value::Uint(message_flags)));
    }
    if !held.is_empty() {
        entries.push((
            key::HELD,
            Value::Array(
                held.iter()
                    .map(|(r, name, k, data)| {
                        Value::int_map(vec![
                            (1, Value::text(name)),
                            (2, Value::text(k)),
                            (3, Value::Uint(data.len() as u64)),
                            (4, Value::bytes(r)),
                        ])
                    })
                    .collect(),
            ),
        ));
    }
    let message = Message::from_map(&Value::int_map(entries))
        .map_err(|e| format!("not a message: {}", e.0))?;
    Ok(Converted {
        message,
        from_html,
        held,
    })
}

// -- §3: message to email ------------------------------------------------------------------------

/// The footer every email from RadioPost ends with (§3.1).
pub const FOOTER: &str = "-- \n\
Sent by amateur radio through RadioPost. Please reply in plain text, under 20 kB.\n\
Amateur radio carries no business content.\n";

/// Days since 1970-01-01 as (year, month, day): H. Hinnant's civil-from-days.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn split_time(seconds: u64) -> (i64, u32, u32, u64, u64, u64, usize) {
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let (y, m, d) = civil(days);
    let rem = seconds % 86_400;
    let weekday = usize::try_from((days + 4).rem_euclid(7)).unwrap_or(0); // 1970-01-01 a Thursday
    (y, m, d, rem / 3600, rem % 3600 / 60, rem % 60, weekday)
}

/// §3.1: RFC 5322 date-time in UTC, written `+0000`.
#[must_use]
pub fn rfc5322_date(seconds: u64) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (y, m, d, hh, mm, ss, wd) = split_time(seconds);
    format!(
        "{}, {d:02} {} {y:04} {hh:02}:{mm:02}:{ss:02} +0000",
        DAYS[wd],
        MONTHS[(m as usize).saturating_sub(1) % 12]
    )
}

/// ISO 8601 in UTC (`2026-11-09T14:40:41Z`).
#[must_use]
pub fn iso(seconds: u64) -> String {
    let (y, m, d, hh, mm, ss, _) = split_time(seconds);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i)) as usize & 63]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Base64 in lines of 76, each ending in CRLF.
#[must_use]
pub fn base64_lines(data: &[u8]) -> String {
    let b64 = base64(data);
    let mut out = String::with_capacity(b64.len() + b64.len() / 38 + 2);
    for line in b64.as_bytes().chunks(76) {
        out.push_str(std::str::from_utf8(line).unwrap_or(""));
        out.push_str("\r\n");
    }
    out
}

/// RFC 2047, when the value is not ASCII: one valid spelling of it, in encoded words of at most
/// 45 bytes of UTF-8 each.
#[must_use]
pub fn encoded_header(value: &str) -> String {
    if value.is_ascii() {
        return value.to_owned();
    }
    let mut words = Vec::new();
    let mut chunk = String::new();
    for c in value.chars() {
        if chunk.len() + c.len_utf8() > 45 {
            words.push(format!("=?utf-8?b?{}?=", base64(chunk.as_bytes())));
            chunk.clear();
        }
        chunk.push(c);
    }
    if !chunk.is_empty() {
        words.push(format!("=?utf-8?b?{}?=", base64(chunk.as_bytes())));
    }
    words.join("\r\n ")
}

fn display(name: &str, addr: &str) -> String {
    if name.is_empty() {
        return addr.to_owned();
    }
    let atext = |c: char| c.is_ascii_alphanumeric() || " !#$%&'*+/=?^_`{|}~-".contains(c);
    if name.is_ascii() && name.chars().all(atext) {
        return format!("{name} <{addr}>");
    }
    if name.is_ascii() {
        return format!(
            "\"{}\" <{addr}>",
            name.replace('\\', "\\\\").replace('"', "\\\"")
        );
    }
    format!("{} <{addr}>", encoded_header(name))
}

/// RFC 2045 §6.7: quoted-printable, CRLF line breaks, lines of at most 76 characters.
fn quoted_printable(text: &str) -> String {
    let mut out = String::new();
    let lines: Vec<&str> = text.split('\n').collect();
    for (n, line) in lines.iter().enumerate() {
        let bytes = line.as_bytes();
        let mut current = String::new();
        for (i, &b) in bytes.iter().enumerate() {
            let last = i + 1 == bytes.len();
            let plain = (b == b' ' || b == b'\t') && !last || (33..=126).contains(&b) && b != b'=';
            let piece = if plain {
                char::from(b).to_string()
            } else {
                format!("={b:02X}")
            };
            if current.len() + piece.len() > 75 {
                out.push_str(&current);
                out.push_str("=\r\n");
                current.clear();
            }
            current.push_str(&piece);
        }
        out.push_str(&current);
        if n + 1 < lines.len() {
            out.push_str("\r\n");
        }
    }
    if !out.ends_with("\r\n") {
        out.push_str("\r\n");
    }
    out
}

/// §3.1: the body and footer as a text part: its transfer encoding and its CRLF text.
#[must_use]
pub fn body_part(body: &str) -> (&'static str, String) {
    let mut full = body.to_owned();
    if !full.is_empty() && !full.ends_with('\n') {
        full.push('\n');
    }
    full.push_str(FOOTER);
    let long_line = full.split('\n').any(|l| l.len() > 998);
    if full.is_ascii() && !long_line {
        return ("7bit", full.replace('\n', "\r\n"));
    }
    let body = full.strip_suffix('\n').unwrap_or(&full);
    ("quoted-printable", quoted_printable(body))
}

/// Where a message handed in by radio came from, for `Radio-Path` (§3.2) and the part (§4).
#[derive(Clone, Debug, Default)]
pub struct Carried {
    /// The operator's callsign from the session statement.
    pub operator: String,
    /// The gateways, in order.
    pub gateways: Vec<String>,
    /// The session's start (`started`).
    pub started: u64,
    /// The gateway's session ID, in hex.
    pub session: String,
    /// The sender's credential, the statement and its signature: the proof (RP-1 §6.4).
    pub proof: Option<(Vec<u8>, Vec<u8>, Vec<u8>)>,
}

/// §3.2.
#[must_use]
pub fn radio_path(path: &Carried) -> String {
    let mut via = vec![path.operator.clone()];
    via.extend(path.gateways.iter().cloned());
    format!(
        "{}; at={}; session={}",
        via.join(" via "),
        iso(path.started),
        path.session
    )
}

/// §4: the `application/radiopost-message` part's CBOR map.
#[must_use]
pub fn part_bytes(message: &Message, path: &Carried) -> Option<Vec<u8>> {
    let (credential, statement, signature) = path.proof.as_ref()?;
    Value::int_map(vec![
        (
            1,
            Value::bytes(&message.air_form(radiopost::message::Compression::None)),
        ),
        (2, Value::bytes(credential)),
        (3, Value::bytes(statement)),
        (4, Value::bytes(signature)),
        (5, Value::Uint(path.started)),
    ])
    .encode()
    .ok()
}

/// What the post office knows when a message leaves as an email (§3.1).
pub struct Leaving<'a> {
    /// The sender's display name.
    pub display_name: &'a str,
    /// How it came by radio.
    pub path: &'a Carried,
    /// The `Message-ID` of the email a message this post office made answers, by content ID.
    pub message_id: &'a dyn Fn(&[u8]) -> Option<String>,
    /// Add §4's part: every recipient of this SMTP transaction is at a RadioPost post office.
    pub with_part: bool,
}

/// §3.1: the email a message handed in by radio becomes, before DKIM signing: its headers by name
/// and the whole email in CRLF.
#[must_use]
#[allow(clippy::too_many_lines, reason = "§3.1's table, header by header")]
pub fn from_message(message: &Message, ctx: &Leaving<'_>) -> (Vec<(String, String)>, Vec<u8>) {
    let map = message.map();
    let field = |k| map.get(k);
    let list = |k| -> Vec<String> {
        field(k)
            .and_then(Value::as_array)
            .unwrap_or(&[])
            .iter()
            .filter_map(Value::as_text)
            .map(str::to_owned)
            .collect()
    };
    let from = message.from().unwrap_or("").to_owned();
    let cid = message.content_id().text();
    let mut headers: Vec<(String, String)> = vec![
        ("From".into(), display(ctx.display_name, &from)),
        ("To".into(), list(key::TO).join(", ")),
    ];
    let cc = list(key::CC);
    if !cc.is_empty() {
        headers.push(("Cc".into(), cc.join(", ")));
    }
    headers.push((
        "Date".into(),
        rfc5322_date(field(key::DATE).and_then(Value::as_u64).unwrap_or(0)),
    ));
    headers.push((
        "Subject".into(),
        encoded_header(field(key::SUBJECT).and_then(Value::as_text).unwrap_or("")),
    ));
    headers.push(("Message-ID".into(), format!("<{cid}@{}>", domain_of(&from))));
    if let Some(replied) = field(key::REPLY_TO)
        .and_then(Value::as_bytes)
        .and_then(|r| (ctx.message_id)(r))
    {
        headers.push(("In-Reply-To".into(), replied.clone()));
        headers.push(("References".into(), replied));
    }
    if !ctx.path.operator.is_empty() {
        headers.push(("Radio-Path".into(), radio_path(ctx.path)));
    }
    if field(key::PRIORITY).and_then(Value::as_u64).unwrap_or(0) >= 1 {
        headers.push(("Importance".into(), "high".into()));
    }
    headers.push(("MIME-Version".into(), "1.0".into()));
    let (encoding, body) = body_part(field(key::BODY).and_then(Value::as_text).unwrap_or(""));
    let files: Vec<(String, String, Vec<u8>)> = field(key::FILES)
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .map(|f| {
            (
                f.get(1)
                    .and_then(Value::as_text)
                    .unwrap_or("file")
                    .to_owned(),
                f.get(2)
                    .and_then(Value::as_text)
                    .unwrap_or("application/octet-stream")
                    .to_owned(),
                f.get(3).and_then(Value::as_bytes).unwrap_or(&[]).to_vec(),
            )
        })
        .collect();
    let part = ctx
        .with_part
        .then(|| part_bytes(message, ctx.path))
        .flatten();
    let head = |headers: &[(String, String)]| {
        headers.iter().fold(String::new(), |mut s, (k, v)| {
            let _ = write!(s, "{k}: {v}\r\n");
            s
        })
    };
    if part.is_none() && files.is_empty() {
        headers.push(("Content-Type".into(), "text/plain; charset=utf-8".into()));
        headers.push(("Content-Transfer-Encoding".into(), encoding.into()));
        let email = format!("{}\r\n{body}", head(&headers));
        return (headers, email.into_bytes());
    }
    let boundary = format!("radiopost-{cid}");
    headers.push((
        "Content-Type".into(),
        format!("multipart/mixed; boundary=\"{boundary}\""),
    ));
    let mut parts = vec![format!(
        "Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: {encoding}\r\n\r\n{body}"
    )];
    for (name, kind, data) in &files {
        let name = encoded_header(name).replace('"', "");
        parts.push(format!(
            "Content-Type: {kind}\r\nContent-Disposition: attachment; filename=\"{name}\"\r\n\
             Content-Transfer-Encoding: base64\r\n\r\n{}",
            base64_lines(data)
        ));
    }
    if let Some(p) = &part {
        parts.push(format!(
            "Content-Type: application/radiopost-message\r\nContent-Transfer-Encoding: base64\r\n\r\n{}",
            base64_lines(p)
        ));
    }
    // RFC 2046 §5.1.1: the CRLF before a boundary belongs to the boundary
    let middle: String = parts.iter().fold(String::new(), |mut s, p| {
        let _ = write!(s, "--{boundary}\r\n{p}\r\n");
        s
    });
    let email = format!("{}\r\n{middle}--{boundary}--\r\n", head(&headers));
    (headers, email.into_bytes())
}

// -- §4: the message part, at the receiving post office ------------------------------------------

/// The `application/radiopost-message` part of an email, decoded.
#[must_use]
pub fn find_part(raw: &[u8]) -> Option<Vec<u8>> {
    let email = parse(raw)?;
    parts(&email)
        .into_iter()
        .find(|p| kind(p) == "application/radiopost-message")
        .map(|p| p.contents().to_vec())
}

/// §4: the part's message, if its proof holds and it is for `recipient`; otherwise the code of
/// what failed.
///
/// # Errors
///
/// The RP-0 §9.3 code of the first check that failed.
pub fn check_part(
    part: &[u8],
    recipient: &str,
    trust: &Trust,
) -> Result<Message, radiopost::ErrorCode> {
    use radiopost::ErrorCode;
    let m = cbor::decode(part).map_err(|_| ErrorCode::Malformed)?;
    let bytes = |k| m.get(k).and_then(Value::as_bytes);
    let (Some(air), Some(cred), Some(raw_statement), Some(signature), Some(started)) = (
        bytes(1),
        bytes(2),
        bytes(3),
        bytes(4),
        m.get(5).and_then(Value::as_u64),
    ) else {
        return Err(ErrorCode::Malformed);
    };
    let message = Message::from_air(air).map_err(|_| ErrorCode::Malformed)?;
    let stmt = Statement::decode(raw_statement).ok_or(ErrorCode::Malformed)?;
    let body = credential::verify(cred, trust, started, flags::SEND).map_err(|r| r.code)?;
    if stmt.address != body.address || stmt.operator != body.operator {
        return Err(ErrorCode::NotAuthorized);
    }
    if !crypto::verify(context::SESSION, raw_statement, signature, &body.device) {
        return Err(ErrorCode::SignatureInvalid);
    }
    if let Form::Time { time, .. } = stmt.form
        && time.abs_diff(started) > TIME_WINDOW_S
    {
        return Err(ErrorCode::TimeSkew);
    }
    if !stmt.messages.contains(&message.content_id()) {
        return Err(ErrorCode::NotAuthorized);
    }
    if message.from() != Some(stmt.address.as_str()) {
        return Err(ErrorCode::NotAuthorized);
    }
    let recipient = recipient.to_lowercase();
    if !message
        .recipients()
        .iter()
        .any(|r| r.to_lowercase() == recipient)
    {
        return Err(ErrorCode::NotAuthorized);
    }
    Ok(message)
}

// -- §3.3: bounces -------------------------------------------------------------------------------

/// The header blocks of a `message/delivery-status` part: per message, then per recipient.
fn status_blocks(textual: &str) -> Vec<BTreeMap<String, String>> {
    let mut blocks = Vec::new();
    let mut block: BTreeMap<String, String> = BTreeMap::new();
    let mut last: Option<String> = None;
    for line in textual.replace("\r\n", "\n").split('\n') {
        if line.trim().is_empty() {
            if !block.is_empty() {
                blocks.push(std::mem::take(&mut block));
            }
            last = None;
        } else if line.starts_with([' ', '\t']) {
            if let Some(name) = &last
                && let Some(v) = block.get_mut(name)
            {
                v.push(' ');
                v.push_str(line.trim());
            }
        } else if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_lowercase();
            block.insert(name.clone(), value.trim().to_owned());
            last = Some(name);
        }
    }
    if !block.is_empty() {
        blocks.push(block);
    }
    blocks
}

/// What a bounce becomes (§3.3).
pub struct Bounce<'a> {
    /// The post office's domain.
    pub domain: &'a str,
    /// The sender whose message failed, the report's recipient.
    pub sender: &'a str,
    /// When the report arrived.
    pub arrival: u64,
    /// The content ID of a message this post office sent, by its `Message-ID`.
    pub sent: &'a dyn Fn(&str) -> Option<[u8; 12]>,
}

/// §3.3: the short message a delivery failure report becomes in the sender's mailbox.
///
/// # Errors
///
/// It is not an email.
pub fn bounce(raw: &[u8], ctx: &Bounce<'_>) -> Result<Message, String> {
    let report = parse(raw).ok_or("not an email")?;
    let mut lines = Vec::new();
    let mut subject = String::new();
    let mut original_id = None;
    for part in parts(&report) {
        match kind(part).as_str() {
            "message/delivery-status" => {
                let textual = match &part.body {
                    PartType::Message(m) => String::from_utf8_lossy(m.raw_message()).into_owned(),
                    _ => String::from_utf8_lossy(part.contents()).into_owned(),
                };
                for block in status_blocks(&textual).iter().skip(1) {
                    if block.get("action").map(|a| a.to_lowercase()).as_deref() != Some("failed") {
                        continue;
                    }
                    let after = |k: &str| {
                        block
                            .get(k)
                            .and_then(|v| v.split_once(';'))
                            .map(|(_, r)| r.trim().to_owned())
                            .unwrap_or_default()
                    };
                    let failed = after("final-recipient");
                    let diagnostic = after("diagnostic-code");
                    let reason = if diagnostic.is_empty() {
                        block.get("status").cloned().unwrap_or_default()
                    } else {
                        diagnostic
                    };
                    lines.push(format!("{failed}: {reason}"));
                }
            }
            "text/rfc822-headers" | "message/rfc822" => {
                if let Some(original) = original_of(part) {
                    original.subject().unwrap_or("").clone_into(&mut subject);
                    original_id = original.message_id().map(bracketed);
                }
            }
            _ => {}
        }
    }
    not_delivered(
        ctx,
        &subject,
        &lines,
        original_id.and_then(|id| (ctx.sent)(&id)),
    )
}

/// §3.3's short message: from `postmaster@`, to the sender, one line for each recipient that
/// failed; also what the post office says when its own mail server refuses a message for good.
///
/// # Errors
///
/// The text does not make a message.
pub fn not_delivered(
    ctx: &Bounce<'_>,
    subject: &str,
    lines: &[String],
    reply_to: Option<[u8; 12]>,
) -> Result<Message, String> {
    let mut entries = vec![
        (key::V, Value::Uint(1)),
        (
            key::FROM,
            Value::text(&format!("postmaster@{}", ctx.domain)),
        ),
        (key::TO, Value::Array(vec![Value::text(ctx.sender)])),
        (key::DATE, Value::Uint(ctx.arrival)),
        (
            key::SUBJECT,
            Value::text(&text(&format!("Not delivered: {subject}"))),
        ),
        (key::BODY, Value::text(&(lines.join("\n") + "\n"))),
    ];
    if let Some(original) = reply_to {
        entries.push((key::REPLY_TO, Value::bytes(&original)));
    }
    Message::from_map(&Value::int_map(entries)).map_err(|e| format!("not a message: {}", e.0))
}

/// The content ID's 12 bytes, as a mapping keeps them.
#[must_use]
pub fn id_bytes(id: &ContentId) -> [u8; 12] {
    id.0
}

#[cfg(test)]
mod tests {
    use super::{body_part, parse, text};

    /// Quoted-printable that a reader decodes back to the text: `=` itself, a tab and spaces at
    /// a line's end, a line over 76 characters, and text that is not ASCII.
    #[test]
    fn quoted_printable_decodes_to_the_text() {
        let body = format!("x=41 é\ttrailing \t\n{}\nend\n", "ñ".repeat(120));
        let (encoding, part) = body_part(&body);
        assert_eq!(encoding, "quoted-printable");
        assert!(part.split("\r\n").all(|l| l.len() <= 76), "{part}");
        let email = format!(
            "From: a@b.example\r\nContent-Type: text/plain; charset=utf-8\r\n\
             Content-Transfer-Encoding: quoted-printable\r\n\r\n{part}"
        );
        let parsed = parse(email.as_bytes()).expect("an email");
        assert_eq!(
            text(&parsed.body_text(0).expect("text")),
            format!("{body}{}", super::FOOTER)
        );
    }
}
