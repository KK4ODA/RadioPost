// SPDX-License-Identifier: MIT OR Apache-2.0
//! One RPMI host on one connection to Aether's control API: RP-4 §9.1's table, with no input or
//! output of its own.
//!
//! The program gives the bridge each line the host writes ([`Bridge::from_host`]) and each
//! WebSocket message Aether sends ([`Bridge::from_aether`]), and does what it answers: lines to the
//! host, requests to Aether, or closing the host's connection.
//!
//! The host's callsigns are a claim on Aether (ADR-0106), so its calls, its answering and its
//! sessions are its own, and every `state` event says whose session it is (`owner`). An Aether
//! without claims has no `status.claims`: the host has the modem to itself, `callsigns` sets the
//! station's list, and `listen` off is refused.

use std::collections::HashMap;

use rpmi::base64;
use rpmi::wire::{
    Caps, Message, Request, VERSION, admit, answer, answer_with, error, parse_line, speaks,
};
use serde_json::{Value, json};

/// What the bridge asks of the program that drives it.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    /// A line to the host.
    Host(Message),
    /// A request to Aether, one WebSocket message.
    Aether(Value),
    /// Close the host's connection.
    CloseHost,
}

/// What a request to Aether is waiting to answer.
#[derive(Debug)]
enum Waiting {
    HelloStatus {
        rid: Option<Value>,
    },
    HelloCaps {
        rid: Option<Value>,
        status: Value,
    },
    Claim {
        rid: Option<Value>,
        wanted: Vec<String>,
    },
    CallsignsSet {
        rid: Option<Value>,
        wanted: Vec<String>,
    },
    Listen {
        rid: Option<Value>,
        on: bool,
    },
    Connect {
        rid: Option<Value>,
    },
    Send {
        rid: Option<Value>,
        aether_ref: Option<String>,
    },
    Plain {
        rid: Option<Value>,
    },
    Broadcast {
        rid: Option<Value>,
        aether_ref: Option<String>,
    },
    Nothing,
}

/// A call the host placed, not yet connected or failed.
#[derive(Debug)]
struct Call {
    id: Option<Value>,
    aborted: bool,
}

/// The host's link while it is up.
#[derive(Debug)]
struct Link {
    id: String,
    remote: String,
    direction: &'static str,
    queued_told: Option<u64>,
    quality_told: Option<Message>,
}

/// One host's side of the adapter.
#[derive(Debug, Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "the host's state and the channel's, each its own flag"
)]
pub struct Bridge {
    hello: bool,
    modem_lost: bool,
    shared: bool,
    app: String,
    bandwidth_hz: Option<i64>,
    asked: u64,
    waiting: HashMap<String, Waiting>,
    callsigns: Vec<String>,
    listening: bool,
    claim: Option<u64>,
    call: Option<Call>,
    link: Option<Link>,
    links_made: u64,
    refs_made: u64,
    refs: HashMap<String, Value>,
    datagram_refs: HashMap<String, Value>,
    others: bool,
    channel_busy: bool,
    session_on_air: bool,
    busy_told: bool,
    log: Option<String>,
}

/// RP-4 §9.1: how a session that came up ends, in RPMI's words.
#[must_use]
pub fn disconnect_reason(detail: &str) -> &'static str {
    match detail {
        "peer disconnected" => "remote",
        "closed" | "aborted" | "closed (no DISC_ACK)" => "local",
        "link timeout" | "no response" => "timeout",
        _ => "failure",
    }
}

/// RP-4 §9.1: how a call that never came up ends, as `failed`'s code.
#[must_use]
pub fn failure_code(detail: &str, aborted: bool) -> &'static str {
    match detail {
        "no answer" => "timeout",
        "aborted" if aborted => "aborted",
        _ => "refused",
    }
}

/// RP-4 §9.1: an Aether refusal of `connect` as RPMI's code.
#[must_use]
pub fn connect_code(aether: &str) -> &'static str {
    match aether {
        "claimed" | "bad_params" => "not-authorized",
        "already_connected" | "not_idle" => "busy",
        _ => "refused",
    }
}

