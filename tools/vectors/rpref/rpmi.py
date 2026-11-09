# SPDX-License-Identifier: MIT OR Apache-2.0
"""RPMI, the modem interface of RP-4, as plainly as the specification says it.

A `World` is one radio channel with stations on it. Each station has an `Adapter`, and each
adapter serves the hosts that speak RPMI to it. Calls, data and broadcasts cross the channel at
once and in a fixed order, so a conversation comes out the same every time. Nothing here is about
real radios: it is RP-4 §1 to §7, for making and checking test vectors.
"""

from __future__ import annotations

import base64
import json
import re
from dataclasses import dataclass, field
from typing import Any

VERSION = 1
Message = dict[str, Any]

# RFC 4648 §4, standard alphabet, with padding (RP-4 §2).
BASE64 = re.compile(r"(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?")


def encode_data(data: bytes) -> str:
    return base64.b64encode(data).decode("ascii")


def decode_data(text: Any) -> bytes | None:
    """The bytes of a `data` member, or None when it is not standard base64 with padding."""
    if not isinstance(text, str) or not BASE64.fullmatch(text):
        return None
    return base64.b64decode(text)


def line_of(message: Message) -> str:
    """One line of JSON Lines (RP-4 §2), without its LF. Any JSON spelling is as good."""
    return json.dumps(message, ensure_ascii=False, separators=(",", ":"))


def answer(op: str, rid: Any, **members: Any) -> Message:
    out: Message = {"op": op}
    if rid is not None:
        out["id"] = rid
    out.update(members)
    return out


def error(rid: Any, code: str, text: str) -> Message:
    return answer("error", rid, code=code, text=text)


def parse_line(line: str) -> tuple[Message | None, Message | None]:
    """RP-4 §2: (the request, None), or (None, the `error` to answer it with)."""
    try:
        obj = json.loads(line)
    except ValueError:
        return None, error(None, "malformed", "not JSON")
    if not isinstance(obj, dict):
        return None, error(None, "malformed", "not a JSON object")
    rid = obj.get("id")
    if "id" in obj and not is_id(rid):
        return None, error(None, "malformed", "`id` is neither a string nor an integer")
    if not isinstance(obj.get("op"), str):
        return None, error(rid, "malformed", "no `op`")
    if "data" in obj and decode_data(obj["data"]) is None:
        return None, error(rid, "malformed", "`data` is not standard base64 with padding")
    return obj, None


def is_id(value: Any) -> bool:
    """A string or an integer, as `id` and `ref` are (RP-4 §2, §4)."""
    return not isinstance(value, bool) and isinstance(value, str | int)


def is_text_list(value: Any) -> bool:
    return isinstance(value, list) and all(isinstance(c, str) for c in value)


def is_version(value: Any) -> bool:
    return (
        isinstance(value, list)
        and len(value) == 2
        and all(isinstance(v, int) and not isinstance(v, bool) for v in value)
    )


# RP-4 §4: each request's members, those without a `?` in its table needed, with their types.
MEMBERS: dict[str, list[tuple[str, bool, Any]]] = {
    "hello": [("version", True, is_version), ("app", True, lambda v: isinstance(v, str))],
    "callsigns": [("callsigns", True, is_text_list)],
    "listen": [("on", True, lambda v: isinstance(v, bool))],
    "connect": [
        ("id", True, is_id),
        ("remote", True, lambda v: isinstance(v, str)),
        ("as", False, lambda v: isinstance(v, str)),
        ("bandwidth", False, lambda v: isinstance(v, int) and not isinstance(v, bool)),
    ],
    "send": [
        ("link", True, lambda v: isinstance(v, str)),
        ("data", True, lambda v: isinstance(v, str)),
        ("ref", False, is_id),
    ],
    "disconnect": [("link", True, lambda v: isinstance(v, str))],
    "abort": [("link", False, lambda v: isinstance(v, str))],
    "broadcast": [("data", True, lambda v: isinstance(v, str)), ("ref", False, is_id)],
    "status": [],
}


