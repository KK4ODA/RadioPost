// SPDX-License-Identifier: MIT OR Apache-2.0
//! The service's page: how to reach it, an enrollment code for a callsign, the reports, and the
//! test post office's API under [`API_PATH`]. Plain HTTP, behind an HTTPS front on a server.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::Read as _;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use radiopost::cbor::hex;
use radiopost_postoffice::serve::Running;
use serde_json::{Value as Json, json};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::report::{self, escape};
use crate::{API_PATH, GATEWAY, STATION, Settings, now};

/// At most this many enrollment codes an hour, from everyone together: a test service, not a
/// post office.
pub const CODES_PER_HOUR: usize = 60;

/// What the page says and serves.
pub struct Page {
    /// The service's settings.
    pub settings: Settings,
    /// Its address as the world reaches it.
    pub url: String,
    /// Where calls come in.
    pub air: SocketAddr,
    /// The TLS certificate's fingerprint, when the links are TLS.
    pub fingerprint: Option<String>,
    /// The test post office.
    pub post_office: Arc<Running>,
}

/// Serves the page on `server`, on threads of its own.
pub fn serve(server: Server, page: Page) {
    let page = Arc::new(page);
    let codes = Arc::new(Mutex::new(VecDeque::<u64>::new()));
    let server = Arc::new(server);
    for _ in 0..4 {
        let (server, page, codes) = (server.clone(), page.clone(), codes.clone());
        thread::spawn(move || {
            for request in server.incoming_requests() {
                page.answer(request, &codes);
            }
        });
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("an ASCII header")
}

fn send(request: Request, status: u16, kind: &str, body: String) {
    let response = Response::from_string(body)
        .with_status_code(status)
        .with_header(header("Content-Type", kind));
    let _ = request.respond(response);
}

impl Page {
    fn answer(&self, mut request: Request, codes: &Mutex<VecDeque<u64>>) {
        let url = request.url().to_owned();
        let path = url.split('?').next().unwrap_or("").to_owned();
        let method = request.method().clone();
        let mut body = Vec::new();
        let _ = request.as_reader().take(1 << 20).read_to_end(&mut body);
        if path.starts_with(API_PATH) {
            let token = request
                .headers()
                .iter()
                .find(|h| h.field.equiv("Authorization"))
                .and_then(|h| h.value.as_str().strip_prefix("Bearer ").map(str::to_owned));
            let (status, kind, text) =
                self.post_office
                    .api(method.as_str(), &url, token.as_deref(), &body);
            send(request, status, kind, text);
            return;
        }
        let runs = crate::setup::runs_dir(&self.settings.dir);
        match (&method, path.as_str()) {
            (Method::Get, "/") => send(request, 200, "text/html; charset=utf-8", self.home()),
            (Method::Get, "/key") => send(request, 200, "application/json", self.key().to_string()),
            (Method::Post, "/codes") => {
                let json_wanted = request
                    .headers()
                    .iter()
                    .any(|h| h.field.equiv("Content-Type") && h.value.as_str().contains("json"));
                let callsign = if json_wanted {
                    serde_json::from_slice::<Json>(&body)
                        .ok()
                        .and_then(|b| b["callsign"].as_str().map(str::to_owned))
                } else {
                    form_value(&body, "callsign")
                }
                .unwrap_or_default();
                let answer = self.code(&callsign, codes);
                if json_wanted {
                    let status = if answer.get("error").is_some() {
                        400
                    } else {
                        200
                    };
                    send(request, status, "application/json", answer.to_string());
                } else {
                    send(request, 200, "text/html; charset=utf-8", code_page(&answer));
                }
            }
            (Method::Get, "/runs.json") => send(
                request,
                200,
                "application/json",
                Json::Array(report::list(&runs)).to_string(),
            ),
            (Method::Get, "/runs") => send(
                request,
                200,
                "text/html; charset=utf-8",
                wrap("Sessions", &runs_html(&report::list(&runs))),
            ),
            (Method::Get, p) if p.starts_with("/runs/") => {
                let id = &p["/runs/".len()..];
                let (id, as_json) = match id.strip_suffix(".json") {
                    Some(i) => (i, true),
                    None => (id, false),
                };
                match report::read(&runs, id) {
                    Some(r) if as_json => send(request, 200, "application/json", r.to_string()),
                    Some(r) => send(
                        request,
                        200,
                        "text/html; charset=utf-8",
                        wrap("Session", &report::html(&r)),
                    ),
                    None => send(request, 404, "text/plain", "no such session\n".into()),
                }
            }
            _ => send(request, 404, "text/plain", "not here\n".into()),
        }
    }

    /// What a program under test trusts and where it enrolls.
    fn key(&self) -> Json {
        let domain = self.settings.domain.to_lowercase();
        let public = hex(&crate::setup::public_key(&self.post_office));
        json!({
            "domain": domain,
            "public_hex": public,
            "api": format!("{}{API_PATH}", self.url),
            "trust": format!("radiopost trust {domain} {public}"),
            "air": self.air.to_string(),
            "fingerprint": self.fingerprint,
            "gateway": GATEWAY,
            "station": STATION,
        })
    }

    /// An enrollment code for `callsign`, within the hour's limit.
    fn code(&self, callsign: &str, codes: &Mutex<VecDeque<u64>>) -> Json {
        {
            let mut recent = codes.lock().unwrap_or_else(PoisonError::into_inner);
            let hour_ago = now().saturating_sub(3600);
            while recent.front().is_some_and(|t| *t < hour_ago) {
                recent.pop_front();
            }
            if recent.len() >= CODES_PER_HOUR {
                return json!({"error": "enough codes this hour: try again later"});
            }
            recent.push_back(now());
        }
        match crate::setup::code_for(&self.post_office, &self.settings.domain, callsign) {
            Ok((code, address)) => {
                let mut k = self.key();
                k["code"] = code.into();
                k["address"] = address.into();
                k["operator"] = callsign.trim().to_uppercase().into();
                k
            }
            Err(e) => json!({"error": e}),
        }
    }
}

/// The page that gives a code, or says why there is none.
fn code_page(answer: &Json) -> String {
    {
        if let Some(e) = answer["error"].as_str() {
            return wrap(
                "No code",
                &format!(
                    "<p class=\"bad\">{}</p><p><a href=\"/\">Back</a></p>",
                    escape(e)
                ),
            );
        }
        let t = |k: &str| escape(answer[k].as_str().unwrap_or(""));
        wrap(
            "Your code",
            &format!(
                "<h1>Code {code}</h1>\
                 <p>For <b>{address}</b>, as <b>{operator}</b>, good for a week. With the \
                 RadioPost client:</p>\
                 <pre>{trust}\nradiopost enroll --address {address} --operator {operator} \
                 --code {code} --post-office {api}</pre>\
                 <p>Another program: trust the key above for {domain} (RP-1 §7.3), then enroll \
                 at <code>{api}</code> as RP-5 §6.1 says. Three messages wait for your first call \
                 to {gateway}.</p><p><a href=\"/\">Back</a></p>",
                code = t("code"),
                address = t("address"),
                operator = t("operator"),
                trust = t("trust"),
                api = t("api"),
                domain = t("domain"),
                gateway = GATEWAY,
            ),
        )
    }
}

impl Page {
    fn home(&self) -> String {
        let k = self.key();
        let t = |key: &str| escape(k[key].as_str().unwrap_or(""));
        let air_host = self
            .url
            .split("://")
            .nth(1)
            .and_then(|h| h.split(['/', ':']).next())
            .unwrap_or("this-host")
            .to_owned();
        let port = self.air.port();
        let tcp = match &self.fingerprint {
            Some(f) => format!(
                "radiopost-tcp --peer {GATEWAY}={air_host}:{port} --peer {STATION}={air_host}:{port} \
                 --trust {}",
                escape(f)
            ),
            None => format!(
                "radiopost-tcp --plain --peer {GATEWAY}={air_host}:{port} \
                 --peer {STATION}={air_host}:{port}"
            ),
        };
        let mut recent = String::new();
        for r in report::list(&crate::setup::runs_dir(&self.settings.dir))
            .iter()
            .take(10)
        {
            recent.push_str(&run_row(r));
        }
        wrap(
            "RadioPost test service",
            &format!(
                "<h1>RadioPost test service</h1>\
                 <p>Check your own RadioPost program against RadioPost's, over the Internet. \
                 Nothing here goes on the air, and nothing here is real mail: the test post \
                 office's accounts and credentials are trusted by no real gateway or post office.</p>\
                 <h2>1. Trust the test post office</h2>\
                 <p>Domain <code>{domain}</code>, key <code>{public}</code>, API \
                 <code>{api}</code>. With the RadioPost client:</p><pre>{trust}</pre>\
                 <h2>2. Get an enrollment code</h2>\
                 <form method=\"post\" action=\"/codes\"><label>Your callsign \
                 <input name=\"callsign\" required maxlength=\"20\"></label> \
                 <button>Get a code</button></form>\
                 <h2>3. Call</h2>\
                 <p>Your program reaches the service through a TCP adapter (RP-4 §9.5), at \
                 <code>{air_host}:{port}</code>. With RadioPost's:</p><pre>{tcp}</pre>\
                 <p>Call <b>{GATEWAY}</b>, a gateway of the test post office (RP-3 §9), or \
                 <b>{STATION}</b>, a station that answers station-to-station calls (RP-3 §10). \
                 One session at a time: a call while one is under way is told <code>busy</code>.</p>\
                 <h2>4. Read the report</h2>\
                 <p>Every session's transmissions, decoded, and the checks your program was held to. \
                 <a href=\"/runs\">All sessions</a> (<a href=\"/runs.json\">JSON</a>).</p>\
                 <table><tr><th>Session</th><th>Caller</th><th>Called</th><th>Result</th></tr>\
                 {recent}</table>",
                domain = t("domain"),
                public = t("public_hex"),
                api = t("api"),
                trust = t("trust"),
            ),
        )
    }
}

/// One line of the sessions' table.
fn run_row(r: &Json) -> String {
    let s = |k: &str| escape(r[k].as_str().unwrap_or(""));
    format!(
        "<tr><td><a href=\"/runs/{id}\">{id}</a></td><td>{caller}</td><td>{station}</td>\
         <td class=\"{class}\">{result}</td></tr>",
        id = s("id"),
        caller = s("caller"),
        station = s("station"),
        class = if r["passed"] == true { "ok" } else { "bad" },
        result = if r["passed"] == true {
            "passed"
        } else {
            "failed"
        },
    )
}

fn runs_html(runs: &[Json]) -> String {
    let mut out = String::from(
        "<h1>Sessions</h1><table><tr><th>Session</th><th>Caller</th><th>Called</th><th>Result</th></tr>",
    );
    for r in runs {
        out.push_str(&run_row(r));
    }
    let _ = write!(out, "</table><p>The newest {} are kept.</p>", report::KEEP);
    out
}

/// A whole page.
fn wrap(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title}</title><style>\
         body{{font:16px/1.5 system-ui,sans-serif;max-width:60rem;margin:2rem auto;padding:0 1rem;color:#1b1f24}}\
         pre,code{{font:14px ui-monospace,monospace;background:#f3f4f6}}pre{{padding:.6rem;overflow-x:auto;white-space:pre-wrap}}\
         table{{border-collapse:collapse;width:100%}}td,th{{border-bottom:1px solid #ddd;padding:.3rem .5rem;text-align:left;vertical-align:top}}\
         .ok{{color:#1a7f37}}.bad{{color:#cf222e}}\
         </style></head><body>{body}</body></html>",
        title = escape(title)
    )
}

/// A value from a form's body (`application/x-www-form-urlencoded`).
fn form_value(body: &[u8], name: &str) -> Option<String> {
    let text = std::str::from_utf8(body).ok()?;
    text.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then(|| percent_decode(v))
    })
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match u8::from_str_radix(&text[i + 1..i + 3], 16) {
                Ok(b) => {
                    out.push(b);
                    i += 2;
                }
                Err(_) => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::{form_value, percent_decode};

    #[test]
    fn a_form_is_read() {
        assert_eq!(
            form_value(b"callsign=VE3%2FKK4ODA", "callsign"),
            Some("VE3/KK4ODA".into())
        );
        assert_eq!(percent_decode("a+b%41%"), "a bA%");
        assert_eq!(form_value(b"x=1", "callsign"), None);
    }
}
