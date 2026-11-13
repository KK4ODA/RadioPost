// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the gateway does beside answering calls, on threads of their own while it answers:
//!
//! * **Copies placed ahead** (RP-5 §5.8). A gateway that holds copies polls its own post office
//!   without stopping, and each other post office whose last answer said `placing` true; it keeps
//!   each item before it acknowledges the cursor. It asks the other post offices it knows, with
//!   `wait=0`, when it refreshes their documents and after a session of one of their users, at
//!   most once an hour, and starts polling one that answers `placing` true.
//! * **The key cache** (RP-1 §7.3): each post office it knows, refreshed every six hours, and one
//!   learned when one of its users has called.
//! * **Reports** (RP-5 §5.6): the sessions carried while a post office could not be reached,
//!   reported to it oldest first once it can be, every minute until it takes them; what it took is
//!   remembered as passed on for 30 days (RP-7 §3.5).
//!
//! What it does is said on a channel the answering loop reads between calls.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use radiopost::ErrorCode;
use serde_json::{Value as Json, json};

use crate::keys::{self, Source};
use crate::station::{Settings, now};
use crate::store::{REPORTED_KEPT, Reported, Shared};
use crate::upstream::{Account, Http};

/// How long a poll waits for items: under the HTTP agent's 30 seconds (RP-5 §5.8 allows 60).
pub const WAIT_S: u64 = 25;
/// How often a post office it does not poll is asked whether it places items here.
pub const ASK_EVERY_S: u64 = 3600;
/// How often a report is tried again while the post office cannot be reached.
pub const REPORT_EVERY: Duration = Duration::from_secs(60);
/// How long a post office whose documents could not be fetched waits before the next try.
pub const KEYS_RETRY: Duration = Duration::from_secs(300);
/// A session record left open this long (the gateway stopped mid-session) is reported as it is.
pub const OPEN_RECORD_S: u64 = 3600;
/// How many session records go in one report.
pub const REPORT_AT_ONCE: usize = 20;

/// The threads' life: they end when it is dropped.
pub struct Guard {
    alive: Arc<AtomicBool>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}

type Tokens = Arc<Mutex<HashMap<String, Arc<Mutex<Option<String>>>>>>;

#[derive(Clone)]
struct Ctx {
    dir: PathBuf,
    store: Shared,
    alive: Arc<AtomicBool>,
    say: Sender<String>,
    tokens: Tokens,
    polling: Arc<Mutex<HashSet<String>>>,
    /// One report at a time: the manager's and a poller's would read the same records.
    reporting: Arc<Mutex<()>>,
}

impl Ctx {
    fn alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    fn say(&self, line: &str) {
        let _ = self.say.send(line.to_owned());
    }

    fn http(&self, account: &Arc<Account>, api: &str, domain: &str) -> Http {
        let token = self
            .tokens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(domain.to_owned())
            .or_default()
            .clone();
        Http::new(api, account.clone(), token)
    }
}

/// Where a domain's post office is reached: the settings, the key cache, or DNS (RP-1 §7.1), as
/// the operator's choices allow (RP-1 §7.3).
#[must_use]
pub fn api_of(settings: &Settings, store: &Shared, domain: &str) -> Option<String> {
    if settings.blocked.iter().any(|b| b == domain)
        || settings
            .only
            .as_ref()
            .is_some_and(|o| !o.iter().any(|d| d == domain))
    {
        return None;
    }
    if let Some(url) = settings.post_offices.get(domain) {
        return Some(url.clone());
    }
    let cached = store
        .lock()
        .keys
        .get(domain)
        .map(|k| k.found.api.clone())
        .filter(|a| !a.is_empty());
    if cached.is_some() {
        return cached;
    }
    if settings.resolver.is_empty() {
        return None;
    }
    crate::station::lookup(&settings.resolver, domain)
}

