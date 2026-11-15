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

/// RP-0 §6.1: whether two callsigns share a base, the same licensee on the air two ways.
#[must_use]
pub fn same_base(a: &str, b: &str) -> bool {
    base(a) == base(b)
}

#[cfg(test)]
mod tests {
    use super::{base, same_base};

    #[test]
    fn a_callsign_loses_its_ssid_prefix_and_suffix() {
        assert_eq!(base("VE3/KK4ODA/P-7"), "KK4ODA");
        assert_eq!(base("G4ABC/W4"), "G4ABC");
        assert_eq!(base("VE3X/W1AW"), "W1AW", "of two as long, the last");
        assert!(same_base("kk4oda-1", "KK4ODA/M"));
        assert!(!same_base("KK4ODA", "KK4ODB"));
    }
}
