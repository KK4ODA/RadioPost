// SPDX-License-Identifier: MIT OR Apache-2.0
//! A post office's API (RP-5): one domain's mailboxes, accounts, gateways and sessions.
//!
//! [`PostOffice::handle`] takes a request at the post office's clock and returns the status and
//! JSON body RP-5 says. It does no input or output and draws its random values through
//! [`Draw`], so the vectors' published randomness replays it exactly; the service (`serve`) brings
//! the clock, real random numbers and the network, and saves [`State`] after each change.

use std::collections::{BTreeMap, BTreeSet};

use ed25519_dalek::SigningKey;
use radiopost::ErrorCode;
use radiopost::base64url;
use radiopost::cbor;
use radiopost::credential::{self, Body, Credential, Trust, flags};
use radiopost::crypto::{self, ContentId, context};
use radiopost::message::{Compression, Message, key};
use radiopost::statement::{self, Freshness, REPLAY_S, Statement, TIME_WINDOW_S};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json, json};

/// How long a session lasts (RP-5 §5.2).
pub const SESSION_S: u64 = 3600;
/// How long a token lasts, at most (RP-5 §3.2).
pub const TOKEN_S: u64 = 3600;
/// How long a login nonce is good for (RP-5 §3.1).
pub const NONCE_S: u64 = 300;
/// How long a credential lasts (RP-1 §4.2).
pub const CREDENTIAL_S: u64 = 90 * 86_400;
/// How long an enrollment code is good for (RP-5 §7: within days).
pub const CODE_S: u64 = 7 * 86_400;

/// Where a post office's random values come from: `(what, n)` gives `n` bytes for the named use.
/// The service draws from the system; the vectors' replay from their published stream.
pub type Draw = Box<dyn FnMut(&str, usize) -> Vec<u8> + Send>;

/// What a post office is: its domains, its address on the Internet, its key, and the post offices
/// whose credentials it accepts.
pub struct Config {
    /// The domains it serves.
    pub domains: Vec<String>,
    /// Its API's base URL (RP-5 §1), which logins, enrollments and registrations sign.
    pub base_url: String,
    /// The key that signs its credentials and documents.
    pub key: SigningKey,
    /// The post offices it knows, itself included: their keys and revocations (RP-1 §5, §7.3).
    pub trust: Trust,
}

/// A request: method, path under the base URL with its query, bearer token and JSON body.
#[derive(Clone, Debug)]
pub struct Request {
    /// `GET`, `POST`, `PUT` or `DELETE`.
    pub method: String,
    /// The path under the base URL, with its query.
    pub path: String,
    /// The bearer token, when the request has one.
    pub token: Option<String>,
    /// The JSON body.
    pub body: Option<Json>,
}

/// An answer: its HTTP status and JSON body.
#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    /// The HTTP status.
    pub status: u16,
    /// The JSON body; a problem (RFC 9457) when the status says one.
    pub body: Option<Json>,
}

impl Response {
    fn ok(status: u16, body: Json) -> Response {
        Response {
            status,
            body: Some(body),
        }
    }

    fn empty() -> Response {
        Response {
            status: 204,
            body: None,
        }
    }
}

/// RP-5 §1: the HTTP status of each RP-0 §9.3 code. `not-authorized` is 401 when the request has
/// no valid token (which the caller says), and 403 when the token may not do what is asked.
#[must_use]
pub fn status_of(code: ErrorCode) -> u16 {
    match code {
        ErrorCode::UnsupportedVersion | ErrorCode::Malformed => 400,
        ErrorCode::NotAuthorized | ErrorCode::RefusedByPolicy => 403,
        ErrorCode::UnknownMessage => 404,
        ErrorCode::AlreadyHave => 409,
        ErrorCode::TooLarge => 413,
        ErrorCode::Busy => 429,
        ErrorCode::Internal => 500,
        ErrorCode::PostOfficeUnreachable => 502,
        ErrorCode::Timeout => 504,
        ErrorCode::QuotaExceeded => 507,
        _ => 422,
    }
}

/// RFC 9457 problem details with RP-5 §1's `code` and `name`, at the status given or the code's.
#[must_use]
pub fn problem(code: ErrorCode, status: Option<u16>) -> Response {
    let status = status.unwrap_or_else(|| status_of(code));
    let name = code.to_string();
    let mut title = name.replace('-', " ");
    if let Some(first) = title.get(..1) {
        title = first.to_uppercase() + &title[1..];
    }
    Response::ok(
        status,
        json!({"type": "about:blank", "title": title, "status": status,
               "code": code.code(), "name": name}),
    )
}

fn refused(id: &str, code: ErrorCode, text: &str) -> Json {
    json!({"id": id, "status": "refused", "code": code.code(), "name": code.to_string(),
           "text": text})
}

fn coded(code: ErrorCode) -> Json {
    json!({"code": code.code(), "name": code.to_string()})
}

/// A message in a mailbox, with the air form the post office made for it once (RP-5 §5.3).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Waiting {
    /// Its serial in the mailbox (RP-3 §6.2).
    pub serial: u64,
    /// Its air form, base64url: the same bytes every time (RP-5 §5.3).
    pub air: String,
    /// Its content ID.
    pub id: String,
    /// The sender's address.
    pub from: String,
    /// The subject.
    pub subject: String,
    /// The priority.
    pub priority: u64,
    /// Picked up (RP-5 §5.5).
    #[serde(default)]
    pub picked: bool,
    /// Its sender is not shown to be licensed (RP-6 §5.1).
    #[serde(default)]
    pub third_party: bool,
}

impl Waiting {
    fn size(&self) -> usize {
        base64url::decode(&self.air).map_or(0, |a| a.len())
    }

    fn entry(&self) -> Json {
        json!({"serial": self.serial, "id": self.id, "from": self.from, "subject": self.subject,
               "size": self.size(), "priority": self.priority, "held": false,
               "third_party": self.third_party})
    }
}

/// A one-time enrollment code (RP-5 §6.1).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Code {
    /// The account it is for.
    pub address: String,
    /// The callsign it is for.
    pub operator: String,
    /// The credential's flags (RP-1 §4.1).
    pub flags: u64,
    /// Good until.
    #[serde(default = "never")]
    pub expires: u64,
}

fn never() -> u64 {
    u64::MAX
}

