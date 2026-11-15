// SPDX-License-Identifier: MIT OR Apache-2.0
//! Contact files read into the address list (wishlist W2): vCard (`.vcf`, as phones, Google
//! Contacts, Outlook, Apple Contacts and Thunderbird export them, versions 2.1 to 4.0) and CSV
//! (Google's, Outlook's and Thunderbird's exports). Each contact gives a name, its email
//! addresses, and the groups it was in: a vCard's `CATEGORIES`, a Google export's labels, an
//! Outlook export's categories. Nothing else is read: no telephone numbers, no postal addresses.
//!
//! The file is read as text whatever it claims; an address is checked where it is kept
//! ([`crate::addresses::import`]), so this module only finds what each contact says.

/// One contact as the file has it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Contact {
    /// Its name, if the file gives one.
    pub name: Option<String>,
    /// Its email addresses, as written.
    pub emails: Vec<String>,
    /// The groups it is in, by name.
    pub groups: Vec<String>,
}

/// The contacts in a contact file: vCard when it holds `BEGIN:VCARD`, CSV otherwise.
#[must_use]
pub fn read(text: &str) -> Vec<Contact> {
    let text = text.trim_start_matches('\u{feff}');
    if text.to_ascii_uppercase().contains("BEGIN:VCARD") {
        vcards(text)
    } else {
        csv(text)
    }
}

/// A group name worth keeping: not a system label (Google's `* myContacts`, `* starred`), not
/// empty.
fn group_name(raw: &str) -> Option<String> {
    let g = raw.trim();
    (!g.is_empty() && !g.starts_with('*') && !g.eq_ignore_ascii_case("myContacts"))
        .then(|| g.chars().take(64).collect())
}

fn clean(name: &str) -> Option<String> {
    let n = name.split_whitespace().collect::<Vec<_>>().join(" ");
    (!n.is_empty()).then_some(n)
}

// ── vCard (RFC 6350, and 2.1 as phones still write it) ──────────────────────────────────────

