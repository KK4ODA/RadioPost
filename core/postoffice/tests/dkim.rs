// SPDX-License-Identifier: MIT OR Apache-2.0
//! The DKIM keys rotated by hand, guided (`radiopost_postoffice::dkim`): against a stand-in for
//! Stalwart's DKIM signatures and a stand-in DNS over HTTPS whose records the test publishes as an
//! operator would.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use radiopost_postoffice::documents::Info;
use radiopost_postoffice::serve::{self, Files, Running, Settings};
use serde_json::{Value as Json, json};

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("radiopost-dkim-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    dir
}

/// What the stand-ins hold: Stalwart's DKIM keys by ID, and DNS's TXT records by name.
#[derive(Default)]
struct World {
    keys: BTreeMap<String, Json>,
    txt: BTreeMap<String, String>,
    /// Stalwart takes a new key active whatever stage it is given.
    ignores_stage: bool,
}

/// Stalwart's management API for domains and DKIM signatures, and DNS over HTTPS, on one port.
fn stand_in(world: Arc<Mutex<World>>) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("a port");
    let at = format!("http://{}", server.server_addr());
    std::thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let url = request.url().to_owned();
            let mut w = world.lock().expect("the world");
            let text = if let Some(query) = url.strip_prefix("/resolve?") {
                let name = query
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("name="))
                    .unwrap_or("")
                    .replace("%2E", ".");
                let answers: Vec<Json> = w
                    .txt
                    .get(&name)
                    .map(|t| {
                        // a long record comes in strings of at most 255 characters
                        let quoted: Vec<String> = t
                            .as_bytes()
                            .chunks(255)
                            .map(|c| format!("\"{}\"", String::from_utf8_lossy(c)))
                            .collect();
                        vec![json!({"name": name, "type": 16, "data": quoted.join(" ")})]
                    })
                    .unwrap_or_default();
                json!({"Status": if answers.is_empty() { 3 } else { 0 }, "Answer": answers})
                    .to_string()
            } else {
                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);
                let asked: Json = serde_json::from_str(&body).unwrap_or(Json::Null);
                let mut answers = Vec::new();
                let mut ids: Vec<Json> = Vec::new();
                for c in asked["methodCalls"].as_array().cloned().unwrap_or_default() {
                    match c[0].as_str() {
                        Some("x:Domain/get") => answers.push(json!(["x:Domain/get",
                            {"list": [{"id": "d1", "name": "test.example"}]}, c[2]])),
                        Some("x:DkimSignature/query") => {
                            assert_eq!(c[1]["filter"], json!({"domainId": "d1"}));
                            ids = w.keys.keys().map(|k| json!(k)).collect();
                            answers.push(json!(["x:DkimSignature/query", {"ids": ids}, c[2]]));
                        }
                        Some("x:DkimSignature/get") => {
                            let list: Vec<Json> = w.keys.values().cloned().collect();
                            answers.push(json!(["x:DkimSignature/get", {"list": list}, c[2]]));
                        }
                        Some("x:DkimSignature/set") => {
                            let mut out = json!({});
                            for (cid, k) in c[1]["create"].as_object().cloned().unwrap_or_default()
                            {
                                let id = format!("k{}", w.keys.len() + 1);
                                let pem = k["privateKey"]["secret"].as_str().unwrap_or("");
                                assert!(pem.starts_with("-----BEGIN PRIVATE KEY-----"), "{pem}");
                                let mut made = k.clone();
                                made["id"] = json!(id);
                                made["publicKey"] = json!(format!("PUB-{id}"));
                                made["createdAt"] = json!("2026-11-15T10:00:00Z");
                                made.as_object_mut().expect("a key").remove("privateKey");
                                if w.ignores_stage {
                                    made["stage"] = json!("active");
                                }
                                w.keys.insert(id.clone(), made);
                                out["created"][cid] = json!({"id": id});
                            }
                            for (id, patch) in
                                c[1]["update"].as_object().cloned().unwrap_or_default()
                            {
                                if let Some(k) = w.keys.get_mut(&id) {
                                    k["stage"] = patch["stage"].clone();
                                }
                                out["updated"][id] = Json::Null;
                            }
                            for id in c[1]["destroy"].as_array().cloned().unwrap_or_default() {
                                w.keys.remove(id.as_str().unwrap_or(""));
                            }
                            answers.push(json!(["x:DkimSignature/set", out, c[2]]));
                        }
                        _ => answers.push(json!(["error", {"type": "unknownMethod"}, c[2]])),
                    }
                }
                let _ = ids;
                json!({"methodResponses": answers}).to_string()
            };
            drop(w);
            let _ = request.respond(tiny_http::Response::from_string(text));
        }
    });
    at
}

