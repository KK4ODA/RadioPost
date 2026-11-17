// SPDX-License-Identifier: MIT OR Apache-2.0
//! The modem adapter's tests (the outline's §3.5): A-01 to A-04 and A-06, the tester playing the
//! host of the adapter under test and, through a second instance of it, the far station's host.
//! A-06 applies only to an adapter whose caps say `scan`.
//!
//! Every exchange is checked against RP-4 as written: the answers' `op`, `id` and members, never
//! their `text`, and the events an adapter may send of its own accord (`queued`, `delivered`,
//! `busy`, `quality`, `heard`, `incoming`, `broadcast`) passed over where they are not the point.

use std::time::{Duration, Instant};

use rpmi::tcp::Connection;
use rpmi::wire::{Caps, Message};
use serde_json::{Value as Json, json};

use crate::Verdict;
use crate::harness::Outcome;

/// What to run against an adapter.
pub struct Plan {
    /// The adapter under test, where its host connects.
    pub at: String,
    /// A second instance, the far station, joined to the first: A-02 and A-04 need it.
    pub far: Option<String>,
    /// What the near host names in `connect` to reach the far host's `N1CALL-10`.
    pub reach: String,
    /// Only these tests, when not empty.
    pub only: Vec<String>,
}

/// A test of the modem adapter role.
struct AdapterTest {
    id: &'static str,
    checks: &'static str,
    needs_far: bool,
    run: fn(&Plan, &mut Vec<String>) -> Outcome,
}

const TESTS: &[AdapterTest] = &[
    AdapterTest {
        id: "A-01",
        checks: "The lines it reads and the ones it answers with an error (RP-4 §2), rpmi-lines.json",
        needs_far: false,
        run: a01,
    },
    AdapterTest {
        id: "A-02",
        checks: "Opening, requests, events and errors in conversation (RP-4 §3, §4, §5, §7)",
        needs_far: true,
        run: a02,
    },
    AdapterTest {
        id: "A-03",
        checks: "A second host: turned away when not shared, its own callsigns when shared (RP-4 §1.1, §3)",
        needs_far: false,
        run: a03,
    },
    AdapterTest {
        id: "A-04",
        checks: "session-01 carried between two instances, every byte intact (RP-4 §4, §5)",
        needs_far: true,
        run: a04,
    },
    AdapterTest {
        id: "A-06",
        checks: "A scan: answered with `scan_s`, one host's at a time, ended by another host's `tune` and by the host leaving (RP-4 §1.1, §4.2)",
        needs_far: false,
        run: a06,
    },
];

/// What a test answers when it does not apply to the adapter under test: its verdict is neither
/// passed nor failed.
const NOT_APPLICABLE: &str = "not applicable: ";

/// Runs the adapter's tests, telling `progress` of each verdict as it comes.
///
/// # Errors
///
/// None yet: every failure is a test's.
pub fn run(plan: &Plan, mut progress: impl FnMut(&Verdict)) -> Result<Vec<Verdict>, String> {
    let mut verdicts = Vec::new();
    for t in TESTS {
        if !(plan.only.is_empty() || plan.only.iter().any(|o| o.eq_ignore_ascii_case(t.id))) {
            continue;
        }
        let started = Instant::now();
        let mut transcript = Vec::new();
        let (passed, why) = if t.needs_far && plan.far.is_none() {
            (None, "needs a second instance: --far".to_owned())
        } else {
            match (t.run)(plan, &mut transcript) {
                Ok(()) => (Some(true), String::new()),
                Err(e) if e.starts_with(NOT_APPLICABLE) => (None, e),
                Err(e) => (Some(false), e),
            }
        };
        let v = Verdict {
            id: t.id,
            checks: t.checks,
            passed,
            why,
            transcript,
            seconds: started.elapsed().as_secs_f64(),
        };
        progress(&v);
        verdicts.push(v);
    }
    Ok(verdicts)
}

