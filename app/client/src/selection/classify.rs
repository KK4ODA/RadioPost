// SPDX-License-Identifier: MIT OR Apache-2.0
//! Why a call ended as it did (choosing a gateway, §12.3): a class from the evidence the client
//! holds, so that what is learned later blames the right thing. A dead cable is not the gateway's
//! path, a busy channel is nobody's path, and the operator's own cancel is nothing at all. What the
//! evidence cannot explain is [`Class::Unknown`], never a guess.
//!
//! The rules are versioned ([`RULES`]): a record keeps its evidence and the version that classed
//! it, so a later version can class every record again.

/// The version of these rules, kept with each record they class.
pub const RULES: u32 = 1;

/// What a call came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Both stations said `DONE`, and everything offered was taken.
    Completed,
    /// Both stations said `DONE`, and some of what was offered stayed in the outbox.
    CompletedInPart,
    /// The operator stopped it.
    Cancelled,
    /// This station's own equipment: the modem could not be reached, refused the call, or the
    /// connection to it failed.
    LocalStation,
    /// The channel was in use.
    ChannelBusy,
    /// No answer, and nothing of the station called was heard: the gateway away, or the path too
    /// weak one way or the other.
    NoAnswer,
    /// No answer, though the station called was heard: it is there, and it did not hear the call.
    NoAnswerHeard,
    /// The other station, or its post office, refused: policy, a credential, or the wrong kind of
    /// station.
    Refused,
    /// Linked, but the other station could not serve the session (its post office unreachable,
    /// an internal error, busy).
    ServiceFailed,
    /// The link broke, or went silent, before the session's end.
    LinkLost,
    /// Something crossed that RP-3 does not allow.
    ProtocolError,
    /// The evidence does not say.
    Unknown,
}

impl Class {
    /// The name kept in a record.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Class::Completed => "completed",
            Class::CompletedInPart => "completed-in-part",
            Class::Cancelled => "cancelled",
            Class::LocalStation => "local-station",
            Class::ChannelBusy => "channel-busy",
            Class::NoAnswer => "no-answer",
            Class::NoAnswerHeard => "no-answer-heard",
            Class::Refused => "refused",
            Class::ServiceFailed => "service-failed",
            Class::LinkLost => "link-lost",
            Class::ProtocolError => "protocol-error",
            Class::Unknown => "unknown",
        }
    }

    /// The class a record names, or `None` for a name these rules do not know.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Class> {
        ALL.iter().copied().find(|c| c.name() == name)
    }

    /// In plain words, for the panel and the command line.
    #[must_use]
    pub fn words(self) -> &'static str {
        match self {
            Class::Completed => "completed",
            Class::CompletedInPart => "completed, some mail left in the outbox",
            Class::Cancelled => "stopped by you",
            Class::LocalStation => "this station's modem or radio",
            Class::ChannelBusy => "the channel was busy",
            Class::NoAnswer => "no answer, nothing heard",
            Class::NoAnswerHeard => "no answer, though the station was heard",
            Class::Refused => "refused by the other station",
            Class::ServiceFailed => "the other station could not serve the session",
            Class::LinkLost => "the link was lost",
            Class::ProtocolError => "the session broke the protocol",
            Class::Unknown => "unknown",
        }
    }

    /// Whether the call carried what it set out to: a completed session, whole or in part.
    #[must_use]
    pub fn completed(self) -> bool {
        matches!(self, Class::Completed | Class::CompletedInPart)
    }
}

/// Every class, in the order of [`Class`].
pub const ALL: [Class; 12] = [
    Class::Completed,
    Class::CompletedInPart,
    Class::Cancelled,
    Class::LocalStation,
    Class::ChannelBusy,
    Class::NoAnswer,
    Class::NoAnswerHeard,
    Class::Refused,
    Class::ServiceFailed,
    Class::LinkLost,
    Class::ProtocolError,
    Class::Unknown,
];

/// What the client knows of one call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each a separate fact the modem or the session reported"
)]
pub struct Evidence {
    /// The modem's adapter answered `hello` (RP-4 §3).
    pub opened: bool,
    /// The modem refused a request of the call's own (`connect`, or the `tune` before it) with
    /// an `error` (RP-4 §4, §7): its code.
    pub refused_request: Option<String>,
    /// The connection to the adapter failed or closed during the call.
    pub adapter_fault: bool,
    /// How a call that brought no link ended: RPMI's `failed` code (RP-4 §5).
    pub failed: Option<String>,
    /// The station called was heard during the call (RPMI `heard`).
    pub heard_remote: bool,
    /// A link came up.
    pub connected: bool,
    /// How the link ended: RPMI's `disconnected` `reason`.
    pub disconnected: Option<String>,
    /// Nothing arrived for the silence of RP-3 §8, and the session was given up.
    pub silent: bool,
    /// Both stations said `DONE` (RP-3 §5.7).
    pub over: bool,
    /// The codes of the `ERROR`s the other station sent (RP-3 §5.9).
    pub errors: Vec<String>,
    /// This station refused the other: a gateway answered a station call (RP-3 §10), or its
    /// proof did not check.
    pub refused_other: bool,
    /// Messages offered that the other station did not take.
    pub left_out: usize,
}