def member_problem(message: Message) -> str | None:
    """RP-4 §2, §4: what makes a known request `malformed`, or None."""
    for name, needed, good in MEMBERS[message["op"]]:
        if name not in message:
            if needed:
                return f"no `{name}`"
        elif not good(message[name]):
            return f"`{name}` is not of its type"
    return None


@dataclass
class Host:
    """What an adapter knows of one host connected to it."""

    name: str
    hello: bool = False
    callsigns: list[str] = field(default_factory=list)
    listening: bool = False
    busy: bool = False  # the last `busy` it was told


@dataclass
class End:
    """One station's end of a link."""

    adapter: Adapter
    host: str
    link: str
    local: str
    remote: str
    direction: str
    bandwidth: int | None
    peer: End | None = None


@dataclass
class Call:
    """A call placed and not yet answered."""

    adapter: Adapter
    host: str
    rid: Any
    local: str
    remote: str
    bandwidth: int | None


class World:
    """One channel. `log` holds every line exchanged on every station, in order."""

    def __init__(self) -> None:
        self.adapters: list[Adapter] = []
        self.calls: list[Call] = []
        self.log: list[dict[str, Any]] = []

    def record(self, station: str, host: str, entry: dict[str, Any]) -> None:
        self.log.append({"station": station, "host": host, **entry})

    def note(self, text: str) -> None:
        self.log.append({"note": text})

    def ends(self) -> list[End]:
        return [e for a in self.adapters for e in a.ends]

    def place(self, call: Call) -> None:
        """A call goes out: the first host anywhere that holds the callsign and listens answers."""
        for adapter in self.adapters:
            if adapter is call.adapter or adapter.full():
                continue
            for host in adapter.hosts.values():
                if host.hello and host.listening and call.remote in host.callsigns:
                    self.answer(call, adapter, host)
                    return
        self.calls.append(call)

    def answer(self, call: Call, adapter: Adapter, host: Host) -> None:
        adapter.emit(host.name, answer("incoming", None, remote=call.local, local=call.remote))
        callee = adapter.new_end(host.name, call.remote, call.local, "in", call.bandwidth)
        caller = call.adapter.new_end(call.host, call.local, call.remote, "out", call.bandwidth)
        callee.peer, caller.peer = caller, callee
        adapter.emit(host.name, connected_message(callee, None))
        call.adapter.emit(call.host, connected_message(caller, call.rid))
        self.update_busy()

    def give_up(self) -> None:
        """The modems give up on every call nobody answered."""
        for call in self.calls:
            call.adapter.emit(
                call.host, answer("failed", call.rid, code="timeout", text="no answer")
            )
        self.calls.clear()

    def end(self, end: End, reason: str) -> None:
        assert end.peer is not None
        for e, why in ((end, reason), (end.peer, "remote")):
            e.adapter.ends.remove(e)
            e.adapter.emit(e.host, answer("disconnected", None, link=e.link, reason=why))
        self.update_busy()

    def busy_for(self, adapter: Adapter, host: str) -> bool:
        """The channel is in use by others: some link with no end at this host (RP-4 §1.1)."""
        mine = [e for e in adapter.ends if e.host == host]
        return any(e not in mine and e.peer not in mine for e in self.ends())

    def update_busy(self) -> None:
        for adapter in self.adapters:
            if not adapter.caps.get("busy"):
                continue
            for host in adapter.hosts.values():
                now = self.busy_for(adapter, host.name)
                if host.hello and now != host.busy:
                    host.busy = now
                    adapter.emit(host.name, answer("busy", None, busy=now))


def connected_message(end: End, rid: Any) -> Message:
    out = answer(
        "connected", rid, link=end.link, remote=end.remote, local=end.local, direction=end.direction
    )
    if end.bandwidth is not None:
        out["bandwidth"] = end.bandwidth
    return out