fn aether_error(answer: &Value) -> (String, String) {
    let e = &answer["error"];
    (
        e["code"].as_str().unwrap_or("").to_owned(),
        e["message"].as_str().unwrap_or("").to_owned(),
    )
}

impl Bridge {
    /// A host just connected; Aether reachable.
    #[must_use]
    pub fn new() -> Bridge {
        Bridge::default()
    }

    /// A host connected while Aether cannot be reached: every request after `hello` is answered
    /// `no-modem`.
    #[must_use]
    pub fn without_modem() -> Bridge {
        Bridge {
            modem_lost: true,
            ..Bridge::default()
        }
    }

    /// The program's own log file, offered to Aether once the host has said hello, for the files
    /// a station shares when something went wrong (Aether's `share.include`, from 0.2.0-beta.150;
    /// an earlier Aether says it has no such method, and the answer is not awaited). The log is
    /// where a program says why its connection to the modem ended, which the modem cannot see.
    #[must_use]
    pub fn offering_log(mut self, log: Option<String>) -> Bridge {
        self.log = log;
        self
    }

    fn ask(&mut self, method: &str, params: Value, waiting: Waiting) -> Out {
        self.asked += 1;
        let id = format!("rp{}", self.asked);
        self.waiting.insert(id.clone(), waiting);
        let mut m = serde_json::Map::new();
        m.insert("id".into(), id.into());
        m.insert("method".into(), method.into());
        m.insert("params".into(), params);
        Out::Aether(Value::Object(m))
    }

    fn ok(rid: Option<&Value>, out: &mut Vec<Out>) {
        if rid.is_some() {
            out.push(Out::Host(answer("ok", rid)));
        }
    }

    /// The busy state RPMI reports: another's session on the channel, or, while the host has no
    /// link of its own, the channel heard in use (RP-4 §1.1, §5).
    fn busy_now(&self) -> bool {
        if self.link.is_some() {
            self.others
        } else {
            self.others || self.channel_busy || self.session_on_air
        }
    }

    fn update_busy(&mut self, out: &mut Vec<Out>) {
        let now = self.busy_now();
        if self.hello && now != self.busy_told {
            self.busy_told = now;
            out.push(Out::Host(answer_with(
                "busy",
                None,
                &[("busy", now.into())],
            )));
        }
    }

    /// A line the host wrote, without its LF.
    pub fn from_host(&mut self, line: &str) -> Vec<Out> {
        let mut out = Vec::new();
        let m = match parse_line(line) {
            Ok(m) => m,
            Err(refusal) => return vec![Out::Host(refusal)],
        };
        let rid = m.get("id").cloned();
        let request = match admit(&m, self.hello) {
            Ok(r) => r,
            Err(refusal) => return vec![Out::Host(refusal)],
        };
        let rid_ref = rid.as_ref();
        let speaking = match &request {
            Request::Hello { low, high, .. } => speaks(*low, *high),
            _ => true,
        };
        if !speaking {
            let text = format!("this adapter speaks {VERSION}");
            return vec![
                Out::Host(error(rid_ref, "unsupported-version", &text)),
                Out::CloseHost,
            ];
        }
        if self.modem_lost {
            return vec![Out::Host(error(
                rid_ref,
                "no-modem",
                "Aether cannot be reached",
            ))];
        }
        match request {
            Request::Hello { app, .. } => {
                self.app = app;
                out.push(self.ask("status", json!({}), Waiting::HelloStatus { rid }));
            }
            Request::Callsigns(wanted) => {
                if self.shared {
                    let params = json!({"callsigns": wanted, "app": self.app});
                    out.push(self.ask("claim", params, Waiting::Claim { rid, wanted }));
                } else {
                    let params = json!({"callsigns": wanted});
                    out.push(self.ask(
                        "callsigns.set",
                        params,
                        Waiting::CallsignsSet { rid, wanted },
                    ));
                }
            }
            Request::Listen(on) => {
                if self.callsigns.is_empty() {
                    out.push(Out::Host(error(rid_ref, "refused", "no callsigns yet")));
                } else if self.shared {
                    let params = json!({"enabled": on});
                    out.push(self.ask("listen", params, Waiting::Listen { rid, on }));
                } else if on {
                    self.listening = true;
                    Self::ok(rid_ref, &mut out);
                } else {
                    let text = "this Aether answers its callsigns whatever the host says";
                    out.push(Out::Host(error(rid_ref, "unsupported", text)));
                }
            }
            Request::Connect {
                remote,
                as_call,
                bandwidth,
            } => self.on_connect(&remote, as_call, bandwidth, rid, &mut out),
            Request::Send {
                link,
                data,
                reference,
            } => self.on_send(&link, &data, reference, rid, &mut out),
            Request::Disconnect { link } => {
                if self.link.as_ref().is_some_and(|l| l.id == link) {
                    out.push(self.ask("disconnect", json!({}), Waiting::Plain { rid }));
                } else {
                    let text = "no such link of this host's";
                    out.push(Out::Host(error(rid_ref, "not-connected", text)));
                }
            }
            Request::Abort { link } => self.on_abort(link.as_deref(), rid, &mut out),
            Request::Broadcast { data, reference } => {
                self.on_broadcast(&data, reference, rid, &mut out);
            }
            Request::Status => out.push(Out::Host(self.status(rid_ref))),
        }
        out
    }