/// The class of a call, from its evidence.
#[must_use]
pub fn classify(e: &Evidence) -> Class {
    if !e.opened {
        return Class::LocalStation;
    }
    if e.connected {
        if let Some(code) = e.errors.first() {
            return by_error(code);
        }
        if e.refused_other {
            return Class::Refused;
        }
        if e.over {
            return if e.left_out > 0 {
                Class::CompletedInPart
            } else {
                Class::Completed
            };
        }
        if e.silent {
            return Class::LinkLost;
        }
        if e.adapter_fault {
            return Class::LocalStation;
        }
        return match e.disconnected.as_deref() {
            Some("remote" | "timeout" | "failure") => Class::LinkLost,
            Some("local") => Class::Cancelled,
            _ => Class::Unknown,
        };
    }
    match e.failed.as_deref() {
        Some("aborted") => Class::Cancelled,
        Some("busy") => Class::ChannelBusy,
        Some("refused") => Class::Refused,
        Some("timeout") if e.heard_remote => Class::NoAnswerHeard,
        Some("timeout") => Class::NoAnswer,
        Some(_) => Class::Unknown,
        None => match e.refused_request.as_deref() {
            Some("busy") => Class::ChannelBusy,
            Some(_) => Class::LocalStation,
            None if e.adapter_fault => Class::LocalStation,
            None => Class::Unknown,
        },
    }
}

/// The class of a session ended by an `ERROR` (RP-0 §9.3): a refusal of policy or credential, a
/// broken protocol, or a station that could not serve.
fn by_error(code: &str) -> Class {
    match code {
        "refused-by-policy"
        | "not-authorized"
        | "credential-invalid"
        | "credential-expired"
        | "credential-revoked"
        | "credential-required"
        | "unknown-post-office"
        | "signature-invalid"
        | "time-skew" => Class::Refused,
        "malformed" | "unsupported-version" => Class::ProtocolError,
        _ => Class::ServiceFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linked() -> Evidence {
        Evidence {
            opened: true,
            connected: true,
            disconnected: Some("local".into()),
            ..Evidence::default()
        }
    }

    fn calling(failed: &str) -> Evidence {
        Evidence {
            opened: true,
            failed: Some(failed.into()),
            ..Evidence::default()
        }
    }

    #[test]
    fn names_go_both_ways() {
        for c in ALL {
            assert_eq!(Class::from_name(c.name()), Some(c), "{c:?}");
        }
        assert_eq!(Class::from_name("something else"), None);
    }

    #[test]
    fn a_modem_that_cannot_be_reached_is_the_station_s() {
        assert_eq!(classify(&Evidence::default()), Class::LocalStation);
    }

    #[test]
    fn calls_with_no_link_are_classed_by_how_they_failed() {
        assert_eq!(classify(&calling("aborted")), Class::Cancelled);
        assert_eq!(classify(&calling("busy")), Class::ChannelBusy);
        assert_eq!(classify(&calling("refused")), Class::Refused);
        assert_eq!(classify(&calling("timeout")), Class::NoAnswer);
        let heard = Evidence {
            heard_remote: true,
            ..calling("timeout")
        };
        assert_eq!(classify(&heard), Class::NoAnswerHeard);
        assert_eq!(classify(&calling("something new")), Class::Unknown);
    }

    #[test]
    fn a_request_the_modem_refused_is_busy_or_the_station_s() {
        let refused = |code: &str| Evidence {
            opened: true,
            refused_request: Some(code.into()),
            ..Evidence::default()
        };
        assert_eq!(classify(&refused("busy")), Class::ChannelBusy);
        assert_eq!(classify(&refused("refused")), Class::LocalStation);
        assert_eq!(classify(&refused("not-authorized")), Class::LocalStation);
        let fault = Evidence {
            opened: true,
            adapter_fault: true,
            ..Evidence::default()
        };
        assert_eq!(classify(&fault), Class::LocalStation);
        let nothing = Evidence {
            opened: true,
            ..Evidence::default()
        };
        assert_eq!(classify(&nothing), Class::Unknown);
    }

    #[test]
    fn a_session_both_ended_is_completed_whole_or_in_part() {
        let done = Evidence {
            over: true,
            ..linked()
        };
        assert_eq!(classify(&done), Class::Completed);
        let part = Evidence {
            left_out: 2,
            ..done.clone()
        };
        assert_eq!(classify(&part), Class::CompletedInPart);
        assert!(Class::Completed.completed() && Class::CompletedInPart.completed());
        assert!(!Class::LinkLost.completed());
    }

    #[test]
    fn an_error_names_the_cause_before_anything_else() {
        let ended = |code: &str| Evidence {
            errors: vec![code.into()],
            over: true,
            ..linked()
        };
        assert_eq!(classify(&ended("refused-by-policy")), Class::Refused);
        assert_eq!(classify(&ended("credential-expired")), Class::Refused);
        assert_eq!(
            classify(&ended("post-office-unreachable")),
            Class::ServiceFailed
        );
        assert_eq!(classify(&ended("internal")), Class::ServiceFailed);
        assert_eq!(classify(&ended("malformed")), Class::ProtocolError);
        let not_a_station = Evidence {
            refused_other: true,
            ..linked()
        };
        assert_eq!(classify(&not_a_station), Class::Refused);
    }

    #[test]
    fn a_link_that_ends_early_is_lost_or_stopped() {
        for reason in ["remote", "timeout", "failure"] {
            let e = Evidence {
                disconnected: Some(reason.into()),
                ..linked()
            };
            assert_eq!(classify(&e), Class::LinkLost, "{reason}");
        }
        assert_eq!(classify(&linked()), Class::Cancelled);
        let silent = Evidence {
            silent: true,
            disconnected: None,
            ..linked()
        };
        assert_eq!(classify(&silent), Class::LinkLost);
        let fault = Evidence {
            adapter_fault: true,
            disconnected: None,
            ..linked()
        };
        assert_eq!(classify(&fault), Class::LocalStation);
        let open = Evidence {
            disconnected: None,
            ..linked()
        };
        assert_eq!(classify(&open), Class::Unknown);
    }
}
