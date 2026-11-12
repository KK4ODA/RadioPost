// SPDX-License-Identifier: MIT OR Apache-2.0
//! A RadioPost post office's service (design §5.3): the RP-5 API ([`office`]), its public
//! documents ([`documents`]), and the HTTP service around them ([`serve`]).

pub mod aftercare;
pub mod auth;
pub mod check;
pub mod desk;
pub mod documents;
pub mod email;
pub mod evidence;
pub mod licences;
pub mod lookups;
pub mod mail;
pub mod mailboxes;
pub mod notices;
pub mod office;
pub mod operator;
pub mod qrz;
pub mod serve;

pub use office::{Config, Draw, PostOffice, Request, Response, State};
