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
//!
//! The host's `hello` names it to Aether too (`hello {app}`, Aether's ADR-0114), claiming nothing:
//! a host that holds its connection open between calls, with no callsigns, is named on Aether's
//! panel all the same.
//!
//! A host's scan is Aether's `scan.start` on this connection, so it is the host's and ends with
//! the connection (Aether's ADR-0129): the bridge tells the host when Aether ends it for another
//! reason, and passes `connect`'s `call_s` on.
//!
//! A call in a bandwidth the station is not running moves the station there first
//! (`bandwidth.set`) and back once the call or its link has ended, or the host has left: the
//! bandwidth, like the dial, is the whole station's (RP-4 §1.1). While the host listens, the
//! station runs the widest bandwidth, which answers calls in the narrower too (`bandwidth.set`
//! with `listening`, which Aether holds below any program's own and drops with the connection).
//! `status` is the host's own with the station's dial, the bandwidths a call is answered in, and
//! the dials scanned, read from Aether's. A tuner tone is Aether's `tune`.

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
    Dial {
        rid: Option<Value>,
    },
    TuneStatus {
        rid: Option<Value>,
        hz: u64,
    },
    Tune {
        rid: Option<Value>,
    },
    Scan {
        rid: Option<Value>,
    },
    ScanStop {
        rid: Option<Value>,
    },
    /// A call in another bandwidth: the station's state and bandwidth first, then the move.
    BandwidthStatus {
        rid: Option<Value>,
        hz: i64,
        params: Value,
    },
    BandwidthSet {
        rid: Option<Value>,
        params: Value,
        before: Value,
    },
    Tone {
        rid: Option<Value>,
    },
    /// RP-4 §4 `status`: the host's own, with the station's from Aether's.
    Status {
        rid: Option<Value>,
    },
    Nothing,
}

/// A call the host placed, not yet connected or failed.
#[derive(Debug)]
struct Call {
    id: Option<Value>,
    aborted: bool,
    /// The station called; whether a frame decoded from it was heard during the call, and the
    /// strongest SNR one was heard at (RP-4 §5 `failed`'s `heard` and `snr_db`).
    remote: String,
    heard: bool,
    heard_snr_db: Option<f64>,
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
    /// The bandwidths Aether runs (`capabilities.bandwidths_hz`): what a call may ask for.
    bandwidths: Vec<i64>,
    /// The bandwidth a host program had asked of the station (`status.bandwidth.host_hz`, null
    /// for none) before this host's call moved it: set back once the call or its link ends.
    restore_bandwidth: Option<Value>,
    /// Aether holds each connection's bandwidth request apart and takes one for listening
    /// (`status.bandwidth.listening`, from 0.2.0-beta.174): while the host listens it runs the
    /// widest bandwidth, which answers calls in the narrower too (RP-4 §1.1).
    listen_wide: bool,
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
    dial: bool,
    /// Aether scans (`caps.scan`) and keeps a call going as long as asked (`caps.call_s`).
    scans: bool,
    lasts: bool,
    /// The host's scan as it was told it (§4.2), and Aether's `owner` for it: this connection.
    scan: Option<Vec<u64>>,
    scan_owner: Option<u64>,
    /// Scan requests not yet answered: Aether's `scan` events meanwhile are their own echo.
    scan_asking: u32,
    log: Option<String>,
}

/// RP-4 §9.1: why Aether ended a scan, in RPMI's words.
#[must_use]
pub fn scan_end(aether: &str) -> &'static str {
    match aether {
        "connection closed" => "left",
        "tuned by another program" => "tuned",
        "knob" => "knob",
        "operator" => "operator",
        "radio" => "radio",
        _ => "asked",
    }
}

/// A scan's `scan_s` as RPMI carries it: whole seconds, rounded up, so a call that keeps trying
/// that long is heard.
fn whole_seconds(s: &Value) -> Value {
    s.as_f64().map_or(Value::Null, |s| {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a scan's few minutes, above 0"
        )]
        let whole = s.max(0.0).ceil() as u64;
        whole.into()
    })
}