/// How long anything that crosses to the other station may take: a radio's turnarounds and retries,
/// not a TCP adapter's milliseconds. Answers to requests come at once.
const AIR: Duration = Duration::from_secs(240);

/// Events an adapter sends of its own accord, which an answer may come after.
const ASIDES: &[&str] = &[
    "queued",
    "delivered",
    "busy",
    "quality",
    "heard",
    "incoming",
    "broadcast",
];

/// One host's connection to the adapter under test, keeping what crossed it.
struct Side<'t> {
    conn: Connection,
    name: &'static str,
    log: &'t mut Vec<String>,
}

impl<'t> Side<'t> {
    fn open(at: &str, name: &'static str, log: &'t mut Vec<String>) -> Result<Side<'t>, String> {
        let conn =
            Connection::open(at).map_err(|e| format!("cannot reach the adapter at {at}: {e}"))?;
        conn.set_timeout(Some(Duration::from_millis(200)))
            .map_err(|e| e.to_string())?;
        Ok(Side { conn, name, log })
    }

    fn line(&mut self, line: &str) -> Outcome {
        self.log.push(format!("{} → adapter  {line}", self.name));
        self.conn
            .send_line(line)
            .map_err(|e| format!("the adapter closed the connection: {e}"))
    }

    fn send(&mut self, m: &Json) -> Outcome {
        self.line(&m.to_string())
    }

    /// The adapter's next line within `within`: `Ok(None)` when it closed the connection.
    fn read(&mut self, within: Duration) -> Result<Option<Message>, String> {
        let deadline = Instant::now() + within;
        loop {
            match self.conn.read_message() {
                Ok(Some(m)) => {
                    self.log.push(format!(
                        "adapter → {}  {}",
                        self.name,
                        Json::Object(m.clone())
                    ));
                    return Ok(Some(m));
                }
                Ok(None) => {
                    self.log.push(format!("adapter ✕ {} (closed)", self.name));
                    return Ok(None);
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    if Instant::now() >= deadline {
                        return Err("nothing from the adapter".into());
                    }
                }
                Err(e) => return Err(format!("the adapter wrote what is not RPMI: {e}")),
            }
        }
    }

    /// The next message that is not an aside, which must satisfy `ok`.
    fn expect(&mut self, what: &str, ok: impl Fn(&Message) -> bool) -> Result<Message, String> {
        self.expect_within(what, Duration::from_secs(10), ok)
    }

    fn expect_within(
        &mut self,
        what: &str,
        within: Duration,
        ok: impl Fn(&Message) -> bool,
    ) -> Result<Message, String> {
        let deadline = Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let m = self
                .read(left)
                .map_err(|e| {
                    format!(
                        "{e} within {:.0} s where {what} was due",
                        within.as_secs_f64()
                    )
                })?
                .ok_or_else(|| format!("the adapter closed the connection where {what} was due"))?;
            if ASIDES.contains(&op(&m)) && !ok(&m) {
                continue;
            }
            if ok(&m) {
                return Ok(m);
            }
            return Err(format!("{} where {what} was due", Json::Object(m)));
        }
    }

    /// [`Side::expect_within`], a scan's own moves aside: the `dial` events that are not `ok`'s.
    fn expect_moving(
        &mut self,
        what: &str,
        within: Duration,
        ok: impl Fn(&Message) -> bool,
    ) -> Result<Message, String> {
        let deadline = Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let m = self.expect_within(what, left, |m| ok(m) || op(m) == "dial")?;
            if ok(&m) {
                return Ok(m);
            }
        }
    }

    fn answer(
        &mut self,
        request: &Json,
        what: &str,
        ok: impl Fn(&Message) -> bool,
    ) -> Result<Message, String> {
        self.send(request)?;
        self.expect(what, ok)
    }

    /// `error` with this `id` and `code` (RP-4 §7).
    fn refused(&mut self, request: &Json, code: &str, section: &str) -> Outcome {
        let id = request["id"].clone();
        self.answer(request, &format!("`error` `{code}` ({section})"), |m| {
            op(m) == "error"
                && m.get("id") == Some(&id)
                && m.get("code").and_then(Json::as_str) == Some(code)
        })
        .map(|_| ())
    }

    fn ok(&mut self, request: &Json, section: &str) -> Outcome {
        let id = request["id"].clone();
        self.answer(request, &format!("`ok` ({section})"), |m| {
            op(m) == "ok" && m.get("id") == Some(&id)
        })
        .map(|_| ())
    }

    /// `hello` and `callsigns`, and `listen` on when asked: the adapter's caps.
    fn open_as(&mut self, callsigns: &[&str], listen: bool) -> Result<Caps, String> {
        let h = self.answer(
            &json!({"op": "hello", "id": "h", "version": [1, 1], "app": "radiopost-tester"}),
            "`hello` (RP-4 §3)",
            |m| op(m) == "hello" && m.get("id") == Some(&json!("h")),
        )?;
        if h.get("version") != Some(&json!(1)) {
            return Err("its `hello` does not choose version 1 (RP-4 §3)".into());
        }
        let caps = h
            .get("caps")
            .filter(|c| c.is_object())
            .ok_or("its `hello` has no `caps` (RP-4 §3)")?;
        self.ok(
            &json!({"op": "callsigns", "id": "c", "callsigns": callsigns}),
            "RP-4 §4",
        )?;
        if listen {
            self.ok(&json!({"op": "listen", "id": "l", "on": true}), "RP-4 §4")?;
        }
        Ok(Caps::from_json(caps))
    }

    /// Joins `received` on `link` until `n` bytes have come.
    fn receive(&mut self, link: &str, n: usize) -> Result<Vec<u8>, String> {
        let mut got = Vec::new();
        while got.len() < n {
            let m = self.expect_within(
                &format!("`received` on {link} ({} of {n} bytes so far)", got.len()),
                AIR,
                |m| op(m) == "received",
            )?;
            if m.get("link").and_then(Json::as_str) != Some(link) {
                return Err(format!(
                    "`received` on {:?}, not the link {link}",
                    m.get("link")
                ));
            }
            let data = m.get("data").and_then(Json::as_str).unwrap_or_default();
            got.extend(
                rpmi::base64::decode(data)
                    .ok_or("`received` whose `data` is not base64 (RP-4 §2)")?,
            );
        }
        Ok(got)
    }
}

