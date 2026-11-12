// SPDX-License-Identifier: MIT OR Apache-2.0
//! An RPMI adapter (RP-4) for the Mercury HF modem (Rhizomatica's, GPL-3.0), after RP-4 §9.6. It
//! talks to Mercury over TCP only, through the VARA-style TNC interface Mercury's `docs/TNC.md`
//! describes: text commands on a control port, the session's bytes on a data port.
//!
//! Mercury serves one program at a time, and gives its control port to the newest that connects,
//! so the adapter serves one host (`shared` false) and connects to Mercury only while it has one.
//!
//! * [`bridge`]: one host and Mercury's two ports, RP-4 §9.6's table, with no input or output.
//! * [`serve`]: the TCP port hosts connect to, and the connections to Mercury.

pub mod bridge;
pub mod serve;
