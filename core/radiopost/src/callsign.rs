// SPDX-License-Identifier: MIT OR Apache-2.0
//! Callsigns (RP-0 §6.1): a callsign's base, the licensee's own callsign without the SSID, prefix
//! or suffix a station is on the air with, which RP-1 §6.3's checks compare.

/// RP-0 §6.1: the callsign without its SSID, prefix and suffix: of the parts its `/` separate,
/// the SSID taken off, the longest, or of two as long the last. In uppercase.
#[must_use]
pub fn base(callsign: &str) -> String {
    let call = callsign.trim();
    let call = call.split_once('-').map_or(call, |(c, _)| c);
    call.split('/')
        .fold(
            "",
            |best, part| {
                if part.len() >= best.len() { part } else { best }
            },
        )
        .to_ascii_uppercase()
}

/// RP-0 §6.1: whether `callsign` is one as it is sent: upper-case letters and digits, a prefix
/// and a suffix of its own after `/`, an SSID from `-0` to `-15`, at most 20 characters.
#[must_use]
pub fn valid(callsign: &str) -> bool {
    if callsign.is_empty() || callsign.len() > 20 {
        return false;
    }
    let (call, ssid) = match callsign.split_once('-') {
        Some((c, n)) => (c, Some(n)),
        None => (callsign, None),
    };
    let ssid_ok = ssid.is_none_or(|n| {
        !n.is_empty()
            && n.len() <= 2
            && n.bytes().all(|b| b.is_ascii_digit())
            && !(n.len() == 2 && n.starts_with('0'))
            && n.parse::<u8>().is_ok_and(|v| v <= 15)
    });
    let parts: Vec<&str> = call.split('/').collect();
    ssid_ok
        && (1..=3).contains(&parts.len())
        && parts.iter().all(|p| {
            (1..=10).contains(&p.len())
                && p.bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        })
}

/// RP-0 §6.1: whether two callsigns share a base, the same licensee on the air two ways.
#[must_use]
pub fn same_base(a: &str, b: &str) -> bool {
    base(a) == base(b)
}

#[cfg(test)]
mod tests {
    use super::{base, same_base, valid};

    #[test]
    fn a_callsign_as_it_is_sent() {
        for good in [
            "KK4ODA",
            "KK4ODA-15",
            "VE3/KK4ODA/P",
            "W4/KK4ODA/M-2",
            "N0CALL-0",
        ] {
            assert!(valid(good), "{good}");
        }
        for bad in [
            "kk4oda",
            "KK4ODA-16",
            "KK4ODA-01",
            "A/B/C/D",
            "KK4ODA-",
            "",
            "K K",
        ] {
            assert!(!valid(bad), "{bad}");
        }
    }

    #[test]
    fn a_callsign_loses_its_ssid_prefix_and_suffix() {
        assert_eq!(base("VE3/KK4ODA/P-7"), "KK4ODA");
        assert_eq!(base("G4ABC/W4"), "G4ABC");
        assert_eq!(base("VE3X/W1AW"), "W1AW", "of two as long, the last");
        assert!(same_base("kk4oda-1", "KK4ODA/M"));
        assert!(!same_base("KK4ODA", "KK4ODB"));
    }
}