fn op(m: &Message) -> &str {
    m.get("op").and_then(Json::as_str).unwrap_or_default()
}

fn text<'m>(m: &'m Message, key: &str) -> &'m str {
    m.get(key).and_then(Json::as_str).unwrap_or_default()
}

/// A message as RP-4 compares it: as a JSON value, `text` aside.
fn without_text(m: &Message) -> Json {
    let mut m = m.clone();
    m.remove("text");
    Json::Object(m)
}

/// A-01: each line of `rpmi-lines.json`, after `hello` and the callsign KK4ODA, answered as the
/// vector says.
fn a01(plan: &Plan, log: &mut Vec<String>) -> Outcome {
    let file: Json = serde_json::from_str(crate::RPMI_LINES_JSON).map_err(|e| e.to_string())?;
    let mut near = Side::open(&plan.at, "host", log)?;
    near.open_as(&["KK4ODA"], false)?;
    for case in file["lines"].as_array().into_iter().flatten() {
        let what = case["what"].as_str().unwrap_or_default();
        let line = case["line"].as_str().unwrap_or_default();
        near.line(line)?;
        for want in case["answers"].as_array().into_iter().flatten() {
            let want = want.as_object().cloned().unwrap_or_default();
            let got = near.expect(&format!("an answer to {what}"), |_| true)?;
            if without_text(&got) != without_text(&want) {
                return Err(format!(
                    "{what}: {} where {} was due (RP-4 §2, §4)",
                    without_text(&got),
                    without_text(&want)
                ));
            }
        }
    }
    Ok(())
}