/// Starts the gateway's upkeep for the gateway in `dir`, with its store; what it does is said on
/// `say`. It runs until the guard returned is dropped.
#[must_use]
pub fn start(dir: &Path, store: Shared, say: Sender<String>) -> Guard {
    let alive = Arc::new(AtomicBool::new(true));
    let ctx = Ctx {
        dir: dir.to_owned(),
        store,
        alive: alive.clone(),
        say,
        tokens: Arc::default(),
        polling: Arc::default(),
        reporting: Arc::default(),
    };
    thread::spawn(move || manage(&ctx));
    Guard { alive }
}

/// The station's settings and account, as they stand on disk.
fn station(dir: &Path) -> Option<(Settings, Arc<Account>, String, Vec<u8>)> {
    let (settings, account) = crate::station::identity(dir).ok()?;
    let body = radiopost::credential::Credential::decode(&account.credential)?.body;
    let own = body.address.rsplit_once('@')?.1.to_lowercase();
    let credential = account.credential.clone();
    Some((settings, account, own, credential))
}

fn manage(ctx: &Ctx) {
    let mut tried_keys: HashMap<String, Instant> = HashMap::new();
    let mut tried_report: HashMap<String, Instant> = HashMap::new();
    let mut expired = Instant::now()
        .checked_sub(Duration::from_secs(120))
        .unwrap_or_else(Instant::now);
    while ctx.alive() {
        if let Some((settings, account, own, credential)) = station(&ctx.dir) {
            if expired.elapsed() >= Duration::from_secs(60) {
                expired = Instant::now();
                if let Ok(said) = ctx.store.change(|s| s.expire(now())) {
                    for line in said {
                        ctx.say(&line);
                    }
                }
            }
            refresh_keys(ctx, &settings, &own, &credential, &mut tried_keys);
            if settings.copies.is_some() {
                ensure_polling(ctx, &settings, &account, &own);
            }
            report_all(ctx, &settings, &account, &mut tried_report);
        }
        let until = Instant::now() + Duration::from_secs(2);
        while Instant::now() < until && ctx.alive() {
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// RP-1 §7.3: the post offices it knows refreshed every six hours, its own and those whose users
/// called learned; and, holding copies, each other post office asked whether it places items here.
fn refresh_keys(
    ctx: &Ctx,
    settings: &Settings,
    own: &str,
    credential: &[u8],
    tried: &mut HashMap<String, Instant>,
) {
    let at = now();
    let (mut due, learn) = {
        let s = ctx.store.lock();
        let due: Vec<String> = s
            .keys
            .iter()
            .filter(|(_, k)| !k.how.starts_with("list:"))
            .filter(|(_, k)| at.saturating_sub(k.fetched) >= keys::REFRESH_S)
            .map(|(d, _)| d.clone())
            .collect();
        (due, s.learn.clone())
    };
    for d in std::iter::once(own.to_owned()).chain(learn.iter().cloned()) {
        if !ctx.store.lock().keys.contains_key(&d) && !due.contains(&d) {
            due.push(d);
        }
    }
    for domain in due {
        if tried.get(&domain).is_some_and(|t| t.elapsed() < KEYS_RETRY)
            || settings.blocked.contains(&domain)
        {
            continue;
        }
        tried.insert(domain.clone(), Instant::now());
        let source = Source {
            resolver: &settings.resolver,
            url: settings.post_offices.get(&domain).map(String::as_str),
            own_credential: (domain == own).then_some(credential),
        };
        match keys::fetch(&domain, &source, at) {
            Ok(found) => {
                let new = !ctx.store.lock().keys.contains_key(&domain);
                let _ = ctx.store.change(|s| keys::keep(&mut s.keys, found));
                if new {
                    ctx.say(&format!(
                        "{domain}'s post office: keys and revocations kept"
                    ));
                }
                if domain != own && settings.copies.is_some() {
                    ask(ctx, settings, &domain);
                }
            }
            Err(e) => {
                let _ = ctx.store.change(|s| {
                    if let Some(k) = s.keys.get_mut(&domain) {
                        k.error = Some(e.clone());
                    }
                });
            }
        }
    }
    // after a session of one of its users: asked whether it places items here
    if settings.copies.is_some() {
        for domain in learn {
            if domain != own {
                ask(ctx, settings, &domain);
            }
        }
    }
    let _ = ctx.store.change(|s| s.learn.clear());
}

/// RP-5 §5.8: asks a post office it does not poll, with `wait=0`, at most once an hour.
fn ask(ctx: &Ctx, settings: &Settings, domain: &str) {
    let recently = ctx
        .store
        .lock()
        .polls
        .get(domain)
        .is_some_and(|p| p.placing || now().saturating_sub(p.asked) < ASK_EVERY_S);
    if recently {
        return;
    }
    let Some((_, account, own, _)) = station(&ctx.dir) else {
        return;
    };
    if domain == own {
        return;
    }
    let _ = poll_once(ctx, settings, &account, domain, 0);
}

/// A thread polling each post office that should be: its own, and those placing items here.
fn ensure_polling(ctx: &Ctx, settings: &Settings, account: &Arc<Account>, own: &str) {
    let mut wanted: Vec<String> = vec![own.to_owned()];
    wanted.extend(
        ctx.store
            .lock()
            .polls
            .iter()
            .filter(|(d, p)| p.placing && *d != own)
            .map(|(d, _)| d.clone()),
    );
    for domain in wanted {
        let mut polling = ctx.polling.lock().unwrap_or_else(PoisonError::into_inner);
        if !polling.insert(domain.clone()) {
            continue;
        }
        drop(polling);
        let (ctx, settings, account, own) = (
            ctx.clone(),
            settings.clone(),
            account.clone(),
            own.to_owned(),
        );
        thread::spawn(move || {
            poll_while_wanted(&ctx, &settings, &account, &domain, &own);
            ctx.polling
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&domain);
        });
    }
}

fn poll_while_wanted(
    ctx: &Ctx,
    settings: &Settings,
    account: &Arc<Account>,
    domain: &str,
    own: &str,
) {
    let mut said_error: Option<String> = None;
    while ctx.alive() {
        let started = Instant::now();
        match poll_once(ctx, settings, account, domain, WAIT_S) {
            Ok((placing, items)) => {
                if said_error.take().is_some() {
                    ctx.say(&format!(
                        "{domain}'s post office answers the gateway's polls again"
                    ));
                    // reachable again: what waits for it is reported now (RP-5 §5.6)
                    if let Err(e) = report(ctx, settings, account, domain) {
                        ctx.say(&format!("reporting to {domain}'s post office: {e}"));
                    }
                }
                if !placing && domain != own {
                    return; // RP-5 §5.8: it places nothing here; asked again later
                }
                // a post office that answers at once (no long poll) is not asked in a tight loop
                if items == 0 && started.elapsed() < Duration::from_secs(2) {
                    pause(ctx, Duration::from_secs(5));
                }
            }
            Err(e) => {
                if said_error.as_deref() != Some(e.as_str()) {
                    ctx.say(&format!("polling {domain}'s post office for copies: {e}"));
                    said_error = Some(e);
                }
                pause(ctx, Duration::from_secs(15));
            }
        }
    }
}

fn pause(ctx: &Ctx, d: Duration) {
    let until = Instant::now() + d;
    while Instant::now() < until && ctx.alive() {
        thread::sleep(Duration::from_millis(100));
    }
}

/// One poll (RP-5 §5.8): the items kept, then the cursor acknowledged. Returns `placing` and how
/// many items came.
fn poll_once(
    ctx: &Ctx,
    settings: &Settings,
    account: &Arc<Account>,
    domain: &str,
    wait: u64,
) -> Result<(bool, usize), String> {
    let api = api_of(settings, &ctx.store, domain)
        .ok_or_else(|| format!("no post office found for {domain}"))?;
    let cursor = ctx
        .store
        .lock()
        .polls
        .get(domain)
        .map(|p| p.cursor.clone())
        .unwrap_or_default();
    let mut http = ctx.http(account, &api, domain);
    let path = format!("/copies?after={cursor}&wait={wait}");
    let answer = match http.call("GET", &path, None) {
        Ok((200, Some(a))) => a,
        Ok((status, body)) => {
            let code = radiopost_api::code_of(body.as_ref());
            let why = format!("{status} {}", code.name());
            let _ = ctx.store.change(|s| {
                let p = s.polls.entry(domain.to_owned()).or_default();
                p.error = Some(why.clone());
                p.asked = now();
            });
            return Err(why);
        }
        Err(code) => {
            let why = code.name().to_owned();
            let _ = ctx.store.change(|s| {
                let p = s.polls.entry(domain.to_owned()).or_default();
                p.error = Some(why.clone());
                p.asked = now();
            });
            return Err(why);
        }
    };
    let items: Vec<Json> = answer["items"].as_array().cloned().unwrap_or_default();
    let placing = answer["placing"].as_bool().unwrap_or(false);
    let next = answer["cursor"].as_str().unwrap_or("").to_owned();
    let users = settings.copies.as_ref().map(|c| c.users);
    let (kept, dropped) = ctx
        .store
        .change(|s| {
            let done = s.apply(domain, &items, users);
            let p = s.polls.entry(domain.to_owned()).or_default();
            p.api.clone_from(&api);
            p.placing = placing;
            p.asked = now();
            p.error = None;
            done
        })
        .map_err(|e| format!("cannot keep the copies: {e}"))?;
    // RP-5 §5.8: the items are held, so the cursor is acknowledged
    match http.call("POST", "/copies/ack", Some(&json!({"cursor": next}))) {
        Ok((204 | 200, _)) => {
            let _ = ctx
                .store
                .change(|s| s.polls.entry(domain.to_owned()).or_default().cursor = next);
        }
        Ok((status, _)) => return Err(format!("acknowledging the copies: {status}")),
        Err(code) => return Err(format!("acknowledging the copies: {}", code.name())),
    }
    if kept + dropped > 0 {
        let (held, users) = {
            let s = ctx.store.lock();
            (s.copies.len(), s.users().len())
        };
        ctx.say(&format!(
            "copies from {domain}: {kept} kept, {dropped} dropped; {held} held for {users} user{}",
            if users == 1 { "" } else { "s" }
        ));
    }
    Ok((placing, items.len()))
}

/// RP-5 §5.6: each post office's session records reported, oldest first, once a minute until it
/// takes them.
fn report_all(
    ctx: &Ctx,
    settings: &Settings,
    account: &Arc<Account>,
    tried: &mut HashMap<String, Instant>,
) {
    let at = now();
    let mut by_domain: BTreeMap<String, usize> = BTreeMap::new();
    for r in &ctx.store.lock().records {
        if r.done || at.saturating_sub(r.started) >= OPEN_RECORD_S {
            *by_domain.entry(r.domain.clone()).or_default() += 1;
        }
    }
    for domain in by_domain.keys() {
        if tried
            .get(domain)
            .is_some_and(|t| t.elapsed() < REPORT_EVERY)
        {
            continue;
        }
        tried.insert(domain.clone(), Instant::now());
        if let Err(e) = report(ctx, settings, account, domain) {
            let _ = ctx.store.change(|s| {
                for r in s.records.iter_mut().filter(|r| r.domain == *domain) {
                    r.tries += 1;
                    r.error = Some(e.clone());
                }
            });
        }
    }
}

/// A code that says the post office could not take the report now, not that it refused it.
fn for_now(code: &str) -> bool {
    matches!(
        code,
        "busy" | "internal" | "post-office-unreachable" | "timeout"
    )
}

/// What a post office answered a report (RP-5 §5.6), acted on: each session taken is dropped
/// from the queue and its messages remembered as passed on; one refused for good is dropped and
/// said; one the post office could not take now stays. Returns what to say.
fn take_answer(
    s: &mut crate::store::Store,
    domain: &str,
    ready: &[(String, u64)],
    results: &[Json],
    at: u64,
) -> Vec<String> {
    let mut lines = Vec::new();
    for (key, result) in ready.iter().zip(results) {
        let Some(i) = s
            .records
            .iter()
            .position(|r| (r.gateway_session.clone(), r.started) == *key)
        else {
            continue;
        };
        let record = s.records[i].clone();
        let mut entry = Reported {
            at,
            address: record.address.clone(),
            pickups: record.pickups.len(),
            ..Reported::default()
        };
        if let Some(code) = result["name"].as_str() {
            if for_now(code) {
                s.records[i].error = Some(code.to_owned());
                continue;
            }
            entry.code = Some(code.to_owned());
            lines.push(format!(
                "{domain}'s post office refused the session of {} ({}): {code}; dropped",
                record.operator, record.address
            ));
        }
        for m in result["results"].as_array().into_iter().flatten() {
            if matches!(m["status"].as_str(), Some("accepted" | "duplicate")) {
                entry.taken += 1;
                if let Some(id) = m["id"].as_str() {
                    s.passed_on.insert(id.to_owned(), at);
                }
            } else {
                entry.refused += 1;
                lines.push(format!(
                    "{domain}'s post office refused a message of {}: {}",
                    record.operator,
                    m["name"].as_str().unwrap_or("refused")
                ));
            }
        }
        s.records.remove(i);
        s.reported.push(entry);
    }
    let excess = s.reported.len().saturating_sub(REPORTED_KEPT);
    s.reported.drain(..excess);
    lines
}

/// One report to `domain`'s post office, and what it said acted on.
fn report(
    ctx: &Ctx,
    settings: &Settings,
    account: &Arc<Account>,
    domain: &str,
) -> Result<(), String> {
    let _one = ctx.reporting.lock().unwrap_or_else(PoisonError::into_inner);
    let at = now();
    let ready: Vec<(String, u64)> = ctx
        .store
        .lock()
        .records
        .iter()
        .filter(|r| r.domain == domain)
        .filter(|r| r.done || at.saturating_sub(r.started) >= OPEN_RECORD_S)
        .take(REPORT_AT_ONCE)
        .map(|r| (r.gateway_session.clone(), r.started))
        .collect();
    if ready.is_empty() {
        return Ok(());
    }
    let sessions: Vec<Json> = {
        let s = ctx.store.lock();
        ready
            .iter()
            .filter_map(|k| {
                s.records
                    .iter()
                    .find(|r| (r.gateway_session.clone(), r.started) == *k)
                    .map(crate::store::Record::report)
            })
            .collect()
    };
    let api = api_of(settings, &ctx.store, domain)
        .ok_or_else(|| format!("no post office found for {domain}"))?;
    let mut http = ctx.http(account, &api, domain);
    let answer = match http.call("POST", "/reports", Some(&json!({"sessions": sessions}))) {
        Ok((200, Some(a))) => a,
        Ok((status, body)) => {
            return Err(format!(
                "{status} {}",
                radiopost_api::code_of(body.as_ref()).name()
            ));
        }
        Err(ErrorCode::PostOfficeUnreachable) => return Err("not reached".into()),
        Err(code) => return Err(code.name().to_owned()),
    };
    let results = answer["sessions"].as_array().cloned().unwrap_or_default();
    let lines = ctx
        .store
        .change(|s| take_answer(s, domain, &ready, &results, at))
        .map_err(|e| format!("cannot keep what was reported: {e}"))?;
    let taken: usize = results
        .iter()
        .flat_map(|r| r["results"].as_array().cloned().unwrap_or_default())
        .filter(|m| matches!(m["status"].as_str(), Some("accepted" | "duplicate")))
        .count();
    ctx.say(&format!(
        "reported {} session{} to {domain}'s post office: {taken} message{} taken",
        results.len(),
        if results.len() == 1 { "" } else { "s" },
        if taken == 1 { "" } else { "s" }
    ));
    for line in lines {
        ctx.say(&line);
    }
    Ok(())
}
