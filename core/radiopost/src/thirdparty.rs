// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-6 §5.1: which stations a gateway may hand third-party mail to, by where the station is: its
//! own country's prefixes, and the countries it has third-party arrangements with.

/// A country's own callsign prefixes.
#[must_use]
pub fn own_prefixes(country: &str) -> Vec<String> {
    match country {
        "US" => [
            "K", "N", "W", "AA", "AB", "AC", "AD", "AE", "AF", "AG", "AH", "AI", "AJ", "AK", "AL",
        ]
        .iter()
        .map(|p| (*p).to_owned())
        .collect(),
        _ => Vec::new(),
    }
}

/// The countries a country has third-party arrangements with: (their prefixes as written, the
/// country). For the US: the ARRL's list, read 2026-11-09.
#[must_use]
pub fn arrangements(country: &str) -> &'static [(&'static str, &'static str)] {
    match country {
        "US" => &[
            ("V2", "Antigua and Barbuda"),
            ("LO–LW", "Argentina"),
            ("VK", "Australia"),
            ("V3", "Belize"),
            ("CP", "Bolivia"),
            ("E7", "Bosnia-Herzegovina"),
            ("PP–PY", "Brazil"),
            ("VE, VO, VY", "Canada"),
            ("CA–CE", "Chile"),
            ("HJ–HK", "Colombia"),
            ("D6", "Comoros"),
            ("TI, TE", "Costa Rica"),
            ("CM, CO", "Cuba"),
            ("HI", "Dominican Republic"),
            ("J7", "Dominica"),
            ("HC–HD", "Ecuador"),
            ("YS", "El Salvador"),
            ("C5", "The Gambia"),
            ("9G", "Ghana"),
            ("J3", "Grenada"),
            ("TG", "Guatemala"),
            ("8R", "Guyana"),
            ("HH", "Haiti"),
            ("HQ–HR", "Honduras"),
            ("4X, 4Z", "Israel"),
            ("6Y", "Jamaica"),
            ("JY", "Jordan"),
            ("EL", "Liberia"),
            ("V7", "Marshall Islands"),
            ("XA–XI", "Mexico"),
            ("V6", "Micronesia"),
            ("YN", "Nicaragua"),
            ("HO–HP", "Panama"),
            ("ZP", "Paraguay"),
            ("OA–OC", "Peru"),
            ("DU–DZ", "Philippines"),
            ("V4", "St. Kitts and Nevis"),
            ("J6", "St. Lucia"),
            ("J8", "St. Vincent and the Grenadines"),
            ("9L", "Sierra Leone"),
            ("ZR–ZU", "South Africa"),
            ("3DA", "Eswatini"),
            ("9Y–9Z", "Trinidad and Tobago"),
            ("TA–TC", "Turkey"),
            ("GB", "United Kingdom"),
            ("CV–CX", "Uruguay"),
            ("YV–YY", "Venezuela"),
            ("4U1ITU", "ITU, Geneva"),
            ("4U1VIC", "VIC, Vienna"),
        ],
        _ => &[],
    }
}

/// `VE, VO, VY` and `LO–LW` (every prefix between, by the last character) as a list.
#[must_use]
pub fn prefixes(written: &str) -> Vec<String> {
    let mut out = Vec::new();
    for item in written.split(',') {
        let item = item.trim();
        let Some((first, last)) = item.split_once('–') else {
            out.push(item.to_owned());
            continue;
        };
        let (Some(a), Some(b)) = (first.chars().last(), last.chars().last()) else {
            continue;
        };
        let stem: String = first.chars().take(first.chars().count() - 1).collect();
        for c in a..=b {
            out.push(format!("{stem}{c}"));
        }
    }
    out
}

/// A whole callsign: one to three letters or digits, a digit, one to four letters.
fn whole_callsign(part: &str) -> bool {
    let b = part.as_bytes();
    let letters = b
        .iter()
        .rev()
        .take_while(|c| c.is_ascii_uppercase())
        .count();
    if !(1..=4).contains(&letters) || b.len() < letters + 2 {
        return false;
    }
    let digit = b.len() - letters - 1;
    let prefix = &b[..digit];
    b[digit].is_ascii_digit()
        && (1..=3).contains(&prefix.len())
        && prefix.iter().all(u8::is_ascii_alphanumeric)
}

/// RP-6 §5.1: the part of an operating callsign that says where the station is, if any.
#[must_use]
pub fn station_designator(callsign: &str) -> Option<String> {
    let upper = callsign.to_uppercase();
    let parts: Vec<&str> = upper.split('/').collect();
    if parts.iter().any(|p| *p == "MM" || *p == "AM") {
        return None;
    }
    let parts: Vec<&str> = parts
        .into_iter()
        .filter(|p| {
            !p.is_empty()
                && !matches!(*p, "P" | "M" | "QRP" | "A")
                && !p.chars().all(|c| c.is_ascii_digit())
        })
        .collect();
    parts
        .iter()
        .find(|p| !whole_callsign(p))
        .or_else(|| parts.first())
        .map(|p| (*p).to_owned())
}

/// RP-6 §5.1: may a gateway of `country` hand third-party mail to the station `callsign`?
#[must_use]
pub fn may_hand(callsign: &str, country: &str) -> bool {
    let Some(where_) = station_designator(callsign) else {
        return false;
    };
    let mut allowed = own_prefixes(country);
    for (written, _) in arrangements(country) {
        allowed.extend(prefixes(written));
    }
    allowed.iter().any(|p| where_.starts_with(p.as_str()))
}
