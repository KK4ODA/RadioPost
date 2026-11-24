// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-0 §6.2: an address is written in lowercase, both parts, and two addresses are one when they
//! differ only in the case of ASCII letters: the domain as DNS compares names (RFC 4343), and the
//! local part because RadioPost writes it in lowercase. Nothing else is folded.
//!
//! Every program lowercases an address with [`lowercase`] where it enters (typed, imported, read
//! from an email or a file, given in a request) so that what it keeps, signs and sends is one
//! spelling. [`same`] is for what came from elsewhere as it was: a signed message or credential
//! from a program written before the rule keeps its capitals, and is not refused for them.

/// `address` as a program writes it (RP-0 §6.2): its ASCII capitals made small, nothing else
/// changed. Not a check: what is not an address comes back lowercased all the same.
#[must_use]
pub fn lowercase(address: &str) -> String {
    address.to_ascii_lowercase()
}

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
    fn written_in_lowercase_both_parts() {
        assert_eq!(lowercase("Ana.Lopez@HamPost.ORG"), "ana.lopez@hampost.org");
        // a letter outside ASCII stays as it is
        assert_eq!(lowercase("JOSÉ@RadioPost.org"), "josÉ@radiopost.org");
        assert!(same(&lowercase("Ana@HamPost.org"), "ana@hampost.org"));
    }

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
