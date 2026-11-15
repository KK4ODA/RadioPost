// SPDX-License-Identifier: MIT OR Apache-2.0
//! The test post office and its two stations: a key and a directory of the post office's own, the
//! stations' keys kept beside it, their credentials issued again before they run out, and the
//! enrollment codes and welcome mail the page hands out.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use radiopost::cbor::{Value, hex};
use radiopost::credential::{Credential, flags};
use radiopost::message::{Compression, Message, key};
use radiopost_postoffice::documents::Info;
use radiopost_postoffice::serve::{self, Files, Running};
use serde_json::json;

use crate::{API_PATH, GATEWAY, STATION, Settings, now, random};

/// A credential is issued again when it has less than this left (it lasts 90 days, RP-5 §6.1).
const RENEW_WITHIN_S: u64 = 30 * 86_400;

/// The test post office, made in `<dir>/post-office` the first time, its settings written again
/// at every start from `settings` and `url`.
///
/// # Errors
///
/// The directory cannot be written, or the post office's files cannot be read.
pub fn post_office(settings: &Settings, url: &str) -> Result<Arc<Running>, String> {
    let dir = settings.dir.join("post-office");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let files = Files::new(&dir);
    if !dir.join("key.hex").exists() {
        files.write("key.hex", &hex(&random::<32>()))?;
    }
    let domain = settings.domain.to_lowercase();
    let po = serve::Settings {
        domains: vec![domain.clone()],
        base_url: format!("{url}{API_PATH}"),
        listen: "127.0.0.1:0".into(),
        admin: "127.0.0.1:0".into(),
        info: Info {
            operator: GATEWAY.into(),
            contact: format!("postmaster@{domain}"),
            country: "US".into(),
            max_air_bytes: 65_536,
            max_email_bytes: 100_000,
            gateways: "any".into(),
            mail: json!({}),
            updated: now(),
            not_before: 0,
            not_after: u64::MAX,
            post_offices: Vec::new(),
            applications: None,
        },
        known: Vec::new(),
        blocked: Vec::new(),
        resolver: None,
        hooks: None,
        hook_token: None,
        smtp: None,
        operators: Vec::new(),
        lookups: radiopost_postoffice::lookups::LookupSettings::default(),
        mailboxes: radiopost_postoffice::mailboxes::MailboxSettings::default(),
        health: serve::HealthSettings::default(),
        compress: true,
    };
    files.write(
        "config.json",
        &serde_json::to_string_pretty(&po).map_err(|e| e.to_string())?,
    )?;
    Ok(Arc::new(Running::load(&dir, serve::system_draw())?))
}

/// The test post office's public key.
#[must_use]
pub fn public_key(po: &Running) -> [u8; 32] {
    po.office().config.key.verifying_key().to_bytes()
}

/// One of the service's stations: its callsign, its account, its key and its credential.
#[derive(Clone)]
pub struct Station {
    /// Its callsign.
    pub callsign: &'static str,
    /// Its account at the test post office.
    pub address: String,
    /// Its device key.
    pub key: SigningKey,
    /// Its credential.
    pub credential: Vec<u8>,
    /// When the credential runs out.
    pub expires: u64,
    flags: u64,
}

impl Station {
    /// A credential issued again when it has less than 30 days left.
    pub fn fresh(&mut self, po: &Running) {
        if self.expires.saturating_sub(now()) < RENEW_WITHIN_S {
            let device = self.key.verifying_key().to_bytes();
            self.credential =
                po.change(|o| o.issue(&self.address, self.callsign, device, self.flags));
            self.expires = Credential::decode(&self.credential).map_or(0, |c| c.body.expires);
        }
    }
}

/// The gateway and the station.
#[derive(Clone)]
pub struct Stations {
    /// [`GATEWAY`].
    pub gateway: Station,
    /// [`STATION`].
    pub station: Station,
}

impl Stations {
    /// Their keys, made in `<dir>/stations` the first time, and fresh credentials.
    ///
    /// # Errors
    ///
    /// A key cannot be read or written.
    pub fn load(settings: &Settings, po: &Running) -> Result<Stations, String> {
        let dir = settings.dir.join("stations");
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let domain = settings.domain.to_lowercase();
        let station = |callsign: &'static str, flags: u64| -> Result<Station, String> {
            let mut s = Station {
                callsign,
                address: format!("{}@{domain}", callsign.to_lowercase()),
                key: key_in(&dir.join(format!("{}.key", callsign.to_lowercase())))?,
                credential: Vec::new(),
                expires: 0,
                flags,
            };
            s.fresh(po);
            Ok(s)
        };
        Ok(Stations {
            gateway: station(GATEWAY, flags::GATEWAY)?,
            station: station(STATION, flags::SEND | flags::RECEIVE)?,
        })
    }
}

