// SPDX-License-Identifier: MIT OR Apache-2.0
//! Renewing this device's credential before it runs out (RP-1 §4.2, RP-5 §6.2).
//!
//! A post office issues credentials for 90 days, and one that has run out cannot be renewed: the
//! device enrolls again with a new code. So the client renews by itself in the credential's last
//! [`WITHIN`]: over the Internet while its window is open (the panel asks when it starts and every
//! [`EVERY`]), and by radio on a gateway call, which asks with `renew` in `HELLO` (RP-3 §5.1,
//! §5.8). Earlier than that, each session or start would make the post office issue a new
//! credential for nothing and, by radio, spend air on it; a month leaves weeks of chances.

use std::time::Duration;

use crate::online;
use crate::store::{Error, Store};

/// How long before its credential runs out the client starts renewing it: a third of the 90 days
/// a post office gives (RP-1 §4.2), as for a gateway.
pub const WITHIN: u64 = radiopost::credential::RENEW_WITHIN;

/// How often an open window asks whether the credential is due.
pub const EVERY: Duration = Duration::from_secs(3600);

/// Whether a credential that runs out at `expires` is to be renewed at `now`, renewing in the last
/// `within` seconds: from then until it runs out, and not after (it can no longer log in).
#[must_use]
pub fn due(expires: u64, now: u64, within: u64) -> bool {
    radiopost::credential::renewal_due(expires, now, within)
}

/// Renews over the Internet when the credential is due: what the post office issued, or `None`
/// when it is not due.
///
/// # Errors
///
/// No credential, or the post office could not be reached or refused.
pub fn renew_if_due(store: &Store, now: u64, within: u64) -> Result<Option<String>, Error> {
    let me = store.me()?;
    if !due(me.expires, now, within) {
        return Ok(None);
    }
    online::renew(store).map(Some)
}

/// Whole days from `now` to `expires`, for what the client says.
#[must_use]
pub fn days_left(expires: u64, now: u64) -> u64 {
    expires.saturating_sub(now) / 86_400
}

#[cfg(test)]
mod tests {
    use super::{WITHIN, days_left, due};

    #[test]
    fn a_credential_is_renewed_in_its_last_month_and_not_after() {
        let expires = 1_000_000_000;
        assert!(!due(expires, expires - WITHIN - 1, WITHIN), "too early");
        assert!(
            due(expires, expires - WITHIN, WITHIN),
            "the first second of the month"
        );
        assert!(due(expires, expires - 1, WITHIN), "its last second");
        assert!(
            !due(expires, expires, WITHIN),
            "run out: it cannot log in to be renewed"
        );
        assert!(!due(expires, expires + 86_400, WITHIN), "long run out");
        assert_eq!(days_left(expires, expires - WITHIN), 30);
        assert_eq!(days_left(expires, expires + 5), 0);
    }
}
