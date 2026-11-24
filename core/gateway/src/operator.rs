// SPDX-License-Identifier: MIT OR Apache-2.0
//! The gateway's operator's business with a post office: registering the gateway and
//! withdrawing its registration (RP-5 §6.4), logged in as the operator with their own client's
//! key and credential: the account they choose among the client's (wishlist W46).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
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

/// Where the client keeps the homes of the callsigns added (its `callsigns` module): under the
/// first home, one directory a number, from 2.
const CALLSIGNS: &str = "callsigns";

/// The file in the first home that names the callsign the client has chosen.
const CHOSEN: &str = "chosen";

/// One account of the RadioPost app on this computer (wishlist W46): the first home, or a
/// callsign added under it, each its own credential and so its own post office.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientAccount {
    /// Its number in the client's list: 1 for the first home.
    pub id: u32,
    /// Its home.
    pub home: PathBuf,
    /// The account's address; its domain is the post office's.
    pub address: String,
    /// The callsign its credential names.
    pub operator: String,
    /// Whether the client has it chosen for its calls.
    pub chosen: bool,
}

impl ClientAccount {
    /// Its post office's domain.
    #[must_use]
    pub fn domain(&self) -> &str {
        self.address.rsplit_once('@').map_or("", |(_, d)| d)
    }

    /// As the panel shows it.
    #[must_use]
    pub fn json(&self) -> Json {
        json!({"id": self.id, "home": self.home.display().to_string(), "address": self.address,
               "operator": self.operator, "domain": self.domain(), "chosen": self.chosen})
    }

    /// How a person names it: `CALLSIGN · ADDRESS`.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{} · {}", self.operator, self.address)
    }
}

/// The accounts the RadioPost app whose first home is `base` holds, enrolled ones only, the
/// first first, as the client lists them.
#[must_use]
pub fn accounts(base: &Path) -> Vec<ClientAccount> {
    let mut added: Vec<u32> = std::fs::read_dir(base.join(CALLSIGNS))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str()?.parse().ok())
        .filter(|&n| n > 1)
        .collect();
    added.sort_unstable();
    let chosen = std::fs::read_to_string(base.join(CHOSEN))
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok())
        .filter(|n| *n == 1 || added.contains(n))
        .unwrap_or(1);
    std::iter::once(1)
        .chain(added)
        .filter_map(|id| {
            let home = if id == 1 {
                base.to_owned()
            } else {
                base.join(CALLSIGNS).join(id.to_string())
            };
            let body = enrolled_as(&home).ok()?;
            Some(ClientAccount {
                id,
                home,
                // in lowercase (RP-0 §6.2), whatever case the credential was issued in
                address: radiopost::address::lowercase(&body.address),
                operator: body.operator,
                chosen: id == chosen,
            })
        })
        .collect()
}

/// The account `who` names among those of the RadioPost app at `base`: its number, its callsign
/// or its address, in any case. With no `who`, the only one there is.
///
/// # Errors
///
/// None enrolled, none or several that `who` names, or several and no `who`.
pub fn account(base: &Path, who: Option<&str>) -> Result<ClientAccount, String> {
    let all = accounts(base);
    let listed = || {
        all.iter().fold(String::new(), |mut s, a| {
            let _ = write!(
                s,
                "
  {}  {}",
                a.id,
                a.label()
            );
            s
        })
    };
    if all.is_empty() {
        return Err(format!(
            "the RadioPost app's home {} holds no enrolled account: enroll it first (its Setup), \
             or give the home of one that is",
            base.display()
        ));
    }
    let Some(who) = who.map(str::trim).filter(|w| !w.is_empty()) else {
        if all.len() == 1 {
            return Ok(all[0].clone());
        }
        return Err(format!(
            "the RadioPost app at {} holds {} accounts: name the one to register with, with \
             --account and its address, callsign or number:{}",
            base.display(),
            all.len(),
            listed()
        ));
    };
    let mut found: Vec<&ClientAccount> = all
        .iter()
        .filter(|a| {
            a.id.to_string() == who
                || a.address.eq_ignore_ascii_case(who)
                || a.operator.eq_ignore_ascii_case(who)
        })
        .collect();
    match found.len() {
        1 => Ok(found.remove(0).clone()),
        0 => Err(format!(
            "no account of the RadioPost app at {} is {who}; it holds:{}",
            base.display(),
            listed()
        )),
        _ => Err(format!(
            "{who} names more than one account: give its address or number:{}",
            listed()
        )),
    }
}

