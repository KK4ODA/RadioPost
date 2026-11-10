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

/// Registers the gateway in `dir` with the post office of the user whose client's home is
/// `home`, with `record`'s fields (channels, grid, place, hours) beside its callsign and
/// operator, and keeps the credential the post office issues (`credential.hex`).
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
    record["callsign"] = settings.callsign.clone().into();
    record["operator"] = credential.body.operator.clone().into();
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
