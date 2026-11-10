// SPDX-License-Identifier: MIT OR Apache-2.0
//! The sender checks the door makes itself (RP-6 §2.1) when the mail server hands it no
//! verdicts, as Stalwart 1.1 does not: SPF (RFC 7208) for the envelope sender, and DKIM
//! (RFC 6376) and DMARC (RFC 7489) for the message, from the connecting client's address and
//! EHLO name, the envelope sender and the message as the client sent it. They are made with
//! `mail-auth`, the library Stalwart's authors publish, asking the system's DNS resolver.
//!
//! `Authentication-Results` headers that came with the message are never read: whoever sends a
//! message can write them.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::Hash;
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use mail_auth::dmarc::verify::DmarcParameters;
use mail_auth::spf::verify::SpfParameters;
use mail_auth::{
    AuthenticatedMessage, DkimResult, DmarcResult, MessageAuthenticator, Parameters, ResolverCache,
    Txt,
};

/// What the checks found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The DMARC result for the message's `From:` domain: `pass`, `fail`, `none` (the domain
    /// publishes no policy), `temperror` (a lookup failed for now) or `permerror`.
    pub dmarc: String,
    /// The domains a DKIM signature passes for, in lowercase.
    pub dkim_pass: Vec<String>,
}

/// The TXT records looked up (DKIM keys, SPF and DMARC policies), kept until they expire.
#[derive(Default)]
pub struct TxtCache(Mutex<HashMap<Box<str>, (Txt, Instant)>>);

/// More names than this, and the expired ones go; still more, and all go.
const CACHE_MAX: usize = 4096;

impl TxtCache {
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<Box<str>, (Txt, Instant)>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `record` at `name`, as a lookup would have found it, for an hour: how tests give the
    /// checks their DNS.
    pub fn put(&self, name: &str, record: impl Into<Txt>) {
        let mut name = name.to_lowercase();
        if !name.ends_with('.') {
            name.push('.');
        }
        let until = Instant::now() + std::time::Duration::from_secs(3600);
        self.insert(name.into_boxed_str(), record.into(), until);
    }
}

impl ResolverCache<Box<str>, Txt> for TxtCache {
    fn get<Q>(&self, name: &Q) -> Option<Txt>
    where
        Box<str>: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.map()
            .get(name)
            .filter(|(_, until)| *until > Instant::now())
            .map(|(record, _)| record.clone())
    }

    fn remove<Q>(&self, name: &Q) -> Option<Txt>
    where
        Box<str>: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.map().remove(name).map(|(record, _)| record)
    }

    fn insert(&self, name: Box<str>, record: Txt, valid_until: Instant) {
        let mut map = self.map();
        if map.len() >= CACHE_MAX {
            let now = Instant::now();
            map.retain(|_, (_, until)| *until > now);
            if map.len() >= CACHE_MAX {
                map.clear();
            }
        }
        map.insert(name, (record, valid_until));
    }
}

/// Makes the checks, with a DNS resolver and a small runtime of its own.
pub struct Checker {
    runtime: tokio::runtime::Runtime,
    authenticator: MessageAuthenticator,
    cache: TxtCache,
    host: String,
}

