// SPDX-License-Identifier: MIT OR Apache-2.0
//! The band rules the gateway checks itself (RP-4 §8, wishlist W40), for a modem that does not
//! check them but says where the radio's dial is (RP-4 §4.1): an unattended gateway answers only
//! with its signal inside a segment where automatic control is allowed (RP-7 §5.1's table).

use radiopost::forwarding::window_allowed;

/// What a signal on single sideband may occupy at most above the dial, hertz: a transmitter's
/// audio passband. The band a modem the gateway does not know occupies is somewhere in it.
pub const USB_AUDIO_HZ: u64 = 3000;

/// Whether an unattended gateway in `country` may answer with the radio at `dial_hz`: the dial and
/// the [`USB_AUDIO_HZ`] above it lie inside one segment of the country's table.
#[must_use]
pub fn may_answer_at(dial_hz: u64, country: &str) -> bool {
    dial_hz
        .checked_add(USB_AUDIO_HZ)
        .is_some_and(|top| window_allowed((dial_hz, top), country))
}

/// What the gateway says of its dial: `None` when it may answer there, else why it does not.
#[must_use]
pub fn verdict(dial_hz: u64, country: &str) -> Option<String> {
    (!may_answer_at(dial_hz, country)).then(|| {
        format!(
            "the radio is at {} MHz: a signal there, up to {} kHz above the dial, is outside the \
             segments where an unattended station may answer in {country} (RP-7 §5.1), so the \
             gateway does not answer until the radio is back in one",
            mhz(dial_hz),
            USB_AUDIO_HZ / 1000
        )
    })
}

/// A dial in MHz, to the kilohertz or finer when it needs it: 7.101, 3.5915.
#[must_use]
pub fn mhz(hz: u64) -> String {
    let fraction = format!("{:06}", hz % 1_000_000);
    let fraction = fraction.trim_end_matches('0');
    format!("{}.{fraction:0<3}", hz / 1_000_000)
}

#[cfg(test)]
mod tests {
    use super::{may_answer_at, mhz, verdict};

    /// §97.221(b)'s segments, ends included: 40 m's 7.100–7.105 MHz takes a signal from 7.101
    /// up to 3 kHz above it, not one from 7.103; 80 m's 3.585–3.600 MHz takes 3.5915.
    #[test]
    fn a_dial_is_judged_with_the_three_kilohertz_above_it() {
        assert!(may_answer_at(7_101_000, "US"));
        assert!(
            may_answer_at(7_102_000, "US"),
            "the segment's top end included"
        );
        assert!(!may_answer_at(7_103_000, "US"));
        assert!(may_answer_at(3_591_500, "US"));
        assert!(
            !may_answer_at(14_105_000, "XX"),
            "a country with no table: nowhere"
        );
        assert!(!may_answer_at(u64::MAX, "US"));
        assert_eq!(verdict(7_101_000, "US"), None);
        assert!(verdict(14_074_000, "US").is_some_and(|v| v.contains("14.074 MHz")));
        assert_eq!(
            (mhz(7_101_000), mhz(3_591_500), mhz(50_000)),
            ("7.101".into(), "3.5915".into(), "0.050".into())
        );
    }
}