    fn on_send(
        &mut self,
        link: &str,
        data: &[u8],
        reference: Option<Value>,
        rid: Option<Value>,
        out: &mut Vec<Out>,
    ) {
        let Some(l) = self.link.as_mut().filter(|l| l.id == link) else {
            let text = "no such link of this host's";
            out.push(Out::Host(error(rid.as_ref(), "not-connected", text)));
            return;
        };
        l.queued_told = None; // the next reading is told, whatever it is
        let mut params = json!({"data": base64::encode(data)});
        let aether_ref = reference.map(|r| {
            self.refs_made += 1;
            let ours = format!("rp{}", self.refs_made);
            self.refs.insert(ours.clone(), r);
            params["ref"] = ours.clone().into();
            ours
        });
        out.push(self.ask("send", params, Waiting::Send { rid, aether_ref }));
    }

    fn on_abort(&mut self, link: Option<&str>, rid: Option<Value>, out: &mut Vec<Out>) {
        let under_way = match link {
            Some(l) => self.link.as_ref().is_some_and(|x| x.id == l),
            None => self.call.as_mut().is_some_and(|c| {
                c.aborted = true;
                true
            }),
        };
        if under_way {
            out.push(self.ask("abort", json!({}), Waiting::Plain { rid }));
        } else {
            let text = if link.is_some() {
                "no such link of this host's"
            } else {
                "no call under way"
            };
            out.push(Out::Host(error(rid.as_ref(), "not-connected", text)));
        }
    }

    fn on_broadcast(
        &mut self,
        data: &[u8],
        reference: Option<Value>,
        rid: Option<Value>,
        out: &mut Vec<Out>,
    ) {
        if self.callsigns.is_empty() {
            out.push(Out::Host(error(
                rid.as_ref(),
                "refused",
                "no callsigns yet",
            )));
            return;
        }
        // RadioPost's frames are Aether's unformatted datagrams (type 2)
        let mut params = json!({"data": base64::encode(data), "frame_type": 2});
        let aether_ref = reference.map(|r| {
            self.refs_made += 1;
            let ours = format!("rp{}", self.refs_made);
            self.datagram_refs.insert(ours.clone(), r);
            params["ref"] = ours.clone().into();
            ours
        });
        out.push(self.ask(
            "datagram.send",
            params,
            Waiting::Broadcast { rid, aether_ref },
        ));
    }

