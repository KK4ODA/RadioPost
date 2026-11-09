// SPDX-License-Identifier: MIT OR Apache-2.0
//! A simulated channel: stations on one frequency, each with an adapter serving the hosts
//! connected to it, under RP-4 §1 to §7. It is the loopback adapter of the detailed design (§7
//! there) and the start of the conformance tester's test adapter (its outline, §2.2).
//!
//! Calls, data and broadcasts cross the channel at once and in a fixed order, so a conversation
//! comes out the same every time, as the reference that made `rpmi-conversations.json` does. It
//! has no input or output and no clock: lines go in through [`Channel::request_line`], what the
//! adapters say comes out of [`Channel::take`], and the program that drives it decides when a
//! call nobody answered is given up ([`Channel::give_up`]).

use serde_json::Value;

use crate::base64;
use crate::wire::{Caps, Message, Request, VERSION, answer, answer_with, error, op_of, parse_line};

/// One station's adapter and modem.
#[derive(Clone, Debug)]
pub struct StationConfig {
    /// The station, for the program's own records.
    pub name: String,
    /// The modem's name in `hello`.
    pub modem: String,
    /// What the modem can do.
    pub caps: Caps,
    /// Link IDs are this adapter's own: this prefix and a count from 1.
    pub prefix: String,
    /// The most bytes one `received` carries.
    pub frame_bytes: usize,
}

/// What an adapter does on a host's connection.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    /// A line to a host.
    Line {
        /// The station.
        station: usize,
        /// The host.
        host: String,
        /// The message.
        message: Message,
    },
    /// The adapter closes the host's connection.
    Close {
        /// The station.
        station: usize,
        /// The host.
        host: String,
    },
}

#[derive(Debug)]
struct Host {
    name: String,
    hello: bool,
    callsigns: Vec<String>,
    listening: bool,
    busy: bool, // the last `busy` it was told
}

#[derive(Clone, Debug)]
struct End {
    station: usize,
    host: String,
    link: String,
    local: String,
    remote: String,
    direction: &'static str,
    bandwidth: Option<i128>,
}

/// A link: two ends, the callee's first.
#[derive(Debug)]
struct Link {
    ends: [End; 2],
}

/// A call placed and not yet answered.
#[derive(Debug)]
struct Call {
    serial: u64,
    station: usize,
    host: String,
    id: Option<Value>,
    local: String,
    remote: String,
    bandwidth: Option<i128>,
}

#[derive(Debug)]
struct Station {
    cfg: StationConfig,
    hosts: Vec<Host>,
    links_made: u64,
}

/// One channel and the stations on it.
#[derive(Debug, Default)]
pub struct Channel {
    stations: Vec<Station>,
    links: Vec<Link>,
    calls: Vec<Call>,
    calls_made: u64,
    out: Vec<Out>,
}

impl Channel {
    /// An empty channel.
    #[must_use]
    pub fn new() -> Channel {
        Channel::default()
    }

    /// Adds a station; returns its number.
    pub fn add_station(&mut self, cfg: StationConfig) -> usize {
        self.stations.push(Station {
            cfg,
            hosts: Vec::new(),
            links_made: 0,
        });
        self.stations.len() - 1
    }

    /// The station's configuration.
    #[must_use]
    pub fn station(&self, station: usize) -> &StationConfig {
        &self.stations[station].cfg
    }

    /// What the adapters said since the last call, in order.
    pub fn take(&mut self) -> Vec<Out> {
        std::mem::take(&mut self.out)
    }

    /// The serial numbers of calls placed and not answered.
    #[must_use]
    pub fn pending_calls(&self) -> Vec<u64> {
        self.calls.iter().map(|c| c.serial).collect()
    }

    /// A host connects to a station's adapter.
    ///
    /// # Panics
    ///
    /// If a host of that name is connected to the station already: names are the driver's own.
    pub fn open(&mut self, station: usize, host: &str) {
        assert!(
            self.host_index(station, host).is_none(),
            "a host's name is its own at a station"
        );
        self.stations[station].hosts.push(Host {
            name: host.into(),
            hello: false,
            callsigns: Vec::new(),
            listening: false,
            busy: false,
        });
    }

    /// The host's connection closes: its calls, links and callsigns go with it (RP-4 §1.1). The
    /// other end of each link hears `disconnected` `remote`.
    pub fn close(&mut self, station: usize, host: &str) {
        let Some(i) = self.host_index(station, host) else {
            return;
        };
        self.stations[station].hosts.remove(i);
        while let Some(link) = self.link_index(station, host, None) {
            self.end(link, station, host, "failure");
        }
        self.calls
            .retain(|c| !(c.station == station && c.host == host));
    }