/// A bandwidth the adapter does not list.
fn missing_bandwidth(caps: &Caps) -> i64 {
    caps.bandwidths.iter().copied().max().unwrap_or(0).max(2750) + 1
}

/// A call from the near host to the far one: the near `connected` and the far one.
fn call(
    near: &mut Side<'_>,
    far: &mut Side<'_>,
    reach: &str,
    id: i64,
) -> Result<(String, String), String> {
    near.ok(
        &json!({"op": "connect", "id": id, "remote": reach}),
        "RP-4 §4",
    )?;
    let up = near.expect_within("`connected` (RP-4 §5)", AIR, |m| {
        matches!(op(m), "connected" | "failed")
    })?;
    if op(&up) != "connected" {
        return Err(format!("the call to {reach} failed: {}", Json::Object(up)));
    }
    if up.get("id") != Some(&json!(id))
        || text(&up, "direction") != "out"
        || text(&up, "local") != "KK4ODA"
    {
        return Err(format!(
            "`connected` must carry the `connect`'s id, `direction` out and `local` KK4ODA (RP-4 §5): {}",
            Json::Object(up)
        ));
    }
    if text(&up, "remote") != "N1CALL-10" {
        return Err(format!(
            "`connected` names {} as `remote`, not N1CALL-10",
            text(&up, "remote")
        ));
    }
    let theirs = far.expect_within("the far host's `connected`", AIR, |m| op(m) == "connected")?;
    if text(&theirs, "direction") != "in"
        || text(&theirs, "remote") != "KK4ODA"
        || text(&theirs, "local") != "N1CALL-10"
    {
        return Err(format!(
            "the far `connected` must say `direction` in, `remote` KK4ODA, `local` N1CALL-10 (RP-4 §5): {}",
            Json::Object(theirs)
        ));
    }
    Ok((
        text(&up, "link").to_owned(),
        text(&theirs, "link").to_owned(),
    ))
}

/// The `disconnected` both hosts must hear: `local` for the one that ended it, `remote` for the
/// other.
fn ended(near: &mut Side<'_>, far: &mut Side<'_>, near_link: &str, far_link: &str) -> Outcome {
    let gone = near.expect_within("`disconnected` `local` (RP-4 §5)", AIR, |m| {
        op(m) == "disconnected"
    })?;
    if text(&gone, "link") != near_link || text(&gone, "reason") != "local" {
        return Err(format!(
            "{} where `disconnected` {near_link} `local` was due",
            Json::Object(gone)
        ));
    }
    let gone = far.expect_within("the far `disconnected` `remote` (RP-4 §5)", AIR, |m| {
        op(m) == "disconnected"
    })?;
    if text(&gone, "link") != far_link || text(&gone, "reason") != "remote" {
        return Err(format!(
            "{} where `disconnected` {far_link} `remote` was due",
            Json::Object(gone)
        ));
    }
    Ok(())
}

/// A-02: the opening and its errors, the requests' errors, a call, bytes both ways, `status`,
/// an orderly end, an abort, a host that leaves with its link up, and a call nobody answers.
fn a02(plan: &Plan, log: &mut Vec<String>) -> Outcome {
    let far_at = plan.far.as_deref().ok_or("no --far")?;
    let mut far_log = Vec::new();
    let result = (|| {
        opening_errors(plan, log)?;
        let mut near = Side::open(&plan.at, "host", log)?;
        let caps = request_errors(&mut near, plan)?;
        let mut far = Side::open(far_at, "far host", &mut far_log)?;
        far.open_as(&["N1CALL-10"], true)?;
        exchange(&mut near, &mut far, plan, &caps)?;
        leaving(near, &mut far, plan)?;
        let mut near = Side::open(&plan.at, "host", log)?;
        near.open_as(&["KK4ODA"], false)?;
        unanswered(&mut near, &mut far, plan)
    })();
    log.extend(far_log);
    result
}

