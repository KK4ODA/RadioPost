// SPDX-License-Identifier: MIT OR Apache-2.0
//! The RadioPost Modem Interface (RP-4): how a RadioPost program, the host, drives a modem through
//! an adapter, in JSON Lines.
//!
//! * [`wire`]: the lines, their `op`, `id` and `data`, and the requests read member by member
//!   (RP-4 §2, §4).
//! * [`host`]: the host's side: requests numbered on a connection, the adapter's lines read as
//!   events, and a carrier joining one RP-3 session to one link.
//! * [`channel`]: a simulated channel of stations and their adapters, RP-4 §1 to §7 as the
//!   reference that made the vectors has them: the loopback adapter, and the conformance tester's
//!   test adapter.
//! * [`tcp`]: the channel served on loopback ports, and a host's connection to any adapter.
//!
//! None of it but [`tcp`] does input or output, and nothing has a clock.

pub mod base64;
pub mod channel;
pub mod host;
pub mod tcp;
pub mod wire;

pub use wire::{Caps, DEFAULT_PORT, Message, VERSION};
