// SPDX-License-Identifier: MIT OR Apache-2.0
//! A stand-in mail server for the post office tests: an SMTP submission port that takes every
//! message (refusing only `refused@example.net`) and keeps it, and a reader for what it took —
//! headers unfolded and RFC 2047 decoded, the text, and the MIME parts — written here from RFC
//! 5322, 2045 to 2047 and 2046, so that nothing of the post office's own email code judges it.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, PoisonError};

/// One message taken: the envelope sender, the recipients, and the email.
#[derive(Clone, Debug)]
pub struct Taken {
    /// `MAIL FROM`.
    pub from: String,
    /// `RCPT TO`, each taken.
    pub to: Vec<String>,
    /// The email, dot-stuffing undone.
    pub email: String,
}

/// The stand-in's port and what it took.
#[derive(Clone)]
pub struct MailServer {
    /// `host:port`.
    pub at: String,
    took: Arc<Mutex<Vec<Taken>>>,
}

impl MailServer {
    /// Starts it on a free port of the loopback interface.
    ///
    /// # Errors
    ///
    /// No port.
    pub fn start() -> Result<MailServer, String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let at = listener
            .local_addr()
            .map_err(|e| e.to_string())?
            .to_string();
        let took: Arc<Mutex<Vec<Taken>>> = Arc::default();
        let keep = took.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let keep = keep.clone();
                std::thread::spawn(move || {
                    let Ok(mut out) = stream.try_clone() else {
                        return;
                    };
                    let mut lines = BufReader::new(stream);
                    let mut say = |l: &str| {
                        let _ = out.write_all(format!("{l}\r\n").as_bytes());
                    };
                    say("220 radiopost-tester's stand-in mail server");
                    let (mut from, mut to) = (String::new(), Vec::new());
                    let mut line = String::new();
                    while lines.read_line(&mut line).is_ok_and(|n| n > 0) {
                        let l = line.trim_end().to_owned();
                        line.clear();
                        let upper = l.to_uppercase();
                        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                            say("250-stand-in");
                            say("250 8BITMIME");
                        } else if upper.starts_with("MAIL FROM:") {
                            l[10..]
                                .split_whitespace()
                                .next()
                                .unwrap_or("")
                                .trim_matches(['<', '>'])
                                .clone_into(&mut from);
                            to.clear();
                            say("250 2.1.0 OK");
                        } else if upper.starts_with("RCPT TO:") {
                            let r = l[8..]
                                .split_whitespace()
                                .next()
                                .unwrap_or("")
                                .trim_matches(['<', '>'])
                                .to_owned();
                            if r == "refused@example.net" {
                                say("550 5.1.1 No such user");
                            } else {
                                to.push(r);
                                say("250 2.1.5 OK");
                            }
                        } else if upper == "DATA" {
                            say("354 go on");
                            let mut email = String::new();
                            while lines.read_line(&mut line).is_ok_and(|n| n > 0) {
                                if line == ".\r\n" {
                                    line.clear();
                                    break;
                                }
                                email.push_str(line.strip_prefix('.').unwrap_or(&line));
                                line.clear();
                            }
                            keep.lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .push(Taken {
                                    from: from.clone(),
                                    to: to.clone(),
                                    email,
                                });
                            say("250 2.0.0 queued");
                        } else if upper == "QUIT" {
                            say("221 bye");
                            break;
                        } else if upper == "RSET" || upper == "NOOP" {
                            say("250 OK");
                        } else {
                            say("502 5.5.2 not here");
                        }
                    }
                });
            }
        });
        Ok(MailServer { at, took })
    }

    /// Everything taken so far.
    #[must_use]
    pub fn took(&self) -> Vec<Taken> {
        self.took
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Standard base64 (RFC 4648 §4), line breaks and padding allowed.
#[must_use]
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let url: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '=')
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            c => c,
        })
        .collect();
    radiopost::base64url::decode(&url)
}

/// Standard base64, padded.
#[must_use]
pub fn base64_encode(data: &[u8]) -> String {
    let mut s = radiopost::base64url::encode(data)
        .replace('-', "+")
        .replace('_', "/");
    while s.len() % 4 != 0 {
        s.push('=');
    }
    s
}

/// RFC 2047 encoded words in a header value, decoded (`b` and `q`, UTF-8 or ASCII).
#[must_use]
pub fn decode_words(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    let mut last_was_word = false;
    while let Some(i) = rest.find("=?") {
        let between = &rest[..i];
        if !(last_was_word && between.trim().is_empty()) {
            out.push_str(between);
        }
        let after = &rest[i + 2..];
        let parts: Vec<&str> = after.splitn(3, '?').collect();
        let Some(end) = parts.get(2).and_then(|p| p.find("?=")) else {
            out.push_str(&rest[i..]);
            return out;
        };
        let (enc, text) = (parts[1], &parts[2][..end]);
        let bytes = if enc.eq_ignore_ascii_case("b") {
            base64_decode(text).unwrap_or_default()
        } else {
            quoted_printable(&text.replace('_', " "))
        };
        out.push_str(&String::from_utf8_lossy(&bytes));
        let used = 2 + parts[0].len() + 1 + parts[1].len() + 1 + end + 2;
        rest = &rest[i + used..];
        last_was_word = true;
    }
    out.push_str(rest);
    out
}

/// Quoted-printable (RFC 2045 §6.7), soft breaks joined.
#[must_use]
pub fn quoted_printable(text: &str) -> Vec<u8> {
    let text = text.replace("=\r\n", "").replace("=\n", "");
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'='
            && i + 3 <= b.len()
            && let Some(v) = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// An email's (or a part's) headers, unfolded, and its body.
#[must_use]
pub fn split(email: &str) -> (Vec<(String, String)>, String) {
    let email = email.replace("\r\n", "\n");
    let (head, body) = email.split_once("\n\n").unwrap_or((&email, ""));
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in head.split('\n') {
        if line.starts_with([' ', '\t']) {
            if let Some((_, v)) = headers.last_mut() {
                v.push(' ');
                v.push_str(line.trim());
            }
        } else if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }
    (headers, body.to_owned())
}

/// A header's value, decoded, if there is one.
#[must_use]
pub fn header(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| decode_words(v))
}

/// A part's decoded body by its `Content-Transfer-Encoding`.
#[must_use]
pub fn decoded_body(headers: &[(String, String)], body: &str) -> Vec<u8> {
    match header(headers, "Content-Transfer-Encoding")
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "base64" => base64_decode(body).unwrap_or_default(),
        "quoted-printable" => quoted_printable(body),
        _ => body.as_bytes().to_vec(),
    }
}

/// The parts of a `multipart/*` email, each as headers and body; the whole email as one part
/// when it is not multipart.
#[must_use]
pub fn parts(email: &str) -> Vec<(Vec<(String, String)>, String)> {
    let (headers, body) = split(email);
    let kind = header(&headers, "Content-Type").unwrap_or_default();
    let Some(boundary) = kind
        .split(';')
        .find_map(|p| p.trim().strip_prefix("boundary="))
        .map(|b| b.trim_matches('"').to_owned())
    else {
        return vec![(headers, body)];
    };
    body.split(&format!("--{boundary}"))
        .skip(1)
        .filter(|p| !p.starts_with("--"))
        .map(|p| split(p.trim_start_matches('\n')))
        .collect()
}
