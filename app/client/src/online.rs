// SPDX-License-Identifier: MIT OR Apache-2.0
//! The client and its post office over the Internet (RP-5): enrolling a device with a one-time
//! code (§6.1), renewing the credential (§6.2), the settings (§6.3) and the accept list (§6.5),
//! the station-to-station sessions reported (§6.6), and the gateways the post office lists with
//! those of every post office it knows (§4.4), when the user has the Internet.
//!
//! The post office's API is the one the home names (`post_office`), or the one its domain's DNS
//! records name (RP-1 §7.1). Its key is trusted as `trust` added it, or from the same records.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use ed25519_dalek::SigningKey;
use radiopost::credential::PostOfficeKey;
use radiopost::{cbor, crypto, gateway_list, jws};
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
    let key = device_key(store)?;
    let credential = radiopost_api::enroll(&base, address, &operator.to_uppercase(), code, &key)?;
    let body = store.set_credential(&credential)?;
    store.post_office = Some(base.clone());
    // a code by post answers the application for the same account: it is done
    if store
        .application
        .as_ref()
        .is_some_and(|a| a["address"].as_str() == Some(body.address.as_str()))
    {
        store.application = None;
    }
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

/// This device's key, made now if there is none: it never leaves the computer.
fn device_key(store: &Store) -> Result<SigningKey, Error> {
    if let Ok(k) = store.device() {
        return Ok(k);
    }
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| e.to_string())?;
    let k = SigningKey::from_bytes(&seed);
    store.set_device(&k)?;
    Ok(k)
}

/// RP-5 §6.7: asks the post office of `address`'s domain for an account, from this device (its
/// key made now if there is none), and keeps the application to ask after. Returns what to tell
/// the user.
///
/// # Errors
///
/// No post office found, or it refused the application.
pub fn apply(
    store: &mut Store,
    url: Option<&str>,
    address: &str,
    operator: &str,
    name: &str,
    contact: &str,
) -> Result<String, Error> {
    let address = address.trim().to_lowercase();
    let operator = operator.trim().to_uppercase();
    let domain = domain_of(&address);
    let base = match url {
        Some(u) => u.trim_end_matches('/').to_owned(),
        None => post_office(store, &domain)?,
    };
    let key = device_key(store)?;
    let answer = radiopost_api::apply(
        &base,
        &address,
        &operator,
        name.trim(),
        contact.trim(),
        &key,
    )?;
    let id = answer["application"].as_str().unwrap_or("").to_owned();
    store.application = Some(json!({
        "id": id, "base": base, "address": address, "operator": operator,
        "name": name.trim(), "contact": contact.trim(), "status": "pending",
        "expires": answer["expires"],
    }));
    store.save()?;
    Ok(format!(
        "Applied for {address} ({operator}) at {base}. The post office's operators decide; \
         this client asks how it stands and takes the credential once it is approved."
    ))
}

/// RP-5 §6.7: asks where the application stands. Approved, the credential is kept, the post
/// office remembered and its key trusted from DNS, and the application set aside; declined or
/// sent by postcard, the application keeps what the post office said. Returns the application,
/// with `said`, what to tell the user.
///
/// # Errors
///
/// No application waiting, or no answer.
pub fn check_application(store: &mut Store) -> Result<Value, Error> {
    let mut app = store.application.clone().ok_or("no application waiting")?;
    let base = app["base"].as_str().unwrap_or("").to_owned();
    let id = app["id"].as_str().unwrap_or("").to_owned();
    let answer = match radiopost_api::application(&base, &id) {
        Ok(a) => a,
        Err(e) if e.contains("404") => json!({"status": "forgotten"}),
        Err(e) => return Err(e),
    };
    let status = answer["status"].as_str().unwrap_or("pending").to_owned();
    app["status"] = json!(status);
    let said = match status.as_str() {
        "approved" => {
            let bytes = answer["credential"]
                .as_str()
                .and_then(radiopost::base64url::decode)
                .ok_or("the post office's answer held no credential")?;
            let body = store.set_credential(&bytes)?;
            store.post_office = Some(base.clone());
            store.application = None;
            store.save()?;
            trust_from_dns(store, &domain_of(&body.address));
            format!(
                "Approved: {} ({}), credential {}. Welcome to RadioPost.",
                body.address, body.operator, body.serial
            )
        }
        "declined" => {
            app["text"] = answer["text"].clone();
            format!(
                "Declined: {}",
                answer["text"]
                    .as_str()
                    .unwrap_or("the post office gave no reason")
            )
        }
        "postcard" => "The post office is sending a code by post to the address on your licence. \
                       When the card comes, enroll with its code."
            .to_owned(),
        "forgotten" => "The post office no longer knows this application: apply again.".to_owned(),
        _ => "Still waiting for the post office's operators.".to_owned(),
    };
    if status != "approved" {
        store.application = Some(app.clone());
        store.save()?;
    }
    app["said"] = json!(said);
    Ok(app)
}

