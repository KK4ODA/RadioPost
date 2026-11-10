// SPDX-License-Identifier: MIT OR Apache-2.0
//! `radiopost-po check`: a post office looked at from outside, as on the day it goes live. Its
//! DNS records (RP-1 §7.1, RP-6 §1) and the reverse DNS of its mail host; its API over HTTPS,
//! with the signed `info` document checked against its own key; the door, called with its
//! token; and the mail server's submission port, signed in to as the post office and asked to
//! take mail from a user's address, then told to forget it, so no mail is sent.
//!
//! The judgements are functions of what was found, so that each can be tested; the lookups and
//! connections around them are thin.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use lettre::Address;
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::client::{SmtpConnection, TlsParameters};
use lettre::transport::smtp::commands::{Mail, Rset};
use lettre::transport::smtp::extension::ClientId;
use serde_json::{Value as Json, json};

use crate::serve::Settings;

/// How one check came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// As it should be.
    Ok,
    /// Wrong, or missing: the post office is not ready.
    Fail,
    /// Worth knowing, or not checked.
    Note,
}

/// One check: how it came out, in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// How it came out.
    pub verdict: Verdict,
    /// What was found.
    pub text: String,
}

fn ok(text: impl Into<String>) -> Line {
    Line {
        verdict: Verdict::Ok,
        text: text.into(),
    }
}

fn fail(text: impl Into<String>) -> Line {
    Line {
        verdict: Verdict::Fail,
        text: text.into(),
    }
}

fn note(text: impl Into<String>) -> Line {
    Line {
        verdict: Verdict::Note,
        text: text.into(),
    }
}

/// The host of a URL: what lies between `://` and the next `/` or `:`.
#[must_use]
pub fn host_of(url: &str) -> String {
    url.split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split(['/', ':', '?'])
        .next()
        .unwrap_or_default()
        .to_lowercase()
}

fn bare(name: &str) -> String {
    name.trim_end_matches('.').to_lowercase()
}

/// The `_radiopost` records (RP-1 §7.1): one `t=api` record naming the base URL, and a `t=key`
/// record for the post office's own key.
#[must_use]
pub fn radiopost_records(
    domain: &str,
    txt: &[String],
    base_url: &str,
    public: &[u8; 32],
) -> Vec<Line> {
    let tags: Vec<_> = txt
        .iter()
        .filter_map(|t| radiopost::dns::parse(t))
        .collect();
    let api: Vec<&String> = tags
        .iter()
        .filter(|t| t.get("t").map(String::as_str) == Some("api"))
        .filter_map(|t| t.get("url"))
        .collect();
    let mut out = vec![match api.as_slice() {
        [] => fail(format!(
            "_radiopost.{domain}: no `t=api` record (publish what `init` printed)"
        )),
        [url] if url.as_str() == base_url => ok(format!("_radiopost.{domain}: api {url}")),
        [url] => fail(format!(
            "_radiopost.{domain}: api {url}, where config.json says {base_url}"
        )),
        _ => fail(format!(
            "_radiopost.{domain}: {} `t=api` records, where there must be one",
            api.len()
        )),
    }];
    let kid = radiopost::cbor::hex(&radiopost::crypto::kid(public));
    let ours = tags
        .iter()
        .filter(|t| t.get("t").map(String::as_str) == Some("key"))
        .any(|t| radiopost::dns::key_of(t).as_ref() == Some(public));
    out.push(if ours {
        ok(format!(
            "_radiopost.{domain}: key {kid}, the post office's own"
        ))
    } else {
        fail(format!(
            "_radiopost.{domain}: no `t=key` record for the post office's key {kid}"
        ))
    });
    out
}

/// The domain's MX records: present, and not the null MX (RFC 7505). The hosts they name.
#[must_use]
pub fn mx(domain: &str, answers: &[String]) -> (Line, Vec<String>) {
    let hosts: Vec<String> = answers
        .iter()
        .filter_map(|a| a.split_whitespace().nth(1))
        .map(bare)
        .collect();
    let line = if hosts.is_empty() {
        fail(format!(
            "{domain}: no MX record, so no mail is delivered here"
        ))
    } else if hosts.iter().any(String::is_empty) {
        fail(format!(
            "{domain}: the null MX (`0 .`), which says this domain takes no mail"
        ))
    } else {
        ok(format!("{domain}: MX {}", answers.join(", ")))
    };
    (line, hosts)
}

