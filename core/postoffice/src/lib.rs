// SPDX-License-Identifier: MIT OR Apache-2.0
//! A RadioPost post office's service (design §5.3): the RP-5 API ([`office`]), its public
//! documents ([`documents`]), and the HTTP service around them ([`serve`]).

pub mod aftercare;
pub mod auth;
pub mod backups;
pub mod check;
pub mod desk;
pub mod documents;
pub mod email;
pub mod evidence;
pub mod health;
pub mod install;
pub mod jobs;
pub mod known;
pub mod licences;
pub mod lookups;
pub mod mail;
pub mod mailboxes;
pub mod notices;
pub mod office;
pub mod operator;
pub mod qrz;
pub mod seen;
pub mod serve;

pub use office::{Config, Draw, PostOffice, Request, Response, State};

use radiopost::message::{Compression, Message};

/// RP-5 §5.3: the air form a post office makes of a message for the radio, once, and serves the
/// same to every gateway and in every copy placed ahead. It never sees the caller's `caps` (RP-3
/// §5.1), so it uses compression 0 or 1, which every station takes (RP-2 §4): DEFLATE when
/// `compress` is on and that gives fewer bytes, otherwise none; never 2 or 3. What it makes is
/// kept and served as it is: an air form already made is never compressed again, so a download
/// resumes on the same bytes (RP-3 §7).
#[must_use]
pub fn air_form(message: &Message, compress: bool) -> Vec<u8> {
    let plain = message.air_form(Compression::None);
    if !compress {
        return plain;
    }
    let deflated = message.air_form(Compression::Deflate);
    if deflated.len() < plain.len() {
        deflated
    } else {
        plain
    }
}
