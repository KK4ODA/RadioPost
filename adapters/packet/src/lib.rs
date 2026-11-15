// SPDX-License-Identifier: MIT OR Apache-2.0
//! An RPMI adapter (RP-4) for AX.25 connected-mode packet, after RP-4 §9.4. It talks to a TNC's
//! AGWPE port (Dire Wolf, and the other programs that offer one) over TCP: frames of a 36-byte
//! header and their data, as SV2AGW's *AGWPE TCP/IP API* describes them.
//!
//! The adapter serves one host at a time (`shared` false), with a connection of its own to the
//! TNC for it.
//!
//! * [`frame`]: AGWPE's frames, written and read.
//! * [`bridge`]: one host and its AGWPE connection, RP-4 §9.4's table, with no input or output.
//! * [`serve`]: the TCP port hosts connect to, and the connection to the TNC.

pub mod bridge;
pub mod frame;
pub mod serve;