/// The domain's SPF record (RFC 7208): exactly one, and not one that lets no server send.
#[must_use]
pub fn spf(domain: &str, txt: &[String]) -> Line {
    let records: Vec<&String> = txt
        .iter()
        .filter(|t| t.to_lowercase().starts_with("v=spf1"))
        .collect();
    match records.as_slice() {
        [] => fail(format!(
            "{domain}: no SPF record (RP-6 §1: `v=spf1 mx -all`)"
        )),
        [r] if r.split_whitespace().skip(1).all(|m| m.ends_with("all")) => fail(format!(
            "{domain}: SPF \"{r}\" lets no server send for the domain"
        )),
        [r] => ok(format!("{domain}: SPF \"{r}\"")),
        _ => fail(format!(
            "{domain}: {} SPF records, which makes them all invalid",
            records.len()
        )),
    }
}

/// The domain's DMARC record (RFC 7489): present, with its policy.
#[must_use]
pub fn dmarc(domain: &str, txt: &[String]) -> Line {
    match txt.iter().find(|t| t.starts_with("v=DMARC1")) {
        None => fail(format!("_dmarc.{domain}: no DMARC record")),
        Some(r) => ok(format!("_dmarc.{domain}: \"{r}\"")),
    }
}

/// A DKIM key record (RFC 6376 §3.6.1): present, and its key type (`rsa` unless it says).
#[must_use]
pub fn dkim(name: &str, txt: &[String]) -> (Line, Option<String>) {
    let Some(record) = txt.iter().find(|t| t.contains("p=")) else {
        return (fail(format!("{name}: no DKIM key")), None);
    };
    let tags: HashMap<String, String> = record
        .split(';')
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_owned()))
        .collect();
    if tags.get("p").is_none_or(String::is_empty) {
        return (fail(format!("{name}: the key is revoked (empty p=)")), None);
    }
    let kind = tags.get("k").cloned().unwrap_or_else(|| "rsa".into());
    (ok(format!("{name}: a DKIM key, {kind}")), Some(kind))
}

/// The DNS name to ask for an address's PTR record.
#[must_use]
pub fn reverse_name(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            format!("{}.{}.{}.{}.in-addr.arpa", o[3], o[2], o[1], o[0])
        }
        IpAddr::V6(v6) => {
            let mut nibbles: Vec<String> = v6
                .octets()
                .iter()
                .flat_map(|b| [b >> 4, b & 0xf])
                .map(|n| format!("{n:x}"))
                .collect();
            nibbles.reverse();
            format!("{}.ip6.arpa", nibbles.join("."))
        }
    }
}

/// Reverse DNS for one of the mail host's addresses (RP-6 §1): it names the host.
#[must_use]
pub fn ptr(host: &str, ip: IpAddr, names: &[String]) -> Line {
    let names: Vec<String> = names.iter().map(|n| bare(n)).collect();
    if names.iter().any(|n| *n == bare(host)) {
        ok(format!("{ip}: reverse DNS {host}"))
    } else if names.is_empty() {
        fail(format!(
            "{ip}: no reverse DNS; set it to {host} at the provider"
        ))
    } else {
        fail(format!(
            "{ip}: reverse DNS {}, where it must name {host}",
            names.join(", ")
        ))
    }
}

/// The `info` document as fetched (RP-5 §4.1): a JWS the post office's own key verifies.
#[must_use]
pub fn info(url: &str, status: u16, body: &str, public: &[u8; 32]) -> Line {
    if status != 200 {
        return fail(format!(
            "{url}: HTTP {status} (is the post office running, and Caddy in front of it?)"
        ));
    }
    let kid = radiopost::cbor::hex(&radiopost::crypto::kid(public));
    let keys = HashMap::from([(kid.clone(), *public)]);
    match radiopost::jws::verify(body.trim(), &keys) {
        Ok(_) => ok(format!("{url}: signed by key {kid}")),
        Err(e) => fail(format!(
            "{url}: the document does not verify with key {kid} ({})",
            e.name()
        )),
    }
}

/// The door's answer to a call for `postmaster@` (RP-6 §2.1), which it lets through.
#[must_use]
pub fn door_answer(at: &str, status: u16, body: &str) -> Line {
    let accepted = serde_json::from_str::<Json>(body).is_ok_and(|j| j["action"] == "accept");
    match status {
        200 if accepted => ok(format!("the door at {at} answers, and the token is right")),
        401 => fail(format!(
            "the door at {at} refuses the token: give Stalwart's hook the one in config.json"
        )),
        _ => fail(format!("the door at {at} answered {status} {body}")),
    }
}

