// SPDX-License-Identifier: MIT OR Apache-2.0
//! A gateway on Aether claims its callsign again when Aether restarts (RP-4 §2.1).
//!
//! Found on the air (2026-11-12): two Setup saves restarted the gateway's Aether, and RadioPost
//! Gateway went on listening on an adapter whose Aether had gone. Aether, the claim gone, answered
//! the next call to KK4ODA-1 itself, and the caller's opening reached nobody.

use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use radiopost_gateway::monitor::Monitor;
use radiopost_gateway::station::{self, Settings};
use serde_json::{Value, json};
use tungstenite::Message;

/// What Aether asks of each WebSocket: the claims made on it, and whether it stays up.
fn serve(stream: TcpStream, claims: &mpsc::Sender<(usize, Value)>, n: usize, restart: bool) {
    let Ok(mut ws) = tungstenite::accept(stream) else {
        return;
    };
    while let Ok(message) = ws.read() {
        let Message::Text(text) = message else {
            continue;
        };
        let request: Value = serde_json::from_str(&text).expect("JSON");
        let method = request["method"].as_str().unwrap_or("");
        let result = match method {
            "status" => json!({"version": "0.2.0-beta.146", "state": "idle", "owner": null,
                               "channel_busy": false, "claims": []}),
            "capabilities" => json!({"bandwidth_hz": 2300, "bandwidths_hz": [2300, 500]}),
            "claim" => {
                let _ = claims.send((n, request["params"]["callsigns"].clone()));
                json!({"id": n, "callsigns": request["params"]["callsigns"],
                       "refused": [], "listening": false})
            }
            _ => json!({}),
        };
        let reply = json!({"id": request["id"], "ok": true, "result": result});
        if ws.send(Message::text(reply.to_string())).is_err() {
            return;
        }
        // the first connection ends as a daemon restarted by a Setup save does: dropped
        if restart && method == "listen" {
            return;
        }
    }
}

#[test]
fn a_gateway_claims_again_after_aether_restarts() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let aether = format!("ws://{}/v1", listener.local_addr().expect("bound"));
    let (tx, claims) = mpsc::channel();
    thread::spawn(move || {
        for (n, stream) in listener.incoming().flatten().enumerate() {
            let tx = tx.clone();
            thread::spawn(move || serve(stream, &tx, n, n == 0));
        }
    });

    let dir: PathBuf = std::env::temp_dir().join(format!(
        "radiopost-modem-restart-{}-{}",
        std::process::id(),
        station::now()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    station::make(
        &dir,
        &Settings {
            callsign: "KK4ODA-1".into(),
            modem: aether,
            country: "US".into(),
            post_offices: std::collections::BTreeMap::new(),
            resolver: "https://dns.invalid/resolve".into(),
            blocked: Vec::new(),
            only: None,
            operator_checks_rules: false,
            answer_at_start: true,
            copies: None,
        },
    )
    .expect("a gateway");
    // a credential is all the answering loop reads before it opens the modem
    station::write(&dir, "credential.hex", "00").expect("written");

    let monitor = Arc::new(Monitor::new());
    let running = {
        let monitor = Arc::clone(&monitor);
        let dir = dir.clone();
        thread::spawn(move || station::run(&dir, &monitor, &mut |_| {}))
    };
    let first = claims
        .recv_timeout(Duration::from_secs(10))
        .expect("the gateway claimed its callsign");
    assert_eq!(first, (0, json!(["KK4ODA-1"])));
    // Aether goes away after the gateway turned listen on; the gateway opens it again
    let started = Instant::now();
    let again = claims.recv_timeout(Duration::from_secs(20));
    monitor.stop();
    let ended = running.join().expect("the answering loop");
    let _ = std::fs::remove_dir_all(&dir);
    let again = again.expect("the gateway never claimed again after Aether restarted");
    assert_eq!(
        again,
        (1, json!(["KK4ODA-1"])),
        "claimed on the new connection"
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    assert!(ended.is_ok(), "{ended:?}");
}
