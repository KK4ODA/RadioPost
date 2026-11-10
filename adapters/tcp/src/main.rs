// SPDX-License-Identifier: MIT OR Apache-2.0
//! `radiopost-tcp`: the TCP adapter (RP-4 §9.5).

use std::collections::HashMap;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitCode;

use radiopost_tcp::{Adapter, Config, DEFAULT_PORT, Security, tls};

const USAGE: &str = "\
usage: radiopost-tcp [--rpmi 127.0.0.1:8600] [--listen ADDRESS] [--plain]
                     [--cert FILE --key FILE] [--trust FINGERPRINT]... [--peer NAME=ADDRESS]...

  --rpmi       where the host (the radiopost client, a gateway) connects
  --listen     take calls from other adapters here, as 0.0.0.0:8774
  --plain      plain TCP, for an amateur mesh network: it is on the air, where nothing is
               encrypted. Without it, links are TLS, for the Internet.
  --cert/--key this adapter's certificate and key (PEM), for calls taken over TLS. Without them
               it makes its own, radiopost-tcp.crt and .key in the current directory, once, and
               prints its fingerprint: give that to the operators who call you.
  --trust      accept the certificate with this SHA-256 fingerprint, besides those of the web's
               authorities
  --peer       a name the host may call: N1CALL-10=gw.example.net:8774

A call's `remote` is CALLSIGN@HOST:PORT, HOST:PORT, or a --peer name.";

struct Options {
    rpmi: String,
    listen: Option<String>,
    plain: bool,
    cert: Option<PathBuf>,
    key: Option<PathBuf>,
    trust: Vec<String>,
    peers: HashMap<String, String>,
}

fn options() -> Result<Options, String> {
    let mut o = Options {
        rpmi: format!("127.0.0.1:{}", rpmi::DEFAULT_PORT),
        listen: None,
        plain: false,
        cert: None,
        key: None,
        trust: Vec::new(),
        peers: HashMap::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--plain" {
            o.plain = true;
            continue;
        }
        let value = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
        match arg.as_str() {
            "--rpmi" => o.rpmi = value,
            "--listen" => o.listen = Some(value),
            "--cert" => o.cert = Some(value.into()),
            "--key" => o.key = Some(value.into()),
            "--trust" => o.trust.push(value),
            "--peer" => {
                let (name, addr) = value
                    .split_once('=')
                    .ok_or_else(|| format!("--peer {value}: NAME=HOST:PORT"))?;
                o.peers.insert(name.to_owned(), addr.to_owned());
            }
            _ => return Err(format!("unknown option {arg}")),
        }
    }
    Ok(o)
}

/// The certificate and key for calls taken over TLS: the files given, or this adapter's own.
fn identity(o: &Options) -> Result<(Vec<u8>, Vec<u8>), String> {
    let cert = o.cert.clone().unwrap_or_else(|| "radiopost-tcp.crt".into());
    let key = o.key.clone().unwrap_or_else(|| "radiopost-tcp.key".into());
    if o.cert.is_none() && o.key.is_none() && !cert.exists() && !key.exists() {
        let (cert_pem, key_pem, _) = tls::self_signed()?;
        std::fs::write(&cert, cert_pem).map_err(|e| format!("{}: {e}", cert.display()))?;
        std::fs::write(&key, key_pem).map_err(|e| format!("{}: {e}", key.display()))?;
        eprintln!(
            "radiopost-tcp: made a certificate for this adapter: {} and {}",
            cert.display(),
            key.display()
        );
    }
    let read = |p: &PathBuf| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    Ok((read(&cert)?, read(&key)?))
}

fn run() -> Result<(), String> {
    let o = options()?;
    let security = if o.plain {
        Security::Plain
    } else {
        let pins = o
            .trust
            .iter()
            .map(|t| tls::parse_fingerprint(t))
            .collect::<Result<_, _>>()?;
        let server = match &o.listen {
            Some(_) => {
                let (cert, key) = identity(&o)?;
                let (server, print) = tls::server(&cert, &key)?;
                eprintln!("radiopost-tcp: this adapter's certificate: SHA-256 {print}");
                Some(server)
            }
            None => None,
        };
        Security::Tls {
            server,
            client: tls::client(pins)?,
        }
    };
    let adapter = Adapter::new(Config {
        security,
        peers: o.peers,
        name: format!(
            "radiopost-tcp {} ({})",
            env!("CARGO_PKG_VERSION"),
            if o.plain { "plain TCP" } else { "TLS" }
        ),
    });
    let hosts = TcpListener::bind(&o.rpmi).map_err(|e| format!("{}: {e}", o.rpmi))?;
    eprintln!(
        "radiopost-tcp: RPMI on {}",
        hosts.local_addr().map_err(|e| e.to_string())?
    );
    adapter.serve_hosts(hosts);
    if let Some(listen) = &o.listen {
        let calls = TcpListener::bind(listen).map_err(|e| format!("{listen}: {e}"))?;
        eprintln!(
            "radiopost-tcp: calls taken on {} ({})",
            calls.local_addr().map_err(|e| e.to_string())?,
            if o.plain { "plain TCP" } else { "TLS" }
        );
        adapter.serve_calls(calls);
    } else {
        eprintln!("radiopost-tcp: no --listen: calls out only (port {DEFAULT_PORT} is usual)");
    }
    loop {
        std::thread::park();
    }
}

fn main() -> ExitCode {
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("radiopost-tcp: {e}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
