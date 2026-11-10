// SPDX-License-Identifier: MIT OR Apache-2.0
//! A RadioPost post office's service (design §5.3): the RP-5 API ([`office`]), its public
//! documents ([`documents`]), and the HTTP service around them ([`serve`]).

pub mod documents;
pub mod email;
pub mod mail;
pub mod office;
pub mod serve;

pub use office::{Config, Draw, PostOffice, Request, Response, State};