fn post_office(dir: &Path, at: &str) -> Arc<Running> {
    let files = Files::new(dir);
    files.write("key.hex", &"07".repeat(32)).expect("written");
    let mailboxes = radiopost_postoffice::mailboxes::MailboxSettings {
        kind: "stalwart".into(),
        url: at.to_owned(),
        api_key: Some("API_admin".into()),
        ..Default::default()
    };
    let settings = Settings {
        domains: vec!["test.example".into()],
        base_url: "https://po.test.example/rp/v1".into(),
        listen: "127.0.0.1:0".into(),
        admin: "127.0.0.1:0".into(),
        info: Info {
            operator: "KK4ODA".into(),
            contact: "postmaster@test.example".into(),
            country: "US".into(),
            max_air_bytes: 65_536,
            max_email_bytes: 100_000,
            gateways: "any".into(),
            mail: json!({}),
            updated: 0,
            not_before: 0,
            not_after: u64::MAX,
            post_offices: Vec::new(),
            applications: None,
        },
        known: Vec::new(),
        blocked: Vec::new(),
        resolver: Some(format!("{at}/resolve")),
        hooks: None,
        hook_token: None,
        smtp: None,
        operators: Vec::new(),
        lookups: radiopost_postoffice::lookups::LookupSettings::default(),
        mailboxes,
        health: radiopost_postoffice::serve::HealthSettings {
            dkim: vec!["v1-rsa-20260101".into(), "v1-ed25519-20260101".into()],
        },
        compress: true,
    };
    files
        .write(
            "config.json",
            &serde_json::to_string(&settings).expect("JSON"),
        )
        .expect("written");
    Arc::new(Running::load(dir, serve::system_draw()).expect("loaded"))
}