/// A device key kept as hex in `path`, made there the first time.
fn key_in(path: &PathBuf) -> Result<SigningKey, String> {
    if let Ok(text) = std::fs::read_to_string(path) {
        let seed: [u8; 32] = (0..text.trim().len())
            .step_by(2)
            .filter_map(|i| u8::from_str_radix(text.trim().get(i..i + 2)?, 16).ok())
            .collect::<Vec<u8>>()
            .try_into()
            .map_err(|_| format!("{}: not a key", path.display()))?;
        return Ok(SigningKey::from_bytes(&seed));
    }
    let seed = random::<32>();
    std::fs::write(path, hex(&seed)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Whether `text` is a callsign as RP-0 §6.1 writes it: parts of letters and digits joined by
/// `/`, an SSID from 0 to 15, at most 20 characters, and a base with a letter and a digit.
#[must_use]
pub fn is_callsign(text: &str) -> bool {
    let (call, ssid) = text.split_once('-').unwrap_or((text, "0"));
    let parts_ok = call
        .split('/')
        .all(|p| (1..=10).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()));
    let base = radiopost::callsign::base(text);
    text.len() <= 20
        && parts_ok
        && call.split('/').count() <= 3
        && ssid.parse::<u8>().is_ok_and(|n| n <= 15)
        && base.bytes().any(|b| b.is_ascii_digit())
        && base.bytes().any(|b| b.is_ascii_alphabetic())
}

/// An enrollment code (RP-5 §6.1) for `callsign`'s account at the test post office, good for a
/// week, with the account's welcome mail waiting. Returns the code and the account.
///
/// # Errors
///
/// It is not a callsign.
pub fn code_for(po: &Running, domain: &str, callsign: &str) -> Result<(String, String), String> {
    let callsign = callsign.trim().to_uppercase();
    if !is_callsign(&callsign) {
        return Err(format!("{callsign} is not a callsign (RP-0 §6.1)"));
    }
    let address = format!(
        "{}@{}",
        radiopost::callsign::base(&callsign).to_lowercase(),
        domain.to_lowercase()
    );
    let code = new_code();
    let fresh = po.change(|o| {
        let fresh = !o.state.accounts.contains(&address);
        o.add_code(&code, &address, &callsign, flags::SEND | flags::RECEIVE);
        fresh
    });
    if fresh {
        welcome(po, &address, domain);
    }
    Ok((code, address))
}

/// A code like `7Q4F-AZ9K`, from letters and digits that cannot be misread.
fn new_code() -> String {
    const ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let draw = random::<8>();
    let chars: String = draw
        .iter()
        .map(|b| char::from(ALPHABET[usize::from(*b) % ALPHABET.len()]))
        .collect();
    format!("{}-{}", &chars[..4], &chars[4..])
}

/// A message from the service to `to`.
///
/// # Panics
///
/// Never: the service's own fields always make a message (RP-2 §2).
#[must_use]
pub fn message(
    domain: &str,
    to: &str,
    subject: &str,
    body: &str,
    (priority, file): (u64, Option<(&str, Vec<u8>)>),
) -> Message {
    let mut entries = vec![
        (key::V, Value::Uint(1)),
        (
            key::FROM,
            Value::text(format!("{}@{domain}", GATEWAY.to_lowercase()).as_str()),
        ),
        (key::TO, Value::Array(vec![Value::text(to)])),
        (key::DATE, Value::Uint(now())),
        (key::SUBJECT, Value::text(subject)),
        (key::BODY, Value::text(body)),
    ];
    if let Some((name, data)) = file {
        entries.push((
            key::FILES,
            Value::Array(vec![Value::int_map(vec![
                (1, Value::text(name)),
                (2, Value::text("application/octet-stream")),
                (3, Value::bytes(&data)),
            ])]),
        ));
    }
    if priority > 0 {
        entries.push((key::PRIORITY, Value::Uint(priority)));
    }
    Message::from_map(&Value::int_map(entries)).expect("the service's own messages are messages")
}

/// The mail a new account finds waiting, so that its first call to [`GATEWAY`] fetches
/// something: a short note, one with a 6 kB file the caller may leave (RP-3 §5.6), and a priority
/// message (RP-2 §2).
pub fn welcome(po: &Running, address: &str, domain: &str) {
    let file: Vec<u8> = (0u32..6000)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
        .collect();
    let mail = [
        message(
            domain,
            address,
            "Welcome to the RadioPost test service",
            "This message waited for your first call to RPTEST-1. The service's page shows \
             what crossed in each session, and how it was judged.\n",
            (0, None),
        ),
        message(
            domain,
            address,
            "A file to fetch, or to leave",
            "This one carries a 6 kB file, so that a client can fetch it, or ask for the list \
             only and leave it (RP-3 §5.6).\n",
            (0, Some(("test-6kB.bin", file))),
        ),
        message(
            domain,
            address,
            "Priority",
            "A priority message (RP-2 §2): it is listed first.\n",
            (1, None),
        ),
    ];
    po.change(|o| {
        for m in &mail {
            let serial = o.next_mailbox_serial(address);
            let subject = m
                .map()
                .get(key::SUBJECT)
                .and_then(Value::as_text)
                .unwrap_or_default()
                .to_owned();
            let priority = m
                .map()
                .get(key::PRIORITY)
                .and_then(Value::as_u64)
                .unwrap_or(0);
            o.deliver(
                address,
                serial,
                &m.air_form(Compression::Deflate),
                m.from().unwrap_or_default(),
                &subject,
                priority,
                false,
            );
        }
    });
}

/// The directory the reports are kept in.
#[must_use]
pub fn runs_dir(dir: &Path) -> PathBuf {
    dir.join("runs")
}

#[cfg(test)]
mod tests {
    use super::{is_callsign, new_code};

    #[test]
    fn callsigns_are_read_as_rp0_writes_them() {
        for good in ["KK4ODA", "VE3/KK4ODA/P", "KK4ODA-15", "N0CALL-1", "W1AW"] {
            assert!(is_callsign(good), "{good}");
        }
        for bad in [
            "",
            "KK4ODA-16",
            "RPTEST",
            "12345",
            "KK4ODA/",
            "A/B/C/KK4ODA",
            "kk 4oda",
        ] {
            assert!(!is_callsign(bad), "{bad}");
        }
        let code = new_code();
        assert_eq!((code.len(), &code[4..5]), (9, "-"));
    }
}
