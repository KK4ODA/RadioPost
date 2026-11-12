// SPDX-License-Identifier: MIT OR Apache-2.0
//! A RadioPost gateway (design §5.2): users call it by radio, and it carries each session to the
//! user's post office (RP-3 §9, RP-5 §5), online.
//!
//! [`radiopost::session::gateway::Gateway`] is the session's engine; [`upstream`] reaches the post
//! office; [`station`] answers calls on an RPMI modem and keeps what outlives a session;
//! [`operator`] registers the gateway with its operator's post office; [`monitor`] keeps what it
//! is doing for its operator, [`runner`] runs it on a thread of its own, and [`panel`] shows it.
//! What it says goes to [`LOG_FILE`] in its folder too ([`logfile`]), when the program names it.

/// The gateway's log file, in its folder: what it says, dated, and any panic.
pub const LOG_FILE: &str = "gateway.log";

pub use radiopost::logfile;

pub mod monitor;
pub mod operator;
pub mod panel;
pub mod runner;
pub mod station;
pub mod upstream;