/// A vCard value with its backslash escapes undone.
fn unescape(v: &str) -> String {
    let mut out = String::new();
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => out.push(' '),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Splits at `sep` where it is not escaped with a backslash.
fn split_unescaped(v: &str, sep: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut escaped = false;
    for c in v.chars() {
        if escaped {
            if let Some(p) = parts.last_mut() {
                p.push('\\');
                p.push(c);
            }
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == sep {
            parts.push(String::new());
        } else if let Some(p) = parts.last_mut() {
            p.push(c);
        }
    }
    parts.iter().map(|p| unescape(p)).collect()
}

/// Quoted-printable (vCard 2.1's `ENCODING=QUOTED-PRINTABLE`), its bytes read as UTF-8.
fn quoted_printable(v: &str) -> String {
    let bytes = v.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'='
            && let Some(hex) = v.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The logical lines: folded lines joined (a line that starts with a space or a tab goes on the
/// one before), and a quoted-printable value's soft breaks (a line ending in `=`) joined too.
fn unfolded(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.lines() {
        let soft = lines.last().is_some_and(|l: &String| {
            l.to_ascii_uppercase().contains("QUOTED-PRINTABLE") && l.ends_with('=')
        });
        if soft {
            if let Some(last) = lines.last_mut() {
                last.pop();
                last.push_str(raw.trim_start());
            }
        } else if (raw.starts_with(' ') || raw.starts_with('\t')) && !lines.is_empty() {
            if let Some(last) = lines.last_mut() {
                last.push_str(&raw[1..]);
            }
        } else {
            lines.push(raw.to_owned());
        }
    }
    lines
}

/// A property line: its name in capitals (without a group prefix such as `item1.`), whether its
/// value is quoted-printable, and the value.
fn property(line: &str) -> Option<(String, bool, String)> {
    let mut quoted = false;
    let colon = line.char_indices().find_map(|(i, c)| match c {
        '"' => {
            quoted = !quoted;
            None
        }
        ':' if !quoted => Some(i),
        _ => None,
    })?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut params = head.split(';');
    let name = params.next()?.rsplit('.').next()?.to_ascii_uppercase();
    let qp = params.any(|p| p.to_ascii_uppercase().contains("QUOTED-PRINTABLE"));
    Some((name, qp, value.to_owned()))
}

fn vcards(text: &str) -> Vec<Contact> {
    let mut out = Vec::new();
    let mut card: Option<(Contact, Option<String>)> = None;
    for line in unfolded(text) {
        let Some((name, qp, value)) = property(&line) else {
            continue;
        };
        let value = if qp { quoted_printable(&value) } else { value };
        match name.as_str() {
            "BEGIN" if value.trim().eq_ignore_ascii_case("VCARD") => {
                card = Some((Contact::default(), None));
            }
            "END" if value.trim().eq_ignore_ascii_case("VCARD") => {
                if let Some((mut c, structured)) = card.take() {
                    if c.name.is_none() {
                        c.name = structured;
                    }
                    out.push(c);
                }
            }
            "FN" => {
                if let Some((c, _)) = card.as_mut() {
                    c.name = clean(&unescape(&value));
                }
            }
            "N" => {
                if let Some((_, structured)) = card.as_mut() {
                    // family; given; additional; prefixes; suffixes
                    let parts = split_unescaped(&value, ';');
                    let get = |i: usize| parts.get(i).map_or("", String::as_str);
                    *structured = clean(&format!("{} {} {} {}", get(3), get(1), get(2), get(0)));
                }
            }
            "EMAIL" => {
                if let Some((c, _)) = card.as_mut() {
                    let v = unescape(&value);
                    let v = v.trim().trim_start_matches("mailto:").trim();
                    if !v.is_empty() {
                        c.emails.push(v.to_owned());
                    }
                }
            }
            "CATEGORIES" => {
                if let Some((c, _)) = card.as_mut() {
                    c.groups.extend(
                        split_unescaped(&value, ',')
                            .iter()
                            .filter_map(|g| group_name(g)),
                    );
                }
            }
            _ => {}
        }
    }
    out
}

// ── CSV (RFC 4180, as Google, Outlook and Thunderbird export contacts) ──────────────────────

/// The rows, each a list of fields: quoted fields may hold commas, quotes doubled and line
/// breaks.
fn rows(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

fn csv(text: &str) -> Vec<Contact> {
    let mut rows = rows(text).into_iter();
    let Some(header) = rows.next() else {
        return Vec::new();
    };
    let header: Vec<String> = header.iter().map(|h| h.trim().to_lowercase()).collect();
    let find = |names: &[&str]| header.iter().position(|h| names.contains(&h.as_str()));
    let emails: Vec<usize> = header
        .iter()
        .enumerate()
        .filter(|(_, h)| h.contains("email") || h.contains("e-mail"))
        .filter(|(_, h)| !["type", "display", "label"].iter().any(|w| h.contains(w)))
        .map(|(i, _)| i)
        .collect();
    let full = find(&["name", "display name", "full name"]);
    let first = find(&["first name", "given name"]);
    let middle = find(&["middle name", "additional name"]);
    let last = find(&["last name", "family name"]);
    let labels = find(&["labels", "group membership"]);
    let categories = find(&["categories"]);
    let mut out = Vec::new();
    for row in rows {
        let cell = |i: Option<usize>| i.and_then(|i| row.get(i)).map_or("", |s| s.trim());
        let name = clean(cell(full))
            .or_else(|| clean(&format!("{} {} {}", cell(first), cell(middle), cell(last))));
        let mut contact = Contact {
            name,
            ..Contact::default()
        };
        for &i in &emails {
            for e in cell(Some(i)).split(":::") {
                let e = e.trim();
                if !e.is_empty() {
                    contact.emails.push(e.to_owned());
                }
            }
        }
        contact
            .groups
            .extend(cell(labels).split(":::").filter_map(group_name));
        contact
            .groups
            .extend(cell(categories).split([';', ',']).filter_map(group_name));
        if !contact.emails.is_empty() {
            out.push(contact);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Contact, read};

    fn contact(name: Option<&str>, emails: &[&str], groups: &[&str]) -> Contact {
        Contact {
            name: name.map(str::to_owned),
            emails: emails.iter().map(|e| (*e).to_owned()).collect(),
            groups: groups.iter().map(|g| (*g).to_owned()).collect(),
        }
    }

    /// A vCard 3.0 file as Google and Apple write one: folded lines, a group prefix, escaped
    /// commas, categories; a 2.1 card as phones write one, its name in quoted-printable with a
    /// soft line break; and a 4.0 card with no FN, named from N.
    #[test]
    fn vcards() {
        let text = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Perez\\, Ana\r\nN:Perez;Ana;;;\r\n\
            item1.EMAIL;TYPE=INTERNET:ana@hampost.org\r\nEMAIL;TYPE=INTERNET;TYPE=HOME:ana.p\r\n \
            erez@example.com\r\nTEL:+1 555 0100\r\nCATEGORIES:myContacts,Shelter team,Net\\, Tue\
            sday\r\nEND:VCARD\r\n\
            BEGIN:VCARD\nVERSION:2.1\nFN;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:Jos=C3=A9 =\n\
            Ortega\nEMAIL;PREF;INTERNET:jose@example.net\nEND:VCARD\n\
            BEGIN:VCARD\nVERSION:4.0\nN:Lee;Zoe;;Dr.;\nEMAIL:mailto:zoe@example.com\nEND:VCARD\n\
            BEGIN:VCARD\nVERSION:4.0\nFN:No Email\nTEL:1\nEND:VCARD\n";
        assert_eq!(
            read(text),
            [
                contact(
                    Some("Perez, Ana"),
                    &["ana@hampost.org", "ana.perez@example.com"],
                    &["Shelter team", "Net, Tuesday"],
                ),
                contact(Some("José Ortega"), &["jose@example.net"], &[]),
                contact(Some("Dr. Zoe Lee"), &["zoe@example.com"], &[]),
                contact(Some("No Email"), &[], &[]),
            ]
        );
    }

    /// Google's CSV export (labels with ` ::: `, two addresses in one cell), Outlook's (first
    /// and last names, categories with `;`, a display-name column left alone) and
    /// Thunderbird's; quoted fields with commas and doubled quotes; a contact with no address
    /// left out.
    #[test]
    fn csv_exports() {
        let google = "\u{feff}First Name,Middle Name,Last Name,E-mail 1 - Label,E-mail 1 - Value,\
            Labels\nAna,,Perez,* Home,ana@hampost.org ::: ana@example.com,\
            * myContacts ::: Shelter team\n\"Ortega, Jr.\",,,,jose@example.net,* starred\n\
            Nobody,,,,,\n";
        assert_eq!(
            read(google),
            [
                contact(
                    Some("Ana Perez"),
                    &["ana@hampost.org", "ana@example.com"],
                    &["Shelter team"],
                ),
                contact(Some("Ortega, Jr."), &["jose@example.net"], &[]),
            ]
        );
        let outlook = "First Name,Last Name,E-mail Address,E-mail Display Name,E-mail 2 Address,\
            Categories\r\nZoe,Lee,zoe@example.com,\"Zoe \"\"Z\"\" Lee\",zlee@example.org,\
            Net;Shelter team\r\n";
        assert_eq!(
            read(outlook),
            [contact(
                Some("Zoe Lee"),
                &["zoe@example.com", "zlee@example.org"],
                &["Net", "Shelter team"],
            )]
        );
        let thunderbird = "First Name,Last Name,Display Name,Primary Email,Secondary Email\n\
            ,,Club Net,net@radiopost.org,\n";
        assert_eq!(
            read(thunderbird),
            [contact(Some("Club Net"), &["net@radiopost.org"], &[])]
        );
    }
}