/// What the gateway is registered as now: its credential's account and post office, and, from
/// `record.json`, the post office's base URL and the account's callsign. `None` when it holds no
/// credential.
#[must_use]
pub fn registration(dir: &Path) -> Option<Json> {
    let hex = std::fs::read_to_string(dir.join("credential.hex")).ok()?;
    let body = Credential::decode(&station::unhex(hex.trim())?)?.body;
    let kept = kept_record(dir).unwrap_or(Json::Null);
    let domain = body
        .address
        .rsplit_once('@')
        .map_or("", |(_, d)| d)
        .to_owned();
    // record.json speaks of this registration when it names the same account
    let same = kept["address"].as_str() == Some(body.address.as_str());
    let of = |k: &str| if same { kept[k].clone() } else { Json::Null };
    Some(json!({
        "callsign": body.operator, "address": body.address, "domain": domain,
        "serial": body.serial, "expires": body.expires,
        "post_office": of("post_office"), "operator": of("operator"), "home": of("home"),
    }))
}

/// `record.json` as it stands.
fn kept_record(dir: &Path) -> Option<Json> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(RECORD)).ok()?).ok()
}

/// The record last registered, or withdrawn, with all the operator's channels: what the panel
/// fills the registration's form with.
#[must_use]
pub fn last_record(dir: &Path) -> Option<Json> {
    kept_record(dir)?.get("record").cloned()
}

/// The record as last registered, and the client's home it was registered from, in the gateway's
/// directory: `record` with all the operator's channels, `sent` as the post office has it.
pub const RECORD: &str = "record.json";

