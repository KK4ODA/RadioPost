// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-0 §6.2: when two addresses are one. Both parts compare without regard to the case of ASCII
//! letters: the domain as DNS compares names (RFC 4343), and the local part because RadioPost's
//! post offices treat accounts so. Nothing else is folded.

/// Whether `a` and `b` are the same address (RP-0 §6.2).
#[must_use]
pub fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Whether `address` is one of `addresses` (RP-0 §6.2): for "addressed to" (RP-3 §10, RP-5
/// §6.6).
#[must_use]
pub fn among(address: &str, addresses: &[&str]) -> bool {
    addresses.iter().any(|a| same(a, address))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_of_ascii_letters_only() {
        assert!(same("ana@hampost.org", "Ana@HamPost.org"));
        assert!(!same("ana@hampost.org", "ana@hampost.com"));
        // a letter outside ASCII is compared as it is
        assert!(!same("josé@radiopost.org", "JOSÉ@radiopost.org"));
        assert!(among(
            "ana@hampost.org",
            &["maria@example.com", "ANA@hampost.org"]
        ));
        assert!(!among("ana@hampost.org", &[]));
    }
}
