// SPDX-License-Identifier: MIT OR Apache-2.0
//! QRZ.com's callbook, through its published XML interface, for the operators' evidence (D35).
//!
//! The operator gives the post office their own QRZ account. A session key comes from logging in,
//! and each lookup sends only the callsign. Without an XML subscription QRZ answers with a few
//! fields (name, city, state, country); with one, the rest. What QRZ says is what its user wrote
//! there: evidence for the operator, never a decision.

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;

/// QRZ's XML interface.
pub const QRZ_URL: &str = "https://xmldata.qrz.com/xml/current/";

/// What QRZ says of a callsign.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Entry {
    /// The callsign.
    pub call: String,
    /// The first name, as QRZ's user gave it.
    pub first: String,
    /// The last name.
    pub last: String,
    /// The city.
    pub city: String,
    /// The state, for US entries.
    pub state: String,
    /// The country.
    pub country: String,
    /// The licence class, when QRZ has it.
    pub class: String,
    /// The email address, when its user shows it.
    pub email: String,
    /// Whether its user uploads to the ARRL's Logbook of the World: `1` yes, `0` no.
    pub lotw: String,
}

/// The text of the first `<tag>…</tag>` in `xml`, unescaped.
fn tag(xml: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    xml.find(&open)
        .and_then(|i| {
            let rest = &xml[i + open.len()..];
            rest.find(&close).map(|j| &rest[..j])
        })
        .map(|v| {
            v.replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&apos;", "'")
                .replace("&amp;", "&")
                .trim()
                .to_owned()
        })
        .unwrap_or_default()
}

/// One answer read: the entry, or the error QRZ gave in its `<Session>`.
///
/// # Errors
///
/// QRZ's own error text: `Not found: …`, `Session Timeout`, `Username/password incorrect`.
pub fn read(xml: &str) -> Result<Option<Entry>, String> {
    let session = tag(xml, "Session");
    let error = tag(&session, "Error");
    if error.starts_with("Not found") {
        return Ok(None);
    }
    if !error.is_empty() {
        return Err(error);
    }
    let c = tag(xml, "Callsign");
    if c.is_empty() {
        return Ok(None);
    }
    Ok(Some(Entry {
        call: tag(&c, "call").to_uppercase(),
        first: tag(&c, "fname"),
        last: tag(&c, "name"),
        city: tag(&c, "addr2"),
        state: tag(&c, "state"),
        country: tag(&c, "country"),
        class: tag(&c, "class"),
        email: tag(&c, "email"),
        lotw: tag(&c, "lotw"),
    }))
}

/// A QRZ account, logged in when first asked, again when its session runs out.
pub struct Qrz {
    url: String,
    username: String,
    password: String,
    key: Mutex<Option<String>>,
}

impl Qrz {
    /// The account, at QRZ's interface or another for tests.
    #[must_use]
    pub fn new(url: &str, username: &str, password: &str) -> Qrz {
        Qrz {
            url: url.to_owned(),
            username: username.to_owned(),
            password: password.to_owned(),
            key: Mutex::new(None),
        }
    }

    fn get(&self, query: &str) -> Result<String, String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .build()
            .into();
        let url = format!("{}?{query}", self.url);
        let mut r = agent.get(&url).call().map_err(|e| format!("QRZ: {e}"))?;
        r.body_mut()
            .read_to_string()
            .map_err(|e| format!("QRZ: {e}"))
    }

    fn login(&self) -> Result<String, String> {
        let enc = |s: &str| -> String {
            s.bytes()
                .map(|b| {
                    if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                        char::from(b).to_string()
                    } else {
                        format!("%{b:02X}")
                    }
                })
                .collect()
        };
        let xml = self.get(&format!(
            "username={};password={};agent=radiopost-po",
            enc(&self.username),
            enc(&self.password)
        ))?;
        let key = tag(&tag(&xml, "Session"), "Key");
        if key.is_empty() {
            let why = tag(&tag(&xml, "Session"), "Error");
            return Err(format!("QRZ refused the login: {why}"));
        }
        Ok(key)
    }

    /// What QRZ says of `call`.
    ///
    /// # Errors
    ///
    /// QRZ could not be reached or refused the account.
    pub fn find(&self, call: &str) -> Result<Option<Entry>, String> {
        let call: String = call
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '/')
            .collect();
        for attempt in 0..2 {
            let held = self
                .key
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let key = match held {
                Some(k) if attempt == 0 => k,
                _ => self.login()?,
            };
            *self
                .key
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(key.clone());
            match read(&self.get(&format!("s={key};callsign={call}"))?) {
                Err(e) if attempt == 0 && (e.contains("Session") || e.contains("Invalid")) => {}
                other => return other,
            }
        }
        Err("QRZ would not keep a session".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_as_qrz_writes_them() {
        let found = r#"<?xml version="1.0" encoding="utf-8" ?>
<QRZDatabase version="1.34" xmlns="http://xmldata.qrz.com">
<Callsign><call>N5CALL</call><fname>José</fname><name>Pérez</name><addr2>Atlanta</addr2>
<state>GA</state><country>United States</country><lotw>1</lotw></Callsign>
<Session><Key>2331uf894c4bd29f3923f3bacf02c532d7bd9</Key><Count>123</Count></Session>
</QRZDatabase>"#;
        let e = read(found).expect("read").expect("an entry");
        assert_eq!(
            (e.call.as_str(), e.first.as_str(), e.last.as_str()),
            ("N5CALL", "José", "Pérez")
        );
        assert_eq!(
            (e.city.as_str(), e.state.as_str(), e.lotw.as_str()),
            ("Atlanta", "GA", "1")
        );
        let none = "<QRZDatabase><Session><Error>Not found: N0CALL</Error></Session></QRZDatabase>";
        assert_eq!(read(none), Ok(None));
        let lapsed = "<QRZDatabase><Session><Error>Session Timeout</Error></Session></QRZDatabase>";
        assert_eq!(read(lapsed), Err("Session Timeout".into()));
        assert_eq!(tag("<a>x &amp; y</a>", "a"), "x & y");
    }
}