impl Checker {
    /// A checker asking the system's resolver, for a mail server that receives as `host` (the
    /// name SPF's `%{r}` macro gives).
    ///
    /// # Errors
    ///
    /// No runtime, or the system's resolver settings cannot be read.
    pub fn new(host: &str) -> Result<Checker, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|e| format!("no runtime for the sender checks: {e}"))?;
        let authenticator = {
            let _inside = runtime.enter();
            MessageAuthenticator::new_system_conf()
                .map_err(|e| format!("no DNS resolver for the sender checks: {e}"))?
        };
        Ok(Checker {
            runtime,
            authenticator,
            cache: TxtCache::default(),
            host: host.to_owned(),
        })
    }

    /// The records it has looked up.
    #[must_use]
    pub fn cache(&self) -> &TxtCache {
        &self.cache
    }

    /// The checks for `raw`, sent by the client at `ip` that gave `helo` in EHLO, from the
    /// envelope sender `sender` (empty for the null sender).
    #[must_use]
    pub fn check(&self, ip: IpAddr, helo: &str, sender: &str, raw: &[u8]) -> Found {
        self.runtime.block_on(self.checks(ip, helo, sender, raw))
    }

    async fn checks(&self, ip: IpAddr, helo: &str, sender: &str, raw: &[u8]) -> Found {
        let Some(message) = AuthenticatedMessage::parse(raw) else {
            return Found {
                dmarc: "permerror".into(),
                dkim_pass: Vec::new(),
            };
        };
        let cache = &self.cache;
        let dkim = self
            .authenticator
            .verify_dkim(Parameters::new(&message).with_txt_cache(cache))
            .await;
        let spf = self
            .authenticator
            .verify_spf(
                Parameters::new(SpfParameters::verify_mail_from(
                    ip, helo, &self.host, sender,
                ))
                .with_txt_cache(cache),
            )
            .await;
        let mail_from_domain = sender.rsplit_once('@').map_or(helo, |(_, d)| d);
        let dmarc = self
            .authenticator
            .verify_dmarc(
                Parameters::new(DmarcParameters::new(
                    &message,
                    &dkim,
                    mail_from_domain,
                    &spf,
                ))
                .with_txt_cache(cache),
            )
            .await;
        let mut dkim_pass: Vec<String> = Vec::new();
        for output in &dkim {
            if *output.result() == DkimResult::Pass
                && let Some(signature) = output.signature()
            {
                let domain = signature.d.to_lowercase();
                if !dkim_pass.contains(&domain) {
                    dkim_pass.push(domain);
                }
            }
        }
        let dmarc = match dmarc.result() {
            DmarcResult::Pass => "pass",
            DmarcResult::Fail(_) => "fail",
            DmarcResult::TempError(_) => "temperror",
            DmarcResult::PermError(_) => "permerror",
            DmarcResult::None => "none",
        };
        Found {
            dmarc: dmarc.into(),
            dkim_pass,
        }
    }
}

#[cfg(test)]
mod tests {
    use mail_auth::common::crypto::Ed25519Key;
    use mail_auth::common::headers::HeaderWriter as _;
    use mail_auth::common::parse::TxtRecordParser as _;
    use mail_auth::common::verify::DomainKey;
    use mail_auth::dkim::DkimSigner;
    use mail_auth::dmarc::Dmarc;
    use mail_auth::spf::Spf;

    use super::{Checker, Found};

    const SEED: [u8; 32] = [7; 32];
    const CLIENT: &str = "192.0.2.1";
    const EMAIL: &str = "From: Maria <maria@example.com>\r\nTo: facundo@radiopost.org\r\n\
                         Subject: Are you safe?\r\n\r\nWe are fine.\r\n";

    /// Maria's email, signed for `example.com` with the key at `s1._domainkey.example.com`.
    fn signed(email: &str) -> Vec<u8> {
        let key = ed25519_dalek::SigningKey::from_bytes(&SEED);
        let ed = Ed25519Key::from_seed_and_public_key(&SEED, key.verifying_key().as_bytes())
            .expect("a key");
        let signature = DkimSigner::from_key(ed)
            .domain("example.com")
            .selector("s1")
            .headers(["From", "To", "Subject"])
            .sign(email.as_bytes())
            .expect("a signature");
        let mut raw = signature.to_header().into_bytes();
        raw.extend_from_slice(email.as_bytes());
        raw
    }