    fn status(&self, rid: Option<&Value>) -> Message {
        let links: Vec<Value> = self
            .link
            .iter()
            .map(|l| {
                json!({"link": l.id, "remote": l.remote, "direction": l.direction,
                       "queued": l.queued_told.unwrap_or(0)})
            })
            .collect();
        answer_with(
            "status",
            rid,
            &[
                ("listening", self.listening.into()),
                ("callsigns", self.callsigns.clone().into()),
                ("busy", self.busy_told.into()),
                ("links", links.into()),
            ],
        )
    }

    fn on_connect(
        &mut self,
        remote: &str,
        as_call: Option<String>,
        bandwidth: Option<i128>,
        rid: Option<Value>,
        out: &mut Vec<Out>,
    ) {
        let rid_ref = rid.as_ref();
        let Some(first) = self.callsigns.first() else {
            out.push(Out::Host(error(rid_ref, "refused", "no callsigns yet")));
            return;
        };
        let local = as_call.unwrap_or_else(|| first.clone());
        if !self.callsigns.contains(&local) {
            let text = format!("{local} is not this host's");
            out.push(Out::Host(error(rid_ref, "not-authorized", &text)));
            return;
        }
        if let Some(bw) = bandwidth
            && self.bandwidth_hz.map(i128::from) != Some(bw)
        {
            let text = format!("no {bw} Hz on this modem now");
            out.push(Out::Host(error(rid_ref, "unsupported", &text)));
            return;
        }
        if self.call.is_some() || self.link.is_some() {
            out.push(Out::Host(error(rid_ref, "busy", "a link or a call is up")));
            return;
        }
        self.call = Some(Call {
            id: rid.clone(),
            aborted: false,
        });
        let params = json!({"remote": remote, "callsign": local,
                            "keep_bandwidth": bandwidth.is_some()});
        out.push(self.ask("connect", params, Waiting::Connect { rid }));
    }

    /// A WebSocket message from Aether.
    pub fn from_aether(&mut self, text: &str) -> Vec<Out> {
        let Ok(message) = serde_json::from_str::<Value>(text) else {
            return Vec::new();
        };
        if let Some(event) = message.get("event").and_then(Value::as_str) {
            let data = message.get("data").cloned().unwrap_or(Value::Null);
            return self.on_event(event, &data);
        }
        let Some(id) = message.get("id").and_then(Value::as_str) else {
            return Vec::new();
        };
        match self.waiting.remove(id) {
            Some(waiting) => self.on_answer(waiting, &message),
            None => Vec::new(),
        }
    }

