// SPDX-License-Identifier: MIT OR Apache-2.0
//! The RadioPost client: the user's device key and credential, their mail, and RP-3 sessions
//! over any modem that has an RPMI adapter (RP-4).
//!
//! * [`store`]: the client's home directory: identity, trusted post offices, messages, folders,
//!   and what is kept between sessions.
//! * [`compose`]: writing a message (RP-2).
//! * [`session`]: calling a gateway or a station, or answering a station, over RPMI.
//! * [`online`]: the post office over the Internet: enrollment, renewal, settings, the accept
//!   list, and station-to-station sessions reported (RP-5 §6).
//! * [`panel`]: the page in `app/ui/` and its JSON API, served on the loopback interface.
//!
//! `radiopost` is its command-line program; the desktop application (Tauri, D27) is built on the
//! same library.

pub mod compose;
pub mod online;
pub mod panel;
pub mod session;
pub mod store;