/// A record's channels with the scan its modem last answered (RP-4 §4.2, RP-5 §4.4): `scan_s` on
/// each channel the radio scans, none on the others, and the channels it is set to scan but no
/// longer listens on left out, since a gateway lists only the channels it listens on.
pub fn with_scan(record: &mut Json, store: &Store) {
    let Some(channels) = record.get_mut("channels").and_then(Json::as_array_mut) else {
        return;
    };
    channels.retain(|c| {
        !c.get("dial_hz")
            .and_then(Json::as_u64)
            .is_some_and(|hz| store.scan_left.contains(&hz))
    });
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
/// the record would say another scan, or other channels, than the post office has: `None` when it
/// says the same, or the gateway was never registered from this computer.
#[must_use]
pub fn register_again(dir: &Path) -> Option<Result<String, String>> {
    let kept: Json = serde_json::from_str(&std::fs::read_to_string(dir.join(RECORD)).ok()?).ok()?;
    let home = std::path::PathBuf::from(kept["home"].as_str()?);
    let record = kept.get("record")?.clone();
    // a record.json written before `sent` was kept holds what was sent, all its channels
    let sent = kept.get("sent").unwrap_or(&record);
    let mut updated = record.clone();
    with_scan(&mut updated, &Store::load(dir));
    (&updated != sent).then(|| register(dir, &home, record))
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
    if !credential.body.address.contains('@') {
        return Err("the credential has no address".into());
    }
    // in lowercase, as the settings keep post offices' domains
    let domain = credential.body.domain();
    let base = settings
        .post_offices
        .get(&domain)
        .cloned()
        .or_else(|| station::lookup(&settings.resolver, &domain))
        .ok_or_else(|| {
            format!("no post office found for {domain}: name it with `post-office --domain {domain} --url URL`")
        })?;
    let base = base.trim_end_matches('/').to_owned();
    // RP-5 §6.4: one post office at a time, or a third reader finds the callsign ambiguous
    if let Some(now) = registration(dir) {
        let (at, address) = (text(&now, "domain"), text(&now, "address"));
        if !at.eq_ignore_ascii_case(&domain) {
            return Err(format!(
                "{} is registered with {at}'s post office now, for {address}: unregister it \
                 there first (Setup, Unregister; or `radiopost-gateway unregister`), then \
                 register it with {domain}'s",
                settings.callsign
            ));
        }
    }
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
    // all the operator's channels are kept, to publish again when a scan of them runs again
    let all = record.clone();
    with_scan(&mut record, &Store::load(dir));
    let none = |r: &Json| r["channels"].as_array().is_none_or(Vec::is_empty);
    if none(&record) && !none(&all) {
        return Err(
            "the radio is on none of the gateway's channels now: register it with the channel \
             it listens on"
                .into(),
        );
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
            let kept = json!({
                "home": home.to_string_lossy(), "address": credential.body.address,
                "operator": credential.body.operator, "domain": domain, "post_office": base,
                "record": all, "sent": record,
            });
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

fn text<'a>(v: &'a Json, key: &str) -> &'a str {
    v.get(key).and_then(Json::as_str).unwrap_or("")
}

/// Withdraws the gateway's registration at its post office (RP-5 §6.4), logged in as the account
/// it was registered from, and forgets it here ([`forget`]). Refused while sessions it carried
/// wait to be reported: they hold users' mail, and with its credential revoked it could not hand
/// them in. A post office that answers `404` is asked for its gateway list: one that still lists
/// the gateway takes no withdrawals (RP-5 before 1.2.0), and its operators remove it.
///
/// # Errors
///
/// Not registered, sessions waiting, the account not on this computer, the post office not
/// found, not reached, or refusing.
pub fn unregister(dir: &Path) -> Result<String, String> {
    let settings = Settings::load(dir)?;
    let now = registration(dir).ok_or_else(|| {
        format!(
            "{} is not registered: it holds no credential",
            settings.callsign
        )
    })?;
    // the callsign registered, as its credential names it, though the settings say another now
    let callsign = text(&now, "callsign").to_owned();
    let waiting = Store::load(dir).records.len();
    if waiting > 0 {
        return Err(format!(
            "{waiting} session{} carried while cut off still wait{} to be reported: they hold \
             users' mail, which the post office would never get once the gateway's credential is \
             revoked. Let the gateway report them (it does while it answers with the Internet), \
             then unregister",
            if waiting == 1 { "" } else { "s" },
            if waiting == 1 { "s" } else { "" }
        ));
    }
    let (domain, address) = (
        text(&now, "domain").to_owned(),
        text(&now, "address").to_owned(),
    );
    let base = now["post_office"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| settings.post_offices.get(&domain).cloned())
        .or_else(|| station::lookup(&settings.resolver, &domain))
        .ok_or_else(|| {
            format!("no post office found for {domain}: name it with `post-office --domain {domain} --url URL`")
        })?;
    let base = base.trim_end_matches('/').to_owned();
    // the account that registered it: the home record.json names, or the client's that has its
    // address
    let home = now["home"]
        .as_str()
        .map(PathBuf::from)
        .filter(|h| enrolled_as(h).is_ok_and(|b| b.address.eq_ignore_ascii_case(&address)))
        .or_else(|| {
            accounts(&client_home())
                .into_iter()
                .find(|a| a.address.eq_ignore_ascii_case(&address))
                .map(|a| a.home)
        })
        .ok_or_else(|| {
            format!(
                "{address}, the account {callsign} was registered from, is not in the RadioPost \
                 app on this computer: enroll it there, or ask {domain}'s operators to remove \
                 {callsign}"
            )
        })?;
    let (user, credential) = user_of(&home)?;
    let mut http = Http::new(&base, Arc::new(user), Arc::new(Mutex::new(None)));
    match http.call("DELETE", &format!("/gateways/{callsign}"), None) {
        Ok((200, answer)) => {
            forget(dir)?;
            let revoked = answer
                .as_ref()
                .and_then(|a| a["revoked"].as_array())
                .map(|r| r.iter().map(Json::to_string).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            Ok(format!(
                "{callsign} is no longer registered with {domain}'s post office ({base}): off its \
                 gateway list, its credential {revoked} revoked, and the copies placed here \
                 dropped"
            ))
        }
        Ok((404, _)) => match listed(&base, &callsign) {
            Some(false) => {
                forget(dir)?;
                Ok(format!(
                    "{callsign} was no longer registered with {domain}'s post office: forgotten \
                     here too"
                ))
            }
            Some(true) => Err(format!(
                "{domain}'s post office keeps {callsign} on its gateway list: it takes no \
                 withdrawal from a gateway's operator (RP-5 1.2.0 §6.4). Ask its operators to \
                 remove it (on their desk, Gateways; or `radiopost-po unregister --callsign \
                 {callsign}`), then unregister here again"
            )),
            None => Err(format!(
                "{domain}'s post office says {callsign} is not registered there, and its gateway \
                 list could not be read to check: try again later"
            )),
        },
        Ok((403, _)) => Err(format!(
            "{domain}'s post office refused: {} ({}) is not the operator {callsign}'s record \
             names",
            credential.body.address, credential.body.operator
        )),
        Ok((status, body)) => Err(format!(
            "the post office refused ({status}): {}",
            body.map(|b| b.to_string()).unwrap_or_default()
        )),
        Err(ErrorCode::PostOfficeUnreachable) => Err(format!("cannot reach {base}")),
        Err(code) => Err(format!("the login as {address} failed: {}", code.name())),
    }
}

/// Whether the post office at `base` lists `callsign` in its gateway list (RP-5 §4.4); `None`
/// when the list cannot be read. Read without checking its signature: it only decides whether
/// the gateway forgets a registration the post office says it does not hold.
fn listed(base: &str, callsign: &str) -> Option<bool> {
    let list = crate::keys::unchecked_payload(&crate::keys::get(base, "/gateways")?)?;
    Some(
        list["gateways"]
            .as_array()?
            .iter()
            .any(|g| text(g, "callsign").eq_ignore_ascii_case(callsign)),
    )
}

/// Forgets the gateway's registration here, the post office not asked: `credential.hex` gone,
/// `record.json` keeping the record (all the operator's channels, to register again from) and
/// where it was withdrawn, and the copies placed ahead, forward items and local mail it held
/// dropped, with its polls, since no post office places at it now (RP-5 §5.8). `unregister`
/// does this once the post office has withdrawn it; `unregister --here` alone, for a post office
/// that cannot be reached, whose operators remove the record.
///
/// # Errors
///
/// A file could not be written or removed.
pub fn forget(dir: &Path) -> Result<(), String> {
    let now = registration(dir);
    match std::fs::remove_file(dir.join("credential.hex")) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(format!("credential.hex: {e}"));
        }
        _ => {}
    }
    match last_record(dir) {
        Some(record) => {
            let kept = json!({
                "record": record,
                "withdrawn": {
                    "at": station::now(),
                    "domain": now.as_ref().map(|n| n["domain"].clone()),
                    "address": now.as_ref().map(|n| n["address"].clone()),
                },
            });
            let kept = serde_json::to_string_pretty(&kept).map_err(|e| e.to_string())?;
            station::write(dir, RECORD, &kept)?;
        }
        None => {
            let _ = std::fs::remove_file(dir.join(RECORD));
        }
    }
    crate::store::Shared::open(dir).change(|s| {
        s.copies.clear();
        s.forwards.clear();
        s.forward_serials.clear();
        s.local.clear();
        s.copies_seen.clear();
        s.polls.clear();
    })
}