    #[allow(clippy::too_many_lines, reason = "RP-4 §9.1's table, one arm a row")]
    fn on_answer(&mut self, waiting: Waiting, answer_msg: &Value) -> Vec<Out> {
        let mut out = Vec::new();
        let ok = answer_msg["ok"].as_bool() == Some(true);
        let result = &answer_msg["result"];
        let (code, message) = aether_error(answer_msg);
        match waiting {
            Waiting::HelloStatus { rid } => {
                if ok {
                    let status = result.clone();
                    out.push(self.ask(
                        "capabilities",
                        json!({}),
                        Waiting::HelloCaps { rid, status },
                    ));
                } else {
                    out.push(Out::Host(error(rid.as_ref(), "no-modem", &message)));
                }
            }
            Waiting::HelloCaps { rid, status } => {
                if !ok {
                    out.push(Out::Host(error(rid.as_ref(), "no-modem", &message)));
                    return out;
                }
                self.bandwidth_hz = result["bandwidth_hz"].as_i64();
                self.shared = status["claims"].is_array();
                self.channel_busy = status["channel_busy"].as_bool().unwrap_or(false);
                self.others = status["state"].as_str().is_some_and(|s| s != "idle");
                self.hello = true;
                let caps = Caps {
                    bandwidths: self.bandwidth_hz.into_iter().collect(),
                    max_links: 1,
                    delivery: true,
                    broadcast: true,
                    quality: true,
                    busy: true,
                    regulatory: true,
                    max_queue: 65536,
                    shared: self.shared,
                };
                let modem = format!(
                    "Aether HF {}",
                    status["version"].as_str().unwrap_or("(version unknown)")
                );
                out.push(Out::Host(answer_with(
                    "hello",
                    rid.as_ref(),
                    &[
                        ("version", VERSION.into()),
                        ("modem", modem.into()),
                        ("caps", caps.to_json()),
                    ],
                )));
                self.update_busy(&mut out);
                if self.shared
                    && let Some(path) = self.log.clone()
                {
                    let params = json!({"path": path, "app": self.app});
                    out.push(self.ask("share.include", params, Waiting::Nothing));
                }
            }
            Waiting::Claim { rid, wanted } => {
                if !ok {
                    out.push(Out::Host(error(rid.as_ref(), "refused", &message)));
                    return out;
                }
                let refused: Vec<String> = result["refused"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|r| r["callsign"].as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                self.claim = result["id"].as_u64().or(self.claim);
                if refused.is_empty() {
                    self.callsigns = wanted;
                    Self::ok(rid.as_ref(), &mut out);
                } else {
                    // RPMI keeps the host's list whole: Aether took the rest, so take it back
                    let text = format!("{} is held by another program", refused.join(", "));
                    out.push(Out::Host(error(rid.as_ref(), "claimed", &text)));
                    let params = json!({"callsigns": self.callsigns, "app": self.app});
                    out.push(self.ask("claim", params, Waiting::Nothing));
                }
            }
            Waiting::CallsignsSet { rid, wanted } => {
                if ok {
                    self.callsigns = wanted;
                    Self::ok(rid.as_ref(), &mut out);
                } else {
                    out.push(Out::Host(error(rid.as_ref(), "refused", &message)));
                }
            }
            Waiting::Listen { rid, on } => {
                if ok {
                    self.listening = on;
                    Self::ok(rid.as_ref(), &mut out);
                } else {
                    out.push(Out::Host(error(rid.as_ref(), "refused", &message)));
                }
            }
            Waiting::Connect { rid } => {
                if ok {
                    Self::ok(rid.as_ref(), &mut out);
                } else {
                    self.call = None;
                    out.push(Out::Host(error(
                        rid.as_ref(),
                        connect_code(&code),
                        &message,
                    )));
                }
            }
            Waiting::Send { rid, aether_ref } => {
                if ok {
                    Self::ok(rid.as_ref(), &mut out);
                } else {
                    if let Some(r) = aether_ref {
                        self.refs.remove(&r);
                    }
                    out.push(Out::Host(error(
                        rid.as_ref(),
                        Self::link_code(&code),
                        &message,
                    )));
                }
            }
            Waiting::Plain { rid } => {
                if ok {
                    Self::ok(rid.as_ref(), &mut out);
                } else {
                    out.push(Out::Host(error(
                        rid.as_ref(),
                        Self::link_code(&code),
                        &message,
                    )));
                }
            }
            Waiting::Broadcast { rid, aether_ref } => {
                if ok {
                    Self::ok(rid.as_ref(), &mut out);
                } else {
                    if let Some(r) = aether_ref {
                        self.datagram_refs.remove(&r);
                    }
                    let rpmi = if code == "queue_full" {
                        "busy"
                    } else {
                        "refused"
                    };
                    out.push(Out::Host(error(rid.as_ref(), rpmi, &message)));
                }
            }
            Waiting::Nothing => {}
        }
        out
    }

    fn link_code(aether: &str) -> &'static str {
        match aether {
            "not_owner" | "not_connected" => "not-connected",
            _ => "refused",
        }
    }

    fn mine(&self, data: &Value) -> bool {
        if !self.shared {
            return true; // the host has the modem to itself
        }
        self.claim.is_some() && data["owner"].as_u64() == self.claim
    }