/// Who a token speaks for: the credential it was given for (RP-5 §3.2).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Holder {
    /// The account.
    pub address: String,
    /// The callsign.
    pub operator: String,
    /// The device key, hex.
    pub device: String,
    /// The credential's flags.
    pub flags: u64,
    /// The credential's serial.
    pub serial: u64,
    /// The token's end.
    pub expires: u64,
}

impl Holder {
    fn gateway(&self) -> bool {
        self.flags & flags::GATEWAY != 0
    }

    fn device(&self) -> [u8; 32] {
        let bytes: Vec<u8> = (0..self.device.len())
            .step_by(2)
            .filter_map(|i| u8::from_str_radix(self.device.get(i..i + 2)?, 16).ok())
            .collect();
        bytes.try_into().unwrap_or([0; 32])
    }
}

/// A session a gateway opened for a user (RP-5 §5.2).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    /// The gateway's callsign.
    pub gateway: String,
    /// The user's account.
    pub address: String,
    /// The content IDs the user's statement offers.
    pub offers: Vec<String>,
    /// The serials its `waiting` listed.
    pub waiting: Vec<u64>,
    /// Its end.
    pub expires: u64,
    /// The gateway's RP-3 session ID, for `Radio-Path` (RP-6 §3.2).
    pub gateway_session: String,
    /// How the session came, for the mail it hands in (RP-6 §3.2, §4).
    #[serde(default)]
    pub route: Route,
}

/// How mail handed in by radio came: for its `Radio-Path` (RP-6 §3.2) and the message part
/// between post offices (RP-6 §4).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Route {
    /// The operator's callsign from the session statement.
    pub operator: String,
    /// The gateways, the one called first.
    pub gateways: Vec<String>,
    /// The session's start (`started`).
    pub started: u64,
    /// The gateway's session ID, hex.
    pub session: String,
    /// The proof (RP-1 §6.4): the credential, the statement and its signature, base64url.
    pub credential: String,
    /// The statement.
    pub statement: String,
    /// Its signature.
    pub signature: String,
}

impl Route {
    fn of(body: &Json, operator: &str, gateways: Vec<String>) -> Route {
        Route {
            operator: operator.to_owned(),
            gateways,
            started: body.get("started").and_then(Json::as_u64).unwrap_or(0),
            session: text(body, "gateway_session").to_owned(),
            credential: text(body, "credential").to_owned(),
            statement: text(body, "statement").to_owned(),
            signature: text(body, "signature").to_owned(),
        }
    }
}

/// Mail a user sent that goes on to addresses this post office does not serve (RP-6).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Outgoing {
    /// Its content ID.
    pub id: String,
    /// Its air form, uncompressed, base64url.
    pub air: String,
    /// The recipients it goes to.
    pub to: Vec<String>,
    /// How it came by radio, when it did (RP-6 §3.2, §4).
    #[serde(default)]
    pub route: Option<Route>,
    /// When it was queued, Unix seconds.
    #[serde(default)]
    pub queued: u64,
}

/// Everything a post office keeps between requests.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// The serial of the next credential it issues.
    pub next_serial: u64,
    /// How many random values it has drawn, so each draw is new.
    pub issued: u64,
    /// Each account's mail.
    pub mailboxes: BTreeMap<String, Vec<Waiting>>,
    /// The content IDs of every message users sent that it holds.
    pub had: BTreeSet<String>,
    /// Each account's settings (RP-5 §6.3).
    pub settings: BTreeMap<String, Json>,
    /// Each account's accept list (RP-5 §6.5).
    pub accept: BTreeMap<String, Vec<String>>,
    /// Registered gateways' records, by callsign (RP-5 §6.4).
    pub gateways: BTreeMap<String, Json>,
    /// Copies placed ahead, and drops, by gateway (RP-5 §5.8).
    pub copies: BTreeMap<String, Vec<Json>>,
    /// When each gateway last polled for copies (RP-7 §4.1).
    pub last_poll: BTreeMap<String, u64>,
    /// Enrollment codes not yet used.
    pub codes: BTreeMap<String, Code>,
    /// Login nonces issued, base64url, and when each ends.
    pub challenges: BTreeMap<String, u64>,
    /// Tokens issued.
    pub tokens: BTreeMap<String, Holder>,
    /// Sessions open.
    pub sessions: BTreeMap<String, Session>,
    /// The replay cache: (address, nonce hex, `started`) of each statement taken (RP-1 §6.3).
    pub replay: Vec<(String, String, u64)>,
    /// Content IDs users picked up from a cut-off gateway's local delivery (RP-5 §5.6).
    pub picked_locally: BTreeMap<String, BTreeSet<String>>,
    /// Its own credentials it has revoked, by serial (RP-1 §8).
    pub revoked: Vec<u64>,
    /// Every credential serial issued for each device key, hex (RP-5 §6.2).
    pub serials_of: BTreeMap<String, Vec<u64>>,
    /// Mail waiting to go on by email (RP-6).
    pub outgoing: Vec<Outgoing>,
    /// Every account it has made (RP-6 §2.1: the recipient exists).
    pub accounts: BTreeSet<String>,
    /// Each account's display name, for the email it sends (RP-6 §3.1).
    pub names: BTreeMap<String, String>,
    /// The content ID, hex, of each email it sent from RadioPost, by `Message-ID` (RP-6 §3.1).
    pub sent_ids: BTreeMap<String, String>,
    /// The `Message-ID` of each email it made a message of, by content ID, hex (RP-6 §2.4 step 8).
    pub email_ids: BTreeMap<String, String>,
}

/// A post office: what it is, what it keeps, and where its random values come from.
pub struct PostOffice {
    /// What it is.
    pub config: Config,
    /// What it keeps.
    pub state: State,
    draw: Draw,
    now: u64,
}

fn hex(bytes: &[u8]) -> String {
    cbor::hex(bytes)
}

fn text<'a>(body: &'a Json, k: &str) -> &'a str {
    body.get(k).and_then(Json::as_str).unwrap_or("")
}

fn unb64(body: &Json, k: &str) -> Option<Vec<u8>> {
    base64url::decode(body.get(k)?.as_str()?)
}

/// The domain of an address, after its last `@`.
#[must_use]
pub fn domain_of(address: &str) -> &str {
    address.rsplit_once('@').map_or("", |(_, d)| d)
}

/// RP-6 §2.2: an address entry matches ignoring case; `@domain` matches that domain exactly.
#[must_use]
pub fn on_list(address: &str, entries: &[String]) -> bool {
    let address = address.to_lowercase();
    entries.iter().any(|e| {
        let e = e.to_lowercase();
        e == address || e.strip_prefix('@') == Some(domain_of(&address))
    })
}

