// SPDX-License-Identifier: MIT OR Apache-2.0
//! RadioPost's objects, as the specifications define them: deterministic CBOR (RP-0 §4.1),
//! signatures, key IDs and content IDs (RP-0 §5, §6.3), messages and their air forms (RP-2),
//! credentials (RP-1 §4, §5), session statements (RP-1 §6), and PDU framing (RP-3 §2, §4).
//!
//! Every type here is checked against the test vectors in `vectors/` (`tests/vectors.rs`). Where
//! this library and a specification disagree, the specification is right.

pub mod cbor;
pub mod credential;
pub mod crypto;
pub mod error;
pub mod message;
pub mod pdu;
pub mod statement;

pub use cbor::Value;
pub use crypto::ContentId;
pub use error::ErrorCode;