fn dials_of(scan: &Value) -> Vec<u64> {
    scan["dials"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default()
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
                call_s,
            } => self.on_connect(&remote, as_call, bandwidth, call_s, rid, &mut out),
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
            Request::Broadcast {
                data,
                reference,
                dial_hz,
            } => {
                self.on_broadcast(&data, reference, dial_hz, rid, &mut out);
            }
            Request::Status => out.push(self.ask("status", json!({}), Waiting::Status { rid })),
            radio @ (Request::Dial | Request::Tune(_) | Request::Scan(_)) => {
                self.on_radio(radio, rid, &mut out);
            }
            Request::Tone(seconds) => self.on_tone(seconds, rid, &mut out),
        }
        out
    }

    /// RP-4 §4.1, §4.2: the radio's dial, read and set through Aether, and its scan.
    fn on_radio(&mut self, request: Request, rid: Option<Value>, out: &mut Vec<Out>) {
        let rid_ref = rid.as_ref();
        match request {
            Request::Scan(dials) => self.on_scan_request(&dials, rid, out),
            Request::Dial | Request::Tune(_) if !self.dial => {
                let text = "this Aether has no way to ask the radio";
                out.push(Out::Host(error(rid_ref, "unsupported", text)));
            }
            Request::Dial => out.push(self.ask("status", json!({}), Waiting::Dial { rid })),
            Request::Tune(hz) => {
                if self.call.is_some() || self.link.is_some() || self.others {
                    let text = "a link or a call is up on the station";
                    out.push(Out::Host(error(rid_ref, "busy", text)));
                } else {
                    // another program's call is no event of Aether's: its state says so
                    out.push(self.ask("status", json!({}), Waiting::TuneStatus { rid, hz }));
                }
            }
            _ => {}
        }
    }

    /// RP-4 §4.3: a tuner tone is Aether's `tune`, `duration_s` the seconds, 0 to stop it. It is
    /// the station's transmitter, so it is refused while a link or a call is up on the station;
    /// a stop is always passed on.
    fn on_tone(&mut self, seconds: u64, rid: Option<Value>, out: &mut Vec<Out>) {
        if seconds > 0 && (self.call.is_some() || self.link.is_some() || self.others) {
            let text = "a link or a call is up on the station";
            out.push(Out::Host(error(rid.as_ref(), "busy", text)));
            return;
        }
        out.push(self.ask(
            "tune",
            json!({"duration_s": seconds}),
            Waiting::Tone { rid },
        ));
    }

    /// The bandwidth this host's call moved the station from is set back (RP-4 §1.1): withdrawing
    /// the request always succeeds, and Aether goes back once it is idle. An Aether that holds
    /// each connection's request apart is asked for this connection's again: none, or the one
    /// for listening.
    fn restore_bandwidth(&mut self, out: &mut Vec<Out>) {
        if let Some(before) = self.restore_bandwidth.take() {
            if self.listen_wide {
                self.listen_bandwidth(out);
            } else {
                out.push(self.ask("bandwidth.set", json!({"hz": before}), Waiting::Nothing));
            }
        }
    }

    /// RP-4 §1.1: while the host listens, the station runs the widest bandwidth Aether runs,
    /// which answers calls in every narrower one too; otherwise this connection asks for none. A
    /// profile had left KK4ODA-1 at 500 Hz while its `RadioPost` client listened, and a 2300 Hz
    /// call to it went unanswered (2026-11-23). Aether holds the request below any program's
    /// own, waits for the station to be idle, and drops it when this connection closes.
    fn listen_bandwidth(&mut self, out: &mut Vec<Out>) {
        if !self.listen_wide || self.bandwidths.len() < 2 {
            return;
        }
        let params = match self.bandwidths.last() {
            Some(&widest) if self.listening => json!({"hz": widest, "listening": true}),
            _ => json!({"hz": null}),
        };
        out.push(self.ask("bandwidth.set", params, Waiting::Nothing));
    }

    /// RP-4 §4.2: a scan is Aether's `scan.start` on this connection, and no dials its
    /// `scan.stop`.
    fn on_scan_request(&mut self, dials: &[u64], rid: Option<Value>, out: &mut Vec<Out>) {
        if !self.scans {
            let text = "this Aether does not scan: it cannot set the radio's dial, or is older \
                        than 0.2.0-beta.162";
            out.push(Out::Host(error(rid.as_ref(), "unsupported", text)));
        } else if !dials.is_empty() {
            self.scan_asking += 1;
            out.push(self.ask("scan.start", json!({"dials": dials}), Waiting::Scan { rid }));
        } else if self.scan.take().is_some() {
            self.scan_asking += 1;
            out.push(self.ask("scan.stop", json!({}), Waiting::ScanStop { rid }));
        } else {
            // a scan.stop from a connection with no scan of its own and no claim would stop
            // another program's scan as the operator's: nothing to stop is said here
            let none: Vec<Value> = Vec::new();
            out.push(Out::Host(answer_with(
                "scan",
                rid.as_ref(),
                &[("dials", none.into())],
            )));
        }
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
        let moving = link.is_none()
            && self.waiting.values().any(|w| {
                matches!(
                    w,
                    Waiting::BandwidthStatus { .. } | Waiting::BandwidthSet { .. }
                )
            });
        if under_way && moving {
            // nothing is out at Aether yet: the call ends once the station has moved
            Self::ok(rid.as_ref(), out);
        } else if under_way {
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
        dial_hz: Option<u64>,
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
        // the dial it belongs on: Aether takes its scan there to send it (its ADR-0129 §15)
        if let Some(hz) = dial_hz {
            params["dial_hz"] = hz.into();
        }
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

    /// RP-4 §4 `status`: the host's callsigns, listening and links, and the station's dial,
    /// the bandwidths a call to the host is answered in, and the dials scanned, from Aether's
    /// `status` (`station`; null when it could not be read).
    fn status(&self, rid: Option<&Value>, station: &Value) -> Message {
        let mut m = self.host_status(rid);
        if let Some(hz) = station["frequency_hz"].as_u64().filter(|&hz| hz > 0) {
            m.insert("dial_hz".into(), hz.into());
        }
        // a station running a bandwidth answers calls in it and in every narrower one it runs
        // (Aether's ADR-0026: a 2300 Hz station moves to 500 Hz to answer a 500 Hz call)
        if let Some(running) = station["bandwidth"]["bandwidth_hz"].as_i64() {
            let answers: Vec<Value> = self
                .bandwidths
                .iter()
                .filter(|&&hz| hz <= running)
                .map(|&hz| hz.into())
                .collect();
            if !answers.is_empty() {
                m.insert("answers".into(), answers.into());
            }
        }
        let scanning = dials_of(&station["scan"]);
        if !scanning.is_empty() {
            m.insert("scanning".into(), scanning.into());
        }
        m
    }

    fn host_status(&self, rid: Option<&Value>) -> Message {
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
        call_s: Option<u64>,
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
        let bandwidth = match bandwidth.map(i64::try_from) {
            None => None,
            Some(Ok(bw)) if self.bandwidths.contains(&bw) => Some(bw),
            Some(_) => {
                let text = "Aether does not run that bandwidth";
                out.push(Out::Host(error(rid_ref, "unsupported", text)));
                return;
            }
        };
        if self.call.is_some() || self.link.is_some() {
            out.push(Out::Host(error(rid_ref, "busy", "a link or a call is up")));
            return;
        }
        // the station's bandwidth is the whole station's, as its dial is (RP-4 §1.1)
        let moving = bandwidth.filter(|&bw| Some(bw) != self.bandwidth_hz);
        if moving.is_some() && self.others {
            let text = "another program's session is up on the station";
            out.push(Out::Host(error(rid_ref, "busy", text)));
            return;
        }
        self.call = Some(Call {
            id: rid.clone(),
            aborted: false,
            remote: remote.to_owned(),
            heard: false,
            heard_snr_db: None,
        });
        let mut params = json!({"remote": remote, "callsign": local,
                                "keep_bandwidth": bandwidth.is_some()});
        // without `caps.call_s` the call is the modem's own length (§4)
        if let Some(s) = call_s.filter(|_| self.lasts) {
            params["call_s"] = s.into();
        }
        match moving {
            // another program's call is no event of Aether's: its state says so, and what a host
            // program asked of the bandwidth is what goes back after
            Some(hz) => out.push(self.ask(
                "status",
                json!({}),
                Waiting::BandwidthStatus { rid, hz, params },
            )),
            None => out.push(self.ask("connect", params, Waiting::Connect { rid })),
        }
    }

    /// The station's state, read before a call in another bandwidth moves it: idle, it moves.
    fn on_bandwidth_status(
        &mut self,
        (rid, hz, params): (Option<Value>, i64, Value),
        (ok, result, message): (bool, &Value, &str),
        out: &mut Vec<Out>,
    ) {
        let refusal = if !ok {
            Some(("refused", message.to_owned()))
        } else if result["state"].as_str().is_some_and(|s| s != "idle") {
            Some((
                "busy",
                "a call or a session is up on the station".to_owned(),
            ))
        } else {
            None
        };
        if let Some((code, text)) = refusal {
            self.call = None;
            out.push(Out::Host(error(rid.as_ref(), code, &text)));
            return;
        }
        if self.call.as_ref().is_some_and(|c| c.aborted) {
            // the host's `abort` came first: nothing moves, and the call ends here
            self.call = None;
            Self::ok(rid.as_ref(), out);
            out.push(Out::Host(answer_with(
                "failed",
                rid.as_ref(),
                &[("code", "aborted".into()), ("text", "aborted".into())],
            )));
            return;
        }
        let before = result["bandwidth"]["host_hz"].clone();
        out.push(self.ask(
            "bandwidth.set",
            json!({"hz": hz}),
            Waiting::BandwidthSet {
                rid,
                params,
                before,
            },
        ));
    }

    /// The station moved for the call, or would not: the call goes out, or is refused `busy`.
    fn on_bandwidth_set(
        &mut self,
        (rid, params, before): (Option<Value>, Value, Value),
        (ok, message): (bool, &str),
        out: &mut Vec<Out>,
    ) {
        if !ok {
            // Aether stays where it is while anything is under way on the air
            self.call = None;
            out.push(Out::Host(error(rid.as_ref(), "busy", message)));
            return;
        }
        self.restore_bandwidth = Some(before);
        if self.call.as_ref().is_some_and(|c| c.aborted) {
            // the host's `abort` came while the station moved: the call ends before it went out
            self.call = None;
            Self::ok(rid.as_ref(), out);
            out.push(Out::Host(answer_with(
                "failed",
                rid.as_ref(),
                &[("code", "aborted".into()), ("text", "aborted".into())],
            )));
            self.restore_bandwidth(out);
            return;
        }
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
                // every bandwidth Aether runs: a call in another moves the station for it
                self.bandwidths = result["bandwidths_hz"]
                    .as_array()
                    .map(|a| a.iter().filter_map(Value::as_i64).collect())
                    .unwrap_or_default();
                if self.bandwidths.is_empty() {
                    self.bandwidths.extend(self.bandwidth_hz);
                }
                self.bandwidths.sort_unstable();
                self.bandwidths.dedup();
                self.shared = status["claims"].is_array();
                self.listen_wide = status["bandwidth"]
                    .as_object()
                    .is_some_and(|b| b.contains_key("listening"));
                self.channel_busy = status["channel_busy"].as_bool().unwrap_or(false);
                self.others = status["state"].as_str().is_some_and(|s| s != "idle");
                self.dial = status["can_tune"].as_bool() == Some(true);
                // `status.scan` is there, null or not, from 0.2.0-beta.162 (its ADR-0129)
                self.lasts = status.as_object().is_some_and(|s| s.contains_key("scan"));
                self.scans = self.lasts && self.dial;
                self.hello = true;
                let caps = Caps {
                    bandwidths: self.bandwidths.clone(),
                    max_links: 1,
                    delivery: true,
                    broadcast: true,
                    quality: true,
                    busy: true,
                    regulatory: true,
                    max_queue: 65536,
                    shared: self.shared,
                    dial: self.dial,
                    scan: self.scans,
                    call_s: self.lasts,
                    tone: true,
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
                if self.shared {
                    // Aether names the program among those using the station for as long as this
                    // connection lasts, and claims nothing for it (its ADR-0114, 0.2.0-beta.159):
                    // a host between calls, which says hello and nothing more, is seen too. An
                    // earlier Aether answers `unknown_method`, and nothing waits on the answer.
                    if !self.app.trim().is_empty() {
                        let params = json!({"app": self.app});
                        out.push(self.ask("hello", params, Waiting::Nothing));
                    }
                    if let Some(path) = self.log.clone() {
                        let params = json!({"path": path, "app": self.app});
                        out.push(self.ask("share.include", params, Waiting::Nothing));
                    }
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
                    // a call of this connection's holds its own request until it ends
                    if self.restore_bandwidth.is_none() {
                        self.listen_bandwidth(&mut out);
                    }
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
                    self.restore_bandwidth(&mut out);
                }
            }
            Waiting::BandwidthStatus { rid, hz, params } => {
                self.on_bandwidth_status((rid, hz, params), (ok, result, &message), &mut out);
            }
            Waiting::BandwidthSet {
                rid,
                params,
                before,
            } => self.on_bandwidth_set((rid, params, before), (ok, &message), &mut out),
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
            Waiting::Dial { rid } => match result["frequency_hz"].as_u64() {
                Some(hz) if ok => out.push(Out::Host(answer_with(
                    "dial",
                    rid.as_ref(),
                    &[("dial_hz", hz.into())],
                ))),
                _ if ok => {
                    let text = "the radio has not said where its dial is";
                    out.push(Out::Host(error(rid.as_ref(), "refused", text)));
                }
                _ => out.push(Out::Host(error(rid.as_ref(), "refused", &message))),
            },
            Waiting::TuneStatus { rid, hz } => {
                if !ok {
                    out.push(Out::Host(error(rid.as_ref(), "refused", &message)));
                } else if result["state"].as_str().is_some_and(|s| s != "idle") {
                    let text = "a link or a call is up on the station";
                    out.push(Out::Host(error(rid.as_ref(), "busy", text)));
                } else {
                    let params = json!({"hz": hz});
                    out.push(self.ask("frequency.set", params, Waiting::Tune { rid }));
                }
            }
            // the dial set, or the tuner tone keyed or stopped
            Waiting::Status { rid } => {
                // Aether's own unread: the host's part still answers
                let station = if ok { result.clone() } else { Value::Null };
                out.push(Out::Host(self.status(rid.as_ref(), &station)));
            }
            Waiting::Tune { rid } | Waiting::Tone { rid } => {
                if ok {
                    Self::ok(rid.as_ref(), &mut out);
                } else {
                    out.push(Out::Host(error(rid.as_ref(), "refused", &message)));
                }
            }
            Waiting::Scan { rid } => {
                self.scan_asking = self.scan_asking.saturating_sub(1);
                if ok {
                    let dials = dials_of(result);
                    self.scan = Some(dials.clone());
                    self.scan_owner = result["owner"].as_u64();
                    out.push(Out::Host(answer_with(
                        "scan",
                        rid.as_ref(),
                        &[
                            ("dials", dials.into()),
                            ("scan_s", whole_seconds(&result["scan_s"])),
                        ],
                    )));
                } else {
                    let rpmi = match code.as_str() {
                        "claimed" => "busy",
                        "unsupported" => "unsupported",
                        _ => "refused",
                    };
                    out.push(Out::Host(error(rid.as_ref(), rpmi, &message)));
                }
            }
            Waiting::ScanStop { rid } => {
                self.scan_asking = self.scan_asking.saturating_sub(1);
                // stopped, or ended already: either way the host has no scan now
                let none: Vec<Value> = Vec::new();
                out.push(Out::Host(answer_with(
                    "scan",
                    rid.as_ref(),
                    &[("dials", none.into())],
                )));
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
            "frame" => self.on_frame(data),
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
                    // where Aether heard it whole, when it reads the dial (RP-4 §6)
                    if let Some(hz) = data["dial_hz"].as_u64().filter(|&hz| hz > 0) {
                        m.insert("dial_hz".into(), hz.into());
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
            "scan" => self.on_scan(data, &mut out),
            // every change of the radio's dial, a tune or the knob, is told to every host (§4.1)
            "dial" => {
                if self.hello
                    && self.dial
                    && let Some(hz) = data["frequency_hz"].as_u64()
                {
                    out.push(Out::Host(answer_with(
                        "dial",
                        None,
                        &[("dial_hz", hz.into())],
                    )));
                }
            }
            _ => {}
        }
        out
    }

    /// Aether's `scan` event (§4.2): the host hears that its scan ended, and why, or that a dial
    /// the radio would not take was dropped from it. Another program's scan is not its business,
    /// and while a scan request of its own is unanswered the event is that request's echo.
    fn on_scan(&mut self, data: &Value, out: &mut Vec<Out>) {
        if self.scan_asking > 0 || self.scan.is_none() {
            return;
        }
        let scan = &data["scan"];
        if scan.is_null() {
            self.scan = None;
            let why = scan_end(data["why"].as_str().unwrap_or(""));
            let none: Vec<Value> = Vec::new();
            out.push(Out::Host(answer_with(
                "scan",
                None,
                &[("dials", none.into()), ("why", why.into())],
            )));
            return;
        }
        let dials = dials_of(scan);
        if scan["owner"].as_u64() == self.scan_owner && self.scan.as_ref() != Some(&dials) {
            self.scan = Some(dials.clone());
            out.push(Out::Host(answer_with(
                "scan",
                None,
                &[
                    ("dials", dials.into()),
                    ("scan_s", whole_seconds(&scan["scan_s"])),
                ],
            )));
        }
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
                    self.restore_bandwidth(out);
                } else if let Some(call) = self.call.take() {
                    let mut m = answer_with(
                        "failed",
                        call.id.as_ref(),
                        &[
                            ("code", failure_code(detail, call.aborted).into()),
                            ("text", detail.into()),
                            ("heard", call.heard.into()),
                        ],
                    );
                    if let Some(snr) = call.heard_snr_db {
                        m.insert("snr_db".into(), snr.into());
                    }
                    out.push(Out::Host(m));
                    self.restore_bandwidth(out);
                }
            }
            _ => {}
        }
    }

    /// A frame the receiver found: one decoded from the station called, during the call, says it
    /// was heard (RP-4 §5 `failed`'s `heard`), however the call ends.
    fn on_frame(&mut self, data: &Value) {
        let Some(call) = self.call.as_mut() else {
            return;
        };
        let from = data["from"].as_str().unwrap_or("");
        if data["decoded"] == true && from.eq_ignore_ascii_case(&call.remote) {
            call.heard = true;
            if let Some(snr) = data["snr_db"].as_f64() {
                call.heard_snr_db = Some(call.heard_snr_db.map_or(snr, |s| s.max(snr)));
            }
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
                ("peer_snr_db", "peer_snr_db"),
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
        let mut out = Vec::new();
        if self.modem_lost {
            return out;
        }
        if link || call {
            out.push(self.ask("abort", json!({}), Waiting::Nothing));
        }
        // a call it moved the station for: the station goes back once the abort has ended it,
        // and it no longer listens (Aether drops the connection's request with it too)
        self.listening = false;
        self.restore_bandwidth(&mut out);
        out
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
        // an Aether that comes back starts from its own settings
        self.restore_bandwidth = None;
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
                | Waiting::Broadcast { rid, .. }
                | Waiting::Dial { rid }
                | Waiting::TuneStatus { rid, .. }
                | Waiting::Tune { rid }
                | Waiting::Scan { rid }
                | Waiting::ScanStop { rid }
                | Waiting::BandwidthStatus { rid, .. }
                | Waiting::BandwidthSet { rid, .. }
                | Waiting::Tone { rid }
                | Waiting::Status { rid } => rid,
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
