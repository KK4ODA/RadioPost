// SPDX-License-Identifier: MIT OR Apache-2.0
//! The RadioPost client: the user's device key and credential, their mail, and RP-3 sessions
//! over any modem that has an RPMI adapter (RP-4).
//!
//! * [`store`]: the client's home directory: identity, trusted post offices, messages, folders,
//!   and what is kept between sessions.
//! * [`compose`]: writing a message (RP-2).
//! * [`session`]: calling a gateway or a station, or answering a station, over RPMI.
//! * [`online`]: the post office over the Internet: enrollment, renewal, settings, the accept
//!   list, station-to-station sessions reported (RP-5 §6), and the gateways it lists (§4.4).
//! * [`renewal`]: renewing the credential by itself in its last month, over the Internet or by
//!   radio.
//! * [`grid`]: Maidenhead grid squares, to rank gateways by distance.
//! * [`panel`]: the page in `app/ui/` and its JSON API, served on the loopback interface.
//!
//! `radiopost` is its command-line program; the desktop application (Tauri, D27) is built on the
//! same library. Both keep what they say in [`LOG_FILE`] in the home ([`logfile`]).

/// The client's log file, in its home: what the program and its window say, dated, and any panic.
pub const LOG_FILE: &str = "radiopost.log";

pub mod compose;
pub mod grid;
pub use radiopost::logfile;
pub mod online;
pub mod panel;
pub mod renewal;
pub mod session;
pub mod store;