/// A request before `hello`, and a `hello` with no version in common, which closes the
/// connection (RP-4 §3).
fn opening_errors(plan: &Plan, log: &mut Vec<String>) -> Outcome {
    let mut first = Side::open(&plan.at, "host", log)?;
    first.refused(
        &json!({"op": "connect", "id": 1, "remote": plan.reach}),
        "refused",
        "RP-4 §3: hello first",
    )?;
    first.refused(
        &json!({"op": "hello", "id": 2, "version": [2, 3], "app": "radiopost-tester"}),
        "unsupported-version",
        "RP-4 §3",
    )?;
    if first.read(Duration::from_secs(5))?.is_some() {
        return Err("the adapter kept the connection after `unsupported-version` (RP-4 §3)".into());
    }
    Ok(())
}

/// A good `hello`, then each request's own refusals (RP-4 §4, §7): the adapter's caps.
fn request_errors(near: &mut Side<'_>, plan: &Plan) -> Result<Caps, String> {
    let h = near.answer(
        &json!({"op": "hello", "id": 3, "version": [1, 2], "app": "radiopost-tester"}),
        "`hello` choosing version 1 (RP-4 §3)",
        |m| {
            op(m) == "hello"
                && m.get("id") == Some(&json!(3))
                && m.get("version") == Some(&json!(1))
        },
    )?;
    let caps = Caps::from_json(h.get("caps").ok_or("`hello` without `caps`")?);
    let first = "RP-4 §4: callsigns first";
    near.refused(
        &json!({"op": "listen", "id": 4, "on": true}),
        "refused",
        first,
    )?;
    near.refused(
        &json!({"op": "connect", "id": 5, "remote": plan.reach}),
        "refused",
        first,
    )?;
    near.ok(
        &json!({"op": "callsigns", "id": 6, "callsigns": ["KK4ODA"]}),
        "RP-4 §4",
    )?;
    near.refused(
        &json!({"op": "connect", "id": 7, "remote": plan.reach, "as": "W1AW"}),
        "not-authorized",
        "RP-4 §4",
    )?;
    near.refused(
        &json!({"op": "connect", "id": 8, "remote": plan.reach, "bandwidth": missing_bandwidth(&caps)}),
        "unsupported",
        "RP-4 §4: a bandwidth not in `caps`",
    )?;
    near.refused(&json!({"op": "rig", "id": 9}), "unsupported", "RP-4 §2")?;
    near.refused(
        &json!({"op": "send", "id": 10, "link": "no-such-link", "data": "AA=="}),
        "not-connected",
        "RP-4 §4",
    )?;
    near.refused(
        &json!({"op": "abort", "id": 11}),
        "not-connected",
        "RP-4 §4: no call under way",
    )?;
    Ok(caps)
}

