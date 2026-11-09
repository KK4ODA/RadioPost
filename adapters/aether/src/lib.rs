// SPDX-License-Identifier: MIT OR Apache-2.0
//! An RPMI adapter (RP-4) for the Aether HF modem, after RP-4 §9.1: each host that connects is
//! given its own WebSocket connection to Aether's control API, and its callsigns are a claim
//! there (Aether's ADR-0106), so several hosts, and Aether's own VARA-compatible port, can share
//! one station.
//!
//! * [`bridge`]: one host and one Aether connection, RP-4 §9.1's table, with no input or output.
//! * [`serve`]: the TCP port hosts connect to, and each host's WebSocket to Aether.

pub mod bridge;
pub mod serve;
