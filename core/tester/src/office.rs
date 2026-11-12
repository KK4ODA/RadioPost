// SPDX-License-Identifier: MIT OR Apache-2.0
//! The test post office (the outline's §2.1, §2.3): the tester's own post office for
//! `radiopost.org`, signing with the published test key, for the programs that talk to one (a
//! gateway carrying sessions, a client reporting), with every exchange kept for the tests to
//! read; and the test DNS zone those programs look post offices up in, served as DNS over HTTPS
//! (RP-1 §7.1), so that nothing asks the Internet's DNS about the test domains.
//!
//! The post office is the core's (`radiopost-postoffice`), which the vectors check: a program
//! under test is judged by what it sends and how it answers, which the tests read here.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ed25519_dalek::SigningKey;
use radiopost::credential::flags;
use radiopost_postoffice::{Config, PostOffice, Request, State};
use serde_json::{Value as Json, json};
use tiny_http::{Header, Response, Server};

use crate::harness::{PostOffices, new_key, now};

/// The test post office's domain.
pub const DOMAIN: &str = "radiopost.org";

/// One request a program made, and the answer.
#[derive(Clone, Debug)]
pub struct Exchange {
    /// `GET`, `POST`, …
    pub method: String,
    /// Under the base URL, with its query.
    pub path: String,
    /// The JSON body, when there was one.
    pub body: Option<Json>,
    /// The answer's status.
    pub status: u16,
    /// The answer's body.
    pub answer: Option<Json>,
}

/// A user of the test post office, as the tester plays one: a device key and its credential.
pub struct User {
    /// The account.
    pub address: String,
    /// The callsign.
    pub operator: String,
    /// The device key.
    pub key: SigningKey,
    /// The credential the post office issued for it.
    pub credential: Vec<u8>,
}

type Shared<T> = Arc<Mutex<T>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The test post office and the test DNS zone, on one port of this machine.
pub struct TestOffice {
    office: Shared<PostOffice>,
    log: Shared<Vec<Exchange>>,
    zone: Shared<BTreeMap<String, Vec<String>>>,
    asked: Shared<Vec<String>>,
    /// The API's base URL (RP-5 §1).
    pub base: String,
    /// The DNS-over-HTTPS resolver serving the test zone.
    pub resolver: String,
}

/// The answer to `/dns-query?name=…&type=TXT`, as DNS-over-HTTPS JSON resolvers give it.
fn dns_answer(zone: &BTreeMap<String, Vec<String>>, query: &str) -> Json {
    let name = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("name="))
        .unwrap_or("")
        .trim_end_matches('.')
        .to_lowercase();
    let records = zone.get(&name).cloned().unwrap_or_default();
    json!({
        "Status": if records.is_empty() { 3 } else { 0 },
        "Answer": records.iter().map(|r| json!({
            "name": format!("{name}."), "type": 16, "TTL": 300, "data": format!("\"{r}\""),
        })).collect::<Vec<_>>(),
    })
}

