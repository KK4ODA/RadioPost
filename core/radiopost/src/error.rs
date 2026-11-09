// SPDX-License-Identifier: MIT OR Apache-2.0
//! RP-0 §9.3: the error codes shared by the session protocol (RP-3) and the post office API (RP-5).

use std::fmt;

/// An RP-0 §9.3 error: its code on the air, its name everywhere else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
#[allow(missing_docs)] // each variant is its name in RP-0 §9.3
pub enum ErrorCode {
    UnsupportedVersion = 1,
    Malformed = 2,
    CredentialInvalid = 3,
    CredentialExpired = 4,
    CredentialRevoked = 5,
    CredentialRequired = 6,
    UnknownPostOffice = 7,
    SignatureInvalid = 8,
    TimeSkew = 9,
    NotAuthorized = 10,
    TooLarge = 11,
    QuotaExceeded = 12,
    RefusedByPolicy = 13,
    AlreadyHave = 14,
    UnknownMessage = 15,
    PostOfficeUnreachable = 16,
    Busy = 17,
    Timeout = 18,
    Internal = 19,
}

const NAMES: [(ErrorCode, &str); 19] = [
    (ErrorCode::UnsupportedVersion, "unsupported-version"),
    (ErrorCode::Malformed, "malformed"),
    (ErrorCode::CredentialInvalid, "credential-invalid"),
    (ErrorCode::CredentialExpired, "credential-expired"),
    (ErrorCode::CredentialRevoked, "credential-revoked"),
    (ErrorCode::CredentialRequired, "credential-required"),
    (ErrorCode::UnknownPostOffice, "unknown-post-office"),
    (ErrorCode::SignatureInvalid, "signature-invalid"),
    (ErrorCode::TimeSkew, "time-skew"),
    (ErrorCode::NotAuthorized, "not-authorized"),
    (ErrorCode::TooLarge, "too-large"),
    (ErrorCode::QuotaExceeded, "quota-exceeded"),
    (ErrorCode::RefusedByPolicy, "refused-by-policy"),
    (ErrorCode::AlreadyHave, "already-have"),
    (ErrorCode::UnknownMessage, "unknown-message"),
    (ErrorCode::PostOfficeUnreachable, "post-office-unreachable"),
    (ErrorCode::Busy, "busy"),
    (ErrorCode::Timeout, "timeout"),
    (ErrorCode::Internal, "internal"),
];

impl ErrorCode {
    /// The number carried on the air.
    #[must_use]
    pub fn code(self) -> u8 {
        self as u8
    }

    /// The name, as RP-0 §9.3 and the post office API write it.
    #[must_use]
    pub fn name(self) -> &'static str {
        NAMES
            .iter()
            .find(|(c, _)| *c == self)
            .map_or("internal", |(_, n)| n)
    }

    /// The error a name stands for.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        NAMES.iter().find(|(_, n)| *n == name).map(|(c, _)| *c)
    }

    /// The error a code stands for.
    #[must_use]
    pub fn from_code(code: u64) -> Option<Self> {
        NAMES
            .iter()
            .find(|(c, _)| u64::from(c.code()) == code)
            .map(|(c, _)| *c)
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl std::error::Error for ErrorCode {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_and_name_round_trip() {
        for (code, name) in NAMES {
            assert_eq!(ErrorCode::from_name(name), Some(code));
            assert_eq!(ErrorCode::from_code(u64::from(code.code())), Some(code));
            assert_eq!(code.name(), name);
        }
    }
}
