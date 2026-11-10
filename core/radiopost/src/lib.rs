// SPDX-License-Identifier: MIT OR Apache-2.0
//! RadioPost's objects, as the specifications define them: deterministic CBOR (RP-0 §4.1),
//! signatures, key IDs and content IDs (RP-0 §5, §6.3), messages and their air forms (RP-2),
//! credentials (RP-1 §4, §5), session statements (RP-1 §6), PDU framing (RP-3 §2, §4), and the
//! session protocol's engines (RP-3).
//!
//! Every type here is checked against the test vectors in `vectors/` (`tests/vectors.rs`). Where
//! this library and a specification disagree, the specification is right.

pub mod base64url;
pub mod cbor;
pub mod credential;
pub mod crypto;
pub mod error;
pub mod jws;
pub mod message;
pub mod pdu;
pub mod session;
pub mod statement;

pub use cbor::Value;
pub use crypto::ContentId;
pub use error::ErrorCode;