impl TestOffice {
    /// Starts it on a free port of the loopback interface, with `radiopost.org` in the test zone.
    ///
    /// # Errors
    ///
    /// No port, or no test key for `radiopost.org`.
    ///
    /// # Panics
    ///
    /// When the system has no random numbers for the post office.
    #[allow(
        clippy::too_many_lines,
        reason = "the post office, the zone and the server, together"
    )]
    pub fn start(offices: &PostOffices) -> Result<TestOffice, String> {
        let server = Server::http("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = server
            .server_addr()
            .to_ip()
            .map(|a| a.port())
            .ok_or("no port")?;
        let base = format!("http://127.0.0.1:{port}/rp/v1");
        let key = offices
            .key_of(DOMAIN)
            .ok_or("no test key for radiopost.org")?
            .clone();
        let config = Config {
            domains: vec![DOMAIN.into()],
            base_url: base.clone(),
            key,
            trust: offices.trust(),
            operators: Vec::new(),
            applications_open: false,
        };
        let draw = Box::new(|_: &str, n: usize| {
            let mut b = vec![0u8; n];
            getrandom::fill(&mut b).expect("the system's random numbers");
            b
        });
        let mut office = PostOffice::new(
            config,
            State {
                next_serial: 9000,
                ..State::default()
            },
            draw,
        );
        office.at(now());
        let mut zone = BTreeMap::new();
        let records: Vec<String> = office
            .dns_records()
            .iter()
            .filter_map(|r| {
                r.split_once('"')
                    .map(|(_, t)| t.trim_end_matches('"').to_owned())
            })
            .collect();
        zone.insert(format!("_radiopost.{DOMAIN}"), records);
        let me = TestOffice {
            office: Arc::new(Mutex::new(office)),
            log: Arc::default(),
            zone: Arc::new(Mutex::new(zone)),
            asked: Arc::default(),
            base,
            resolver: format!("http://127.0.0.1:{port}/dns-query"),
        };
        let (office, log, zone, asked) = (
            me.office.clone(),
            me.log.clone(),
            me.zone.clone(),
            me.asked.clone(),
        );
        std::thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let url = request.url().to_owned();
                if let Some(query) = url.strip_prefix("/dns-query?") {
                    let answer = dns_answer(&lock(&zone), query);
                    lock(&asked).push(query.to_owned());
                    let _ = request.respond(
                        Response::from_string(answer.to_string()).with_header(
                            Header::from_bytes("Content-Type", "application/dns-json")
                                .expect("a header"),
                        ),
                    );
                    continue;
                }
                let mut text = Vec::new();
                let _ = request.as_reader().read_to_end(&mut text);
                let body: Option<Json> = serde_json::from_slice(&text).ok();
                let token = request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Authorization"))
                    .and_then(|h| h.value.as_str().strip_prefix("Bearer ").map(str::to_owned));
                let method = request.method().as_str().to_owned();
                let path = url.strip_prefix("/rp/v1").unwrap_or(&url).to_owned();
                let answer = lock(&office).handle(
                    now(),
                    &Request {
                        method: method.clone(),
                        path: path.clone(),
                        token,
                        body: body.clone(),
                    },
                );
                lock(&log).push(Exchange {
                    method,
                    path,
                    body,
                    status: answer.status,
                    answer: answer.body.clone(),
                });
                let text = answer.body.map(|b| b.to_string()).unwrap_or_default();
                let mut response = Response::from_string(text).with_status_code(answer.status);
                if answer.status != 204 {
                    response = response.with_header(
                        Header::from_bytes("Content-Type", "application/json").expect("a header"),
                    );
                }
                let _ = request.respond(response);
            }
        });
        Ok(me)
    }

    /// The post office itself, to put mail in a mailbox or look at what it holds.
    pub fn office(&self) -> MutexGuard<'_, PostOffice> {
        let mut o = lock(&self.office);
        o.at(now());
        o
    }

    /// Publishes `records` (TXT strings) under `_radiopost.<domain>` in the test zone.
    pub fn publish(&self, domain: &str, records: &[String]) {
        lock(&self.zone).insert(format!("_radiopost.{domain}"), records.to_vec());
    }

    /// Every exchange so far.
    #[must_use]
    pub fn exchanges(&self) -> Vec<Exchange> {
        lock(&self.log).clone()
    }

    /// The DNS questions asked so far, as their queries.
    #[must_use]
    pub fn asked(&self) -> Vec<String> {
        lock(&self.asked).clone()
    }

    /// A user of the test post office: a new device key and a credential for it.
    #[must_use]
    pub fn user(&self, address: &str, operator: &str) -> User {
        let key = new_key();
        let credential = self.office().issue(
            address,
            operator,
            key.verifying_key().to_bytes(),
            flags::SEND | flags::RECEIVE,
        );
        User {
            address: address.into(),
            operator: operator.into(),
            key,
            credential,
        }
    }

    /// A gateway's credential (RP-5 §6.4: the operator's account, the gateway's callsign, `send`
    /// and `gateway`).
    #[must_use]
    pub fn gateway_credential(&self, callsign: &str, device: &[u8; 32]) -> Vec<u8> {
        self.office().issue(
            "operator@radiopost.org",
            callsign,
            *device,
            flags::SEND | flags::GATEWAY,
        )
    }
}