/// A call, a second call while it is up, `status`, bytes both ways, the link ended in order;
/// then another call, aborted.
fn exchange(near: &mut Side<'_>, far: &mut Side<'_>, plan: &Plan, caps: &Caps) -> Outcome {
    let (link, far_link) = call(near, far, &plan.reach, 12)?;
    near.refused(
        &json!({"op": "connect", "id": 13, "remote": plan.reach}),
        "busy",
        "RP-4 §4: a link is up",
    )?;
    let status = near.answer(
        &json!({"op": "status", "id": 14}),
        "`status` (RP-4 §4)",
        |m| op(m) == "status" && m.get("id") == Some(&json!(14)),
    )?;
    let listed = status["links"]
        .as_array()
        .is_some_and(|ls| ls.iter().any(|l| l["link"] == json!(link)));
    if !listed || status.get("callsigns") != Some(&json!(["KK4ODA"])) {
        return Err(format!(
            "`status` does not list the link {link} and the callsign KK4ODA: {}",
            Json::Object(status)
        ));
    }
    let changed = "the far host received other bytes than were sent (RP-4 §5)";
    near.send(&json!({"op": "send", "link": link, "data": rpmi::base64::encode(b"no ref")}))?;
    if far.receive(&far_link, 6)? != b"no ref" {
        return Err(changed.into());
    }
    near.send(
        &json!({"op": "send", "link": link, "data": rpmi::base64::encode(b"ref 7"), "ref": 7}),
    )?;
    if far.receive(&far_link, 5)? != b"ref 7" {
        return Err(changed.into());
    }
    if caps.delivery {
        near.expect_within("`delivered` with `ref` 7 (RP-4 §5)", AIR, |m| {
            op(m) == "delivered" && m.get("ref") == Some(&json!(7))
        })?;
    }
    far.send(
        &json!({"op": "send", "link": far_link, "data": rpmi::base64::encode(b"back again")}),
    )?;
    if near.receive(&link, 10)? != b"back again" {
        return Err("the near host received other bytes than were sent (RP-4 §5)".into());
    }
    near.ok(
        &json!({"op": "disconnect", "id": 15, "link": link}),
        "RP-4 §4",
    )?;
    ended(near, far, &link, &far_link)?;
    let (link, far_link) = call(near, far, &plan.reach, 16)?;
    near.ok(&json!({"op": "abort", "id": 17, "link": link}), "RP-4 §4")?;
    ended(near, far, &link, &far_link)
}

/// A host that leaves with a link up takes the link with it: the far host hears `disconnected`
/// (RP-4 §1.1).
fn leaving(mut near: Side<'_>, far: &mut Side<'_>, plan: &Plan) -> Outcome {
    let (_, far_link) = call(&mut near, far, &plan.reach, 19)?;
    near.log.push("host ✕ adapter (the host leaves)".into());
    drop(near);
    let gone = far.expect_within(
        "the far `disconnected` once the near host has left (RP-4 §1.1)",
        AIR,
        |m| op(m) == "disconnected",
    )?;
    if text(&gone, "link") != far_link || text(&gone, "reason") == "local" {
        return Err(format!(
            "{} where the far link {far_link} should end as the other station's (RP-4 §1.1)",
            Json::Object(gone)
        ));
    }
    Ok(())
}

/// A call to a host that has stopped listening ends `failed` (RP-4 §1.1, §5).
fn unanswered(near: &mut Side<'_>, far: &mut Side<'_>, plan: &Plan) -> Outcome {
    far.ok(
        &json!({"op": "listen", "id": "off", "on": false}),
        "RP-4 §4",
    )?;
    near.ok(
        &json!({"op": "connect", "id": 18, "remote": plan.reach}),
        "RP-4 §4",
    )?;
    let failed = near.expect_within("`failed` for a call nobody answers (RP-4 §5)", AIR, |m| {
        matches!(op(m), "failed" | "connected")
    })?;
    let code = text(&failed, "code");
    if op(&failed) != "failed"
        || failed.get("id") != Some(&json!(18))
        || !matches!(code, "timeout" | "refused")
    {
        return Err(format!(
            "a call to a host that does not listen ended {}, not `failed` `timeout` or `refused` \
             with the `connect`'s id (RP-4 §1.1, §5)",
            Json::Object(failed)
        ));
    }
    Ok(())
}