/// The mail server's submission port, as `smtp` in config.json names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submission {
    /// The host to connect to, and the name its certificate must carry.
    pub host: String,
    /// The port.
    pub port: u16,
    /// `smtps://`: TLS from the first byte.
    pub implicit_tls: bool,
    /// `?tls=required` or `opportunistic`: STARTTLS.
    pub starttls: bool,
    /// The account signed in as.
    pub user: String,
    /// Its password.
    pub password: String,
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(v) = text
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `smtp://` or `smtps://`, an account and password, a host and port, and `?tls=…`.
///
/// # Errors
///
/// Another scheme, or no host.
pub fn submission(url: &str) -> Result<Submission, String> {
    let (scheme, rest) = url.split_once("://").ok_or("`smtp` is not a URL")?;
    let implicit_tls = match scheme {
        "smtps" => true,
        "smtp" => false,
        other => return Err(format!("`smtp` has the scheme {other}, not smtp or smtps")),
    };
    let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
    let rest = rest.trim_end_matches('/');
    let (userinfo, hostport) = rest.rsplit_once('@').unwrap_or(("", rest));
    let (user, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().map_err(|_| format!("`smtp`: the port {p}"))?),
        None => (hostport, if implicit_tls { 465 } else { 587 }),
    };
    if host.is_empty() {
        return Err("`smtp` names no host".into());
    }
    let tls = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("tls="))
        .unwrap_or("");
    Ok(Submission {
        host: host.to_owned(),
        port,
        implicit_tls,
        starttls: !implicit_tls && matches!(tls, "required" | "opportunistic"),
        user: percent_decode(user),
        password: percent_decode(password),
    })
}

/// What the mail server said when asked to take mail from `sender`, signed in as `user`.
#[must_use]
pub fn mail_from(user: &str, sender: &str, reply: Result<u16, (u16, String)>) -> Line {
    match reply {
        Ok(code) if (200..300).contains(&code) => ok(format!(
            "submission: signed in as {user}, and may send as {sender}"
        )),
        Ok(code) | Err((code, _)) if code == 501 || code == 550 || code == 553 => fail(format!(
            "submission: {user} may not send as {sender} ({code}): in Stalwart's AUTH stage, \
             Must Match Sender needs `false` when authenticated_as == '{user}'"
        )),
        Ok(code) => fail(format!("submission: MAIL FROM <{sender}> answered {code}")),
        Err((code, text)) => fail(format!(
            "submission: MAIL FROM <{sender}> answered {code} {text}"
        )),
    }
}

/// What came of signing in.
#[must_use]
pub fn sign_in(user: &str, at: &str, error: &str) -> Line {
    if error.contains("535") || error.to_lowercase().contains("credentials") {
        fail(format!(
            "submission: {at} refuses {user}'s password: the account's password in Stalwart \
             and the one in config.json's `smtp` differ"
        ))
    } else {
        fail(format!(
            "submission: cannot sign in to {at} as {user}: {error}"
        ))
    }
}