    /// A line from a host.
    pub fn request_line(&mut self, station: usize, host: &str, line: &str) {
        match parse_line(line) {
            Ok(m) => self.request(station, host, &m),
            Err(refusal) => self.emit(station, host, refusal),
        }
    }

    /// A line from a host as the bytes it sent, without its LF: text that is not UTF-8 is not
    /// JSON.
    pub fn request_bytes(&mut self, station: usize, host: &str, line: &[u8]) {
        match std::str::from_utf8(line) {
            Ok(line) => self.request_line(station, host, line),
            Err(_) => self.emit(station, host, error(None, "malformed", "not JSON")),
        }
    }

    /// The adapter refuses something a host sent that never became a line: `error` `malformed`.
    pub fn refuse(&mut self, station: usize, host: &str, text: &str) {
        self.emit(station, host, error(None, "malformed", text));
    }

    /// A message from a host, already read by [`parse_line`].
    pub fn request(&mut self, station: usize, host: &str, m: &Message) {
        let Some(h) = self.host_index(station, host) else {
            return;
        };
        let op = op_of(m).to_owned();
        let id = m.get("id").cloned();
        let id = id.as_ref();
        if !self.stations[station].hosts[h].hello && op != "hello" {
            self.emit(station, host, error(id, "refused", "hello first"));
            return;
        }
        let request = match Request::read(m) {
            Ok(Some(r)) => r,
            Ok(None) => {
                self.emit(
                    station,
                    host,
                    error(id, "unsupported", &format!("no op {op}")),
                );
                return;
            }
            Err(why) => {
                self.emit(station, host, error(id, "malformed", &why));
                return;
            }
        };
        match request {
            Request::Hello { low, high, .. } => self.on_hello(station, h, low, high, id),
            Request::Callsigns(list) => self.on_callsigns(station, h, list, id),
            Request::Listen(on) => self.on_listen(station, h, on, id),
            Request::Connect {
                remote,
                as_call,
                bandwidth,
            } => self.on_connect(station, h, &remote, as_call, bandwidth, id),
            Request::Send {
                link,
                data,
                reference,
            } => self.on_send(station, host, &link, &data, reference, id),
            Request::Disconnect { link } => self.on_disconnect(station, host, &link, id),
            Request::Abort { link } => self.on_abort(station, host, link.as_deref(), id),
            Request::Broadcast { data, reference } => {
                self.on_broadcast(station, h, &data, reference, id);
            }
            Request::Status => self.on_status(station, h, id),
        }
    }

    /// The modems give up on every call nobody answered: `failed` `timeout`.
    pub fn give_up(&mut self) {
        for call in std::mem::take(&mut self.calls) {
            self.fail(&call, "timeout", "no answer");
        }
    }

    /// The modem gives up on one call, if it is still unanswered.
    pub fn give_up_call(&mut self, serial: u64) {
        if let Some(i) = self.calls.iter().position(|c| c.serial == serial) {
            let call = self.calls.remove(i);
            self.fail(&call, "timeout", "no answer");
        }
    }

    // -- the channel ----------------------------------------------------------------------------

    fn host_index(&self, station: usize, host: &str) -> Option<usize> {
        self.stations[station]
            .hosts
            .iter()
            .position(|h| h.name == host)
    }

    fn emit(&mut self, station: usize, host: &str, message: Message) {
        if self.host_index(station, host).is_some() {
            self.out.push(Out::Line {
                station,
                host: host.into(),
                message,
            });
        }
    }

    fn ok(&mut self, station: usize, host: &str, id: Option<&Value>) {
        if id.is_some() {
            self.emit(station, host, answer("ok", id));
        }
    }

    fn fail(&mut self, call: &Call, code: &str, text: &str) {
        let m = answer_with(
            "failed",
            call.id.as_ref(),
            &[("code", code.into()), ("text", text.into())],
        );
        self.emit(call.station, &call.host, m);
    }

    /// The link with an end at this host (and with this link ID, if given), and that end's side.
    fn link_index(&self, station: usize, host: &str, link: Option<&str>) -> Option<usize> {
        self.links.iter().position(|l| {
            l.ends.iter().any(|e| {
                e.station == station && e.host == host && link.is_none_or(|id| e.link == id)
            })
        })
    }

    fn ends_at(&self, station: usize) -> usize {
        self.links
            .iter()
            .flat_map(|l| &l.ends)
            .filter(|e| e.station == station)
            .count()
    }

    fn full(&self, station: usize) -> bool {
        self.ends_at(station) >= self.stations[station].cfg.caps.max_links
    }

