// SPDX-License-Identifier: MIT OR Apache-2.0
//! The gateway's operator's business with a post office: registering the gateway (RP-5 §6.4),
//! logged in as the operator with their own client's key and credential.

use std::path::Path;
use std::sync::{Arc, Mutex};

use ed25519_dalek::SigningKey;
use radiopost::credential::Credential;
use radiopost::crypto::{self, context};
use radiopost::{ErrorCode, base64url};
use serde_json::{Value as Json, json};

use crate::station::{self, Settings};
use crate::store::Store;
use crate::upstream::{Account, Http};

/// The user's own credential and device key, from their client's home.
fn user_of(home: &Path) -> Result<(Account, Credential), String> {
    let seed: [u8; 32] = std::fs::read_to_string(home.join("device.key"))
        .ok()
        .and_then(|t| station::unhex(&t))
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| format!("{} holds no device key", home.display()))?;
    let bytes = std::fs::read(home.join("credential"))
        .map_err(|_| format!("{} holds no credential", home.display()))?;
    let credential = Credential::decode(&bytes).ok_or("that home's credential is not one")?;
    Ok((
        Account {
            credential: bytes,
            key: SigningKey::from_bytes(&seed),
        },
        credential,
    ))
}

/// The RadioPost app's home on this computer, where its operator's enrollment is:
/// `$RADIOPOST_HOME`, else `.radiopost` in the user's home directory (as the client has it).
#[must_use]
pub fn client_home() -> std::path::PathBuf {
    if let Ok(h) = std::env::var("RADIOPOST_HOME") {
        return h.into();
    }
    let base = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(base).join(".radiopost")
}

/// Who the client's home at `home` is enrolled as: its credential's body.
///
/// # Errors
///
/// It holds no device key or no credential.
pub fn enrolled_as(home: &Path) -> Result<radiopost::credential::Body, String> {
    user_of(home).map(|(_, c)| c.body)
}

/// The record as last registered, and the client's home it was registered from, in the gateway's
/// directory.
pub const RECORD: &str = "record.json";

/// A record's channels with the scan its modem last answered (RP-4 §4.2, RP-5 §4.4): `scan_s` on
/// each channel the radio scans, none on the others.
pub fn with_scan(record: &mut Json, store: &Store) {
    let Some(channels) = record.get_mut("channels").and_then(Json::as_array_mut) else {
        return;
    };
    for c in channels.iter_mut().filter_map(Json::as_object_mut) {
        let scanned = c
            .get("dial_hz")
            .and_then(Json::as_u64)
            .is_some_and(|hz| store.scan_dials.contains(&hz));
        match store.scan_s {
            Some(s) if scanned => {
                c.insert("scan_s".into(), s.into());
            }
            _ => {
                c.remove("scan_s");
            }
        }
    }
}

/// Registers the gateway in `dir` again, from the client's home it was last registered from, when
/// the record says another scan than its modem last answered: `None` when it says the same, or
/// the gateway was never registered from this computer.
#[must_use]
pub fn register_again(dir: &Path) -> Option<Result<String, String>> {
    let kept: Json = serde_json::from_str(&std::fs::read_to_string(dir.join(RECORD)).ok()?).ok()?;
    let home = std::path::PathBuf::from(kept["home"].as_str()?);
    let record = kept.get("record")?.clone();
    let mut updated = record.clone();
    with_scan(&mut updated, &Store::load(dir));
    (updated != record).then(|| register(dir, &home, updated))
}

/// Registers the gateway in `dir` with the post office of the user whose client's home is
/// `home`, with `record`'s fields (channels, grid, place, hours) beside its callsign and
/// operator, and keeps the credential the post office issues (`credential.hex`) and the record
/// (`record.json`). A channel the gateway scans carries the `scan_s` its modem last answered
/// (RP-5 §4.4); a gateway that scans and has not run yet gains it at its first scan, registered
/// again from this computer.
///
/// # Errors
///
/// No post office found for the user's domain, or it could not be reached or refused.
pub fn register(dir: &Path, home: &Path, mut record: Json) -> Result<String, String> {
    let settings = Settings::load(dir)?;
    let gateway = station::key(dir)?;
    let (user, credential) = user_of(home)?;
    let domain = credential
        .body
        .address
        .rsplit_once('@')
        .map(|(_, d)| d.to_owned())
        .ok_or("the credential has no address")?;
    let base = settings
        .post_offices
        .get(&domain)
        .cloned()
        .or_else(|| station::lookup(&settings.resolver, &domain))
        .ok_or_else(|| {
            format!("no post office found for {domain}: name it with `post-office --domain {domain} --url URL`")
        })?;
    let base = base.trim_end_matches('/').to_owned();
    with_scan(&mut record, &Store::load(dir));
    record["callsign"] = settings.callsign.clone().into();
    record["operator"] = credential.body.operator.clone().into();
    // RP-5 §4.4: the record says whether it holds copies placed ahead, and its limits
    match &settings.copies {
        Some(c) => record["copies"] = c.record(),
        None => {
            if let Some(o) = record.as_object_mut() {
                o.remove("copies");
            }
        }
    }
    // RP-7 §5: its windows, those its country's rules let it publish
    let windows: Vec<Json> = settings
        .windows
        .iter()
        .filter(|w| w.refused(&settings.country).is_none())
        .map(station::WindowSettings::record)
        .collect();
    if let Some(o) = record.as_object_mut() {
        if windows.is_empty() {
            o.remove("forwarding");
        } else {
            o.insert("forwarding".into(), Json::Array(windows));
        }
    }
    let mut signed = settings.callsign.clone().into_bytes();
    signed.extend_from_slice(base.as_bytes());
    let signature = crypto::sign(context::API_GATEWAY, &signed, &gateway);
    let body = json!({
        "record": record,
        "device": base64url::encode(gateway.verifying_key().as_bytes()),
        "signature": base64url::encode(&signature),
    });
    let mut http = Http::new(&base, Arc::new(user), Arc::new(Mutex::new(None)));
    match http.call("POST", "/gateways", Some(&body)) {
        Ok((201, Some(answer))) => {
            let bytes = answer
                .get("credential")
                .and_then(Json::as_str)
                .and_then(base64url::decode)
                .ok_or("the post office's answer held no credential")?;
            station::write(dir, "credential.hex", &radiopost::cbor::hex(&bytes))?;
            // what was registered, and from where, so that a change of the scan registers it
            // again as its operator did
            let kept = json!({"home": home.to_string_lossy(), "record": record});
            let kept = serde_json::to_string_pretty(&kept).map_err(|e| e.to_string())?;
            station::write(dir, RECORD, &kept)?;
            Ok(format!(
                "{} registered with {domain}'s post office ({base}) for {}",
                settings.callsign, credential.body.address
            ))
        }
        Ok((status, body)) => Err(format!(
            "the post office refused ({status}): {}",
            body.map(|b| b.to_string()).unwrap_or_default()
        )),
        Err(ErrorCode::PostOfficeUnreachable) => Err(format!("cannot reach {base}")),
        Err(code) => Err(format!("the login failed: {}", code.name())),
    }
}
