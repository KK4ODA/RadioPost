// SPDX-License-Identifier: MIT OR Apache-2.0
//! TLS for links on the Internet (RP-4 §9.5): the answering adapter's certificate, and how a caller
//! checks it, by the roots of the web or by a fingerprint its operator was given.

use std::sync::Arc;

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, ring};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, ServerConfig, SignatureScheme};
use sha2::{Digest, Sha256};

fn provider() -> Arc<CryptoProvider> {
    Arc::new(ring::default_provider())
}

/// A certificate's SHA-256 fingerprint, as operators exchange it: 64 hexadecimal digits.
#[must_use]
pub fn fingerprint(der: &[u8]) -> String {
    use std::fmt::Write;
    Sha256::digest(der)
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// A fingerprint as an operator writes it: hexadecimal, with or without colons, any case.
///
/// # Errors
///
/// It is not 32 bytes in hexadecimal.
pub fn parse_fingerprint(text: &str) -> Result<[u8; 32], String> {
    let digits: String = text.chars().filter(|c| *c != ':').collect();
    let bytes: Option<Vec<u8>> = (0..digits.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(digits.get(i..i + 2)?, 16).ok())
        .collect();
    bytes
        .filter(|b| digits.len() == 64 && b.len() == 32)
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| format!("{text} is not a SHA-256 fingerprint (64 hexadecimal digits)"))
}

/// A certificate made for this adapter alone, and its key, both in PEM, with its fingerprint: what
/// an operator with no certificate of a web authority gives the operators who call it.
///
/// # Errors
///
/// The certificate could not be made.
pub fn self_signed() -> Result<(String, String, String), String> {
    let made = rcgen::generate_simple_self_signed(vec!["radiopost-tcp".to_owned()])
        .map_err(|e| e.to_string())?;
    Ok((
        made.cert.pem(),
        made.signing_key.serialize_pem(),
        fingerprint(made.cert.der()),
    ))
}

/// The answering side: a certificate chain and its key, in PEM. Returns the configuration and the
/// fingerprint of the first certificate.
///
/// # Errors
///
/// The PEM does not hold a certificate and a key, or they do not go together.
pub fn server(cert_pem: &[u8], key_pem: &[u8]) -> Result<(Arc<ServerConfig>, String), String> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
        .collect::<Result<_, _>>()
        .map_err(|e| format!("the certificate: {e:?}"))?;
    let first = certs
        .first()
        .ok_or("no certificate in the certificate file")?;
    let print = fingerprint(first);
    let key = PrivateKeyDer::from_pem_slice(key_pem).map_err(|e| format!("the key: {e:?}"))?;
    let config = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| e.to_string())?;
    Ok((Arc::new(config), print))
}

/// The calling side: a certificate passes when its fingerprint is one of `pins`, or else when it
/// chains to the roots of the web and names the host called.
///
/// # Errors
///
/// TLS could not be set up.
pub fn client(pins: Vec<[u8; 32]>) -> Result<Arc<ClientConfig>, String> {
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let web = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider())
        .build()
        .map_err(|e| e.to_string())?;
    let config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned { pins, web }))
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// The name a caller checks the certificate against: the host of `host:port`.
///
/// # Errors
///
/// The host is not a name or an address.
pub fn server_name(addr: &str) -> Result<ServerName<'static>, String> {
    let host = addr.rsplit_once(':').map_or(addr, |(h, _)| h);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    ServerName::try_from(host.to_owned()).map_err(|_| format!("{host} is not a host name"))
}

#[derive(Debug)]
struct Pinned {
    pins: Vec<[u8; 32]>,
    web: Arc<WebPkiServerVerifier>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let print: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        if self.pins.contains(&print) {
            return Ok(ServerCertVerified::assertion());
        }
        self.web
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.web.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.web.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.web.supported_verify_schemes()
    }
}