    fn on_event(&mut self, event: &str, data: &Value) -> Vec<Out> {
        let mut out = Vec::new();
        match event {
            "state" => self.on_state(data, &mut out),
            "data" => {
                if let (Some(l), Some(bytes)) =
                    (&self.link, data["data"].as_str().and_then(base64::decode))
                {
                    out.push(Out::Host(answer_with(
                        "received",
                        None,
                        &[
                            ("link", l.id.clone().into()),
                            ("data", base64::encode(&bytes).into()),
                        ],
                    )));
                }
            }
            "sent" => {
                let theirs = data["ref"].as_str().unwrap_or("");
                if let Some(ours) = self.refs.remove(theirs)
                    && data["delivered"].as_bool() == Some(true)
                    && let Some(l) = &self.link
                {
                    out.push(Out::Host(answer_with(
                        "delivered",
                        None,
                        &[("link", l.id.clone().into()), ("ref", ours)],
                    )));
                }
            }
            "metrics" => self.on_metrics(data, &mut out),
            "datagram" => {
                if let (Some(source), Some(bytes)) = (
                    data["source"].as_str(),
                    data["data"].as_str().and_then(base64::decode),
                ) {
                    let mut m = answer_with(
                        "broadcast",
                        None,
                        &[
                            ("source", source.into()),
                            ("data", base64::encode(&bytes).into()),
                        ],
                    );
                    if let Some(snr) = data["snr_db"].as_f64() {
                        m.insert("snr_db".into(), snr.into());
                    }
                    out.push(Out::Host(m));
                }
            }
            "datagram-sent" => {
                let theirs = data["ref"].as_str().unwrap_or("");
                if let Some(ours) = self.datagram_refs.remove(theirs) {
                    let sent = data["sent"].as_bool().unwrap_or(false);
                    let mut m = answer_with(
                        "broadcast-sent",
                        None,
                        &[("ref", ours), ("sent", sent.into())],
                    );
                    if let Some(why) = data["reason"].as_str() {
                        m.insert("text".into(), why.into());
                    }
                    out.push(Out::Host(m));
                }
            }
            "bandwidth" => {
                if let Some(hz) = data["bandwidth_hz"].as_i64() {
                    self.bandwidth_hz = Some(hz);
                }
            }
            _ => {}
        }
        out
    }

    fn on_state(&mut self, data: &Value, out: &mut Vec<Out>) {
        let name = data["name"].as_str().unwrap_or("");
        let detail = data["detail"].as_str().unwrap_or("");
        if !self.mine(data) {
            match name {
                "connected" => self.others = true,
                "disconnected" => self.others = false,
                _ => return,
            }
            self.update_busy(out);
            return;
        }
        match name {
            "connected" if self.link.is_none() => {
                let remote = data["remote"].as_str().unwrap_or("").to_owned();
                let local = data["callsign"].as_str().unwrap_or("").to_owned();
                let answered = detail.ends_with("(slave)");
                let id = if answered {
                    out.push(Out::Host(answer_with(
                        "incoming",
                        None,
                        &[
                            ("remote", remote.clone().into()),
                            ("local", local.clone().into()),
                        ],
                    )));
                    None
                } else {
                    self.call.take().and_then(|c| c.id)
                };
                self.links_made += 1;
                let link = format!("a{}", self.links_made);
                let direction = if answered { "in" } else { "out" };
                let mut m = answer_with(
                    "connected",
                    id.as_ref(),
                    &[
                        ("link", link.clone().into()),
                        ("remote", remote.clone().into()),
                        ("local", local.into()),
                        ("direction", direction.into()),
                    ],
                );
                if let Some(hz) = self.bandwidth_hz {
                    m.insert("bandwidth".into(), hz.into());
                }
                out.push(Out::Host(m));
                self.link = Some(Link {
                    id: link,
                    remote,
                    direction,
                    queued_told: Some(0),
                    quality_told: None,
                });
                self.update_busy(out);
            }
            "disconnected" => {
                if let Some(l) = self.link.take() {
                    self.refs.clear();
                    // the readings taken in the host's own session say nothing of others':
                    // the next `metrics` says how the channel stands
                    self.channel_busy = false;
                    self.session_on_air = false;
                    out.push(Out::Host(answer_with(
                        "disconnected",
                        None,
                        &[
                            ("link", l.id.into()),
                            ("reason", disconnect_reason(detail).into()),
                            ("text", detail.into()),
                        ],
                    )));
                    self.update_busy(out);
                } else if let Some(call) = self.call.take() {
                    out.push(Out::Host(answer_with(
                        "failed",
                        call.id.as_ref(),
                        &[
                            ("code", failure_code(detail, call.aborted).into()),
                            ("text", detail.into()),
                        ],
                    )));
                }
            }
            _ => {}
        }
    }