    /// The channel is in use by others: a link with no end at this host (RP-4 §1.1).
    fn busy_for(&self, station: usize, host: &str) -> bool {
        self.links.iter().any(|l| {
            !l.ends
                .iter()
                .any(|e| e.station == station && e.host == host)
        })
    }

    fn update_busy(&mut self) {
        for s in 0..self.stations.len() {
            if !self.stations[s].cfg.caps.busy {
                continue;
            }
            for h in 0..self.stations[s].hosts.len() {
                let name = self.stations[s].hosts[h].name.clone();
                let now = self.busy_for(s, &name);
                let host = &mut self.stations[s].hosts[h];
                if host.hello && now != host.busy {
                    host.busy = now;
                    self.emit(s, &name, answer_with("busy", None, &[("busy", now.into())]));
                }
            }
        }
    }

    fn new_end(
        &mut self,
        station: usize,
        host: &str,
        local: &str,
        remote: &str,
        direction: &'static str,
        bandwidth: Option<i128>,
    ) -> End {
        let s = &mut self.stations[station];
        s.links_made += 1;
        End {
            station,
            host: host.into(),
            link: format!("{}{}", s.cfg.prefix, s.links_made),
            local: local.into(),
            remote: remote.into(),
            direction,
            bandwidth,
        }
    }

    /// A call goes out: the first host anywhere that holds the callsign and listens answers.
    fn place(&mut self, call: Call) {
        for s in 0..self.stations.len() {
            if s == call.station || self.full(s) {
                continue;
            }
            let answering = self.stations[s]
                .hosts
                .iter()
                .find(|h| h.hello && h.listening && h.callsigns.contains(&call.remote))
                .map(|h| h.name.clone());
            if let Some(host) = answering {
                self.answer(&call, s, &host);
                return;
            }
        }
        self.calls.push(call);
    }

    fn answer(&mut self, call: &Call, station: usize, host: &str) {
        let incoming = answer_with(
            "incoming",
            None,
            &[
                ("remote", call.local.clone().into()),
                ("local", call.remote.clone().into()),
            ],
        );
        self.emit(station, host, incoming);
        let answering = self.new_end(
            station,
            host,
            &call.remote,
            &call.local,
            "in",
            call.bandwidth,
        );
        let calling = self.new_end(
            call.station,
            &call.host,
            &call.local,
            &call.remote,
            "out",
            call.bandwidth,
        );
        self.emit(station, host, connected_message(&answering, None));
        self.emit(
            call.station,
            &call.host,
            connected_message(&calling, call.id.as_ref()),
        );
        self.links.push(Link {
            ends: [answering, calling],
        });
        self.update_busy();
    }

    /// The link ends, from this host's side: it hears `reason`, the other end `remote`.
    fn end(&mut self, link: usize, station: usize, host: &str, reason: &str) {
        let Link { ends } = self.links.remove(link);
        let mine = usize::from(!(ends[0].station == station && ends[0].host == host));
        for (e, why) in [(&ends[mine], reason), (&ends[1 - mine], "remote")] {
            let m = answer_with(
                "disconnected",
                None,
                &[("link", e.link.clone().into()), ("reason", why.into())],
            );
            self.emit(e.station, &e.host, m);
        }
        self.update_busy();
    }

    // -- requests -------------------------------------------------------------------------------

    fn on_hello(&mut self, station: usize, h: usize, low: i128, high: i128, id: Option<&Value>) {
        let name = self.stations[station].hosts[h].name.clone();
        if !(low..=high).contains(&i128::from(VERSION)) {
            let m = error(
                id,
                "unsupported-version",
                &format!("this adapter speaks {VERSION}"),
            );
            self.emit(station, &name, m);
            self.out.push(Out::Close {
                station,
                host: name.clone(),
            });
            self.close(station, &name);
            return;
        }
        let s = &self.stations[station];
        let unshared_and_taken = !s.cfg.caps.shared
            && s.hosts
                .iter()
                .enumerate()
                .any(|(i, other)| i != h && other.hello);
        if unshared_and_taken {
            self.emit(
                station,
                &name,
                error(id, "busy", "another program has this modem"),
            );
            self.out.push(Out::Close {
                station,
                host: name.clone(),
            });
            self.close(station, &name);
            return;
        }
        self.stations[station].hosts[h].hello = true;
        let s = &self.stations[station];
        let m = answer_with(
            "hello",
            id,
            &[
                ("version", VERSION.into()),
                ("modem", s.cfg.modem.clone().into()),
                ("caps", s.cfg.caps.to_json()),
            ],
        );
        self.emit(station, &name, m);
    }