/// Every check, in order. `resolver` answers DNS over HTTPS; `selectors` are the DKIM selectors
/// to look for (Stalwart's zone file names them).
#[must_use]
pub fn run(
    settings: &Settings,
    public: &[u8; 32],
    resolver: &str,
    selectors: &[String],
) -> Vec<Line> {
    let mut out = Vec::new();
    let ask = |name: &str, kind: &str, out: &mut Vec<Line>| -> Vec<String> {
        radiopost_api::query(resolver, name, kind).unwrap_or_else(|e| {
            out.push(fail(format!("DNS: {e}")));
            Vec::new()
        })
    };
    let api_host = host_of(&settings.base_url);
    for domain in &settings.domains {
        let txt = ask(&format!("_radiopost.{domain}"), "TXT", &mut out);
        out.extend(radiopost_records(domain, &txt, &settings.base_url, public));
        let answers = ask(domain, "MX", &mut out);
        let (line, hosts) = mx(domain, &answers);
        out.push(line);
        let txt = ask(domain, "TXT", &mut out);
        out.push(spf(domain, &txt));
        let txt = ask(&format!("_dmarc.{domain}"), "TXT", &mut out);
        out.push(dmarc(domain, &txt));
        let mut kinds = Vec::new();
        for s in selectors {
            let name = format!("{s}._domainkey.{domain}");
            let txt = ask(&name, "TXT", &mut out);
            let (line, kind) = dkim(&name, &txt);
            out.push(line);
            kinds.extend(kind);
        }
        if selectors.is_empty() {
            out.push(note(format!(
                "{domain}: DKIM not checked; give the selectors in Stalwart's zone file with --dkim"
            )));
        } else if !kinds.iter().any(|k| k == "rsa") {
            out.push(fail(format!(
                "{domain}: no RSA DKIM key among the selectors (RP-6 §1)"
            )));
        }
        let mail_hosts = if hosts.iter().any(|h| !h.is_empty()) {
            hosts.into_iter().filter(|h| !h.is_empty()).collect()
        } else {
            vec![api_host.clone()]
        };
        for host in mail_hosts {
            let mut ips: Vec<IpAddr> = Vec::new();
            for kind in ["A", "AAAA"] {
                ips.extend(
                    ask(&host, kind, &mut out)
                        .iter()
                        .filter_map(|a| a.parse::<IpAddr>().ok()),
                );
            }
            if ips.is_empty() {
                out.push(fail(format!("{host}: no address in DNS")));
            }
            for ip in ips {
                let names = ask(&reverse_name(ip), "PTR", &mut out);
                out.push(ptr(&host, ip, &names));
            }
        }
    }
    let url = format!("{}/info", settings.base_url.trim_end_matches('/'));
    out.push(fetch_info(&url, public));
    match (&settings.hooks, &settings.hook_token) {
        (Some(at), token) => out.push(door(at, token.as_deref(), &settings.domains)),
        (None, _) => out.push(fail(
            "no `hooks` in config.json: no door, so no email comes in",
        )),
    }
    match &settings.smtp {
        Some(url) => out.push(smtp(url, &api_host, &settings.domains)),
        None => out.push(fail(
            "no `smtp` in config.json: email to the Internet waits in `outgoing`",
        )),
    }
    out
}

fn fetch_info(url: &str, public: &[u8; 32]) -> Line {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .into();
    match agent.get(url).call() {
        Ok(mut r) => {
            let status = r.status().as_u16();
            let body = r.body_mut().read_to_string().unwrap_or_default();
            info(url, status, &body, public)
        }
        Err(e) => fail(format!("{url}: {e}")),
    }
}

/// Calls the door at `at` as Stalwart would, at `RCPT TO` for `postmaster@` the first domain.
#[must_use]
pub fn door(at: &str, token: Option<&str>, domains: &[String]) -> Line {
    let domain = domains.first().map_or("", String::as_str);
    let body = json!({
        "context": {"stage": "rcpt", "protocol": {"version": 1},
                    "client": {"ip": "127.0.0.1", "helo": "localhost"}},
        "envelope": {"from": {"address": "check@example.com"},
                     "to": [{"address": format!("postmaster@{domain}")}]}
    });
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut request = agent.post(format!("http://{at}/"));
    if let Some(t) = token {
        request = request.header("Authorization", format!("Bearer {t}"));
    }
    match request.send(body.to_string()) {
        Ok(mut r) => {
            let status = r.status().as_u16();
            let text = r.body_mut().read_to_string().unwrap_or_default();
            door_answer(at, status, &text)
        }
        Err(e) => fail(format!(
            "the door at {at}: {e} (is `radiopost-po serve` running?)"
        )),
    }
}