    /// A checker whose DNS holds example.com's key, its SPF (only 192.0.2.1 sends) and,
    /// with `policy`, its DMARC record; every other name does not exist.
    fn checker(policy: Option<&str>) -> Checker {
        let c = Checker::new("po.radiopost.org").expect("a checker");
        let key = ed25519_dalek::SigningKey::from_bytes(&SEED);
        let txt = format!(
            "v=DKIM1; k=ed25519; p={}",
            radiopost::base64url::encode(key.verifying_key().as_bytes())
                .replace('-', "+")
                .replace('_', "/")
        );
        c.cache().put(
            "s1._domainkey.example.com",
            DomainKey::parse(txt.as_bytes()).expect("a DKIM record"),
        );
        c.cache().put(
            "example.com",
            Spf::parse(b"v=spf1 ip4:192.0.2.1 -all").expect("an SPF record"),
        );
        if let Some(p) = policy {
            c.cache().put(
                "_dmarc.example.com",
                Dmarc::parse(format!("v=DMARC1; p={p}").as_bytes()).expect("a DMARC record"),
            );
        }
        c
    }

    fn found(dmarc: &str, dkim: &[&str]) -> Found {
        Found {
            dmarc: dmarc.into(),
            dkim_pass: dkim.iter().map(|d| (*d).to_owned()).collect(),
        }
    }

    #[test]
    fn a_signed_message_passes_dkim_and_dmarc() {
        let c = checker(Some("none"));
        let got = c.check(
            CLIENT.parse().expect("an address"),
            "mail.example.com",
            "maria@example.com",
            &signed(EMAIL),
        );
        assert_eq!(got, found("pass", &["example.com"]));
    }

    #[test]
    fn spf_alone_passes_dmarc() {
        let c = checker(Some("none"));
        let got = c.check(
            CLIENT.parse().expect("an address"),
            "mail.example.com",
            "maria@example.com",
            EMAIL.as_bytes(),
        );
        assert_eq!(got, found("pass", &[]));
    }

    /// A forger's server, no signature: DMARC fails, whatever the policy says to do about it.
    #[test]
    fn a_forgery_fails_dmarc_under_a_policy_of_none() {
        let c = checker(Some("none"));
        let got = c.check(
            "203.0.113.9".parse().expect("an address"),
            "forger.example.net",
            "maria@example.com",
            EMAIL.as_bytes(),
        );
        assert_eq!(got, found("fail", &[]));
    }

    #[test]
    fn a_domain_without_a_policy_gives_none() {
        let c = checker(None);
        let got = c.check(
            "203.0.113.9".parse().expect("an address"),
            "forger.example.net",
            "maria@example.com",
            EMAIL.as_bytes(),
        );
        assert_eq!(got, found("none", &[]));
    }

    /// A message changed after it was signed does not pass DKIM.
    #[test]
    fn a_changed_message_does_not_pass_dkim() {
        let c = checker(Some("reject"));
        let mut raw = signed(EMAIL);
        let at = raw.len() - 4;
        raw[at] = b'X';
        let got = c.check(
            "203.0.113.9".parse().expect("an address"),
            "forger.example.net",
            "maria@example.com",
            &raw,
        );
        assert_eq!(got, found("fail", &[]));
    }

    /// The sender's own `Authentication-Results` count for nothing.
    #[test]
    fn authentication_results_in_the_message_are_not_read() {
        let c = checker(Some("none"));
        let raw = format!(
            "Authentication-Results: po.radiopost.org; dkim=pass header.d=example.com; \
             dmarc=pass header.from=example.com\r\n{EMAIL}"
        );
        let got = c.check(
            "203.0.113.9".parse().expect("an address"),
            "forger.example.net",
            "maria@example.com",
            raw.as_bytes(),
        );
        assert_eq!(got, found("fail", &[]));
    }

    /// A lookup that fails for now (`mail-auth`'s test resolver fails any name with
    /// `_dns_error.` in it) gives `temperror`.
    #[test]
    fn a_failed_lookup_is_a_temporary_error() {
        let c = checker(None);
        let email = EMAIL.replace("maria@example.com", "maria@_dns_error.example.com");
        let got = c.check(
            "203.0.113.9".parse().expect("an address"),
            "forger.example.net",
            "maria@_dns_error.example.com",
            email.as_bytes(),
        );
        assert_eq!(got.dmarc, "temperror");
    }
}
