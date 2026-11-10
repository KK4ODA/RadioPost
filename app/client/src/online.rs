// SPDX-License-Identifier: MIT OR Apache-2.0
//! The client and its post office over the Internet (RP-5): enrolling a device with a one-time
//! code (§6.1), renewing the credential (§6.2), the settings (§6.3) and the accept list (§6.5),
//! and the station-to-station sessions reported (§6.6), when the user has the Internet.
//!
//! The post office's API is the one the home names (`post_office`), or the one its domain's DNS
//! records name (RP-1 §7.1). Its key is trusted as `trust` added it, or from the same records.

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use ed25519_dalek::SigningKey;
use radiopost::credential::PostOfficeKey;
use radiopost_api::{Account, Http, RESOLVER, code_of};
use serde_json::{Value, json};

use crate::store::{Error, Store};

/// The domain of an address.
fn domain_of(address: &str) -> String {
    address
        .rsplit_once('@')
        .map_or(address, |(_, d)| d)
        .to_lowercase()
}

/// The API base URL of the post office for `domain`: the home's, or DNS's.
///
/// # Errors
///
/// None set and none found in DNS.
pub fn post_office(store: &Store, domain: &str) -> Result<String, Error> {
    if let Some(url) = &store.post_office {
        return Ok(url.clone());
    }
    radiopost_api::lookup(RESOLVER, domain).ok_or_else(|| {
        format!(
            "no post office found for {domain} in DNS: name it with `radiopost post-office URL`"
        )
    })
}

/// The post office's keys from DNS, trusted when the home trusts none for the domain yet
/// (RP-1 §7.1, §7.3). Returns how many were added.
fn trust_from_dns(store: &Store, domain: &str) -> usize {
    if store
        .trust()
        .is_ok_and(|t| t.keys.get(domain).is_some_and(|k| !k.is_empty()))
    {
        return 0;
    }
    let keys = radiopost_api::keys(RESOLVER, domain);
    for public in &keys {
        let _ = store.add_trust(
            domain,
            &PostOfficeKey {
                public: *public,
                not_before: 0,
                not_after: u64::MAX,
            },
        );
    }
    keys.len()
}

/// RP-5 §6.1: enrolls this device (its key, made now if there is none) for `address` and
/// `operator` with the operator's one-time code, keeps the credential, and remembers where the
/// post office is. Returns what to tell the user.
///
/// # Errors
///
/// No post office found, or it refused the code.
pub fn enroll(
    store: &mut Store,
    url: Option<&str>,
    address: &str,
    operator: &str,
    code: &str,
) -> Result<String, Error> {
    let domain = domain_of(address);
    let base = match url {
        Some(u) => u.trim_end_matches('/').to_owned(),
        None => post_office(store, &domain)?,
    };
    let key = if let Ok(k) = store.device() {
        k
    } else {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|e| e.to_string())?;
        let k = SigningKey::from_bytes(&seed);
        store.set_device(&k)?;
        k
    };
    let credential = radiopost_api::enroll(&base, address, &operator.to_uppercase(), code, &key)?;
    let body = store.set_credential(&credential)?;
    store.post_office = Some(base.clone());
    store.save()?;
    let added = trust_from_dns(store, &domain);
    let trusted = store
        .trust()
        .is_ok_and(|t| t.keys.get(&domain).is_some_and(|k| !k.is_empty()));
    let mut said = format!(
        "{} ({}) enrolled at {base}, credential {}",
        body.address, body.operator, body.serial
    );
    if added > 0 {
        let _ = write!(said, "; {domain}'s key trusted from DNS");
    } else if !trusted {
        let _ = write!(
            said,
            "; trust {domain}'s key with the line its operator gives (`radiopost trust …`)"
        );
    }
    Ok(said)
}

/// The API as this user, logged in with the home's credential.
fn http(store: &Store) -> Result<Http, Error> {
    let credential = store.credential()?;
    let me = store.me()?;
    let base = post_office(store, &domain_of(&me.address))?;
    Ok(Http::new(
        &base,
        Arc::new(Account {
            credential,
            key: store.device()?,
        }),
        Arc::new(Mutex::new(None)),
    ))
}