    fn on_callsigns(&mut self, station: usize, h: usize, wanted: Vec<String>, id: Option<&Value>) {
        let name = self.stations[station].hosts[h].name.clone();
        for (i, other) in self.stations[station].hosts.iter().enumerate() {
            if i == h {
                continue;
            }
            if let Some(taken) = wanted.iter().find(|c| other.callsigns.contains(c)) {
                let m = error(
                    id,
                    "claimed",
                    &format!("{taken} is held by another program"),
                );
                self.emit(station, &name, m);
                return;
            }
        }
        self.stations[station].hosts[h].callsigns = wanted;
        self.ok(station, &name, id);
    }

    fn on_listen(&mut self, station: usize, h: usize, on: bool, id: Option<&Value>) {
        let host = &mut self.stations[station].hosts[h];
        let name = host.name.clone();
        if host.callsigns.is_empty() {
            self.emit(station, &name, error(id, "refused", "no callsigns yet"));
            return;
        }
        host.listening = on;
        self.ok(station, &name, id);
    }

    fn on_connect(
        &mut self,
        station: usize,
        h: usize,
        remote: &str,
        as_call: Option<String>,
        bandwidth: Option<i128>,
        id: Option<&Value>,
    ) {
        let host = &self.stations[station].hosts[h];
        let name = host.name.clone();
        let Some(first) = host.callsigns.first() else {
            self.emit(station, &name, error(id, "refused", "no callsigns yet"));
            return;
        };
        let local = as_call.unwrap_or_else(|| first.clone());
        if !host.callsigns.contains(&local) {
            let m = error(id, "not-authorized", &format!("{local} is not this host's"));
            self.emit(station, &name, m);
            return;
        }
        let caps = &self.stations[station].cfg.caps;
        if let Some(bw) = bandwidth
            && !caps.bandwidths.iter().any(|&b| i128::from(b) == bw)
        {
            let m = error(id, "unsupported", &format!("no {bw} Hz on this modem"));
            self.emit(station, &name, m);
            return;
        }
        let calling = self
            .calls
            .iter()
            .any(|c| c.station == station && c.host == name);
        if self.full(station) || calling || self.busy_for(station, &name) {
            self.emit(station, &name, error(id, "busy", "a link or a call is up"));
            return;
        }
        self.ok(station, &name, id);
        self.calls_made += 1;
        let call = Call {
            serial: self.calls_made,
            station,
            host: name,
            id: id.cloned(),
            local,
            remote: remote.into(),
            bandwidth,
        };
        self.place(call);
    }

    fn on_send(
        &mut self,
        station: usize,
        host: &str,
        link: &str,
        data: &[u8],
        reference: Option<Value>,
        id: Option<&Value>,
    ) {
        let Some(l) = self.link_index(station, host, Some(link)) else {
            let m = error(id, "not-connected", "no such link of this host's");
            self.emit(station, host, m);
            return;
        };
        self.ok(station, host, id);
        let queued = |bytes: usize| {
            answer_with(
                "queued",
                None,
                &[("link", link.into()), ("bytes", bytes.into())],
            )
        };
        self.emit(station, host, queued(data.len()));
        let ends = &self.links[l].ends;
        let peer = if ends[0].station == station && ends[0].host == host {
            ends[1].clone()
        } else {
            ends[0].clone()
        };
        let frame = self.stations[peer.station].cfg.frame_bytes.max(1);
        for piece in data.chunks(frame) {
            let m = answer_with(
                "received",
                None,
                &[
                    ("link", peer.link.clone().into()),
                    ("data", base64::encode(piece).into()),
                ],
            );
            self.emit(peer.station, &peer.host, m);
        }
        if let Some(r) = reference
            && self.stations[station].cfg.caps.delivery
        {
            let m = answer_with("delivered", None, &[("link", link.into()), ("ref", r)]);
            self.emit(station, host, m);
        }
        self.emit(station, host, queued(0));
    }

    fn on_disconnect(&mut self, station: usize, host: &str, link: &str, id: Option<&Value>) {
        let Some(l) = self.link_index(station, host, Some(link)) else {
            let m = error(id, "not-connected", "no such link of this host's");
            self.emit(station, host, m);
            return;
        };
        self.ok(station, host, id);
        self.end(l, station, host, "local");
    }

    fn on_abort(&mut self, station: usize, host: &str, link: Option<&str>, id: Option<&Value>) {
        if let Some(link) = link {
            self.on_disconnect(station, host, link, id);
            return;
        }
        let Some(i) = self
            .calls
            .iter()
            .position(|c| c.station == station && c.host == host)
        else {
            self.emit(
                station,
                host,
                error(id, "not-connected", "no call under way"),
            );
            return;
        };
        let call = self.calls.remove(i);
        self.ok(station, host, id);
        self.fail(&call, "aborted", "the host aborted");
    }