/// A rotation, step by step: new keys pending until their records are visible, the switch
/// refused until then, the old keys retiring and kept published a week, the rotation ended only
/// once they are gone from DNS; the hourly check's selectors follow it. A rotation can be stopped
/// before the switch, and a Stalwart that signs with a new key at once is refused.
#[test]
#[allow(clippy::too_many_lines, reason = "one story, told in order")]
fn the_dkim_keys_are_rotated_one_checked_step_at_a_time() {
    let dir = scratch("rotate");
    let world = Arc::new(Mutex::new(World::default()));
    {
        let mut w = world.lock().expect("the world");
        for (id, kind, selector) in [
            ("k0a", "Dkim1RsaSha256", "v1-rsa-20260101"),
            ("k0b", "Dkim1Ed25519Sha256", "v1-ed25519-20260101"),
        ] {
            w.keys.insert(
                id.into(),
                json!({"id": id, "@type": kind, "selector": selector, "stage": "active",
                       "publicKey": format!("OLD-{id}"), "domainId": "d1"}),
            );
            w.txt.insert(
                format!("{selector}._domainkey.test.example"),
                format!("v=DKIM1; k=rsa; h=sha256; p=OLD-{id}"),
            );
        }
    }
    let at = stand_in(Arc::clone(&world));
    let running = post_office(&dir, &at);

    // the keys are old: the hourly check says so
    let dkim_line = || -> String {
        running
            .dkim_lines(serve::now())
            .first()
            .map(|l| l.text.clone())
            .unwrap_or_default()
    };
    let health_dkim = || Files::new(&dir).settings().expect("settings").health.dkim;
    assert!(dkim_line().contains("rotate them"), "{}", dkim_line());

    // started: a new key for each algorithm, pending, signing nothing
    let view = running.dkim_step("start", "KK4ODA").expect("started");
    assert_eq!(view["next"], "publish", "{view}");
    let publish = view["rotation"]["publish"]
        .as_array()
        .expect("records")
        .clone();
    assert_eq!(publish.len(), 2, "{view}");
    let today = radiopost_postoffice::lookups::day(serve::now()).replace('-', "");
    assert_eq!(
        publish[0]["name"],
        format!("v1-rsa-{today}._domainkey.test.example")
    );
    assert!(
        publish[0]["value"]
            .as_str()
            .is_some_and(|v| v.starts_with("v=DKIM1; k=rsa; h=sha256; p=PUB-")),
        "{view}"
    );
    let stages = |w: &World| -> Vec<(String, String)> {
        w.keys
            .values()
            .map(|k| {
                (
                    k["selector"].as_str().unwrap_or("").to_owned(),
                    k["stage"].as_str().unwrap_or("").to_owned(),
                )
            })
            .collect()
    };
    assert!(
        stages(&world.lock().expect("the world"))
            .iter()
            .filter(|(s, _)| s.ends_with(&today))
            .all(|(_, stage)| stage == "pending")
    );
    assert!(
        running
            .dkim_step("start", "KK4ODA")
            .expect_err("one at a time")
            .contains("under way")
    );

    // not published yet: no switch
    let refused = running
        .dkim_step("switch", "KK4ODA")
        .expect_err("not visible");
    assert!(refused.contains("not visible in DNS yet"), "{refused}");
    // one published: still no switch
    let publish_one = |p: &Json| {
        world.lock().expect("the world").txt.insert(
            p["name"].as_str().expect("a name").to_owned(),
            p["value"].as_str().expect("a value").to_owned(),
        );
    };
    publish_one(&publish[0]);
    let refused = running
        .dkim_step("switch", "KK4ODA")
        .expect_err("one missing");
    assert!(
        refused.contains(publish[1]["name"].as_str().expect("a name")),
        "{refused}"
    );
    assert!(
        !refused.contains(publish[0]["name"].as_str().expect("a name")),
        "{refused}"
    );
    // both: switched, the old keys retiring, the check reading all four
    publish_one(&publish[1]);
    let view = running.dkim_step("switch", "KK4ODA").expect("switched");
    assert_eq!(view["next"], "wait", "{view}");
    {
        let w = world.lock().expect("the world");
        for (selector, stage) in stages(&w) {
            let want = if selector.ends_with(&today) {
                "active"
            } else {
                "retiring"
            };
            assert_eq!(stage, want, "{selector}");
        }
    }
    assert_eq!(health_dkim().len(), 4);
    assert!(
        dkim_line().contains("the old records stay published until"),
        "the check says what waits: {}",
        dkim_line()
    );

    // the old records stay a week
    let refused = running
        .dkim_step("removed", "KK4ODA")
        .expect_err("too soon");
    assert!(
        refused.contains("keep the old records published until"),
        "{refused}"
    );
    let rotation_file = dir.join("dkim-rotation.json");
    let mut r: Json = serde_json::from_str(&std::fs::read_to_string(&rotation_file).expect("kept"))
        .expect("JSON");
    r["switched"] = json!(serve::now() - 8 * 86_400);
    std::fs::write(&rotation_file, r.to_string()).expect("written");
    assert!(
        dkim_line().contains("remove the old records"),
        "after a week the check asks for them to go: {}",
        dkim_line()
    );
    // still in DNS: not ended
    let refused = running
        .dkim_step("removed", "KK4ODA")
        .expect_err("still there");
    assert!(refused.contains("still in DNS"), "{refused}");
    {
        let mut w = world.lock().expect("the world");
        w.txt.retain(|name, _| name.contains(&today));
    }
    let view = running.dkim_step("removed", "KK4ODA").expect("ended");
    assert_eq!(view["next"], "start", "{view}");
    assert!(view["rotation"].is_null());
    {
        let w = world.lock().expect("the world");
        for (selector, stage) in stages(&w) {
            let want = if selector.ends_with(&today) {
                "active"
            } else {
                "retired"
            };
            assert_eq!(stage, want, "{selector}");
        }
    }
    assert_eq!(
        health_dkim(),
        [format!("v1-rsa-{today}"), format!("v1-ed25519-{today}")]
    );
    assert_eq!(dkim_line(), "DKIM: the keys are 0 days old");

    // a rotation stopped before the switch leaves nothing behind
    let before = world.lock().expect("the world").keys.len();
    let view = running.dkim_step("start", "KK4ODA").expect("started");
    let names: Vec<String> = view["rotation"]["publish"]
        .as_array()
        .expect("records")
        .iter()
        .filter_map(|p| p["name"].as_str().map(str::to_owned))
        .collect();
    assert!(
        names[0].contains(&format!("v1-rsa-{today}-2")),
        "a fresh selector: {names:?}"
    );
    running.dkim_step("cancel", "KK4ODA").expect("stopped");
    assert_eq!(world.lock().expect("the world").keys.len(), before);
    assert!(!rotation_file.exists());

    // a Stalwart that signs with a new key at once: refused, and the key taken back
    world.lock().expect("the world").ignores_stage = true;
    let refused = running
        .dkim_step("start", "KK4ODA")
        .expect_err("signs at once");
    assert!(
        refused.contains("did not keep the new DKIM keys pending"),
        "{refused}"
    );
    assert_eq!(world.lock().expect("the world").keys.len(), before);
    assert!(!rotation_file.exists());
    let _ = std::fs::remove_dir_all(dir);
}
