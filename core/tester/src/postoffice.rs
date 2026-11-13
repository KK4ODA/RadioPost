// SPDX-License-Identifier: MIT OR Apache-2.0
//! The post office role's tests (the outline's §3.4): P-01 to P-04 and P-07 to P-10. The tester
//! starts the post office from its driver file in a fresh directory for each test, and plays its
//! users' clients, a gateway, another post office's user, and the mail server around it: its
//! door called as an MTA hook (the MTA Hooks draft's shape), and a stand-in submission port that
//! takes the email it sends (`mailserver`).
//!
//! Every request is scripted here from RP-5's and RP-6's text; the verdicts are theirs.

use std::fmt::Write as _;
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ed25519_dalek::SigningKey;
use radiopost::cbor::Value;
use radiopost::credential::{self, PostOfficeKey, Trust, flags};
use radiopost::crypto::{self, context};
use radiopost::message::{Compression, Message, key};
use radiopost::statement::{Form, Statement};
use radiopost::{ContentId, base64url};
use serde_json::{Value as Json, json};

use crate::gateway::mail;
use crate::harness::{Driver, Outcome, PostOffices, Running, new_key, now, random};
use crate::mailserver::{self, MailServer};

const DOMAIN: &str = "radiopost.org";
const GATEWAY: &str = "N1CALL-10";
const SESSION_ID: &str = "ec7b31841ec47c47";

fn free_port() -> Result<u16, String> {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| e.to_string())
}