class Adapter:
    """One station's adapter: RP-4's rules between its hosts and the channel."""

    def __init__(
        self,
        world: World,
        station: str,
        *,
        modem: str,
        caps: dict[str, Any],
        prefix: str,
        frame_bytes: int,
    ) -> None:
        self.world = world
        self.station = station
        self.modem = modem
        self.caps = caps
        self.prefix = prefix  # link IDs are this adapter's own: prefix and a number
        self.frame_bytes = frame_bytes  # how much a `received` carries at most
        self.hosts: dict[str, Host] = {}
        self.ends: list[End] = []
        self.links = 0
        world.adapters.append(self)

    # -- the connection to a host ----------------------------------------------------------------

    def open(self, host: str) -> None:
        self.world.record(self.station, host, {"connection": "opened"})
        self.hosts[host] = Host(host)

    def close(self, host: str) -> None:
        """The host's connection closes: its calls, links and callsigns go with it (RP-4 §1.1)."""
        self.world.record(self.station, host, {"connection": "closed"})
        del self.hosts[host]  # nothing more is said on a closed connection
        for end in [e for e in self.ends if e.host == host]:
            self.world.end(end, "failure")
        self.world.calls = [
            c for c in self.world.calls if not (c.adapter is self and c.host == host)
        ]

    def emit(self, host: str, message: Message) -> None:
        if host in self.hosts:
            self.world.record(self.station, host, {"to": "host", "message": message})

    def request(self, host: str, message: Message) -> None:
        self.world.record(self.station, host, {"to": "adapter", "message": message})
        parsed, refused = parse_line(line_of(message))
        assert refused is None and parsed == message, "the reference writes only good lines"
        self.dispatch(host, message)

    def request_line(self, host: str, line: str) -> None:
        self.world.record(self.station, host, {"to": "adapter", "line": line})
        parsed, refused = parse_line(line)
        if refused is not None:
            self.emit(host, refused)
        else:
            assert parsed is not None
            self.dispatch(host, parsed)

    # -- requests --------------------------------------------------------------------------------

    def dispatch(self, host: str, message: Message) -> None:
        h = self.hosts[host]
        op, rid = message["op"], message.get("id")
        if not h.hello and op != "hello":
            self.emit(host, error(rid, "refused", "hello first"))
            return
        handler = {
            "hello": self.on_hello,
            "callsigns": self.on_callsigns,
            "listen": self.on_listen,
            "connect": self.on_connect,
            "send": self.on_send,
            "disconnect": self.on_disconnect,
            "abort": self.on_abort,
            "broadcast": self.on_broadcast,
            "status": self.on_status,
        }.get(op)
        if handler is None:
            self.emit(host, error(rid, "unsupported", f"no op {op}"))
            return
        problem = member_problem(message)
        if problem is not None:
            self.emit(host, error(rid, "malformed", problem))
            return
        handler(h, message, rid)

    def ok(self, h: Host, rid: Any) -> None:
        if rid is not None:
            self.emit(h.name, answer("ok", rid))

    def full(self) -> bool:
        return len(self.ends) >= self.caps["max_links"]

    def end_of(self, h: Host, link: Any) -> End | None:
        return next((e for e in self.ends if e.host == h.name and e.link == link), None)

    def new_end(
        self, host: str, local: str, remote: str, direction: str, bandwidth: int | None
    ) -> End:
        self.links += 1
        end = End(self, host, f"{self.prefix}{self.links}", local, remote, direction, bandwidth)
        self.ends.append(end)
        return end

    def on_hello(self, h: Host, m: Message, rid: Any) -> None:
        low, high = m["version"]
        if not low <= VERSION <= high:
            self.emit(h.name, error(rid, "unsupported-version", f"this adapter speaks {VERSION}"))
            self.close(h.name)
            return
        if not self.caps.get("shared") and any(
            o.hello for o in self.hosts.values() if o is not h
        ):  # RP-4 §1.1, §3: a modem that is not shared serves one host
            self.emit(h.name, error(rid, "busy", "another program has this modem"))
            self.close(h.name)
            return
        h.hello = True
        self.emit(
            h.name, answer("hello", rid, version=VERSION, modem=self.modem, caps=dict(self.caps))
        )

    def on_callsigns(self, h: Host, m: Message, rid: Any) -> None:
        wanted = list(m["callsigns"])
        for other in self.hosts.values():
            taken = [c for c in wanted if other is not h and c in other.callsigns]
            if taken:
                self.emit(h.name, error(rid, "claimed", f"{taken[0]} is held by another program"))
                return
        h.callsigns = wanted
        self.ok(h, rid)

    def on_listen(self, h: Host, m: Message, rid: Any) -> None:
        if not h.callsigns:
            self.emit(h.name, error(rid, "refused", "no callsigns yet"))
            return
        h.listening = bool(m["on"])
        self.ok(h, rid)

    def on_connect(self, h: Host, m: Message, rid: Any) -> None:
        if not h.callsigns:
            self.emit(h.name, error(rid, "refused", "no callsigns yet"))
            return
        local = m.get("as", h.callsigns[0])
        if local not in h.callsigns:
            self.emit(h.name, error(rid, "not-authorized", f"{local} is not this host's"))
            return
        bandwidth = m.get("bandwidth")
        if bandwidth is not None and bandwidth not in self.caps["bandwidths"]:
            self.emit(h.name, error(rid, "unsupported", f"no {bandwidth} Hz on this modem"))
            return
        calling = any(c.adapter is self and c.host == h.name for c in self.world.calls)
        if self.full() or calling or self.world.busy_for(self, h.name):
            self.emit(h.name, error(rid, "busy", "a link or a call is up"))
            return
        self.ok(h, rid)
        self.world.place(Call(self, h.name, rid, local, m["remote"], bandwidth))

    def on_send(self, h: Host, m: Message, rid: Any) -> None:
        end = self.end_of(h, m.get("link"))
        if end is None or end.peer is None:
            self.emit(h.name, error(rid, "not-connected", "no such link of this host's"))
            return
        data = decode_data(m["data"])
        assert data is not None
        self.ok(h, rid)
        link = end.link
        self.emit(h.name, answer("queued", None, link=link, bytes=len(data)))
        peer = end.peer
        for start in range(0, len(data), peer.adapter.frame_bytes):
            piece = data[start : start + peer.adapter.frame_bytes]
            peer.adapter.emit(
                peer.host, answer("received", None, link=peer.link, data=encode_data(piece))
            )
        if "ref" in m and self.caps.get("delivery"):
            self.emit(h.name, answer("delivered", None, link=link, ref=m["ref"]))
        self.emit(h.name, answer("queued", None, link=link, bytes=0))

    def on_disconnect(self, h: Host, m: Message, rid: Any) -> None:
        end = self.end_of(h, m.get("link"))
        if end is None:
            self.emit(h.name, error(rid, "not-connected", "no such link of this host's"))
            return
        self.ok(h, rid)
        self.world.end(end, "local")

    def on_abort(self, h: Host, m: Message, rid: Any) -> None:
        if "link" in m:
            self.on_disconnect(h, m, rid)
            return
        call = next((c for c in self.world.calls if c.adapter is self and c.host == h.name), None)
        if call is None:
            self.emit(h.name, error(rid, "not-connected", "no call under way"))
            return
        self.world.calls.remove(call)
        self.ok(h, rid)
        self.emit(h.name, answer("failed", call.rid, code="aborted", text="the host aborted"))

    def on_broadcast(self, h: Host, m: Message, rid: Any) -> None:
        if not self.caps.get("broadcast"):
            self.emit(h.name, error(rid, "unsupported", "no broadcasts on this modem"))
            return
        if not h.callsigns:
            self.emit(h.name, error(rid, "refused", "no callsigns yet"))
            return
        self.ok(h, rid)
        for adapter in self.world.adapters:
            if adapter is self:
                continue
            for other in adapter.hosts.values():
                if other.hello:
                    adapter.emit(
                        other.name, answer("broadcast", None, source=h.callsigns[0], data=m["data"])
                    )
        sent = answer("broadcast-sent", None, sent=True)
        if "ref" in m:
            sent["ref"] = m["ref"]
        self.emit(h.name, sent)

    def on_status(self, h: Host, m: Message, rid: Any) -> None:
        links = [
            {"link": e.link, "remote": e.remote, "direction": e.direction, "queued": 0}
            for e in self.ends
            if e.host == h.name
        ]
        self.emit(
            h.name,
            answer(
                "status",
                rid,
                listening=h.listening,
                callsigns=list(h.callsigns),
                busy=self.world.busy_for(self, h.name),
                links=links,
            ),
        )