/// Signs in to the submission port and asks it to take mail from `postmaster@` the first
/// domain, then resets: nothing is sent.
#[must_use]
pub fn smtp(url: &str, hello: &str, domains: &[String]) -> Line {
    let s = match submission(url) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let at = format!("{}:{}", s.host, s.port);
    let tls = match TlsParameters::new(s.host.clone()) {
        Ok(t) => t,
        Err(e) => return fail(format!("submission: {e}")),
    };
    let hello = ClientId::Domain(hello.to_owned());
    let timeout = Some(Duration::from_secs(15));
    let connected = SmtpConnection::connect(
        (s.host.as_str(), s.port),
        timeout,
        &hello,
        s.implicit_tls.then_some(&tls),
        None,
    );
    let mut conn = match connected {
        Ok(c) => c,
        Err(e) => return fail(format!("submission: cannot connect to {at}: {e}")),
    };
    if s.starttls
        && let Err(e) = conn.starttls(&tls, &hello)
    {
        return fail(format!("submission: STARTTLS at {at}: {e}"));
    }
    if !s.user.is_empty() {
        let credentials = Credentials::new(s.user.clone(), s.password.clone());
        if let Err(e) = conn.auth(&[Mechanism::Plain, Mechanism::Login], &credentials) {
            return sign_in(&s.user, &at, &e.to_string());
        }
    }
    let sender = format!("postmaster@{}", domains.first().map_or("", String::as_str));
    let Ok(address) = sender.parse::<Address>() else {
        return fail(format!("submission: {sender} is not an address"));
    };
    let reply = match conn.command(Mail::new(Some(address), Vec::new())) {
        Ok(r) => Ok(u16::from(r.code())),
        Err(e) => Err((e.status().map_or(0, u16::from), e.to_string())),
    };
    let _ = conn.command(Rset);
    let _ = conn.quit();
    mail_from(&s.user, &sender, reply)
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use ed25519_dalek::SigningKey;

    use super::{
        Verdict, dkim, dmarc, door_answer, info, mail_from, mx, ptr, radiopost_records,
        reverse_name, spf, submission,
    };

    fn verdicts(lines: &[super::Line]) -> Vec<Verdict> {
        lines.iter().map(|l| l.verdict).collect()
    }

    fn txt(records: &[&str]) -> Vec<String> {
        records.iter().map(|r| (*r).to_owned()).collect()
    }

    const BASE: &str = "https://po.radiopost.org/rp/v1";

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7; 32])
    }

    fn key_record(public: &[u8; 32]) -> String {
        let kid = radiopost::cbor::hex(&radiopost::crypto::kid(public));
        let p = radiopost::base64url::encode(public)
            .replace('-', "+")
            .replace('_', "/");
        format!("v=RP1; t=key; kid={kid}; p={p}=")
    }

    #[test]
    fn the_radiopost_records_name_the_base_url_and_the_key() {
        let public = key().verifying_key().to_bytes();
        let good = txt(&[&format!("v=RP1; t=api; url={BASE}"), &key_record(&public)]);
        assert_eq!(
            verdicts(&radiopost_records("radiopost.org", &good, BASE, &public)),
            [Verdict::Ok, Verdict::Ok]
        );
        let other = SigningKey::from_bytes(&[8; 32]).verifying_key().to_bytes();
        let wrong = txt(&[
            "v=RP1; t=api; url=https://elsewhere.example/rp/v1",
            &key_record(&other),
        ]);
        assert_eq!(
            verdicts(&radiopost_records("radiopost.org", &wrong, BASE, &public)),
            [Verdict::Fail, Verdict::Fail]
        );
        assert_eq!(
            verdicts(&radiopost_records("radiopost.org", &[], BASE, &public)),
            [Verdict::Fail, Verdict::Fail]
        );
    }

    #[test]
    fn the_null_mx_takes_no_mail() {
        let (line, hosts) = mx("radiopost.org", &txt(&["10 po.radiopost.org."]));
        assert_eq!(
            (line.verdict, hosts),
            (Verdict::Ok, vec!["po.radiopost.org".to_owned()])
        );
        assert_eq!(mx("radiopost.org", &txt(&["0 ."])).0.verdict, Verdict::Fail);
        assert_eq!(mx("radiopost.org", &[]).0.verdict, Verdict::Fail);
    }

    #[test]
    fn spf_must_be_one_record_that_lets_a_server_send() {
        assert_eq!(spf("d", &txt(&["v=spf1 mx -all"])).verdict, Verdict::Ok);
        assert_eq!(spf("d", &txt(&["v=spf1 -all"])).verdict, Verdict::Fail);
        assert_eq!(
            spf("d", &txt(&["v=spf1 mx -all", "v=spf1 a -all"])).verdict,
            Verdict::Fail
        );
        assert_eq!(
            spf("d", &txt(&["google-site-verification=x"])).verdict,
            Verdict::Fail
        );
    }

    #[test]
    fn dmarc_and_dkim_records() {
        assert_eq!(
            dmarc(
                "d",
                &txt(&["v=DMARC1; p=quarantine; rua=mailto:postmaster@d"])
            )
            .verdict,
            Verdict::Ok
        );
        assert_eq!(dmarc("d", &[]).verdict, Verdict::Fail);
        let (line, kind) = dkim(
            "s._domainkey.d",
            &txt(&["v=DKIM1; k=rsa; h=sha256; p=MIIB"]),
        );
        assert_eq!((line.verdict, kind.as_deref()), (Verdict::Ok, Some("rsa")));
        let (_, kind) = dkim("s._domainkey.d", &txt(&["v=DKIM1; k=ed25519; p=KFTz"]));
        assert_eq!(kind.as_deref(), Some("ed25519"));
        assert_eq!(
            dkim("s._domainkey.d", &txt(&["v=DKIM1; p="])).0.verdict,
            Verdict::Fail
        );
        assert_eq!(dkim("s._domainkey.d", &[]).0.verdict, Verdict::Fail);
    }

    #[test]
    fn reverse_dns_names_the_host() {
        let v4: IpAddr = "2.25.117.37".parse().expect("an address");
        assert_eq!(reverse_name(v4), "37.117.25.2.in-addr.arpa");
        let v6: IpAddr = "2a02:4780:75:d929::1".parse().expect("an address");
        assert_eq!(
            reverse_name(v6),
            "1.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.9.2.9.d.5.7.0.0.0.8.7.4.2.0.a.2.ip6.arpa"
        );
        let host = "po.radiopost.org";
        assert_eq!(
            ptr(host, v4, &txt(&["po.radiopost.org."])).verdict,
            Verdict::Ok
        );
        assert_eq!(
            ptr(host, v4, &txt(&["PO.RadioPost.org"])).verdict,
            Verdict::Ok
        );
        assert_eq!(
            ptr(host, v4, &txt(&["srv2147182.hstgr.cloud."])).verdict,
            Verdict::Fail
        );
        assert_eq!(ptr(host, v4, &[]).verdict, Verdict::Fail);
    }

    #[test]
    fn the_info_document_verifies_with_the_post_offices_key() {
        let k = key();
        let public = k.verifying_key().to_bytes();
        let jws = radiopost::jws::sign(br#"{"version":1}"#, &k);
        assert_eq!(info("u", 200, &jws, &public).verdict, Verdict::Ok);
        let other = SigningKey::from_bytes(&[8; 32]).verifying_key().to_bytes();
        assert_eq!(info("u", 200, &jws, &other).verdict, Verdict::Fail);
        assert_eq!(info("u", 502, "", &public).verdict, Verdict::Fail);
    }

    #[test]
    fn the_doors_answers() {
        assert_eq!(
            door_answer("a", 200, r#"{"action":"accept"}"#).verdict,
            Verdict::Ok
        );
        assert_eq!(door_answer("a", 401, "").verdict, Verdict::Fail);
        assert_eq!(
            door_answer("a", 200, r#"{"action":"reject"}"#).verdict,
            Verdict::Fail
        );
    }

    #[test]
    fn the_submission_url_is_read_as_lettre_reads_it() {
        let s = submission("smtps://radiopost:abc%40123@po.radiopost.org:465").expect("a URL");
        assert_eq!(
            (s.host.as_str(), s.port, s.implicit_tls, s.starttls),
            ("po.radiopost.org", 465, true, false)
        );
        assert_eq!(
            (s.user.as_str(), s.password.as_str()),
            ("radiopost", "abc@123")
        );
        let s = submission("smtp://u:p@127.0.0.1:587?tls=required").expect("a URL");
        assert_eq!((s.port, s.implicit_tls, s.starttls), (587, false, true));
        let s = submission("smtp://127.0.0.1:2525").expect("a URL");
        assert_eq!((s.user.as_str(), s.starttls), ("", false));
        assert!(submission("http://x").is_err());
    }

    #[test]
    fn sending_as_a_user_must_be_allowed() {
        let u = "radiopost";
        let s = "postmaster@radiopost.org";
        assert_eq!(mail_from(u, s, Ok(250)).verdict, Verdict::Ok);
        let refused = mail_from(u, s, Err((501, "501 5.5.4 You are not allowed".into())));
        assert_eq!(refused.verdict, Verdict::Fail);
        assert!(
            refused.text.contains("Must Match Sender"),
            "{}",
            refused.text
        );
        assert_eq!(
            mail_from(u, s, Err((421, "421 later".into()))).verdict,
            Verdict::Fail
        );
    }
}