/// A-03: a second host. Not shared: turned away `busy` at its `hello`, and closed, the first
/// untouched. Shared: its own callsigns, and another program's `claimed`.
fn a03(plan: &Plan, log: &mut Vec<String>) -> Outcome {
    let mut second_log = Vec::new();
    let result = (|| {
        let mut first = Side::open(&plan.at, "host", log)?;
        let caps = first.open_as(&["KK4ODA"], true)?;
        let mut second = Side::open(&plan.at, "second host", &mut second_log)?;
        let hello = json!({"op": "hello", "id": 1, "version": [1, 1], "app": "another program"});
        if caps.shared {
            second.answer(&hello, "`hello` (RP-4 §3)", |m| op(m) == "hello")?;
            second.refused(
                &json!({"op": "callsigns", "id": 2, "callsigns": ["KK4ODA"]}),
                "claimed",
                "RP-4 §1.1",
            )?;
            second.ok(
                &json!({"op": "callsigns", "id": 3, "callsigns": ["KK4ODA-7"]}),
                "RP-4 §1.1",
            )?;
            let s = second.answer(&json!({"op": "status", "id": 4}), "`status`", |m| {
                op(m) == "status"
            })?;
            if s.get("callsigns") != Some(&json!(["KK4ODA-7"]))
                || s.get("listening") != Some(&json!(false))
            {
                return Err(format!(
                    "the second host's `status` is not its own (RP-4 §1.1): {}",
                    Json::Object(s)
                ));
            }
        } else {
            second.refused(&hello, "busy", "RP-4 §1.1, §3: not shared")?;
            if second.read(Duration::from_secs(5))?.is_some() {
                return Err(
                    "the adapter kept the second host's connection after `busy` (RP-4 §3)".into(),
                );
            }
        }
        let s = first.answer(
            &json!({"op": "status", "id": 5}),
            "the first host's `status`",
            |m| op(m) == "status",
        )?;
        if s.get("callsigns") != Some(&json!(["KK4ODA"]))
            || s.get("listening") != Some(&json!(true))
        {
            return Err(format!(
                "the first host's state changed (RP-4 §1.1): {}",
                Json::Object(s)
            ));
        }
        Ok(())
    })();
    log.extend(second_log);
    result
}

/// A-04: session-01's transmissions, the client's from the near host and the gateway's from the
/// far one, each one `send`, each arriving whole and in order.
fn a04(plan: &Plan, log: &mut Vec<String>) -> Outcome {
    let far_at = plan.far.as_deref().ok_or("no --far")?;
    let session: Json = serde_json::from_str(crate::SESSION_01_JSON).map_err(|e| e.to_string())?;
    let mut far_log = Vec::new();
    let result = (|| {
        let mut near = Side::open(&plan.at, "client host", log)?;
        near.open_as(&["KK4ODA"], false)?;
        let mut far = Side::open(far_at, "gateway host", &mut far_log)?;
        far.open_as(&["N1CALL-10"], true)?;
        let (link, far_link) = call(&mut near, &mut far, &plan.reach, 1)?;
        for t in session["transmissions"].as_array().into_iter().flatten() {
            let n = t["number"].as_u64().unwrap_or(0);
            let bytes =
                hex(t["bytes_hex"].as_str().unwrap_or_default()).ok_or("session-01's hex")?;
            let send = |l: &str| json!({"op": "send", "link": l, "data": rpmi::base64::encode(&bytes), "ref": format!("t{n}")});
            let got = if t["from"] == json!("client") {
                near.send(&send(&link))?;
                far.receive(&far_link, bytes.len())?
            } else {
                far.send(&send(&far_link))?;
                near.receive(&link, bytes.len())?
            };
            if got != bytes {
                return Err(format!(
                    "transmission {n} of session-01 arrived changed (RP-4 §5)"
                ));
            }
        }
        near.ok(
            &json!({"op": "disconnect", "id": 2, "link": link}),
            "RP-4 §4",
        )?;
        ended(&mut near, &mut far, &link, &far_link)
    })();
    log.extend(far_log);
    result
}