/// One request as this user: the body of a 2xx answer, or why not.
fn ask(store: &Store, method: &str, path: &str, body: Option<&Value>) -> Result<Value, Error> {
    let mut api = http(store)?;
    match api.call(method, path, body) {
        Ok((status, answer)) if status < 300 => Ok(answer.unwrap_or(Value::Null)),
        Ok((status, answer)) => Err(format!(
            "the post office refused ({status} {})",
            code_of(answer.as_ref()).name()
        )),
        Err(code) => Err(format!("the post office: {}", code.name())),
    }
}

/// RP-5 §6.2: a new credential for this device, kept.
///
/// # Errors
///
/// The post office could not be reached or refused.
pub fn renew(store: &Store) -> Result<String, Error> {
    let answer = ask(store, "POST", "/credentials", Some(&json!({})))?;
    let bytes = answer
        .get("credential")
        .and_then(Value::as_str)
        .and_then(radiopost::base64url::decode)
        .ok_or("the post office's answer held no credential")?;
    let body = store.set_credential(&bytes)?;
    Ok(format!(
        "credential {} for {} ({})",
        body.serial, body.address, body.operator
    ))
}

/// RP-5 §6.3: the settings, after `change`'s members are set (a member set to `null` is
/// removed). `PUT` replaces the settings whole, so the change is merged into what is there.
///
/// # Errors
///
/// The post office could not be reached or refused.
pub fn settings(store: &Store, change: Option<&Value>) -> Result<Value, Error> {
    let now = ask(store, "GET", "/settings", None)?;
    let Some(change) = change else {
        return Ok(now);
    };
    let mut merged = now.as_object().cloned().unwrap_or_default();
    for (k, v) in change.as_object().ok_or("a change is a JSON object")? {
        if v.is_null() {
            merged.remove(k);
        } else {
            merged.insert(k.clone(), v.clone());
        }
    }
    ask(store, "PUT", "/settings", Some(&Value::Object(merged)))?;
    ask(store, "GET", "/settings", None)
}

/// RP-5 §6.5: the accept list, after adding and removing entries.
///
/// # Errors
///
/// The post office could not be reached or refused.
pub fn accept_list(store: &Store, add: &[String], remove: &[String]) -> Result<Vec<String>, Error> {
    let answer = if add.is_empty() && remove.is_empty() {
        ask(store, "GET", "/accept-list", None)?
    } else {
        ask(
            store,
            "POST",
            "/accept-list",
            Some(&json!({"add": add, "remove": remove})),
        )?
    };
    Ok(answer
        .get("entries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| e.as_str().map(str::to_owned))
        .collect())
}

/// RP-5 §6.6: the station-to-station sessions waiting, reported oldest first. Each session the
/// post office answered leaves the home; the lines say what came of each.
///
/// # Errors
///
/// The post office could not be reached, or refused the request as a whole: the sessions stay.
pub fn send_reports(store: &mut Store) -> Result<Vec<String>, Error> {
    if store.reports.is_empty() {
        return Ok(Vec::new());
    }
    let answer = ask(
        store,
        "POST",
        "/direct",
        Some(&json!({"sessions": store.reports})),
    )?;
    let results = answer
        .get("sessions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut said = Vec::new();
    for (session, result) in store.reports.iter().zip(&results) {
        let when = session["started"].as_u64().unwrap_or(0);
        if let Some(name) = result.get("name").and_then(Value::as_str) {
            said.push(format!("session at {when}: refused, {name}"));
            continue;
        }
        let statuses: Vec<String> = result
            .get("results")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|r| {
                let id = r["id"].as_str().unwrap_or("?");
                let status = r["status"]
                    .as_str()
                    .or_else(|| r["name"].as_str())
                    .unwrap_or("?");
                format!("{id} {status}")
            })
            .collect();
        said.push(format!("session at {when}: {}", statuses.join(", ")));
    }
    let answered = results.len().min(store.reports.len());
    store.reports.drain(..answered);
    store.save()?;
    Ok(said)
}