    fn on_metrics(&mut self, data: &Value, out: &mut Vec<Out>) {
        self.channel_busy = data["channel_busy"].as_bool().unwrap_or(false);
        // a session up that is not the host's, though no `state` event said so: one that began
        // before this connection did
        self.session_on_air = !data["link"].is_null();
        if let Some(l) = self.link.as_mut() {
            let queued = data["queued_bytes"].as_u64().unwrap_or(0);
            if l.queued_told != Some(queued) {
                l.queued_told = Some(queued);
                out.push(Out::Host(answer_with(
                    "queued",
                    None,
                    &[("link", l.id.clone().into()), ("bytes", queued.into())],
                )));
            }
            let mut q = answer_with("quality", None, &[("link", l.id.clone().into())]);
            for (theirs, ours) in [
                ("snr_db", "snr_db"),
                ("throughput_bps", "rate_bps"),
                ("receiving", "receiving"),
            ] {
                if !data[theirs].is_null() {
                    q.insert(ours.into(), data[theirs].clone());
                }
            }
            if q.len() > 2 && l.quality_told.as_ref() != Some(&q) {
                l.quality_told = Some(q.clone());
                out.push(Out::Host(q));
            }
        }
        self.update_busy(out);
    }

    /// The host's connection closed (RP-4 §1.1): its link and its call under way end as `abort`
    /// would end them, so the station is not left in a session nobody holds. What to ask Aether.
    pub fn host_left(&mut self) -> Vec<Out> {
        let link = self.link.take().is_some();
        let call = self.call.take().is_some();
        if (link || call) && !self.modem_lost {
            vec![self.ask("abort", json!({}), Waiting::Nothing)]
        } else {
            Vec::new()
        }
    }

    /// Whether a request to Aether is still unanswered.
    #[must_use]
    pub fn awaiting(&self) -> bool {
        !self.waiting.is_empty()
    }

    /// Aether's connection closed, for the reason `why`: the host's link or call goes with it,
    /// told why, and every request from now on is answered `no-modem`.
    pub fn modem_lost(&mut self, why: &str) -> Vec<Out> {
        let text = format!("Aether went away: {why}");
        let mut out = Vec::new();
        self.modem_lost = true;
        if let Some(l) = self.link.take() {
            out.push(Out::Host(answer_with(
                "disconnected",
                None,
                &[
                    ("link", l.id.into()),
                    ("reason", "failure".into()),
                    ("text", text.clone().into()),
                ],
            )));
        }
        if let Some(call) = self.call.take() {
            out.push(Out::Host(answer_with(
                "failed",
                call.id.as_ref(),
                &[("code", "refused".into()), ("text", text.clone().into())],
            )));
        }
        for (_, waiting) in self.waiting.drain() {
            let rid = match waiting {
                Waiting::HelloStatus { rid }
                | Waiting::HelloCaps { rid, .. }
                | Waiting::Claim { rid, .. }
                | Waiting::CallsignsSet { rid, .. }
                | Waiting::Listen { rid, .. }
                | Waiting::Connect { rid }
                | Waiting::Send { rid, .. }
                | Waiting::Plain { rid }
                | Waiting::Broadcast { rid, .. } => rid,
                Waiting::Nothing => None,
            };
            if rid.is_some() {
                out.push(Out::Host(error(
                    rid.as_ref(),
                    "no-modem",
                    "Aether went away",
                )));
            }
        }
        out
    }
}