/// One HTTP request: the status and the body as text.
fn http(
    host: &str,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> Result<(u16, String), String> {
    let mut s = TcpStream::connect(host).map_err(|e| format!("cannot reach {host}: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;
    let body = body.unwrap_or("");
    let auth = token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\n{auth}Content-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .map_err(|e| e.to_string())?;
    let mut answer = Vec::new();
    s.read_to_end(&mut answer).map_err(|e| e.to_string())?;
    let answer = String::from_utf8_lossy(&answer).into_owned();
    let status = answer
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| format!("not an HTTP answer: {answer:?}"))?;
    Ok((
        status,
        answer
            .split_once("\r\n\r\n")
            .map_or("", |(_, b)| b)
            .to_owned(),
    ))
}

/// A user of the post office under test, as the tester plays one.
struct Account {
    address: String,
    operator: String,
    key: SigningKey,
    credential: Vec<u8>,
    token: String,
}

/// The post office under test, running, and the stand-in mail server it sends to.
struct Po {
    _running: Running,
    home: PathBuf,
    host: String,
    prefix: String,
    base: String,
    key: [u8; 32],
    door: String,
    mail: MailServer,
}

impl Po {
    fn call(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&Json>,
    ) -> Result<(u16, Json), String> {
        let text = body.map(Json::to_string);
        let (status, answer) = http(
            &self.host,
            method,
            &format!("{}{path}", self.prefix),
            token,
            text.as_deref(),
        )?;
        Ok((status, serde_json::from_str(&answer).unwrap_or(Json::Null)))
    }

    fn trust(&self) -> Trust {
        let mut t = Trust::default();
        t.keys
            .entry(DOMAIN.into())
            .or_default()
            .push(PostOfficeKey {
                public: self.key,
                not_before: 0,
                not_after: u64::MAX,
            });
        t
    }

    /// RP-5 §3: a token for a credential.
    fn login(&self, credential: &[u8], key: &SigningKey) -> Result<String, String> {
        let (_, c) = self.call("POST", "/auth/challenge", None, None)?;
        let nonce = c["nonce"]
            .as_str()
            .ok_or("no nonce from /auth/challenge")?
            .to_owned();
        let mut signed = base64url::decode(&nonce).ok_or("the nonce is not base64url")?;
        signed.extend_from_slice(self.base.as_bytes());
        let sig = crypto::sign(context::API_LOGIN, &signed, key);
        let (status, t) = self.call(
            "POST",
            "/auth/token",
            None,
            Some(
                &json!({"credential": base64url::encode(credential), "nonce": nonce,
                         "signature": base64url::encode(&sig)}),
            ),
        )?;
        if status != 200 {
            return Err(format!("a login refused: {status} {t}"));
        }
        t["token"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "no token".into())
    }

    /// RP-5 §6.1 with a code the operator made (the driver's `code`).
    fn enroll(
        &self,
        driver: &Driver,
        address: &str,
        operator: &str,
        name: &str,
    ) -> Result<Account, String> {
        let code = self.code(driver, address, operator, name)?;
        let key = new_key();
        let (status, answer) = self.enroll_with(&code, address, operator, &key, false)?;
        if status != 200 {
            return Err(format!("enrollment refused: {status} {answer}"));
        }
        let credential = answer["credential"]
            .as_str()
            .and_then(base64url::decode)
            .ok_or("no credential in the enrollment's answer")?;
        let token = self.login(&credential, &key)?;
        Ok(Account {
            address: address.into(),
            operator: operator.into(),
            key,
            credential,
            token,
        })
    }

    fn code(
        &self,
        driver: &Driver,
        address: &str,
        operator: &str,
        name: &str,
    ) -> Result<String, String> {
        let home = self.home.to_string_lossy().into_owned();
        let out = driver.run(
            "code",
            &[
                ("home", &home),
                ("address", address),
                ("operator", operator),
                ("name", name),
            ],
            "",
        )?;
        out.split_whitespace()
            .find(|w| w.len() == 9 && w.as_bytes()[4] == b'-')
            .map(str::to_owned)
            .ok_or_else(|| format!("`code` printed no code: {out:?}"))
    }

    fn enroll_with(
        &self,
        code: &str,
        address: &str,
        operator: &str,
        key: &SigningKey,
        bad: bool,
    ) -> Result<(u16, Json), String> {
        let mut signed = code.as_bytes().to_vec();
        signed.extend_from_slice(if bad {
            b"https://elsewhere.example"
        } else {
            self.base.as_bytes()
        });
        let sig = crypto::sign(context::API_ENROLL, &signed, key);
        self.call(
            "POST",
            "/enroll",
            None,
            Some(
                &json!({"address": address, "operator": operator, "code": code,
                         "device": base64url::encode(key.verifying_key().as_bytes()),
                         "signature": base64url::encode(&sig)}),
            ),
        )
    }

    /// RP-5 §6.4: a gateway its owner registers, logged in.
    fn gateway(&self, owner: &Account) -> Result<Account, String> {
        let key = new_key();
        let mut signed = GATEWAY.as_bytes().to_vec();
        signed.extend_from_slice(self.base.as_bytes());
        let sig = crypto::sign(context::API_GATEWAY, &signed, &key);
        let (status, answer) = self.call(
            "POST",
            "/gateways",
            Some(&owner.token),
            Some(
                &json!({"record": {"callsign": GATEWAY, "operator": owner.operator,
                                    "channels": [], "hours": "always", "internet": true},
                         "device": base64url::encode(key.verifying_key().as_bytes()),
                         "signature": base64url::encode(&sig)}),
            ),
        )?;
        if status != 201 {
            return Err(format!(
                "gateway registration refused (RP-5 §6.4): {status} {answer}"
            ));
        }
        let credential = answer["credential"]
            .as_str()
            .and_then(base64url::decode)
            .ok_or("no credential in the registration's answer")?;
        let body = credential::verify(&credential, &self.trust(), now(), 0)
            .map_err(|r| {
                format!(
                    "the gateway's credential does not verify: {}",
                    r.code.name()
                )
            })?
            .clone();
        if body.flags & flags::GATEWAY == 0
            || body.operator != GATEWAY
            || body.address != owner.address
        {
            return Err("the gateway's credential is not the owner's account, the gateway's callsign, with the gateway flag (RP-5 §6.4)".into());
        }
        let token = self.login(&credential, &key)?;
        Ok(Account {
            address: owner.address.clone(),
            operator: GATEWAY.into(),
            key,
            credential,
            token,
        })
    }

    /// RP-5 §5.2's body for `user`'s statement.
    fn session_body(
        user: &Account,
        gateway: &str,
        offers: &[&Message],
        form: Form,
        started: u64,
    ) -> Json {
        let stmt = Statement {
            address: user.address.clone(),
            operator: user.operator.clone(),
            gateway: gateway.into(),
            messages: offers.iter().map(|m| m.content_id()).collect(),
            form,
        };
        json!({
            "statement": base64url::encode(&stmt.encode()),
            "signature": base64url::encode(&stmt.sign(&user.key)),
            "credential": base64url::encode(&user.credential),
            "started": started,
            "gateway_session": SESSION_ID,
            "cursor": 0,
        })
    }

    fn time_form() -> Form {
        Form::Time {
            time: now(),
            nonce: random(),
        }
    }

    /// A session opened for `user` and the `201` answer.
    fn open(&self, gw: &Account, user: &Account, offers: &[&Message]) -> Result<Json, String> {
        let (status, answer) = self.call(
            "POST",
            "/sessions",
            Some(&gw.token),
            Some(&Po::session_body(
                user,
                GATEWAY,
                offers,
                Po::time_form(),
                now(),
            )),
        )?;
        if status != 201 {
            return Err(format!("a session refused: {status} {answer}"));
        }
        Ok(answer)
    }

    /// `user` hands `messages` in through `gw`, in one session.
    fn hand_in(&self, gw: &Account, user: &Account, messages: &[&Message]) -> Result<Json, String> {
        let s = self.open(gw, user, messages)?;
        let sid = s["session"].as_str().ok_or("no session ID")?;
        let items: Vec<Json> = messages
            .iter()
            .map(|m| json!({"id": m.content_id().text(), "air": base64url::encode(&m.air_form(Compression::None))}))
            .collect();
        let (_, r) = self.call(
            "POST",
            &format!("/sessions/{sid}/messages"),
            Some(&gw.token),
            Some(&json!({"messages": items})),
        )?;
        let _ = self.call("DELETE", &format!("/sessions/{sid}"), Some(&gw.token), None);
        Ok(r)
    }

    /// The door, called as an MTA hook in Stalwart's shape, the mail server's verdicts in its
    /// `serverHeaders` (the DMARC result, and the domains in `dkim` a DKIM signature passes for):
    /// the reply at `RCPT TO`, then at `DATA` when the recipient was taken.
    fn door(
        &self,
        from: Option<&str>,
        to: &str,
        email: &str,
        dmarc: &str,
        dkim: &[String],
    ) -> Result<(String, Option<String>), String> {
        let (head, contents) = email.split_once("\r\n\r\n").unwrap_or((email, ""));
        let mut headers: Vec<(String, String)> = Vec::new();
        for line in head.split("\r\n") {
            if line.starts_with([' ', '\t']) {
                if let Some((_, v)) = headers.last_mut() {
                    v.push_str(line);
                    v.push_str("\r\n");
                }
            } else if let Some((n, v)) = line.split_once(':') {
                headers.push((n.to_owned(), format!("{v}\r\n")));
            }
        }
        let mut results = format!(" tester.radiopost.org; dmarc={dmarc}");
        for d in dkim {
            let _ = write!(results, "; dkim=pass header.d={d}");
        }
        results.push_str("\r\n");
        let ask = |stage: &str| -> Result<String, String> {
            let body = json!({
                "context": {"stage": stage, "protocol": {"version": 1},
                            "client": {"ip": "192.0.2.1", "helo": "mail.example.com"}},
                "envelope": {"from": {"address": from.unwrap_or("")}, "to": [{"address": to}]},
                "message": {
                    "headers": headers.iter().map(|(n, v)| json!([n, v])).collect::<Vec<_>>(),
                    "serverHeaders": [["Authentication-Results", results]],
                    "contents": contents,
                    "size": email.len(),
                },
            });
            let (_, rest) = self.door.split_once("://").unwrap_or(("", &self.door));
            let (host, path) = rest
                .split_once('/')
                .map_or((rest, "/".to_owned()), |(h, p)| (h, format!("/{p}")));
            let (status, text) = http(host, "POST", &path, None, Some(&body.to_string()))?;
            let j: Json = serde_json::from_str(&text)
                .map_err(|_| format!("the door answered {status} {text:?}"))?;
            if j["action"] == "accept" {
                return Ok(if stage == "rcpt" {
                    "250 2.1.5"
                } else {
                    "250 2.0.0"
                }
                .into());
            }
            let r = &j["response"];
            Ok(format!(
                "{} {}",
                r["status"],
                r["enhanced_status"].as_str().unwrap_or("")
            ))
        };
        let rcpt = ask("rcpt")?;
        if !rcpt.starts_with('2') {
            return Ok((rcpt, None));
        }
        Ok((rcpt, Some(ask("data")?)))
    }

    /// Every message waiting for `user`, fetched: (entry, message).
    fn waiting(&self, gw: &Account, user: &Account) -> Result<Vec<(Json, Message)>, String> {
        let s = self.open(gw, user, &[])?;
        let sid = s["session"].as_str().ok_or("no session ID")?.to_owned();
        let mut out = Vec::new();
        for e in s["waiting"].as_array().cloned().unwrap_or_default() {
            let serial = e["serial"].as_u64().unwrap_or(0);
            let (status, m) = self.call(
                "GET",
                &format!("/sessions/{sid}/messages/{serial}"),
                Some(&gw.token),
                None,
            )?;
            if status != 200 {
                return Err(format!("GET messages/{serial}: {status}"));
            }
            let air = m["air"]
                .as_str()
                .and_then(base64url::decode)
                .ok_or("no air form")?;
            let message = Message::from_air(&air)
                .map_err(|_| format!("serial {serial}'s air form is not a message (RP-2 §4)"))?;
            if Some(message.content_id().text().as_str()) != e["id"].as_str() {
                return Err(format!(
                    "serial {serial}'s air form is not the message its entry names"
                ));
            }
            out.push((e, message));
        }
        let _ = self.call("DELETE", &format!("/sessions/{sid}"), Some(&gw.token), None);
        Ok(out)
    }
}

/// `status` and the problem code name RP-5 §1 gives it.
fn problem(got: &(u16, Json), status: u16, name: &str, what: &str) -> Outcome {
    if got.0 != status || got.1["name"] != name {
        return Err(format!(
            "{what}: {} {} where {status} {name} was due",
            got.0, got.1["name"]
        ));
    }
    Ok(())
}

/// What the post office tests stand on.
pub struct Run<'a> {
    /// The post office's driver.
    pub driver: &'a Driver,
    /// The test keys: `hampost.org`'s plays another post office.
    pub offices: PostOffices,
    /// Where each test's post office goes.
    pub scratch: PathBuf,
}

impl Run<'_> {
    /// A post office for one test, made, told of `hampost.org`, serving, and ready.
    fn start(&self, test: &str) -> Result<Po, String> {
        let home = self.scratch.join(test);
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(self.scratch.as_path()).map_err(|e| e.to_string())?;
        let (api, admin, door) = (free_port()?, free_port()?, free_port()?);
        let host = format!("127.0.0.1:{api}");
        let base = format!("http://{host}/rp/v1");
        let mail = MailServer::start()?;
        let h = home.to_string_lossy().into_owned();
        let vars = [
            ("home", h.as_str()),
            ("base", base.as_str()),
            ("listen", host.as_str()),
            ("admin", &format!("127.0.0.1:{admin}")),
            ("door", &format!("127.0.0.1:{door}")),
            ("smtp", mail.at.as_str()),
        ];
        self.driver.run("init", &vars, "")?;
        let (_, hampost, _, _) = self
            .offices
            .published()
            .into_iter()
            .find(|(d, ..)| d == "hampost.org")
            .ok_or("no test key for hampost.org")?;
        self.driver.run(
            "know",
            &[
                ("home", &h),
                ("domain", "hampost.org"),
                ("public", &radiopost::cbor::hex(&hampost)),
            ],
            "",
        )?;
        let out = self.driver.run("dns", &[("home", &h)], "")?;
        let key = out
            .lines()
            .filter_map(|l| {
                l.split_once('"')
                    .map(|(_, t)| t.trim_end_matches('"').to_owned())
            })
            .filter_map(|t| radiopost::dns::parse(&t))
            .find(|t| t.get("t").map(String::as_str) == Some("key"))
            .as_ref()
            .and_then(radiopost::dns::key_of)
            .ok_or_else(|| format!("`dns` printed no key record RP-1 §7.1 reads: {out:?}"))?;
        let running = self.driver.start("serve", &[("home", &h)])?;
        let door_url = self.driver.template("door", &vars)?;
        let po = Po {
            _running: running,
            home,
            host,
            prefix: "/rp/v1".into(),
            base,
            key,
            door: door_url,
            mail,
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if po
                .call("GET", "/info", None, None)
                .is_ok_and(|(s, _)| s == 200)
            {
                return Ok(po);
            }
            if Instant::now() > deadline {
                return Err("the post office never answered GET /info".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// P-01: its public documents, as JWS under its key, and its DNS records.
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn p01(&mut self, t: &mut Vec<String>) -> Outcome {
        let po = self.start("P-01")?;
        let home = po.home.to_string_lossy().into_owned();
        let out = self.driver.run("dns", &[("home", &home)], "")?;
        t.push(format!("dns: {out}"));
        // the tester's base URL is plain HTTP on loopback, which RP-1 §7.1 would have a reader
        // ignore; its tags are read here as written
        let api = out
            .lines()
            .filter_map(|l| {
                l.split_once('"')
                    .map(|(_, t)| t.trim_end_matches('"').to_owned())
            })
            .any(|t| {
                let tags: Vec<(String, String)> = t
                    .split(';')
                    .filter_map(|p| {
                        p.split_once('=')
                            .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
                    })
                    .collect();
                let has = |k: &str, v: &str| tags.iter().any(|(a, b)| a == k && b == v);
                has("v", "RP1") && has("t", "api") && has("url", &po.base)
            });
        if !api {
            return Err("no `v=RP1; t=api` record naming the base URL (RP-1 §7.1)".into());
        }
        let mut keys = std::collections::HashMap::new();
        keys.insert(radiopost::cbor::hex(&crypto::kid(&po.key)), po.key);
        for path in [
            "/info",
            "/revocations",
            "/post-offices",
            "/gateways",
            "/routes",
        ] {
            let (status, text) =
                http(&po.host, "GET", &format!("{}{path}", po.prefix), None, None)?;
            t.push(format!(
                "GET {path} → {status} {}",
                text.chars().take(120).collect::<String>()
            ));
            if status != 200 {
                return Err(format!("GET {path}: {status} (RP-5 §4)"));
            }
            let payload = radiopost::jws::verify(text.trim(), &keys)
                .map_err(|c| format!("GET {path}: not a JWS its key verifies ({})", c.name()))?;
            if path == "/info" {
                let listed = payload["keys"].as_array().is_some_and(|k| {
                    k.iter()
                        .any(|k| k["public"].as_str() == Some(&base64url::encode(&po.key)))
                });
                if payload["version"] != 1
                    || !payload["domains"]
                        .as_array()
                        .is_some_and(|d| d.iter().any(|d| d == DOMAIN))
                    || !listed
                {
                    return Err(
                        "the info document lacks its version, its domain or its key (RP-5 §4.1)"
                            .into(),
                    );
                }
            }
        }
        Ok(())
    }

    /// P-02: logging in and tokens; `401` against `403` (RP-5 §2, §3).
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn p02(&mut self, t: &mut Vec<String>) -> Outcome {
        let po = self.start("P-02")?;
        let user = po.enroll(self.driver, "facundo@radiopost.org", "KK4ODA", "Facundo")?;
        let (status, c) = po.call("POST", "/auth/challenge", None, None)?;
        t.push(format!("challenge → {status} {c}"));
        let nonce = c["nonce"].as_str().unwrap_or("").to_owned();
        if base64url::decode(&nonce).map(|n| n.len()) != Some(32) {
            return Err("the challenge's nonce is not 32 bytes (RP-5 §3.1)".into());
        }
        if c["expires"].as_u64().is_none_or(|e| e > now() + 330) {
            return Err("the nonce is good for more than 5 minutes (RP-5 §3.1)".into());
        }
        let sign = |n: &str, base: &str| {
            let mut s = base64url::decode(n).unwrap_or_default();
            s.extend_from_slice(base.as_bytes());
            base64url::encode(&crypto::sign(context::API_LOGIN, &s, &user.key))
        };
        let body = |n: &str, sig: String| json!({"credential": base64url::encode(&user.credential), "nonce": n, "signature": sig});
        let ok = po.call(
            "POST",
            "/auth/token",
            None,
            Some(&body(&nonce, sign(&nonce, &po.base))),
        )?;
        t.push(format!("token → {} {}", ok.0, ok.1));
        if ok.0 != 200
            || ok.1["address"] != user.address.as_str()
            || ok.1["operator"] != user.operator.as_str()
        {
            return Err(
                "a good login not answered 200 with the account and operator (RP-5 §3.2)".into(),
            );
        }
        problem(
            &po.call(
                "POST",
                "/auth/token",
                None,
                Some(&body(&nonce, sign(&nonce, &po.base))),
            )?,
            401,
            "not-authorized",
            "a nonce used twice",
        )?;
        let (_, c) = po.call("POST", "/auth/challenge", None, None)?;
        let n2 = c["nonce"].as_str().unwrap_or("").to_owned();
        problem(
            &po.call(
                "POST",
                "/auth/token",
                None,
                Some(&body(&n2, sign(&n2, "https://elsewhere.example/rp/v1"))),
            )?,
            422,
            "signature-invalid",
            "a signature over another base URL",
        )?;
        problem(
            &po.call(
                "POST",
                "/auth/token",
                None,
                Some(&body(
                    &base64url::encode(&random::<32>()),
                    sign(&n2, &po.base),
                )),
            )?,
            401,
            "not-authorized",
            "a nonce never issued",
        )?;
        problem(
            &po.call("GET", "/settings", None, None)?,
            401,
            "not-authorized",
            "no token",
        )?;
        problem(
            &po.call("GET", "/settings", Some("tok_nonsense"), None)?,
            401,
            "not-authorized",
            "an unknown token",
        )?;
        let gw = po.gateway(&user)?;
        problem(
            &po.call("POST", "/sessions", Some(&user.token), Some(&json!({})))?,
            403,
            "not-authorized",
            "a user's token on a gateway endpoint",
        )?;
        problem(
            &po.call("GET", "/settings", Some(&gw.token), None)?,
            403,
            "not-authorized",
            "a gateway's token on a client endpoint",
        )?;
        Ok(())
    }

    /// P-03: enrollment and renewal (RP-5 §6.1, §6.2).
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn p03(&mut self, t: &mut Vec<String>) -> Outcome {
        let po = self.start("P-03")?;
        let (address, operator) = ("facundo@radiopost.org", "KK4ODA");
        let code = po.code(self.driver, address, operator, "Facundo")?;
        let key = new_key();
        problem(
            &po.enroll_with("AAAA-BBBB", address, operator, &key, false)?,
            403,
            "not-authorized",
            "an unknown code",
        )?;
        problem(
            &po.enroll_with(&code, address, "N0CALL", &key, false)?,
            403,
            "not-authorized",
            "the code for another operator",
        )?;
        problem(
            &po.enroll_with(&code, address, operator, &key, true)?,
            422,
            "signature-invalid",
            "a signature over another base URL",
        )?;
        let (status, answer) = po.enroll_with(&code, address, operator, &key, false)?;
        t.push(format!("enroll → {status} {answer}"));
        if status != 200 {
            return Err(format!("a good enrollment answered {status}"));
        }
        let cred = answer["credential"]
            .as_str()
            .and_then(base64url::decode)
            .ok_or("no credential")?;
        let body = credential::verify(&cred, &po.trust(), now(), 0).map_err(|r| {
            format!(
                "the credential does not verify under the post office's key: {}",
                r.code.name()
            )
        })?;
        if body.address != address
            || body.operator != operator
            || body.device != key.verifying_key().to_bytes()
        {
            return Err("the credential is not for the code's account and operator and the device's key (RP-5 §6.1)".into());
        }
        if body.expires.saturating_sub(body.issued) > 90 * 86_400 + 600
            || body.issued.abs_diff(now()) > 600
        {
            return Err("the credential is not issued now for at most 90 days (RP-1 §4.2)".into());
        }
        problem(
            &po.enroll_with(&code, address, operator, &new_key(), false)?,
            403,
            "not-authorized",
            "a code used twice",
        )?;
        let token = po.login(&cred, &key)?;
        let (status, r) = po.call("POST", "/credentials", Some(&token), Some(&json!({})))?;
        if status != 200 {
            return Err(format!("renewal answered {status} {r} (RP-5 §6.2)"));
        }
        let new = r["credential"]
            .as_str()
            .and_then(base64url::decode)
            .ok_or("no renewed credential")?;
        let nb = credential::verify(&new, &po.trust(), now(), 0)
            .map_err(|r| format!("the renewed credential: {}", r.code.name()))?;
        if (nb.address, nb.operator, nb.device, nb.flags)
            != (body.address, body.operator, body.device, body.flags)
            || nb.serial == body.serial
        {
            return Err("the renewed credential is not the same account, operator, device and flags with a new serial (RP-5 §6.2)".into());
        }
        po.login(&cred, &key)
            .map_err(|e| format!("the old credential stops working at renewal: {e} (RP-5 §6.2)"))?;
        Ok(())
    }

    /// P-04: sessions: the checks in order, messages taken and refused, pickups.
    ///
    /// # Errors
    ///
    /// The first deviation found.
    #[allow(
        clippy::too_many_lines,
        clippy::many_single_char_names,
        reason = "RP-5 §5.2 to §5.7, in order"
    )]
    pub fn p04(&mut self, t: &mut Vec<String>) -> Outcome {
        let po = self.start("P-04")?;
        let a = po.enroll(self.driver, "facundo@radiopost.org", "KK4ODA", "Facundo")?;
        let b = po.enroll(self.driver, "shelter3@radiopost.org", "N3CALL", "Shelter 3")?;
        let gw = po.gateway(&a)?;
        let to_a = mail(&b.address, &a.address, "Water at the school", 40);
        let r = po.hand_in(&gw, &b, &[&to_a])?;
        if r["results"][0]["status"] != "accepted" {
            return Err(format!(
                "a message for an account of the same post office not accepted: {r}"
            ));
        }
        let out = mail(&a.address, "maria@example.com", "We are fine", 40);
        let second = mail(&a.address, "maria@example.com", "And the dog", 20);
        let other = mail(&a.address, "maria@example.com", "Not offered", 10);
        let theirs = mail(&b.address, "maria@example.com", "Not his", 10);
        // the checks, in RP-1 §6.3's order
        let mut body = Po::session_body(&a, GATEWAY, &[&out, &theirs], Po::time_form(), now());
        let mut stmt = Statement::decode(
            &base64url::decode(body["statement"].as_str().unwrap_or("")).unwrap_or_default(),
        )
        .ok_or("the tester's statement")?;
        stmt.messages.truncate(1);
        let mut fewer = body.clone();
        fewer["statement"] = base64url::encode(&stmt.encode()).into();
        problem(
            &po.call("POST", "/sessions", Some(&gw.token), Some(&fewer))?,
            422,
            "signature-invalid",
            "a message removed after signing",
        )?;
        let elsewhere = Po::session_body(&a, "N2CALL-5", &[&out], Po::time_form(), now());
        problem(
            &po.call("POST", "/sessions", Some(&gw.token), Some(&elsewhere))?,
            403,
            "not-authorized",
            "a statement naming another gateway",
        )?;
        body["started"] = (now() - 601).into();
        problem(
            &po.call("POST", "/sessions", Some(&gw.token), Some(&body))?,
            422,
            "time-skew",
            "`started` 601 s from the statement's time",
        )?;
        let early = Po::session_body(
            &a,
            GATEWAY,
            &[&out],
            Form::Time {
                time: now() - 601,
                nonce: random(),
            },
            now() - 601,
        );
        problem(
            &po.call("POST", "/sessions", Some(&gw.token), Some(&early))?,
            422,
            "time-skew",
            "the post office's clock 601 s after `started`",
        )?;
        let good = Po::session_body(
            &a,
            GATEWAY,
            &[&out, &theirs, &second],
            Po::time_form(),
            now(),
        );
        let (status, s) = po.call("POST", "/sessions", Some(&gw.token), Some(&good))?;
        t.push(format!("POST /sessions → {status} {s}"));
        if status != 201 {
            return Err(format!("a good session answered {status}"));
        }
        problem(
            &po.call("POST", "/sessions", Some(&gw.token), Some(&good))?,
            422,
            "time-skew",
            "the same statement again: a replay",
        )?;
        let sid = s["session"].as_str().unwrap_or("").to_owned();
        let waiting = s["waiting"].as_array().cloned().unwrap_or_default();
        let entry = waiting
            .iter()
            .find(|e| e["id"] == to_a.content_id().text().as_str())
            .ok_or("the message for the account is not in `waiting` (RP-5 §5.2)")?;
        if entry["from"] != b.address.as_str() || entry["third_party"] != false {
            return Err(format!("the waiting entry is wrong: {entry} (RP-5 §5.2)"));
        }
        if !s["accept"].as_array().is_some_and(|a| {
            a.iter()
                .any(|x| x["id"] == out.content_id().text().as_str())
        }) {
            return Err("the offer is not in `accept`".into());
        }
        let serial = entry["serial"].as_u64().unwrap_or(0);
        let (status, m) = po.call(
            "GET",
            &format!("/sessions/{sid}/messages/{serial}"),
            Some(&gw.token),
            None,
        )?;
        let air = m["air"]
            .as_str()
            .and_then(base64url::decode)
            .unwrap_or_default();
        if status != 200
            || Message::from_air(&air).ok().map(|m| m.content_id()) != Some(to_a.content_id())
        {
            return Err("GET messages/{serial} does not give the message (RP-5 §5.3)".into());
        }
        // RP-5 §5.3: compression 0 or 1, which every station takes; the size listed is its own
        if !matches!(air.first(), Some(0 | 1)) {
            return Err(format!(
                "the air form's compression is {:?}, not 0 or 1 (RP-5 §5.3)",
                air.first()
            ));
        }
        if entry["size"].as_u64() != Some(air.len() as u64) {
            return Err(format!(
                "the waiting entry's size is not its air form's, {} bytes: {entry} (RP-5 §5.2)",
                air.len()
            ));
        }
        problem(
            &po.call(
                "GET",
                &format!("/sessions/{sid}/messages/999"),
                Some(&gw.token),
                None,
            )?,
            404,
            "unknown-message",
            "a serial not listed",
        )?;
        let mut bent = second.air_form(Compression::None);
        if let Some(last) = bent.last_mut() {
            *last ^= 1;
        }
        let item = |m: &Message| json!({"id": m.content_id().text(), "air": base64url::encode(&m.air_form(Compression::None))});
        let items = json!({"messages": [item(&out), item(&out),
            {"id": other.content_id().text(), "air": base64url::encode(&other.air_form(Compression::None))},
            item(&theirs),
            {"id": second.content_id().text(), "air": base64url::encode(&bent)}]});
        let (status, r) = po.call(
            "POST",
            &format!("/sessions/{sid}/messages"),
            Some(&gw.token),
            Some(&items),
        )?;
        t.push(format!("hand in → {status} {r}"));
        let results: Vec<(String, String)> = r["results"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|x| {
                (
                    x["status"].as_str().unwrap_or("").to_owned(),
                    x["name"].as_str().unwrap_or("").to_owned(),
                )
            })
            .collect();
        // RP-5 §5.4's order: in the statement, its air form its ID, from the address; then new
        let want = [
            ("accepted", ""),
            ("duplicate", ""),
            ("refused", "not-authorized"),
            ("refused", "not-authorized"),
            ("refused", "malformed"),
        ];
        if results.len() != want.len()
            || results
                .iter()
                .zip(want)
                .any(|((s, n), (ws, wn))| s != ws || (!wn.is_empty() && n != wn))
        {
            return Err(format!(
                "hand-in results {results:?} where RP-5 §5.4 gives {want:?}"
            ));
        }
        problem(
            &po.call(
                "POST",
                &format!("/sessions/{sid}/pickups"),
                Some(&gw.token),
                Some(&json!({"serials": [999]})),
            )?,
            404,
            "unknown-message",
            "a pickup of a serial not listed",
        )?;
        let (status, _) = po.call(
            "POST",
            &format!("/sessions/{sid}/pickups"),
            Some(&gw.token),
            Some(&json!({"serials": [serial]})),
        )?;
        if status != 204 {
            return Err(format!("a pickup answered {status} (RP-5 §5.5)"));
        }
        let (status, _) = po.call("DELETE", &format!("/sessions/{sid}"), Some(&gw.token), None)?;
        if status != 204 {
            return Err(format!("DELETE /sessions answered {status} (RP-5 §5.7)"));
        }
        let again = po.open(&gw, &a, &[])?;
        if again["waiting"]
            .as_array()
            .is_some_and(|w| w.iter().any(|e| e["serial"].as_u64() == Some(serial)))
        {
            return Err("a message picked up is still waiting (RP-5 §5.5)".into());
        }
        // a statement in challenge form, taken once (RP-5 §5.2)
        let challenge = Form::Challenge(random());
        let c = Po::session_body(&a, GATEWAY, &[], challenge.clone(), now());
        let first = po.call("POST", "/sessions", Some(&gw.token), Some(&c))?;
        if first.0 != 201 {
            return Err(format!(
                "a statement in challenge form answered {} {} (RP-5 §5.2)",
                first.0, first.1
            ));
        }
        problem(
            &po.call(
                "POST",
                "/sessions",
                Some(&gw.token),
                Some(&Po::session_body(&a, GATEWAY, &[], challenge, now())),
            )?,
            422,
            "signature-invalid",
            "the same challenge twice",
        )?;
        Ok(())
    }

    /// P-07: station-to-station sessions reported (RP-5 §6.6).
    ///
    /// # Errors
    ///
    /// The first deviation found.
    #[allow(
        clippy::many_single_char_names,
        clippy::too_many_lines,
        reason = "a, the user; t, the transcript; r, an answer; the report, in order"
    )]
    pub fn p07(&mut self, t: &mut Vec<String>) -> Outcome {
        let po = self.start("P-07")?;
        let a = po.enroll(self.driver, "facundo@radiopost.org", "KK4ODA", "Facundo")?;
        let ana_key = new_key();
        let ana_cred = self.offices.issue(
            "ana@hampost.org",
            "N0CALL",
            &ana_key.verifying_key().to_bytes(),
        );
        let sent = {
            let m = mail(&a.address, "ana@hampost.org", "On the air", 20);
            let mut map = m.map().clone();
            if let Value::Map(entries) = &mut map {
                entries.push((
                    Value::Uint(4),
                    Value::Array(vec![Value::text("maria@example.com")]),
                ));
            }
            Message::from_map(&map).map_err(|_| "a message with a copy")?
        };
        let received = mail("ana@hampost.org", &a.address, "Heard you", 20);
        let not_mine = mail(
            "ana@hampost.org",
            "someone@radiopost.org",
            "Not for you",
            10,
        );
        let started = now();
        let nonce: [u8; 8] = random();
        let mine = Statement {
            address: a.address.clone(),
            operator: a.operator.clone(),
            gateway: "N0CALL".into(),
            messages: vec![sent.content_id()],
            form: Form::Time {
                time: started,
                nonce,
            },
        };
        let theirs = |messages: Vec<ContentId>| Statement {
            address: "ana@hampost.org".into(),
            operator: "N0CALL".into(),
            gateway: a.operator.clone(),
            messages,
            form: Form::Time {
                time: started,
                nonce,
            },
        };
        let proof = |s: &Statement, key: &SigningKey, cred: &[u8]| {
            json!({
            "statement": base64url::encode(&s.encode()), "signature": base64url::encode(&s.sign(key)),
            "credential": base64url::encode(cred)})
        };
        let item = |m: &Message| json!({"id": m.content_id().text(), "air": base64url::encode(&m.air_form(Compression::None))});
        let session = json!({
            "started": started, "role": "caller",
            "mine": proof(&mine, &a.key, &a.credential),
            "theirs": proof(&theirs(vec![received.content_id(), not_mine.content_id()]), &ana_key, &ana_cred),
            "sent": [item(&sent)], "received": [item(&received), item(&not_mine)],
        });
        let (status, r) = po.call(
            "POST",
            "/direct",
            Some(&a.token),
            Some(&json!({"sessions": [session.clone()]})),
        )?;
        t.push(format!("POST /direct → {status} {r}"));
        let results = r["sessions"][0]["results"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let ok = status == 200
            && results.len() == 3
            && results[0]["status"] == "accepted"
            && results[0]["sent_on"] == json!(["maria@example.com"])
            && results[1]["status"] == "accepted"
            && results[1]["serial"].is_u64()
            && results[2]["name"] == "not-authorized";
        if !ok {
            return Err(format!(
                "the report's results {results:?}: sent accepted and sent on to Maria only, received filed with a serial, one not for the user refused (RP-5 §6.6)"
            ));
        }
        let (_, again) = po.call(
            "POST",
            "/direct",
            Some(&a.token),
            Some(&json!({"sessions": [session.clone()]})),
        )?;
        let statuses: Vec<&str> = again["sessions"][0]["results"]
            .as_array()
            .map_or(Vec::new(), |r| {
                r.iter().filter_map(|x| x["status"].as_str()).collect()
            });
        if statuses.first() != Some(&"duplicate") || statuses.get(1) != Some(&"duplicate") {
            return Err(format!(
                "the same report again: {statuses:?}, where every message is a duplicate"
            ));
        }
        let mut bad = session;
        bad["theirs"]["signature"] = base64url::encode(&[0u8; 64]).into();
        let (_, r) = po.call(
            "POST",
            "/direct",
            Some(&a.token),
            Some(&json!({"sessions": [bad]})),
        )?;
        if r["sessions"][0]["name"] != "signature-invalid" {
            return Err(format!(
                "a station called's proof that does not verify: {} where signature-invalid was due",
                r["sessions"][0]
            ));
        }
        Ok(())
    }

    /// P-08: the door: each refusal in order, and the accept list (RP-6 §2.1, §2.2).
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn p08(&mut self, t: &mut Vec<String>) -> Outcome {
        let po = self.start("P-08")?;
        let file: Json = serde_json::from_str(crate::EMAIL_DOOR_JSON).map_err(|e| e.to_string())?;
        let mut accounts = Vec::new();
        for (address, list) in file["context"]["accounts"]
            .as_object()
            .cloned()
            .unwrap_or_default()
        {
            let user = po.enroll(
                self.driver,
                &address,
                if address.starts_with("facundo") {
                    "KK4ODA"
                } else {
                    "N3CALL"
                },
                "",
            )?;
            let (status, _) = po.call(
                "POST",
                "/accept-list",
                Some(&user.token),
                Some(&json!({"add": list})),
            )?;
            if status != 200 {
                return Err(format!("POST /accept-list answered {status}"));
            }
            accounts.push(user);
        }
        // a message this post office sent, for the delivery report about it
        let gw = po.gateway(&accounts[0])?;
        let sent = mail(
            &accounts[0].address,
            "maria@example.com",
            "Re: Are you safe?",
            20,
        );
        po.hand_in(&gw, &accounts[0], &[&sent])?;
        let message_id = wait_sent(&po, 1)?
            .into_iter()
            .next()
            .and_then(|m| mailserver::header(&mailserver::split(&m.email).0, "Message-ID"))
            .ok_or("the post office sent no email with a Message-ID")?;
        for case in file["cases"].as_array().cloned().unwrap_or_default() {
            let what = case["what"].as_str().unwrap_or("");
            if case["max_email_bytes"].is_u64() {
                t.push(format!(
                    "— skipped: {what} (the post office's own max_email_bytes)"
                ));
                continue;
            }
            let email = case["email"]
                .as_str()
                .unwrap_or("")
                .replace("<kdj2eowb2e7wqq4y5ttq@radiopost.org>", &message_id);
            let dkim: Vec<String> = case["dkim_pass"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|d| d.as_str().map(str::to_owned))
                .collect();
            let from = case["mail_from"].as_str().filter(|f| !f.is_empty());
            let (rcpt, data) = po.door(
                from,
                case["rcpt_to"].as_str().unwrap_or(""),
                &email,
                case["dmarc"].as_str().unwrap_or("none"),
                &dkim,
            )?;
            t.push(format!("{what}: RCPT {rcpt}, DATA {data:?}"));
            let want_data = case["expect"]["data"].as_str().map(str::to_owned);
            if rcpt != case["expect"]["rcpt"].as_str().unwrap_or("") || data != want_data {
                return Err(format!(
                    "{what}: RCPT {rcpt}, DATA {data:?} where RP-6 §2.1 gives {}",
                    case["expect"]
                ));
            }
        }
        Ok(())
    }

    /// P-09: email to message and back, `Radio-Path`, the part between post offices, bounces.
    ///
    /// # Errors
    ///
    /// The first deviation found.
    #[allow(clippy::too_many_lines, reason = "RP-6 §2.4, §3 and §4, in order")]
    pub fn p09(&mut self, t: &mut Vec<String>) -> Outcome {
        let po = self.start("P-09")?;
        let file: Json = serde_json::from_str(crate::EMAIL_IN_JSON).map_err(|e| e.to_string())?;
        let a = po.enroll(self.driver, "facundo@radiopost.org", "KK4ODA", "Facundo")?;
        let gw = po.gateway(&a)?;
        let emails = file["emails"].as_array().cloned().unwrap_or_default();
        let senders: Vec<String> = emails
            .iter()
            .filter_map(|e| {
                let (h, _) = mailserver::split(e["email"].as_str()?);
                let from = mailserver::header(&h, "From")?;
                Some(
                    from.rsplit_once('<')
                        .map_or(from.clone(), |(_, a)| a.trim_end_matches('>').to_owned())
                        .to_lowercase(),
                )
            })
            .collect();
        po.call(
            "POST",
            "/accept-list",
            Some(&a.token),
            Some(&json!({"add": senders})),
        )?;
        po.call(
            "PUT",
            "/settings",
            Some(&a.token),
            Some(&json!({"attach_bytes": file["context"]["attach_bytes"]})),
        )?;
        for e in &emails {
            let raw = e["email"].as_str().unwrap_or("");
            let (h, _) = mailserver::split(raw);
            let from = mailserver::header(&h, "From").unwrap_or_default();
            let from = from
                .rsplit_once('<')
                .map_or(from.clone(), |(_, a)| a.trim_end_matches('>').to_owned());
            let (rcpt, data) = po.door(Some(&from), &a.address, raw, "pass", &[])?;
            if data.as_deref() != Some("250 2.0.0") {
                return Err(format!(
                    "{}: the door answered {rcpt} / {data:?} to mail from a sender on the list",
                    e["name"]
                ));
            }
        }
        let held = po.waiting(&gw, &a)?;
        for e in &emails {
            let name = e["name"].as_str().unwrap_or("");
            let want = Message::from_canonical(
                &crate::harness::unhex(e["canonical_hex"].as_str().unwrap_or(""))
                    .unwrap_or_default(),
            )
            .map_err(|_| "a vector's message")?;
            let subject = |m: &Message| {
                m.map()
                    .get(key::SUBJECT)
                    .and_then(Value::as_text)
                    .map(str::to_owned)
            };
            let (entry, got) = held
                .iter()
                .find(|(_, m)| subject(m) == subject(&want))
                .ok_or_else(|| {
                    format!("{name}: no waiting message with its subject (RP-6 §2.4)")
                })?;
            if entry["third_party"] != true {
                return Err(format!(
                    "{name}: a message made from an email not flagged third-party (RP-6 §5.1)"
                ));
            }
            let arrival = file["context"]["arrival"].as_u64();
            let comparable = |m: &Message| -> Vec<(Value, Value)> {
                m.map()
                    .as_map()
                    .unwrap_or(&[])
                    .iter()
                    .filter(|(k, v)| {
                        let k = k.as_i64().unwrap_or(0);
                        !(k == key::REPLY_TO
                            || (k == key::BODY && e["exact"] == false)
                            || (k == key::DATE && v.as_u64() == arrival)
                            || (k == key::DATE
                                && want.map().get(key::DATE).and_then(Value::as_u64) == arrival))
                    })
                    .map(|(k, v)| {
                        if k.as_i64() == Some(key::HELD) {
                            let refless = v
                                .as_array()
                                .unwrap_or(&[])
                                .iter()
                                .map(|h| {
                                    Value::int_map(
                                        h.as_map()
                                            .unwrap_or(&[])
                                            .iter()
                                            .filter(|(hk, _)| hk.as_i64() != Some(4))
                                            .filter_map(|(hk, hv)| Some((hk.as_i64()?, hv.clone())))
                                            .collect(),
                                    )
                                })
                                .collect();
                            (k.clone(), Value::Array(refless))
                        } else {
                            (k.clone(), v.clone())
                        }
                    })
                    .collect()
            };
            if comparable(got) != comparable(&want) {
                t.push(format!("{name}: made {}", got.map().diagnostic()));
                t.push(format!("{name}: due  {}", want.map().diagnostic()));
                return Err(format!(
                    "{name}: the message made is not the one RP-6 §2.4 gives"
                ));
            }
        }
        // out: by radio to Maria, and to Ana at a RadioPost post office (with the part, §4)
        let to_maria = mail(&a.address, "maria@example.com", "We are fine", 20);
        let to_ana = mail(&a.address, "ana@hampost.org", "Hola Ana", 20);
        let opened = Po::session_body(&a, GATEWAY, &[&to_maria, &to_ana], Po::time_form(), now());
        let (_, s) = po.call("POST", "/sessions", Some(&gw.token), Some(&opened))?;
        let sid = s["session"].as_str().unwrap_or("").to_owned();
        let listed: Vec<Json> = [&to_maria, &to_ana]
            .iter()
            .map(|m| json!({"id": m.content_id().text(), "air": base64url::encode(&m.air_form(Compression::None))}))
            .collect();
        let items = json!({"messages": listed});
        po.call(
            "POST",
            &format!("/sessions/{sid}/messages"),
            Some(&gw.token),
            Some(&items),
        )?;
        let took = wait_sent(&po, 2)?;
        for (m, to) in [
            (&to_maria, "maria@example.com"),
            (&to_ana, "ana@hampost.org"),
        ] {
            let taken = took
                .iter()
                .find(|x| x.to == [to])
                .ok_or_else(|| format!("no email to {to} (RP-6 §3)"))?;
            t.push(format!("email to {to}:\n{}", taken.email));
            let (h, _) = mailserver::split(&taken.email);
            let get = |n: &str| mailserver::header(&h, n).unwrap_or_default();
            let cid = m.content_id().text();
            let started = opened["started"].as_u64().unwrap_or(0);
            let path = format!(
                "KK4ODA via {GATEWAY}; at={}; session={SESSION_ID}",
                iso(started)
            );
            if taken.from != a.address
                || get("From") != "Facundo <facundo@radiopost.org>"
                || get("To") != to
                || get("Message-ID") != format!("<{cid}@radiopost.org>")
                || get("Radio-Path") != path
                || get("Subject")
                    != m.map()
                        .get(key::SUBJECT)
                        .and_then(Value::as_text)
                        .unwrap_or("")
            {
                return Err(format!(
                    "the email to {to}: From, To, Subject, Message-ID or Radio-Path not as RP-6 §3.1, §3.2 give (Radio-Path due: {path})"
                ));
            }
            let text_part = mailserver::parts(&taken.email)
                .into_iter()
                .find(|(ph, _)| {
                    mailserver::header(ph, "Content-Type")
                        .unwrap_or_else(|| "text/plain".into())
                        .starts_with("text/plain")
                })
                .ok_or("no text/plain part")?;
            let text =
                String::from_utf8_lossy(&mailserver::decoded_body(&text_part.0, &text_part.1))
                    .replace("\r\n", "\n");
            let body = m
                .map()
                .get(key::BODY)
                .and_then(Value::as_text)
                .unwrap_or("");
            if !text.starts_with(body) || !text.contains("\n-- \n") {
                return Err(format!(
                    "the email to {to}: its text is not the message's, then the signature line (RP-6 §3.1)"
                ));
            }
            let part = mailserver::parts(&taken.email).into_iter().find(|(ph, _)| {
                mailserver::header(ph, "Content-Type")
                    .is_some_and(|c| c.starts_with("application/radiopost-message"))
            });
            match (to, part) {
                ("ana@hampost.org", Some((ph, pb))) => {
                    let bytes = mailserver::decoded_body(&ph, &pb);
                    let map = radiopost::cbor::decode(&bytes).map_err(|_| "the part is not deterministic CBOR (RP-6 §4)")?;
                    let field = |k: i64| map.get(k).and_then(Value::as_bytes).map(<[u8]>::to_vec).unwrap_or_default();
                    let ok = Message::from_air(&field(1)).is_ok_and(|x| x.content_id() == m.content_id())
                        && field(2) == a.credential
                        && Some(base64url::encode(&field(3))) == opened["statement"].as_str().map(str::to_owned)
                        && map.get(5).and_then(Value::as_u64) == Some(started);
                    if !ok {
                        return Err("the part is not the message's air form, the sender's credential, the session's statement and `started` (RP-6 §4)".into());
                    }
                }
                ("ana@hampost.org", None) => return Err("no application/radiopost-message part, though every recipient is at a RadioPost post office it knows (RP-6 §4)".into()),
                (_, Some(_)) => return Err("a message part sent to an address outside RadioPost (RP-6 §4)".into()),
                _ => {}
            }
        }
        // the bounce about the email to Maria (RP-6 §3.3)
        let maria_email = took
            .iter()
            .find(|x| x.to == ["maria@example.com"])
            .map(|x| x.email.clone())
            .unwrap_or_default();
        let mid = mailserver::header(&mailserver::split(&maria_email).0, "Message-ID")
            .unwrap_or_default();
        let report = format!(
            "From: Mail Delivery System <MAILER-DAEMON@mx.example.com>\r\nTo: {}\r\nSubject: Undelivered\r\n\
             MIME-Version: 1.0\r\nContent-Type: multipart/report; report-type=delivery-status; boundary=\"r\"\r\n\r\n\
             --r\r\nContent-Type: text/plain\r\n\r\nNot delivered.\r\n--r\r\nContent-Type: message/delivery-status\r\n\r\n\
             Reporting-MTA: dns; mx.example.com\r\n\r\nFinal-Recipient: rfc822; maria@example.com\r\nAction: failed\r\n\
             Status: 5.2.2\r\nDiagnostic-Code: smtp; 552 5.2.2 Mailbox full\r\n--r\r\nContent-Type: text/rfc822-headers\r\n\r\n\
             From: Facundo <facundo@radiopost.org>\r\nSubject: We are fine\r\nMessage-ID: {mid}\r\n--r--\r\n",
            a.address
        );
        let (_, data) = po.door(None, &a.address, &report, "none", &[])?;
        if data.as_deref() != Some("250 2.0.0") {
            return Err(format!(
                "a delivery report about a message it sent refused at the door: {data:?} (RP-6 §2.1)"
            ));
        }
        let held = po.waiting(&gw, &a)?;
        let bounce = held
            .iter()
            .map(|(_, m)| m)
            .find(|m| m.from() == Some("postmaster@radiopost.org"))
            .ok_or("no message from postmaster@ after the bounce (RP-6 §3.3)")?;
        let map = bounce.map();
        let ok = map.get(key::SUBJECT).and_then(Value::as_text)
            == Some("Not delivered: We are fine")
            && map.get(key::BODY).and_then(Value::as_text)
                == Some("maria@example.com: 552 5.2.2 Mailbox full\n")
            && map.get(key::REPLY_TO).and_then(Value::as_bytes)
                == Some(to_maria.content_id().as_bytes());
        if !ok {
            return Err(format!(
                "the bounce's message is not as RP-6 §3.3 gives: {}",
                map.diagnostic()
            ));
        }
        Ok(())
    }

    /// P-10: third-party mail flagged (RP-6 §5.1, RP-5 §5.2).
    ///
    /// # Errors
    ///
    /// The first deviation found.
    pub fn p10(&mut self, t: &mut Vec<String>) -> Outcome {
        let po = self.start("P-10")?;
        let a = po.enroll(self.driver, "facundo@radiopost.org", "KK4ODA", "Facundo")?;
        let b = po.enroll(self.driver, "shelter3@radiopost.org", "N3CALL", "")?;
        let gw = po.gateway(&a)?;
        po.call(
            "POST",
            "/accept-list",
            Some(&a.token),
            Some(&json!({"add": ["maria@example.com"]})),
        )?;
        let by_radio = mail(&b.address, &a.address, "By radio", 10);
        po.hand_in(&gw, &b, &[&by_radio])?;
        let email = "From: Maria <maria@example.com>\r\nTo: facundo@radiopost.org\r\nSubject: By email\r\n\
                     Date: Mon, 09 Nov 2026 14:40:00 +0000\r\nMIME-Version: 1.0\r\n\
                     Content-Type: text/plain; charset=utf-8\r\n\r\nHello.\r\n";
        po.door(Some("maria@example.com"), &a.address, email, "pass", &[])?;
        let held = po.waiting(&gw, &a)?;
        for (entry, m) in &held {
            t.push(format!("{entry}"));
            let subject = m
                .map()
                .get(key::SUBJECT)
                .and_then(Value::as_text)
                .unwrap_or("");
            let want = subject == "By email";
            if entry["third_party"] != want {
                return Err(format!(
                    "“{subject}”: third_party {} where {want} was due (RP-6 §5.1)",
                    entry["third_party"]
                ));
            }
        }
        if held.len() != 2 {
            return Err(format!("{} messages waiting where 2 were due", held.len()));
        }
        Ok(())
    }
}

/// What the stand-in mail server took, once `n` emails have come (within 45 seconds).
fn wait_sent(po: &Po, n: usize) -> Result<Vec<mailserver::Taken>, String> {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let took = po.mail.took();
        if took.len() >= n {
            return Ok(took);
        }
        if Instant::now() > deadline {
            return Err(format!(
                "{} email(s) submitted in 45 s, where {n} were due (RP-6 §3)",
                took.len()
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// ISO 8601 in UTC.
fn iso(seconds: u64) -> String {
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let rem = seconds % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}
