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
import math
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


MAX_INTEGER = 2**53 - 1
"""RP-4 §2: an integer lies from -(2^53-1) to 2^53-1, as I-JSON's numbers do (RFC 7493 §2.2)."""


class NotIJson(ValueError):
    """A line that is JSON but not I-JSON (RFC 7493): RP-4 §2 reads it as no line at all."""


def _no_repeats(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    names = [n for n, _ in pairs]
    if len(set(names)) != len(names):
        raise NotIJson("a member named twice")
    return dict(pairs)


def _whole(value: Any) -> bool:
    """No lone surrogate in any string, nor number past double precision (RFC 7493 §2.1, §2.2)."""
    if isinstance(value, str):
        return not any(0xD800 <= ord(c) <= 0xDFFF for c in value)
    if isinstance(value, float):
        return math.isfinite(value)
    if isinstance(value, list):
        return all(_whole(v) for v in value)
    if isinstance(value, dict):
        return all(_whole(k) and _whole(v) for k, v in value.items())
    return True


def parse_line(line: str) -> tuple[Message | None, Message | None]:
    """RP-4 §2: (the request, None), or (None, the `error` to answer it with)."""
    try:
        # Python reads NaN and Infinity, which are not JSON; `_whole` refuses them
        obj = json.loads(line, object_pairs_hook=_no_repeats)
    except NotIJson:
        return None, error(None, "malformed", "not I-JSON")
    except ValueError:
        return None, error(None, "malformed", "not JSON")
    if not _whole(obj):
        return None, error(None, "malformed", "not I-JSON")
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


def is_int(value: Any) -> bool:
    """An integer as RP-4 §2 has it: a JSON number with no fraction or exponent, in range."""
    return isinstance(value, int) and not isinstance(value, bool) and abs(value) <= MAX_INTEGER


def is_id(value: Any) -> bool:
    """A string or an integer, as `id` and `ref` are (RP-4 §2, §4)."""
    return isinstance(value, str) or is_int(value)


def is_text_list(value: Any) -> bool:
    return isinstance(value, list) and all(isinstance(c, str) for c in value)


def is_version(value: Any) -> bool:
    return isinstance(value, list) and len(value) == 2 and all(is_int(v) for v in value)


def is_dial_list(value: Any) -> bool:
    """RP-4 §4: `dials` is a list of integers, each above 0."""
    return isinstance(value, list) and all(is_int(v) and v > 0 for v in value)


TONE_MAX_S = 10
"""The longest tuner tone a host may ask for, in seconds (RP-4 §4.3)."""

WITHIN_MAX_S = 600
"""The longest a beacon may wait for a clear channel, in seconds (RP-4 §4.4)."""

PRECISIONS = ("grid4", "grid6", "exact")
"""How precisely a beacon says where its station is (RP-4 §4.4)."""


def is_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def is_position(value: Any) -> bool:
    """RP-4 §4.4: `lat` and `lon` in degrees, in range, and one of the precisions."""
    return (
        isinstance(value, dict)
        and is_number(value.get("lat"))
        and is_number(value.get("lon"))
        and -90 <= value["lat"] <= 90
        and -180 <= value["lon"] <= 180
        and value.get("precision") in PRECISIONS
    )


def locator(lat: float, lon: float, chars: int) -> str:
    """The Maidenhead locator of 4 or 6 characters that holds the position."""
    x, y = lon + 180, lat + 90
    field_x, field_y = min(int(x // 20), 17), min(int(y // 10), 17)
    square_x, square_y = min(int(x % 20 // 2), 9), min(int(y % 10), 9)
    out = f"{chr(65 + field_x)}{chr(65 + field_y)}{square_x}{square_y}"
    if chars == 6:
        sub_x = min(int(x % 2 * 12), 23)
        sub_y = min(int(y % 1 * 24), 23)
        out += f"{chr(97 + sub_x)}{chr(97 + sub_y)}"
    return out


def square_centre(grid: str) -> tuple[float, float]:
    """The centre of a 4- or 6-character Maidenhead square, as (lat, lon). Worked in whole
    twenty-fourths and forty-eighths of a degree, and divided once, so that every program that
    reads the same square gets the same two numbers to the last bit."""
    field_x, field_y = ord(grid[0]) - 65, ord(grid[1]) - 65
    square_x, square_y = int(grid[2]), int(grid[3])
    if len(grid) == 4:
        return float(-90 + (field_y * 10 + square_y)) + 0.5, float(
            -180 + (field_x * 20 + square_x * 2) + 1
        )
    sub_x, sub_y = ord(grid[4]) - 97, ord(grid[5]) - 97
    lon_24ths = (field_x * 20 + square_x * 2) * 24 + sub_x * 2 + 1
    lat_48ths = (field_y * 10 + square_y) * 48 + sub_y * 2 + 1
    return -90 + lat_48ths / 48, -180 + lon_24ths / 24


def position_heard(sent: dict[str, Any]) -> dict[str, Any]:
    """What a station hears of a position a beacon carried (RP-4 §4.4): the latitude and
    longitude themselves for `exact`, the square's centre and its locator for a grid."""
    if sent["precision"] == "exact":
        return {"lat": sent["lat"], "lon": sent["lon"], "precision": "exact"}
    grid = locator(sent["lat"], sent["lon"], 4 if sent["precision"] == "grid4" else 6)
    lat, lon = square_centre(grid)
    return {"lat": lat, "lon": lon, "precision": sent["precision"], "grid": grid}


SCAN_DWELL_S = 10
"""How long the simulated modem stays on each dial of a scan (RP-4 §4.2): the modem's own choice,
from its own calls, here a round figure. `scan_s` is the dials less one, times it."""

# RP-4 §4: each request's members, those without a `?` in its table needed, with their types.
MEMBERS: dict[str, list[tuple[str, bool, Any]]] = {
    "hello": [("version", True, is_version), ("app", True, lambda v: isinstance(v, str))],
    "callsigns": [("callsigns", True, is_text_list)],
    "listen": [("on", True, lambda v: isinstance(v, bool))],
    "connect": [
        ("id", True, is_id),
        ("remote", True, lambda v: isinstance(v, str)),
        ("as", False, lambda v: isinstance(v, str)),
        ("bandwidth", False, is_int),
        ("call_s", False, lambda v: is_int(v) and v >= 0),
    ],
    "send": [
        ("link", True, lambda v: isinstance(v, str)),
        ("data", True, lambda v: isinstance(v, str)),
        ("ref", False, is_id),
    ],
    "disconnect": [("link", True, lambda v: isinstance(v, str))],
    "abort": [("link", False, lambda v: isinstance(v, str))],
    "broadcast": [
        ("data", True, lambda v: isinstance(v, str)),
        ("ref", False, is_id),
        ("dial_hz", False, lambda v: is_int(v) and v > 0),
    ],
    "status": [],
    "dial": [],
    "tune": [
        ("dial_hz", True, lambda v: is_int(v) and v > 0),
    ],
    "scan": [("id", True, is_id), ("dials", True, is_dial_list)],
    "tone": [
        (
            "seconds",
            True,
            lambda v: is_int(v) and 0 <= v <= TONE_MAX_S,
        ),
    ],
    "stations": [],
    "beacon": [
        ("as", False, lambda v: isinstance(v, str)),
        ("ref", False, is_id),
        ("within_s", False, lambda v: is_int(v) and 1 <= v <= WITHIN_MAX_S),
        ("dial_hz", False, lambda v: is_int(v) and v > 0),
        ("position", False, is_position),
        ("modem_position", False, lambda v: isinstance(v, bool)),
    ],
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
    """One channel. `log` holds every line exchanged on every station, in order. `now` is the
    one time the simulated modem has, in Unix seconds: it says when a station was heard."""

    def __init__(self, now: int = 0) -> None:
        self.adapters: list[Adapter] = []
        self.calls: list[Call] = []
        self.log: list[dict[str, Any]] = []
        self.now = now

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
                call.host,
                # nothing of the station called was heard: RP-4 1.4.0's `heard`
                answer("failed", call.rid, code="timeout", text="no answer", heard=False),
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
        dial_hz: int | None = None,
    ) -> None:
        self.world = world
        self.station = station
        self.modem = modem
        self.caps = caps
        self.dial_hz = dial_hz  # the radio's dial, with `caps.dial` (RP-4 §4.1)
        self.prefix = prefix  # link IDs are this adapter's own: prefix and a number
        self.frame_bytes = frame_bytes  # how much a `received` carries at most
        self.hosts: dict[str, Host] = {}
        # the scan under way (RP-4 §4.2): the host it is for, and its dials, the first the home
        self.scanning: tuple[str, list[int]] | None = None
        self.ends: list[End] = []
        self.links = 0
        # the stations this modem has heard, with `caps.heard` (RP-4 §4.4): each callsign's last,
        # the most recently heard first
        self.heard: list[dict[str, Any]] = []
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
        if self.scanning is not None and self.scanning[0] == host:
            # its scan ends with it, and the radio goes home (RP-4 §4.2)
            home = self.scanning[1][0]
            self.scanning = None
            self._go_to(home)

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
            "dial": self._dial,
            "tune": self._dial,
            "scan": self._scan,
            "tone": self._tone,
            "stations": self._stations,
            "beacon": self._beacon,
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
        # RP-4 §6: a broadcast that names one of a running scan's dials goes out there (an
        # answer to one heard there); any other dial, or no scan, and it goes where the radio is
        scanning = self.scanning
        if self.caps.get("dial") and "dial_hz" in m and scanning and m["dial_hz"] in scanning[1]:
            self._go_to(m["dial_hz"])
        for adapter in self.world.adapters:
            if adapter is self:
                continue
            for other in adapter.hosts.values():
                if other.hello:
                    heard = answer("broadcast", None, source=h.callsigns[0], data=m["data"])
                    if adapter.caps.get("dial"):
                        heard["dial_hz"] = adapter.dial_hz  # where it was heard (§6)
                    adapter.emit(other.name, heard)
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

    def _dial(self, h: Host, m: Message, rid: Any) -> None:
        """RP-4 §1.1, §4.1: `dial` reads the radio's dial; `tune` sets it. The dial is the
        station's, so it moves only while nothing is up on it, and every host hears where it
        went."""
        if not self.caps.get("dial"):
            self.emit(h.name, error(rid, "unsupported", "this modem has no dial"))
            return
        if m["op"] == "dial":
            self.emit(h.name, answer("dial", rid, dial_hz=self.dial_hz))
            return
        if self.ends or any(c.adapter is self for c in self.world.calls):
            self.emit(h.name, error(rid, "busy", "a link or a call is up on the station"))
            return
        self.dial_hz = m["dial_hz"]
        self.ok(h, rid)
        if self.scanning is not None:
            # the operator wins: a tune ends the scan, and the radio stays where it was put
            owner = self.scanning[0]
            self.scanning = None
            why = "asked" if owner == h.name else "tuned"
            self.emit(owner, answer("scan", None, dials=[], why=why))
        self._tell_dial()

    def _tone(self, h: Host, m: Message, rid: Any) -> None:
        """RP-4 §1.1, §4.3: a steady tone for an antenna tuner. It keys the station's transmitter,
        so it starts only while nothing is up on the station; a stop is always taken. The
        simulated modem has no radio to key: it says `ok`."""
        if not self.caps.get("tone"):
            self.emit(h.name, error(rid, "unsupported", "this modem keys no tuner tone"))
            return
        if m["seconds"] > 0 and (self.ends or any(c.adapter is self for c in self.world.calls)):
            self.emit(h.name, error(rid, "busy", "a link or a call is up on the station"))
            return
        self.ok(h, rid)

    def _tell_dial(self) -> None:
        """RP-4 §4.1: every host that has said hello hears where the dial went."""
        for other in self.hosts.values():
            if other.hello:
                self.emit(other.name, answer("dial", None, dial_hz=self.dial_hz))

    def _go_to(self, dial_hz: int) -> None:
        """The radio to `dial_hz`, when nothing holds it where it is; every host told."""
        if self.dial_hz == dial_hz or self.ends:
            return
        if any(c.adapter is self for c in self.world.calls):
            return
        self.dial_hz = dial_hz
        self._tell_dial()

    def _scan(self, h: Host, m: Message, rid: Any) -> None:
        """RP-4 §4.2: listen on several dials in turn. One host scans at a time; a list with no
        dials stops the host's own scan and sends the radio home; a new list replaces it. The
        simulated modem has no clock: it says how long a call must keep trying, and puts the
        radio on the first dial when it is on none of them."""
        if not self.caps.get("scan"):
            self.emit(h.name, error(rid, "unsupported", "this modem does not scan"))
            return
        if self.scanning is not None and self.scanning[0] != h.name:
            self.emit(h.name, error(rid, "busy", "another program is scanning this station"))
            return
        dials = list(m["dials"])
        if not dials:
            home = self.scanning[1][0] if self.scanning is not None else None
            self.scanning = None
            self.emit(h.name, answer("scan", rid, dials=[]))
            if home is not None:
                self._go_to(home)
            return
        if len(set(dials)) != len(dials):
            self.emit(h.name, error(rid, "refused", "a dial is given twice"))
            return
        self.scanning = (h.name, dials)
        scan_s = (len(dials) - 1) * SCAN_DWELL_S
        self.emit(h.name, answer("scan", rid, dials=dials, scan_s=scan_s))
        if self.dial_hz not in dials:
            self._go_to(dials[0])

    def _stations(self, h: Host, m: Message, rid: Any) -> None:
        """RP-4 §4.4: the stations this modem has heard, the most recently heard first."""
        if not self.caps.get("heard"):
            self.emit(h.name, error(rid, "unsupported", "this modem keeps no stations heard"))
            return
        self.emit(h.name, answer("stations", rid, stations=[dict(e) for e in self.heard]))

    def _beacon(self, h: Host, m: Message, rid: Any) -> None:
        """RP-4 §4.4: one frame with a callsign, addressed to nobody. The simulated channel is
        always clear, so a beacon taken goes at once; every other station whose modem reports
        the stations it hears hears it. It goes only while nothing is up on the station."""
        if not self.caps.get("beacon"):
            self.emit(h.name, error(rid, "unsupported", "this modem sends no beacons"))
            return
        if not h.callsigns:
            self.emit(h.name, error(rid, "refused", "no callsigns yet"))
            return
        callsign = m.get("as", h.callsigns[0])
        if callsign not in h.callsigns:
            self.emit(h.name, error(rid, "not-authorized", f"{callsign} is not this host's"))
            return
        if self.ends or any(c.adapter is self for c in self.world.calls):
            self.emit(h.name, error(rid, "busy", "a link or a call is up on the station"))
            return
        wanted = m.get("dial_hz") if self.caps.get("dial") else None
        # the radio's dial, or a dial of a running scan, which goes there to send it (§4.4, §6)
        if (
            wanted is not None
            and wanted != self.dial_hz
            and (self.scanning is None or wanted not in self.scanning[1])
        ):
            self.emit(h.name, error(rid, "refused", f"the radio is not on {wanted} Hz"))
            return
        self.ok(h, rid)
        if wanted is not None:
            self._go_to(wanted)
        position = m.get("position") if self.caps.get("position") else None
        for adapter in self.world.adapters:
            if adapter is self or not adapter.caps.get("heard"):
                continue
            entry: dict[str, Any] = {
                "callsign": callsign,
                "kind": "beacon",
                # whether calls to its callsign are answered now: the host listens, and holds it
                "answers": h.listening,
                "at": self.world.now,
            }
            if adapter.caps.get("dial"):
                entry["dial_hz"] = adapter.dial_hz
            if position is not None and adapter.caps.get("position"):
                entry["position"] = position_heard(position)
            adapter.heard = [e for e in adapter.heard if e["callsign"] != callsign]
            adapter.heard.insert(0, entry)
            for other in adapter.hosts.values():
                if other.hello:
                    adapter.emit(other.name, answer("heard", None, **entry))
        sent = answer("beacon-sent", None, sent=True)
        if "ref" in m:
            sent["ref"] = m["ref"]
        if self.caps.get("dial"):
            sent["dial_hz"] = self.dial_hz
        self.emit(h.name, sent)
