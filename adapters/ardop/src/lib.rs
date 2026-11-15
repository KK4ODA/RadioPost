// SPDX-License-Identifier: MIT OR Apache-2.0
//! An RPMI adapter (RP-4) for an ARDOP TNC (ardopcf and the others with its host interface), after
//! RP-4 §9.3. It talks to the TNC over TCP: commands and replies as lines ended by CR on its
//! command port (8515 by default), the session's bytes in blocks on the next port.
//!
//! The TNC serves one program, so the adapter serves one host (`shared` false) and connects to
//! the TNC only while it has one. The server is the Mercury adapter's ([`listen`]), which serves
//! any bridge of that shape.
//!
//! * [`bridge`]: one host and the TNC's two ports, RP-4 §9.3's table, with no input or output.

pub mod bridge;

use std::io;
use std::net::{SocketAddr, ToSocketAddrs};

use radiopost_mercury::serve::{Kind, Note, listen_tnc};

/// ARDOP's command port when none is named; its data port is the next.
pub const DEFAULT_ARDOP: &str = "127.0.0.1:8515";

/// Listens for hosts and serves them, one at a time, through the ARDOP TNC whose command port is
/// at `ardop`. Returns the address taken; the adapter runs on threads of its own.
///
/// # Errors
///
/// The address could not be taken.
pub fn listen(addr: impl ToSocketAddrs, ardop: &str, note: Note) -> io::Result<SocketAddr> {
    let kind = Kind {
        name: "ARDOP",
        fresh: Box::new(bridge::Bridge::new),
        unreached: Box::new(bridge::Bridge::without_tnc),
    };
    listen_tnc(addr, ardop, kind, note)
}
