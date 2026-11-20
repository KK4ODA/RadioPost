// SPDX-License-Identifier: MIT OR Apache-2.0
//! Choosing a gateway (`docs/design/04-gateway-selection.md`): what the client keeps of every
//! call, so that later choices can learn from it.
//!
//! * [`attempts`]: one record of each call placed, kept in the first home and never rewritten.
//! * [`classify`]: why a call ended as it did, from what the modem and the session said.

pub mod attempts;
pub mod classify;
