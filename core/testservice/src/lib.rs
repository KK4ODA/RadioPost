// SPDX-License-Identifier: MIT OR Apache-2.0
//! The public test service (wishlist W43): a post office, a gateway and a station that anyone
//! writing their own RadioPost program can reach over the Internet, and a page that shows what
//! crossed in each session and how it was judged.
//!
//! - [`setup`]: the test post office, in a directory of its own with a key of its own, and the
//!   credentials of its two stations.
//! - [`air`]: [`GATEWAY`], a gateway of the test post office, and [`STATION`], a client's
//!   station, answering calls that come through the TCP adapter (RP-4 §9.5).
//! - [`report`]: each session's record and its checks.
//! - [`web`]: the page, enrollment codes, the reports, and the test post office's API.
//!
//! The test post office publishes nothing in DNS, so no real gateway or post office trusts its
//! credentials (RP-1 §7.3): a program under test trusts its key by hand, from the page.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use radiopost_postoffice::serve::Running;

pub mod air;
pub mod report;
pub mod setup;
pub mod web;

/// The test gateway's callsign.
pub const GATEWAY: &str = "RPTEST-1";
/// The test station's callsign: a client answering station-to-station calls (RP-3 §10).
pub const STATION: &str = "RPTEST-2";
/// Where the test post office's API is, under the service's address (RP-5 §1).
pub const API_PATH: &str = "/rp/v1";

/// The service's settings.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Where it keeps the test post office, its stations' keys and the reports.
    pub dir: PathBuf,
    /// The test post office's domain: its accounts are `<callsign>@<domain>`.
    pub domain: String,
    /// The service's address as the world reaches it, such as `https://test.radiopost.org`; the
    /// test post office's API is under it at [`API_PATH`]. Empty: `http://` and where `web`
    /// listens, for a test on one computer.
    pub url: String,
    /// Where it serves HTTP: behind an HTTPS front on a server.
    pub web: String,
    /// Where it takes calls from other TCP adapters (RP-4 §9.5).
    pub air: String,
    /// Plain TCP links instead of TLS: for a test on one computer only.
    pub plain: bool,
}

/// A service started: where it listens.
pub struct Service {
    /// Its page and the test post office's API.
    pub web: SocketAddr,
    /// Where calls come in.
    pub air: SocketAddr,
    /// The SHA-256 fingerprint of its TLS certificate, which callers pin; none when plain.
    pub fingerprint: Option<String>,
    /// The service's address, as [`Settings::url`] gave it or made from `web`.
    pub url: String,
    /// The test post office.
    pub post_office: Arc<Running>,
}

/// Starts the service: the test post office, its stations answering calls, and the page, each on
/// threads of its own.
///
/// # Errors
///
/// A port cannot be taken, or the directory cannot be written.
pub fn start(settings: &Settings) -> Result<Service, String> {
    let server = tiny_http::Server::http(&settings.web)
        .map_err(|e| format!("cannot serve the page at {}: {e}", settings.web))?;
    let web = server
        .server_addr()
        .to_ip()
        .ok_or("the page's address is not an IP address")?;
    let url = if settings.url.trim().is_empty() {
        format!("http://{web}")
    } else {
        settings.url.trim().trim_end_matches('/').to_owned()
    };
    let post_office = setup::post_office(settings, &url)?;
    let stations = setup::Stations::load(settings, &post_office)?;
    let (security, fingerprint) = air::security(settings)?;
    let air = air::start(settings, &post_office, stations, security)?;
    web::serve(
        server,
        web::Page {
            settings: settings.clone(),
            url: url.clone(),
            air,
            fingerprint: fingerprint.clone(),
            post_office: post_office.clone(),
        },
    );
    Ok(Service {
        web,
        air,
        fingerprint,
        url,
        post_office,
    })
}

/// Now, Unix seconds.
#[must_use]
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Random bytes from the system.
///
/// # Panics
///
/// The system has no random numbers to give.
#[must_use]
pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("the system's random numbers");
    b
}