fn hex(s: &str) -> Option<Vec<u8>> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// A-06 (RP-4 §1.1, §4.2): a host scans two dials and is answered with them and a whole `scan_s`;
/// on a shared adapter another host's scan is `busy`, its `tune` ends the first host's scan (a
/// `scan` event, `why` `tuned`, and a `dial` event to both), and the first host's scan, asked
/// again, ends when it leaves: the radio goes to the home dial, which the other host hears. On an
/// adapter that is not shared the host's own `tune` ends its scan (`asked`).
fn a06(plan: &Plan, log: &mut Vec<String>) -> Outcome {
    const HOME: u64 = 3_590_000;
    const AWAY: u64 = 7_101_000;
    const WAIT: Duration = Duration::from_secs(10);
    let mut second_log = Vec::new();
    let result = (|| {
        let mut first = Side::open(&plan.at, "host", log)?;
        let caps = first.open_as(&["KK4ODA"], false)?;
        if !caps.scan {
            return Err(format!("{NOT_APPLICABLE}its caps do not say `scan`"));
        }
        if !caps.dial {
            return Err("its caps say `scan` without `dial` (RP-4 §3)".into());
        }
        let scan = |id: &str| json!({"op": "scan", "id": id, "dials": [HOME, AWAY]});
        let scanned = |m: &Message, id: &str| {
            op(m) == "scan"
                && m.get("id") == Some(&json!(id))
                && m.get("dials") == Some(&json!([HOME, AWAY]))
        };
        first.send(&scan("s1"))?;
        let a = first.expect_moving("`scan` answered with its dials (RP-4 §4.2)", WAIT, |m| {
            scanned(m, "s1")
        })?;
        if !a.get("scan_s").is_some_and(Json::is_u64) {
            return Err("the `scan` answer has no `scan_s` in whole seconds (RP-4 §4.2)".into());
        }
        let ended = |m: &Message, why: &str| {
            op(m) == "scan"
                && m.get("id").is_none()
                && m.get("dials") == Some(&json!([]))
                && m.get("why") == Some(&json!(why))
        };
        if !caps.shared {
            // the host's own tune ends its scan; `ok`, the `scan` event and the `dial` event come
            // in any order
            first.send(&json!({"op": "tune", "id": "t1", "dial_hz": AWAY}))?;
            let mut due = vec!["ok", "scan", "dial"];
            while !due.is_empty() {
                let m = first.expect_moving(
                    "`ok`, the scan's end (`asked`) and the dial",
                    WAIT,
                    |m| {
                        (op(m) == "ok" && m.get("id") == Some(&json!("t1")))
                            || ended(m, "asked")
                            || (op(m) == "dial" && m.get("dial_hz") == Some(&json!(AWAY)))
                    },
                )?;
                due.retain(|d| *d != op(&m));
            }
            return Ok(());
        }
        let mut second = Side::open(&plan.at, "second host", &mut second_log)?;
        second.open_as(&["KK4ODA-7"], false)?;
        second.refused(
            &json!({"op": "scan", "id": "s2", "dials": [14_105_000, AWAY]}),
            "busy",
            "RP-4 §1.1: one host scans at a time",
        )?;
        // the other host tunes the radio: the operator wins
        second.send(&json!({"op": "tune", "id": "t2", "dial_hz": AWAY}))?;
        let mut due = vec!["ok", "dial"];
        while !due.is_empty() {
            let m = second.expect_moving("`ok` and the dial", WAIT, |m| {
                (op(m) == "ok" && m.get("id") == Some(&json!("t2")))
                    || (op(m) == "dial" && m.get("dial_hz") == Some(&json!(AWAY)))
            })?;
            due.retain(|d| *d != op(&m));
        }
        let mut due = vec!["scan", "dial"];
        while !due.is_empty() {
            let m = first.expect_moving("the scan's end (`tuned`) and the dial", WAIT, |m| {
                ended(m, "tuned") || (op(m) == "dial" && m.get("dial_hz") == Some(&json!(AWAY)))
            })?;
            due.retain(|d| *d != op(&m));
        }
        // scanned again from a dial on its list, then the host leaves: the radio goes home
        first.send(&scan("s3"))?;
        first.expect_moving("`scan` answered again (RP-4 §4.2)", WAIT, |m| {
            scanned(m, "s3")
        })?;
        drop(first);
        second.expect_moving(
            "the radio home once the scanning host left (RP-4 §1.1, §4.2)",
            Duration::from_secs(60),
            |m| op(m) == "dial" && m.get("dial_hz") == Some(&json!(HOME)),
        )?;
        Ok(())
    })();
    log.extend(second_log);
    result
}