    fn on_broadcast(
        &mut self,
        station: usize,
        h: usize,
        data: &[u8],
        reference: Option<Value>,
        id: Option<&Value>,
    ) {
        let host = &self.stations[station].hosts[h];
        let name = host.name.clone();
        if !self.stations[station].cfg.caps.broadcast {
            let m = error(id, "unsupported", "no broadcasts on this modem");
            self.emit(station, &name, m);
            return;
        }
        let Some(source) = host.callsigns.first().cloned() else {
            self.emit(station, &name, error(id, "refused", "no callsigns yet"));
            return;
        };
        self.ok(station, &name, id);
        let text = base64::encode(data);
        for s in 0..self.stations.len() {
            if s == station {
                continue;
            }
            let hearing: Vec<String> = self.stations[s]
                .hosts
                .iter()
                .filter(|o| o.hello)
                .map(|o| o.name.clone())
                .collect();
            for other in hearing {
                let m = answer_with(
                    "broadcast",
                    None,
                    &[
                        ("source", source.clone().into()),
                        ("data", text.clone().into()),
                    ],
                );
                self.emit(s, &other, m);
            }
        }
        let mut sent = answer_with("broadcast-sent", None, &[("sent", true.into())]);
        if let Some(r) = reference {
            sent.insert("ref".into(), r);
        }
        self.emit(station, &name, sent);
    }

    fn on_status(&mut self, station: usize, h: usize, id: Option<&Value>) {
        let host = &self.stations[station].hosts[h];
        let name = host.name.clone();
        let links: Vec<Value> = self
            .links
            .iter()
            .flat_map(|l| &l.ends)
            .filter(|e| e.station == station && e.host == name)
            .map(|e| {
                serde_json::json!({
                    "link": e.link, "remote": e.remote, "direction": e.direction, "queued": 0
                })
            })
            .collect();
        let m = answer_with(
            "status",
            id,
            &[
                ("listening", host.listening.into()),
                ("callsigns", host.callsigns.clone().into()),
                ("busy", self.busy_for(station, &name).into()),
                ("links", links.into()),
            ],
        );
        self.emit(station, &name, m);
    }
}

fn connected_message(end: &End, id: Option<&Value>) -> Message {
    let mut m = answer_with(
        "connected",
        id,
        &[
            ("link", end.link.clone().into()),
            ("remote", end.remote.clone().into()),
            ("local", end.local.clone().into()),
            ("direction", end.direction.into()),
        ],
    );
    if let Some(bw) = end.bandwidth {
        m.insert(
            "bandwidth".into(),
            Value::from(i64::try_from(bw).unwrap_or(0)),
        );
    }
    m
}

#[cfg(test)]
mod tests {
    use super::{Channel, Out, StationConfig};
    use crate::wire::Caps;

    fn channel() -> (Channel, usize) {
        let mut ch = Channel::new();
        let s = ch.add_station(StationConfig {
            name: "KK4ODA".into(),
            modem: "a simulated modem".into(),
            caps: Caps {
                bandwidths: vec![2300],
                max_links: 1,
                delivery: true,
                broadcast: true,
                quality: false,
                busy: true,
                regulatory: false,
                max_queue: 65536,
                shared: false,
            },
            prefix: "c".into(),
            frame_bytes: 128,
        });
        ch.open(s, "host");
        (ch, s)
    }

    /// RP-4 §3: every request but `hello` is refused before `hello`, good or not.
    #[test]
    fn every_op_waits_for_hello() {
        for line in [
            r#"{"op":"callsigns","id":1,"callsigns":["KK4ODA"]}"#,
            r#"{"op":"listen","id":1,"on":true}"#,
            r#"{"op":"connect","id":1,"remote":"N1CALL-10"}"#,
            r#"{"op":"send","id":1,"link":"c1","data":"AQ=="}"#,
            r#"{"op":"disconnect","id":1,"link":"c1"}"#,
            r#"{"op":"abort","id":1}"#,
            r#"{"op":"broadcast","id":1,"data":"AQ=="}"#,
            r#"{"op":"status","id":1}"#,
            r#"{"op":"tune","id":1}"#,
            r#"{"op":"listen","id":1}"#,
        ] {
            let (mut ch, s) = channel();
            ch.request_line(s, "host", line);
            let out = ch.take();
            assert!(
                matches!(&out[..], [Out::Line { message, .. }]
                    if message["code"] == "refused" && message["id"] == 1),
                "{line}: {out:?}"
            );
        }
    }
}
