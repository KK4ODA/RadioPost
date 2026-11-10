// SPDX-License-Identifier: MIT OR Apache-2.0
//! A RadioPost gateway (design §5.2): users call it by radio, and it carries each session to the
//! user's post office (RP-3 §9, RP-5 §5), online.
//!
//! [`radiopost::session::gateway::Gateway`] is the session's engine; [`upstream`] reaches the post
//! office; [`station`] answers calls on an RPMI modem and keeps what outlives a session;
//! [`operator`] registers the gateway with its operator's post office.

pub mod operator;
pub mod station;
pub mod upstream;