/// RP-5 §7, and what the post office adds to it: one request under `/office/` as this device's
/// credential, sharing `token` between requests. Returns the status and the body, so that the
/// caller shows a refusal as the post office gave it.
///
/// # Errors
///
/// No credential, a path outside `/office/`, or no answer.
pub fn office(
    store: &Store,
    token: &Arc<Mutex<Option<String>>>,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<(u16, Value), Error> {
    if !path.starts_with("/office/") || path.contains("..") {
        return Err(format!("{path} is not an operator's request"));
    }
    if !matches!(method, "GET" | "POST" | "PUT" | "DELETE") {
        return Err(format!("no {method}"));
    }
    let me = store.me()?;
    let base = post_office(store, &domain_of(&me.address))?;
    let mut api = Http::new(
        &base,
        Arc::new(Account {
            credential: store.credential()?,
            key: store.device()?,
        }),
        Arc::clone(token),
    );
    match api.call(method, path, body) {
        Ok((status, answer)) => Ok((status, answer.unwrap_or(Value::Null))),
        Err(code) => Err(format!("the post office: {}", code.name())),
    }
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

/// A document from a post office's API, as text.
fn fetch(api: &str, path: &str) -> Result<String, Error> {
    let http = Http::anonymous(api);
    match http.send_text("GET", path, None, None) {
        Ok((200, text)) => Ok(text.trim().to_owned()),
        Ok((status, _)) => Err(format!("{api}{path} answered {status}")),
        Err(_) => Err(format!("cannot reach {api}")),
    }
}

/// RP-5 §4.4: the gateway list of each post office the home's `/post-offices` names (`known`),
/// each API asked once, into `lists` by domain. One that does not answer keeps its last list; one
/// that does not verify is dropped; each says why in `errors`. Those no longer named are dropped.
fn other_lists(
    home: &str,
    known: &Value,
    at: u64,
    lists: &mut serde_json::Map<String, Value>,
    errors: &mut serde_json::Map<String, Value>,
) {
    let entries = gateway_list::entries(known);
    let asked: Vec<&gateway_list::Entry> = gateway_list::by_api(&entries)
        .into_iter()
        .filter(|e| e.domain != home)
        .collect();
    lists.retain(|d, _| asked.iter().any(|e| &e.domain == d));
    for entry in asked {
        let text = match fetch(&entry.api, "/gateways") {
            Ok(text) => text,
            Err(why) => {
                errors.insert(entry.domain.clone(), json!(why));
                continue;
            }
        };
        let info = fetch(&entry.api, "/info").ok();
        match gateway_list::verify_list(entry, info.as_deref(), &text) {
            Ok(records) => {
                lists.insert(
                    entry.domain.clone(),
                    json!({"api": entry.api, "fetched": at, "gateways": records}),
                );
            }
            Err(code) => {
                lists.remove(&entry.domain);
                errors.insert(
                    entry.domain.clone(),
                    json!(format!(
                        "its gateway list is not signed by its keys ({})",
                        code.name()
                    )),
                );
            }
        }
    }
}

/// RP-5 §4.4: the gateways the user's post office lists, and those of every post office it names
/// in `/post-offices`. The home's list is checked with the keys the home trusts for the domain and
/// those its signed info document adds (RP-1 §7.2); each other list with the keys its entry gives,
/// or that its own info document adds, signed by one of them. Each API is asked once; a list that
/// does not come keeps the last one checked, and one that does not verify is left out. The home's
/// record wins a callsign; one listed by two others is kept from each, and named ambiguous. When
/// the home says `gateways: "listed"`, only its own are taken: it lets no other gateway in (§2).
/// Kept in the home, for the radio when there is no Internet. Returns how many it lists.
///
/// # Errors
///
/// No identity or post office, no answer, or a list the post office's keys did not sign.
pub fn gateways(store: &mut Store) -> Result<usize, Error> {
    let domain = domain_of(&store.me()?.address);
    let base = post_office(store, &domain)?;
    trust_from_dns(store, &domain);
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let trusted: HashMap<String, [u8; 32]> = store
        .trust()?
        .keys
        .get(&domain)
        .into_iter()
        .flatten()
        .filter(|k| k.not_before <= at && at <= k.not_after)
        .map(|k| (cbor::hex(&crypto::kid(&k.public)), k.public))
        .collect();
    if trusted.is_empty() {
        return Err(format!(
            "this home trusts no key of {domain}'s post office: add it in Setup (`radiopost trust`)"
        ));
    }
    let info = fetch(&base, "/info").ok();
    let keys = jws::accepted_keys(&trusted, info.as_deref());
    let document = jws::verify(&fetch(&base, "/gateways")?, &keys).map_err(|code| {
        format!(
            "the gateway list is not signed by {domain}'s post office ({})",
            code.name()
        )
    })?;
    let home_gateways = document["gateways"].as_array().cloned().unwrap_or_default();
    let listed = info
        .as_deref()
        .and_then(|i| jws::verify(i, &keys).ok())
        .is_some_and(|i| i["gateways"] == "listed");
    // the lists last checked, kept for a post office that does not answer now
    let mut lists: serde_json::Map<String, Value> = store
        .gateway_list
        .as_ref()
        .and_then(|l| l["lists"].as_object().cloned())
        .unwrap_or_default();
    let mut errors = serde_json::Map::new();
    if listed {
        lists.clear();
    } else if let Some(known) = fetch(&base, "/post-offices")
        .ok()
        .and_then(|t| jws::verify(&t, &keys).ok())
    {
        other_lists(&domain, &known, at, &mut lists, &mut errors);
    } else {
        errors.insert(
            domain.clone(),
            json!("its list of post offices did not come, or is not signed by its keys"),
        );
    }
    let others: Vec<(String, Vec<Value>)> = lists
        .iter()
        .map(|(d, l)| {
            (
                d.clone(),
                l["gateways"].as_array().cloned().unwrap_or_default(),
            )
        })
        .collect();
    let merged = gateway_list::merge(&domain, &home_gateways, &others);
    let count = merged.gateways.len();
    store.gateway_list = Some(json!({
        "domain": domain, "fetched": at, "issued": document["issued"],
        "gateways": merged.gateways, "ambiguous": merged.ambiguous,
        "lists": lists, "errors": errors,
    }));
    store.save()?;
    Ok(count)
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