/// RP-5 §6.5: an accept list entry, in lowercase: an address, or `@domain`.
fn is_entry(e: &str) -> bool {
    let Some((local, domain)) = e.split_once('@') else {
        return false;
    };
    let labels: Vec<&str> = domain.split('.').collect();
    !local.contains(char::is_whitespace)
        && labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

impl PostOffice {
    /// A post office with this configuration and state, drawing random values from `draw`.
    #[must_use]
    pub fn new(config: Config, state: State, draw: Draw) -> PostOffice {
        PostOffice {
            config,
            state,
            draw,
            now: 0,
        }
    }

    fn fresh(&mut self, what: &str, n: usize) -> Vec<u8> {
        self.state.issued += 1;
        let label = format!("post office {what} {}", self.state.issued);
        (self.draw)(&label, n)
    }

    /// The post offices it knows, with its own revocations.
    #[must_use]
    pub fn trust(&self) -> Trust {
        let mut trust = self.config.trust.clone();
        for domain in &self.config.domains {
            let r = trust.revoked.entry(domain.clone()).or_default();
            r.serials.extend(self.state.revoked.iter().copied());
        }
        trust
    }

    fn ours(&self, address: &str) -> bool {
        self.config.domains.iter().any(|d| d == domain_of(address))
    }

    // -- what happens outside the API ---------------------------------------------------------

    /// Sets the clock, for what happens outside a request.
    pub fn at(&mut self, now: u64) {
        self.now = now;
    }

    /// Mail arrives for a user (RP-6); copies go to the user's usual gateways (RP-5 §5.8).
    #[allow(clippy::too_many_arguments, reason = "a mailbox entry's fields")]
    pub fn deliver(
        &mut self,
        address: &str,
        serial: u64,
        air: &[u8],
        from: &str,
        subject: &str,
        priority: u64,
        third_party: bool,
    ) {
        let id = Message::from_air(air).map_or_else(|_| String::new(), |m| m.content_id().text());
        let waiting = Waiting {
            serial,
            air: base64url::encode(air),
            id,
            from: from.to_owned(),
            subject: subject.to_owned(),
            priority,
            picked: false,
            third_party,
        };
        let usual: Vec<String> = self
            .state
            .settings
            .get(address)
            .and_then(|s| s.get("usual_gateways"))
            .and_then(Json::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|g| g.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        for callsign in usual {
            let Some(record) = self.state.gateways.get(&callsign) else {
                continue;
            };
            let Some(copies) = record
                .get("copies")
                .filter(|c| c.as_object().is_some_and(|o| !o.is_empty()))
            else {
                continue;
            };
            let max = copies
                .get("max_air_bytes")
                .and_then(Json::as_u64)
                .unwrap_or(0);
            let days = copies.get("days").and_then(Json::as_u64).unwrap_or(0);
            let mut item = Map::new();
            item.insert("kind".into(), "copy".into());
            item.insert("address".into(), address.into());
            if let Json::Object(entry) = waiting.entry() {
                for (k, v) in entry {
                    if k != "held" {
                        item.insert(k, v);
                    }
                }
            }
            if waiting.size() as u64 <= max {
                item.insert("air".into(), waiting.air.clone().into());
            }
            item.insert("expires".into(), (self.now + days * 86_400).into());
            self.state
                .copies
                .entry(callsign)
                .or_default()
                .push(Json::Object(item));
        }
        self.state
            .mailboxes
            .entry(address.to_owned())
            .or_default()
            .push(waiting);
    }

    /// The next serial of an account's mailbox.
    #[must_use]
    pub fn next_mailbox_serial(&self, address: &str) -> u64 {
        self.state
            .mailboxes
            .get(address)
            .and_then(|b| b.iter().map(|w| w.serial).max())
            .unwrap_or(0)
            + 1
    }

    /// A new enrollment code for an account (RP-5 §6.1), good for a week.
    pub fn add_code(&mut self, code: &str, address: &str, operator: &str, flags: u64) {
        self.state.accounts.insert(address.to_lowercase());
        self.state.codes.insert(
            code.to_owned(),
            Code {
                address: address.to_owned(),
                operator: operator.to_owned(),
                flags,
                expires: self.now.saturating_add(CODE_S),
            },
        );
    }

    // -- email (RP-6) -------------------------------------------------------------------------

    /// Each account's accept list, by address in lowercase, for the door (RP-6 §2.1): every
    /// account it has made.
    #[must_use]
    pub fn door_accounts(&self) -> BTreeMap<String, Vec<String>> {
        let mut out = BTreeMap::new();
        let made = self
            .state
            .accounts
            .iter()
            .chain(self.state.mailboxes.keys())
            .chain(self.state.accept.keys())
            .chain(self.state.codes.values().map(|c| &c.address));
        for a in made {
            let list = self
                .state
                .accept
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(a))
                .map(|(_, l)| l.clone())
                .unwrap_or_default();
            out.insert(a.to_lowercase(), list);
        }
        out
    }

    /// The RadioPost post offices it knows (RP-1 §7.3): its own domains and those whose
    /// credentials it accepts.
    #[must_use]
    pub fn known_post_offices(&self) -> Vec<String> {
        let mut out: Vec<String> = self.config.domains.clone();
        out.extend(self.config.trust.keys.keys().cloned());
        out.sort();
        out.dedup();
        out
    }

    /// The content ID of an email it sent from RadioPost, by its `Message-ID` (RP-6 §3.1).
    #[must_use]
    pub fn sent_content_id(&self, message_id: &str) -> Option<[u8; 12]> {
        let text = self.state.sent_ids.get(message_id)?;
        let bytes: Vec<u8> = (0..text.len())
            .step_by(2)
            .filter_map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
            .collect();
        bytes.try_into().ok()
    }

    /// The `Message-ID` of the email a message was made of, by its content ID (RP-6 §2.4 step 8).
    #[must_use]
    pub fn email_message_id(&self, content_id: &[u8]) -> Option<String> {
        self.state.email_ids.get(&hex(content_id)).cloned()
    }

    /// An email left: it keeps the `Message-ID` it was sent under (RP-6 §3.1).
    pub fn email_sent(&mut self, message_id: &str, content_id: &ContentId) {
        self.state
            .sent_ids
            .insert(message_id.to_owned(), hex(&content_id.0));
    }

    /// An account's `attach_bytes` (RP-5 §6.3), or the post office's own.
    #[must_use]
    pub fn attach_bytes(&self, address: &str, max_air_bytes: u64) -> u64 {
        self.state
            .settings
            .get(address)
            .and_then(|s| s.get("attach_bytes"))
            .and_then(Json::as_u64)
            .unwrap_or(max_air_bytes)
            .min(max_air_bytes)
    }

    /// A message made from an email (RP-6 §2.4, §3.3, §4) arrives for an account: it gets the
    /// next serial, and the email's `Message-ID` is kept. Returns the serial.
    pub fn email_arrived(
        &mut self,
        address: &str,
        message: &Message,
        third_party: bool,
        message_id: Option<&str>,
    ) -> u64 {
        let air = message.air_form(Compression::None);
        let field = |k| message.map().get(k);
        let subject = field(key::SUBJECT)
            .and_then(cbor::Value::as_text)
            .unwrap_or("")
            .to_owned();
        let priority = field(key::PRIORITY)
            .and_then(cbor::Value::as_u64)
            .unwrap_or(0);
        let from = message.from().unwrap_or("").to_owned();
        let serial = self.next_mailbox_serial(address);
        self.deliver(
            address,
            serial,
            &air,
            &from,
            &subject,
            priority,
            third_party,
        );
        if let Some(id) = message_id {
            self.state
                .email_ids
                .insert(hex(&message.content_id().0), id.to_owned());
        }
        serial
    }

    /// Revokes every credential issued for a device key (RP-1 §8, RP-5 §6.2).
    pub fn revoke_device(&mut self, device: &[u8; 32]) -> Vec<u64> {
        let serials = self
            .state
            .serials_of
            .get(&hex(device))
            .cloned()
            .unwrap_or_default();
        for s in &serials {
            if !self.state.revoked.contains(s) {
                self.state.revoked.push(*s);
            }
        }
        self.state
            .tokens
            .retain(|_, h| !serials.contains(&h.serial));
        serials
    }

    /// Issues a credential (RP-1 §4) for 90 days from now.
    pub fn issue(
        &mut self,
        address: &str,
        operator: &str,
        device: [u8; 32],
        flags: u64,
    ) -> Vec<u8> {
        let serial = self.state.next_serial;
        self.state.next_serial += 1;
        self.state.accounts.insert(address.to_lowercase());
        self.state
            .serials_of
            .entry(hex(&device))
            .or_default()
            .push(serial);
        let body = Body {
            v: 1,
            serial,
            address: address.to_owned(),
            operator: operator.to_owned(),
            device,
            issued: self.now,
            expires: self.now + CREDENTIAL_S,
            flags,
        };
        Credential::issue(&body, &self.config.key).encode()
    }

    // -- requests -----------------------------------------------------------------------------

    /// Answers a request at `now` (RP-5).
    pub fn handle(&mut self, now: u64, request: &Request) -> Response {
        self.now = now;
        let empty = json!({});
        let body = request.body.as_ref().unwrap_or(&empty);
        let (route, query) = request.path.split_once('?').unwrap_or((&request.path, ""));
        let parts: Vec<&str> = route.trim_matches('/').split('/').collect();
        let method = request.method.as_str();
        match (method, route) {
            ("POST", "/auth/challenge") => return self.challenge(),
            ("POST", "/auth/token") => return self.login(body),
            ("POST", "/enroll") => return self.enroll(body),
            _ => {}
        }
        let Some(user) = self.authorise(request.token.as_deref()) else {
            return problem(ErrorCode::NotAuthorized, Some(401));
        };
        let gateway_route = matches!(parts[0], "sessions" | "reports" | "copies");
        if route != "/credentials" && gateway_route != user.gateway() {
            return problem(ErrorCode::NotAuthorized, None);
        }
        match (method, route) {
            ("POST", "/sessions") => return self.open_session(&user, body),
            ("POST", "/reports") => return self.reports(&user, body),
            ("GET", "/copies") => return self.poll(&user, query),
            ("POST", "/copies/ack") => return Response::empty(),
            ("PUT", "/settings") => {
                self.state
                    .settings
                    .insert(user.address.clone(), body.clone());
                return Response::empty();
            }
            ("GET", "/settings") => {
                let s = self.state.settings.get(&user.address).cloned();
                return Response::ok(200, s.unwrap_or_else(|| json!({})));
            }
            ("POST", "/gateways") => return self.register(&user, body),
            ("POST", "/credentials") => {
                let credential = self.renewed(&user);
                return Response::ok(200, json!({"credential": base64url::encode(&credential)}));
            }
            ("POST", "/direct") => return self.direct(&user, body),
            (_, "/accept-list") => return self.accept_list(&user.address, method, body),
            _ => {}
        }
        if parts[0] == "sessions" && parts.len() >= 2 {
            return self.in_session(&user, method, parts[1], &parts[2..], body);
        }
        problem(ErrorCode::UnknownMessage, Some(404))
    }

    /// RP-5 §2: the holder of a valid bearer token.
    fn authorise(&self, token: Option<&str>) -> Option<Holder> {
        let held = self.state.tokens.get(token?)?;
        (held.expires > self.now && !self.state.revoked.contains(&held.serial))
            .then(|| held.clone())
    }

    fn challenge(&mut self) -> Response {
        let nonce = base64url::encode(&self.fresh("nonce", 32));
        self.state
            .challenges
            .insert(nonce.clone(), self.now + NONCE_S);
        Response::ok(200, json!({"nonce": nonce, "expires": self.now + NONCE_S}))
    }

    fn login(&mut self, body: &Json) -> Response {
        let nonce_text = text(body, "nonce").to_owned();
        let nonce = base64url::decode(&nonce_text);
        let good = self.state.challenges.remove(&nonce_text).unwrap_or(0) > self.now;
        let Some(nonce) = nonce.filter(|_| good) else {
            return problem(ErrorCode::NotAuthorized, Some(401));
        };
        let (Some(credential), Some(signature)) =
            (unb64(body, "credential"), unb64(body, "signature"))
        else {
            return problem(ErrorCode::Malformed, None);
        };
        let cbody = match credential::verify(&credential, &self.trust(), self.now, 0) {
            Ok(b) => b,
            Err(r) => return problem(r.code, None),
        };
        let mut signed = nonce;
        signed.extend_from_slice(self.config.base_url.as_bytes());
        if !crypto::verify(context::API_LOGIN, &signed, &signature, &cbody.device) {
            return problem(ErrorCode::SignatureInvalid, None);
        }
        let token = format!("tok_{}", base64url::encode(&self.fresh("token", 18)));
        let expires = (self.now + TOKEN_S).min(cbody.expires);
        self.state.tokens.insert(
            token.clone(),
            Holder {
                address: cbody.address.clone(),
                operator: cbody.operator.clone(),
                device: hex(&cbody.device),
                flags: cbody.flags,
                serial: cbody.serial,
                expires,
            },
        );
        Response::ok(
            200,
            json!({"token": token, "expires": expires, "address": cbody.address,
                   "operator": cbody.operator, "flags": cbody.flags}),
        )
    }

    /// RP-5 §5.2, §5.6: the credential at `clock` and the statement (RP-1 §5, §6.3), with
    /// `called` as the station the statement must name.
    fn check_session(
        &self,
        body: &Json,
        called: &str,
        clock: u64,
    ) -> Result<(Statement, Body), ErrorCode> {
        let (Some(raw), Some(signature), Some(credential)) = (
            unb64(body, "statement"),
            unb64(body, "signature"),
            unb64(body, "credential"),
        ) else {
            return Err(ErrorCode::Malformed);
        };
        let stmt = Statement::decode(&raw).ok_or(ErrorCode::Malformed)?;
        let started = body
            .get("started")
            .and_then(Json::as_u64)
            .ok_or(ErrorCode::Malformed)?;
        let need = flags::RECEIVE
            | if stmt.messages.is_empty() {
                0
            } else {
                flags::SEND
            };
        let cbody =
            credential::verify(&credential, &self.trust(), clock, need).map_err(|r| r.code)?;
        let seen: Vec<(String, [u8; 8])> = self
            .state
            .replay
            .iter()
            .filter(|(_, _, t)| started.saturating_sub(*t) < REPLAY_S)
            .filter_map(|(a, n, _)| {
                let bytes: Vec<u8> = (0..n.len())
                    .step_by(2)
                    .filter_map(|i| u8::from_str_radix(n.get(i..i + 2)?, 16).ok())
                    .collect();
                Some((a.clone(), bytes.try_into().ok()?))
            })
            .collect();
        // RP-5 §5.2: the gateway issued a challenge-form statement's challenge and checked it
        // (RP-1 §6.3 step 6); the post office takes it as issued, once.
        let challenge = stmt.challenge().filter(|c| {
            let c = hex(c);
            !self.state.replay.iter().any(|(a, n, t)| {
                a == &stmt.address && n == &c && started.saturating_sub(*t) < REPLAY_S
            })
        });
        let fresh = Freshness {
            now: started,
            seen: &seen,
            challenge,
            sent_nonce: None,
        };
        statement::verify(&stmt, &signature, &cbody, called, &fresh)?;
        Ok((stmt, cbody))
    }

    fn open_session(&mut self, gateway: &Holder, body: &Json) -> Response {
        let (stmt, cbody) = match self.check_session(body, &gateway.operator, self.now) {
            Ok(x) => x,
            Err(code) => return problem(code, None),
        };
        let started = body.get("started").and_then(Json::as_u64).unwrap_or(0);
        if started.abs_diff(self.now) > TIME_WINDOW_S {
            return problem(ErrorCode::TimeSkew, None);
        }
        let once = stmt
            .nonce()
            .map(|n| hex(&n))
            .or_else(|| stmt.challenge().map(|c| hex(&c)));
        if let Some(once) = once {
            let key = (stmt.address.clone(), once);
            self.state
                .replay
                .retain(|(a, n, _)| (a, n) != (&key.0, &key.1));
            self.state.replay.push((key.0, key.1, started));
        }
        let mut waiting: Vec<Waiting> = self
            .state
            .mailboxes
            .get(&stmt.address)
            .map(|b| b.iter().filter(|w| !w.picked).cloned().collect())
            .unwrap_or_default();
        waiting.sort_by_key(|w| (std::cmp::Reverse(w.priority), w.serial));
        let mut accept = Vec::new();
        let mut decline = Vec::new();
        for id in &stmt.messages {
            if self.state.had.contains(&id.text()) {
                decline.push(json!({"id": id.text(), "code": 14, "name": "already-have"}));
            } else {
                accept.push(json!({"id": id.text(), "offset": 0}));
            }
        }
        let sid = format!("s_{}", base64url::encode(&self.fresh("session", 12)));
        let session = Session {
            gateway: gateway.operator.clone(),
            address: stmt.address.clone(),
            offers: stmt.messages.iter().map(ContentId::text).collect(),
            waiting: waiting.iter().map(|w| w.serial).collect(),
            expires: self.now + SESSION_S,
            gateway_session: text(body, "gateway_session").to_owned(),
            route: Route::of(body, &stmt.operator, vec![gateway.operator.clone()]),
        };
        let expires = session.expires;
        self.state.sessions.insert(sid.clone(), session);
        let mut answer = json!({
            "session": sid, "expires": expires,
            "waiting": waiting.iter().map(Waiting::entry).collect::<Vec<_>>(),
            "more": 0, "accept": accept, "decline": decline,
        });
        if body.get("renew").and_then(Json::as_bool) == Some(true) {
            let holder = Holder {
                address: cbody.address.clone(),
                operator: cbody.operator.clone(),
                device: hex(&cbody.device),
                flags: cbody.flags,
                serial: cbody.serial,
                expires: 0,
            };
            answer["credential"] = base64url::encode(&self.renewed(&holder)).into();
        }
        Response::ok(201, answer)
    }

    fn in_session(
        &mut self,
        gateway: &Holder,
        method: &str,
        sid: &str,
        rest: &[&str],
        body: &Json,
    ) -> Response {
        let Some(session) = self
            .state
            .sessions
            .get(sid)
            .filter(|s| s.gateway == gateway.operator && s.expires > self.now)
            .cloned()
        else {
            return problem(ErrorCode::UnknownMessage, Some(404));
        };
        match (method, rest) {
            ("DELETE", []) => {
                self.state.sessions.remove(sid);
                Response::empty()
            }
            ("GET", ["messages", serial]) => {
                let serial = serial.parse::<u64>().ok();
                let Some(serial) = serial.filter(|s| session.waiting.contains(s)) else {
                    return problem(ErrorCode::UnknownMessage, None);
                };
                let w = self
                    .state
                    .mailboxes
                    .get(&session.address)
                    .and_then(|b| b.iter().find(|w| w.serial == serial));
                match w {
                    Some(w) => {
                        Response::ok(200, json!({"serial": serial, "id": w.id, "air": w.air}))
                    }
                    None => problem(ErrorCode::UnknownMessage, None),
                }
            }
            ("GET", ["waiting"]) => {
                let waiting: Vec<Json> = self
                    .state
                    .mailboxes
                    .get(&session.address)
                    .map(|b| {
                        b.iter()
                            .filter(|w| session.waiting.contains(&w.serial))
                            .map(Waiting::entry)
                            .collect()
                    })
                    .unwrap_or_default();
                Response::ok(200, json!({"waiting": waiting, "more": 0}))
            }
            ("POST", ["messages"]) => {
                let messages = body
                    .get("messages")
                    .and_then(Json::as_array)
                    .cloned()
                    .unwrap_or_default();
                let results = self.take(
                    &session.address,
                    &session.offers,
                    &messages,
                    Some(&session.route),
                );
                Response::ok(200, json!({"results": results}))
            }
            ("POST", ["pickups"]) => {
                let serials: Vec<u64> = body
                    .get("serials")
                    .and_then(Json::as_array)
                    .map(|a| a.iter().filter_map(Json::as_u64).collect())
                    .unwrap_or_default();
                if serials.iter().any(|s| !session.waiting.contains(s)) {
                    return problem(ErrorCode::UnknownMessage, None);
                }
                self.picked_up(&session.address, &serials);
                Response::empty()
            }
            _ => problem(ErrorCode::UnknownMessage, Some(404)),
        }
    }

    /// RP-5 §5.4, in its order: in the statement, decodes to its ID, from the address.
    fn take(
        &mut self,
        address: &str,
        offers: &[String],
        messages: &[Json],
        route: Option<&Route>,
    ) -> Vec<Json> {
        let mut results = Vec::new();
        for m in messages {
            let id = text(m, "id");
            let cid = ContentId::from_text(id);
            let offered =
                cid.is_some_and(|c| offers.iter().any(|o| ContentId::from_text(o) == Some(c)));
            let Some(cid) = cid.filter(|_| offered) else {
                results.push(refused(
                    id,
                    ErrorCode::NotAuthorized,
                    "not in the session's statement",
                ));
                continue;
            };
            let message = unb64(m, "air").and_then(|a| Message::from_air(&a).ok());
            let Some(message) = message.filter(|msg| msg.content_id() == cid) else {
                results.push(refused(
                    id,
                    ErrorCode::Malformed,
                    "the air form is not this message",
                ));
                continue;
            };
            if message.from() != Some(address) {
                results.push(refused(
                    id,
                    ErrorCode::NotAuthorized,
                    "not from the statement's address",
                ));
                continue;
            }
            let fresh = self.state.had.insert(cid.text());
            if fresh {
                self.written_to(address, &message);
                self.deliver_here(&message, "");
                self.send_on(&message, "", route.cloned());
            }
            let status = if fresh { "accepted" } else { "duplicate" };
            results.push(json!({"id": id, "status": status}));
        }
        results
    }

    /// Mail between this post office's own accounts goes to their mailboxes at once.
    fn deliver_here(&mut self, message: &Message, has_it: &str) {
        let air = message.air_form(Compression::None);
        let subject = message
            .map()
            .get(key::SUBJECT)
            .and_then(cbor::Value::as_text)
            .unwrap_or("")
            .to_owned();
        let priority = message
            .map()
            .get(key::PRIORITY)
            .and_then(cbor::Value::as_u64)
            .unwrap_or(0);
        let from = message.from().unwrap_or("").to_owned();
        let recipients: Vec<String> = message
            .recipients()
            .iter()
            .map(|r| (*r).to_owned())
            .collect();
        for address in recipients {
            if !self.ours(&address) || address == has_it {
                continue;
            }
            let serial = self.next_mailbox_serial(&address);
            // a user's own mail: its sender is licensed (RP-6 §5.1)
            self.deliver(&address, serial, &air, &from, &subject, priority, false);
            self.picked_locally_here(&address);
        }
    }

    /// What goes on by email: every recipient this post office does not serve, but the one that
    /// has it already (RP-6).
    fn send_on(&mut self, message: &Message, has_it: &str, route: Option<Route>) {
        let to: Vec<String> = message
            .recipients()
            .iter()
            .filter(|r| !self.ours(r) && **r != has_it)
            .map(|r| (*r).to_owned())
            .collect();
        if !to.is_empty() {
            self.state.outgoing.push(Outgoing {
                id: message.content_id().text(),
                air: base64url::encode(&message.air_form(Compression::None)),
                to,
                route,
                queued: self.now,
            });
        }
    }

    /// RP-5 §5.6 `local_pickups`: mark what the user already picked up at a gateway.
    fn picked_locally_here(&mut self, address: &str) {
        let ids = self
            .state
            .picked_locally
            .get(address)
            .cloned()
            .unwrap_or_default();
        let serials: Vec<u64> = self
            .state
            .mailboxes
            .get(address)
            .map(|b| {
                b.iter()
                    .filter(|w| ids.contains(&w.id))
                    .map(|w| w.serial)
                    .collect()
            })
            .unwrap_or_default();
        self.picked_up(address, &serials);
    }

    /// RP-5 §6.5: the recipients of mail a user sent join the user's accept list.
    fn written_to(&mut self, address: &str, message: &Message) {
        let on = self
            .state
            .settings
            .get(address)
            .and_then(|s| s.get("accept_on_send"))
            .and_then(Json::as_bool)
            .unwrap_or(true);
        if !on {
            return;
        }
        let entries = self.state.accept.entry(address.to_owned()).or_default();
        for r in message.recipients() {
            if !on_list(r, entries) {
                entries.push(r.to_lowercase());
            }
        }
    }

    fn renewed(&mut self, held: &Holder) -> Vec<u8> {
        self.issue(&held.address, &held.operator, held.device(), held.flags)
    }

    fn accept_list(&mut self, address: &str, method: &str, body: &Json) -> Response {
        if method == "POST" {
            let list = |k| -> Option<Vec<String>> {
                body.get(k).map_or(Some(Vec::new()), |v| {
                    v.as_array()?
                        .iter()
                        .map(|e| e.as_str().map(str::to_lowercase).filter(|e| is_entry(e)))
                        .collect()
                })
            };
            let (Some(add), Some(remove)) = (list("add"), list("remove")) else {
                return problem(ErrorCode::Malformed, None);
            };
            let entries = self.state.accept.entry(address.to_owned()).or_default();
            for e in add {
                if !entries.contains(&e) {
                    entries.push(e);
                }
            }
            entries.retain(|e| !remove.contains(e));
        } else if method != "GET" {
            return problem(ErrorCode::UnknownMessage, Some(404));
        }
        let entries = self.state.accept.get(address).cloned().unwrap_or_default();
        Response::ok(200, json!({"entries": entries}))
    }

    /// RP-5 §5.5: mark them, and have every copy placed ahead dropped (§5.8).
    fn picked_up(&mut self, address: &str, serials: &[u64]) {
        let mut newly = Vec::new();
        if let Some(b) = self.state.mailboxes.get_mut(address) {
            for w in b
                .iter_mut()
                .filter(|w| serials.contains(&w.serial) && !w.picked)
            {
                w.picked = true;
                newly.push(w.serial);
            }
        }
        for serial in newly {
            for items in self.state.copies.values_mut() {
                let held = items.iter().any(|i| {
                    i.get("address").and_then(Json::as_str) == Some(address)
                        && i.get("serial").and_then(Json::as_u64) == Some(serial)
                });
                if held {
                    items.push(json!({"kind": "drop", "address": address, "serial": serial}));
                }
            }
        }
    }

    /// RP-5 §5.6: each session checked at its own `started`, one answer each, in order.
    fn reports(&mut self, gateway: &Holder, body: &Json) -> Response {
        let mut out = Vec::new();
        for s in body
            .get("sessions")
            .and_then(Json::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let path: Vec<String> = s
                .get("path")
                .and_then(Json::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|c| c.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let called = path
                .first()
                .cloned()
                .unwrap_or_else(|| gateway.operator.clone());
            if path.last().is_some_and(|l| *l != gateway.operator) {
                out.push(coded(ErrorCode::NotAuthorized));
                continue;
            }
            let started = s.get("started").and_then(Json::as_u64).unwrap_or(0);
            let stmt = match self.check_session(&s, &called, started) {
                Ok((stmt, _)) => stmt,
                Err(code) => {
                    out.push(coded(code));
                    continue;
                }
            };
            let offers: Vec<String> = stmt.messages.iter().map(ContentId::text).collect();
            let messages = s
                .get("messages")
                .and_then(Json::as_array)
                .cloned()
                .unwrap_or_default();
            let gateways = if path.is_empty() {
                vec![gateway.operator.clone()]
            } else {
                path.clone()
            };
            let route = Route::of(&s, &stmt.operator, gateways);
            let results = self.take(&stmt.address, &offers, &messages, Some(&route));
            let pickups: Vec<u64> = s
                .get("pickups")
                .and_then(Json::as_array)
                .map(|a| a.iter().filter_map(Json::as_u64).collect())
                .unwrap_or_default();
            self.picked_up(&stmt.address, &pickups);
            let local: Vec<String> = s
                .get("local_pickups")
                .and_then(Json::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Json::as_str)
                        .filter(|t| ContentId::from_text(t).is_some())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            self.state
                .picked_locally
                .entry(stmt.address.clone())
                .or_default()
                .extend(local);
            self.picked_locally_here(&stmt.address);
            out.push(json!({"results": results}));
        }
        Response::ok(200, json!({"sessions": out}))
    }

    /// RP-5 §5.8: the items for this gateway after the cursor. Cursors here are `c_<count>`.
    fn poll(&mut self, gateway: &Holder, query: &str) -> Response {
        self.state
            .last_poll
            .insert(gateway.operator.clone(), self.now);
        let after = query
            .split('&')
            .filter_map(|p| p.split_once('='))
            .find(|(k, _)| *k == "after")
            .map_or("", |(_, v)| v);
        let start = after
            .strip_prefix("c_")
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(0);
        let items = self
            .state
            .copies
            .get(&gateway.operator)
            .cloned()
            .unwrap_or_default();
        let rest: Vec<Json> = items.iter().skip(start).cloned().collect();
        Response::ok(
            200,
            json!({"cursor": format!("c_{}", items.len()), "items": rest}),
        )
    }

    /// RP-5 §6.1: a one-time code, and a signature that proves the new device holds its key.
    fn enroll(&mut self, body: &Json) -> Response {
        let code_text = text(body, "code").to_owned();
        let code = self.state.codes.get(&code_text).cloned();
        let Some(code) = code.filter(|c| {
            c.expires > self.now
                && c.address == text(body, "address")
                && c.operator == text(body, "operator")
        }) else {
            return problem(ErrorCode::NotAuthorized, None);
        };
        let device = unb64(body, "device").and_then(|d| <[u8; 32]>::try_from(d).ok());
        let (Some(device), Some(signature)) = (device, unb64(body, "signature")) else {
            return problem(ErrorCode::Malformed, None);
        };
        let mut signed = code_text.into_bytes();
        signed.extend_from_slice(self.config.base_url.as_bytes());
        if !crypto::verify(context::API_ENROLL, &signed, &signature, &device) {
            return problem(ErrorCode::SignatureInvalid, None);
        }
        // used: only by an enrollment that succeeds (RP-5 §6.1)
        self.state.codes.remove(text(body, "code"));
        let credential = self.issue(&code.address, &code.operator, device, code.flags);
        Response::ok(200, json!({"credential": base64url::encode(&credential)}))
    }

    /// RP-5 §6.4: a user registers a gateway they operate, and gets its credential.
    fn register(&mut self, user: &Holder, body: &Json) -> Response {
        let mut record = body.get("record").cloned().unwrap_or_else(|| json!({}));
        if record.get("operator").and_then(Json::as_str) != Some(user.operator.as_str()) {
            return problem(ErrorCode::NotAuthorized, None);
        }
        let device = unb64(body, "device").and_then(|d| <[u8; 32]>::try_from(d).ok());
        let callsign = text(&record, "callsign").to_owned();
        let (Some(device), Some(signature)) = (device, unb64(body, "signature")) else {
            return problem(ErrorCode::Malformed, None);
        };
        let mut signed = callsign.clone().into_bytes();
        signed.extend_from_slice(self.config.base_url.as_bytes());
        if callsign.is_empty()
            || !crypto::verify(context::API_GATEWAY, &signed, &signature, &device)
        {
            return problem(ErrorCode::SignatureInvalid, None);
        }
        record["updated"] = self.now.into();
        self.state.gateways.insert(callsign.clone(), record);
        let credential = self.issue(
            &user.address,
            &callsign,
            device,
            flags::SEND | flags::GATEWAY,
        );
        Response::ok(201, json!({"credential": base64url::encode(&credential)}))
    }

    // -- station-to-station reports (RP-5 §6.6) -----------------------------------------------

    fn direct(&mut self, user: &Holder, body: &Json) -> Response {
        let mut out = Vec::new();
        for s in body
            .get("sessions")
            .and_then(Json::as_array)
            .cloned()
            .unwrap_or_default()
        {
            match self.check_direct(user, &s) {
                Err(code) => out.push(coded(code)),
                Ok((mine, theirs)) => {
                    let mut results = Vec::new();
                    for m in s.get("sent").and_then(Json::as_array).into_iter().flatten() {
                        results.push(self.sent_direct(&mine, &theirs, m));
                    }
                    for m in s
                        .get("received")
                        .and_then(Json::as_array)
                        .into_iter()
                        .flatten()
                    {
                        results.push(self.received_direct(&mine, &theirs, m));
                    }
                    out.push(json!({"results": results}));
                }
            }
        }
        Response::ok(200, json!({"sessions": out}))
    }

    /// RP-5 §6.6's checks, in order: the two statements, or the first failure.
    fn check_direct(&self, user: &Holder, s: &Json) -> Result<(Statement, Statement), ErrorCode> {
        let started = s.get("started").and_then(Json::as_u64).unwrap_or(0);
        let role = s.get("role").and_then(Json::as_str);
        let proof = |side: &str| -> Option<(Vec<u8>, Statement, Vec<u8>)> {
            let p = s.get(side)?;
            Some((
                unb64(p, "credential")?,
                Statement::decode(&unb64(p, "statement")?)?,
                unb64(p, "signature")?,
            ))
        };
        let (Some(mine), Some(theirs)) = (proof("mine"), proof("theirs")) else {
            return Err(ErrorCode::Malformed);
        };
        if !matches!(role, Some("caller" | "called")) {
            return Err(ErrorCode::Malformed);
        }
        let any = |k: &str| {
            s.get(k)
                .and_then(Json::as_array)
                .is_some_and(|a| !a.is_empty())
        };
        let mine_need = if any("sent") { flags::SEND } else { 0 }
            | if any("received") { flags::RECEIVE } else { 0 };
        let theirs_need = if any("received") { flags::SEND } else { 0 };
        let trust = self.trust();
        let mine_body =
            credential::verify(&mine.0, &trust, started, mine_need).map_err(|r| r.code)?;
        let theirs_body =
            credential::verify(&theirs.0, &trust, started, theirs_need).map_err(|r| r.code)?;
        if mine_body.address != user.address || !self.ours(&user.address) {
            return Err(ErrorCode::NotAuthorized);
        }
        let (calling, answering) = if role == Some("caller") {
            ((&mine, &mine_body), (&theirs, &theirs_body))
        } else {
            ((&theirs, &theirs_body), (&mine, &mine_body))
        };
        let relayed = Freshness {
            now: started,
            seen: &[],
            challenge: None,
            sent_nonce: None,
        };
        statement::verify(
            &calling.0.1,
            &calling.0.2,
            calling.1,
            &answering.1.operator,
            &relayed,
        )?;
        let answered = Freshness {
            sent_nonce: calling.0.1.nonce(),
            ..relayed
        };
        statement::verify(
            &answering.0.1,
            &answering.0.2,
            answering.1,
            &calling.1.operator,
            &answered,
        )?;
        Ok((mine.1, theirs.1))
    }

    /// The message, if its statement lists it and it is it, from the statement's address.
    fn direct_message(stmt: &Statement, m: &Json) -> Option<Message> {
        let id = ContentId::from_text(text(m, "id"))?;
        let message = Message::from_air(&unb64(m, "air")?).ok()?;
        (stmt.messages.contains(&id)
            && message.content_id() == id
            && message.from() == Some(stmt.address.as_str()))
        .then_some(message)
    }

    fn sent_direct(&mut self, mine: &Statement, theirs: &Statement, m: &Json) -> Json {
        let id = text(m, "id");
        let Some(message) = Self::direct_message(mine, m) else {
            return refused(
                id,
                ErrorCode::NotAuthorized,
                "not this user's, in its statement",
            );
        };
        if !self.state.had.insert(message.content_id().text()) {
            return json!({"id": id, "status": "duplicate"});
        }
        self.written_to(&mine.address, &message);
        self.deliver_here(&message, &theirs.address);
        self.send_on(&message, &theirs.address, None);
        let others: Vec<&str> = message
            .recipients()
            .into_iter()
            .filter(|r| *r != theirs.address)
            .collect();
        json!({"id": id, "status": "accepted", "sent_on": others})
    }

    fn received_direct(&mut self, mine: &Statement, theirs: &Statement, m: &Json) -> Json {
        let id = text(m, "id");
        let Some(message) = Self::direct_message(theirs, m) else {
            return refused(
                id,
                ErrorCode::NotAuthorized,
                "not the other station's, signed",
            );
        };
        if !message.recipients().contains(&mine.address.as_str()) {
            return refused(id, ErrorCode::NotAuthorized, "not addressed to this user");
        }
        let box_has = self
            .state
            .mailboxes
            .get(&mine.address)
            .is_some_and(|b| b.iter().any(|w| w.id == message.content_id().text()));
        if box_has {
            return json!({"id": id, "status": "duplicate"});
        }
        let serial = self.next_mailbox_serial(&mine.address);
        let subject = message
            .map()
            .get(key::SUBJECT)
            .and_then(cbor::Value::as_text)
            .unwrap_or("")
            .to_owned();
        let priority = message
            .map()
            .get(key::PRIORITY)
            .and_then(cbor::Value::as_u64)
            .unwrap_or(0);
        self.state
            .mailboxes
            .entry(mine.address.clone())
            .or_default()
            .push(Waiting {
                serial,
                air: base64url::encode(&message.air_form(Compression::None)),
                id: id.to_owned(),
                from: message.from().unwrap_or("").to_owned(),
                subject,
                priority,
                picked: true,
                third_party: false,
            });
        json!({"id": id, "status": "accepted", "serial": serial})
    }
}
