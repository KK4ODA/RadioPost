# SPDX-License-Identifier: MIT OR Apache-2.0
"""A post office's API (RP-5) as a reference: one domain, its mailboxes and sessions, in memory.

`PostOffice.handle` takes a request (method, path, JSON body, bearer token) at the post office's
clock and returns the status and JSON body RP-5 says. It does what RP-5, with RP-1 and RP-2,
requires, as plainly as possible, and nothing about storage, mail or real time.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

from . import cbor, jws, rp

# RP-5 §1: the HTTP status of each RP-0 §9.3 code. `not-authorized` is 401 when the request has
# no valid token, and 403 when the token may not do what is asked.
HTTP_STATUS = {
    "unsupported-version": 400,
    "malformed": 400,
    "credential-invalid": 422,
    "credential-expired": 422,
    "credential-revoked": 422,
    "credential-required": 422,
    "unknown-post-office": 422,
    "signature-invalid": 422,
    "time-skew": 422,
    "not-authorized": 403,
    "too-large": 413,
    "quota-exceeded": 507,
    "refused-by-policy": 403,
    "already-have": 409,
    "unknown-message": 404,
    "post-office-unreachable": 502,
    "busy": 429,
    "timeout": 504,
    "internal": 500,
}

SESSION_S = 3600  # how long a session lasts (RP-5 §5.2)
TOKEN_S = 3600  # how long a token lasts, at most (RP-5 §3.2)
NONCE_S = 300  # how long a login nonce is good for (RP-5 §3.1)
CREDENTIAL_S = 90 * 86400  # RP-1 §4.2

Response = tuple[int, dict[str, Any] | None]


def problem(name: str, status: int | None = None) -> Response:
    """RFC 9457 problem details with RP-5 §1's `code` and `name`."""
    code = status or HTTP_STATUS[name]
    title = name.replace("-", " ").capitalize()
    return code, {
        "type": "about:blank",
        "title": title,
        "status": code,
        "code": rp.ERRORS[name],
        "name": name,
    }


def refused(cid: str, name: str, text: str) -> dict[str, Any]:
    return {"id": cid, "status": "refused", "code": rp.ERRORS[name], "name": name, "text": text}


@dataclass
class Waiting:
    """A message in a mailbox, with the air form the post office made for it once (RP-5 §5.3)."""

    serial: int
    air: bytes
    sender: str
    subject: str
    priority: int
    picked: bool = False

    @property
    def cid(self) -> bytes:
        return rp.content_id(rp.air_decode(self.air))

    def entry(self) -> dict[str, Any]:
        return {
            "serial": self.serial,
            "id": rp.id_text(self.cid),
            "from": self.sender,
            "subject": self.subject,
            "size": len(self.air),
            "priority": self.priority,
            "held": False,
        }


@dataclass
class Session:
    sid: str
    gateway: str
    address: str
    statement: dict[int, Any]
    waiting: list[int]
    expires: int


@dataclass
class PostOffice:
    domains: list[str]
    base_url: str
    secret: bytes  # the key that signs credentials
    public: bytes
    known: dict[str, list[dict[str, Any]]]  # every post office's keys, for RP-1 §5
    draw: Callable[[str, int], bytes]  # published randomness, so the answers repeat
    revoked: dict[str, dict[str, list[Any]]] = field(default_factory=dict)
    now: int = 0
    mailboxes: dict[str, list[Waiting]] = field(default_factory=dict)
    had: set[bytes] = field(default_factory=set)
    settings: dict[str, dict[str, Any]] = field(default_factory=dict)
    gateways: dict[str, dict[str, Any]] = field(default_factory=dict)
    copies: dict[str, list[dict[str, Any]]] = field(default_factory=dict)
    codes: dict[str, dict[str, Any]] = field(default_factory=dict)
    challenges: dict[bytes, int] = field(default_factory=dict)
    tokens: dict[str, dict[str, Any]] = field(default_factory=dict)
    sessions: dict[str, Session] = field(default_factory=dict)
    replay: dict[tuple[str, bytes], int] = field(default_factory=dict)
    issued: int = 0  # how many random values drawn, so each draw is new
    next_serial: int = 1  # the serial of the next credential this post office issues

    # -- what happens outside the API --------------------------------------------------------------

    def fresh(self, what: str, n: int) -> bytes:
        self.issued += 1
        return self.draw(f"post office {what} {self.issued}", n)

    def deliver(
        self, address: str, serial: int, air: bytes, *, sender: str, subject: str, priority: int
    ) -> None:
        """Mail arrives for a user (RP-6); copies go to the user's usual gateways (§5.8)."""
        waiting = Waiting(serial, air, sender, subject, priority)
        self.mailboxes.setdefault(address, []).append(waiting)
        for callsign in self.settings.get(address, {}).get("usual_gateways", []):
            record = self.gateways.get(callsign)
            if record is None or not record.get("copies"):
                continue
            item = {"kind": "copy", "address": address, **waiting.entry()}
            del item["held"]
            if len(air) <= record["copies"]["max_air_bytes"]:
                item["air"] = jws.b64u(air)
            item["expires"] = self.now + record["copies"]["days"] * 86400
            self.copies.setdefault(callsign, []).append(item)

    def issue(self, address: str, operator: str, device: bytes, flags: int) -> bytes:
        serial = self.next_serial
        self.next_serial += 1
        body = {
            1: 1,
            2: serial,
            3: address,
            4: operator,
            5: device,
            6: self.now,
            7: self.now + CREDENTIAL_S,
            8: rp.kid(self.public),
            9: flags,
        }
        return rp.make_credential(body, self.secret)[2]

    # -- requests ----------------------------------------------------------------------------------

    def handle(
        self, method: str, path: str, body: dict[str, Any] | None, token: str | None
    ) -> Response:
        route, _, query = path.partition("?")
        parts = route.strip("/").split("/")
        if (method, route) == ("POST", "/auth/challenge"):
            return self.challenge()
        if (method, route) == ("POST", "/auth/token"):
            return self.login(body or {})
        if (method, route) == ("POST", "/enroll"):
            return self.enroll(body or {})
        user = self.authorise(token)
        if isinstance(user, tuple):
            return user
        is_gateway = bool(user[9] & rp.FLAGS["gateway"])
        gateway_routes = {"sessions", "reports", "copies"}
        if (parts[0] in gateway_routes) != is_gateway:
            return problem("not-authorized")
        if (method, route) == ("POST", "/sessions"):
            return self.open_session(user, body or {})
        if parts[0] == "sessions" and len(parts) >= 2:
            return self.in_session(user, method, parts[1], parts[2:], body or {})
        if (method, route) == ("POST", "/reports"):
            return self.reports(user, body or {})
        if (method, route) == ("GET", "/copies"):
            return self.poll(user, query)
        if (method, route) == ("POST", "/copies/ack"):
            return 204, None
        if route == "/settings":
            if method == "PUT":
                self.settings[user[3]] = dict(body or {})
                return 204, None
            return 200, self.settings.get(user[3], {})
        if (method, route) == ("POST", "/gateways"):
            return self.register(user, body or {})
        return problem("unknown-message", 404)

    def authorise(self, token: str | None) -> dict[int, Any] | Response:
        """RP-5 §2: the credential body behind a valid bearer token, or 401."""
        held = self.tokens.get(token or "")
        if held is None or held["expires"] <= self.now:
            return problem("not-authorized", 401)
        return held["body"]

    def challenge(self) -> Response:
        nonce = self.fresh("nonce", 32)
        self.challenges[nonce] = self.now + NONCE_S
        return 200, {"nonce": jws.b64u(nonce), "expires": self.now + NONCE_S}

    def login(self, body: dict[str, Any]) -> Response:
        nonce = jws.unb64u(body.get("nonce", ""))
        if nonce is None or self.challenges.pop(nonce, 0) <= self.now:
            return problem("not-authorized", 401)
        credential = jws.unb64u(body.get("credential", ""))
        signature = jws.unb64u(body.get("signature", ""))
        if credential is None or signature is None:
            return problem("malformed")
        result, cbody = rp.verify_credential(credential, self.known, self.revoked, self.now, [])
        if result != "ok" or cbody is None:
            return problem(result)
        signed = nonce + self.base_url.encode("utf-8")
        if not rp.verify(rp.CTX_LOGIN, signed, signature, cbody[5]):
            return problem("signature-invalid")
        token = "tok_" + jws.b64u(self.fresh("token", 18))
        expires = min(self.now + TOKEN_S, cbody[7])
        self.tokens[token] = {"body": cbody, "expires": expires}
        return 200, {
            "token": token,
            "expires": expires,
            "address": cbody[3],
            "operator": cbody[4],
            "flags": cbody[9],
        }

    def check_session(
        self, body: dict[str, Any], gateway: str, clock: int
    ) -> tuple[dict[int, Any], Response | None]:
        """RP-5 §5.2, §5.6: the credential (at `clock`) and the statement (RP-1 §5, §6.3).

        Returns the statement and None, or an empty map and the problem to answer with.
        """
        raw = [jws.unb64u(body.get(k, "")) for k in ("statement", "signature", "credential")]
        statement_raw, signature, credential = raw
        if statement_raw is None or signature is None or credential is None:
            return {}, problem("malformed")
        try:
            stmt = cbor.decode(statement_raw)
        except cbor.NotCanonical:
            return {}, problem("malformed")
        started = body["started"]
        need = ["receive"] + (["send"] if stmt.get(6) else [])
        result, cbody = rp.verify_credential(credential, self.known, self.revoked, clock, need)
        if result != "ok" or cbody is None:
            return {}, problem(result)
        seen = [k for k, t in self.replay.items() if started - t < rp.REPLAY_S]
        result = rp.verify_statement(stmt, signature, cbody, me=gateway, now=started, seen=seen)
        if result != "ok":
            return {}, problem(result)
        return stmt, None

    def open_session(self, gateway: dict[int, Any], body: dict[str, Any]) -> Response:
        stmt, failed = self.check_session(body, gateway[4], self.now)
        if failed is not None:
            return failed
        if abs(body["started"] - self.now) > rp.TIME_WINDOW_S:
            return problem("time-skew")
        if 5 in stmt:
            self.replay[stmt[1], stmt[5]] = body["started"]
        mailbox = self.mailboxes.get(stmt[1], [])
        waiting = sorted(
            (w for w in mailbox if not w.picked), key=lambda w: (-w.priority, w.serial)
        )
        accept, decline = [], []
        for cid in stmt.get(6, []):
            if cid in self.had:
                decline.append({"id": rp.id_text(cid), "code": 14, "name": "already-have"})
            else:
                accept.append({"id": rp.id_text(cid), "offset": 0})
        sid = "s_" + jws.b64u(self.fresh("session", 12))
        session = Session(
            sid, gateway[4], stmt[1], stmt, [w.serial for w in waiting], self.now + SESSION_S
        )
        self.sessions[sid] = session
        return 201, {
            "session": sid,
            "expires": session.expires,
            "waiting": [w.entry() for w in waiting],
            "more": 0,
            "accept": accept,
            "decline": decline,
        }

    def in_session(
        self,
        gateway: dict[int, Any],
        method: str,
        sid: str,
        rest: list[str],
        body: dict[str, Any],
    ) -> Response:
        session = self.sessions.get(sid)
        if session is None or session.gateway != gateway[4] or session.expires <= self.now:
            return problem("unknown-message", 404)
        mailbox = {w.serial: w for w in self.mailboxes.get(session.address, [])}
        if method == "DELETE" and not rest:
            del self.sessions[sid]
            return 204, None
        if method == "GET" and len(rest) == 2 and rest[0] == "messages":
            serial = int(rest[1])
            if serial not in session.waiting:
                return problem("unknown-message")
            w = mailbox[serial]
            return 200, {"serial": serial, "id": rp.id_text(w.cid), "air": jws.b64u(w.air)}
        if method == "POST" and rest == ["messages"]:
            return 200, {"results": self.take(session.statement, body.get("messages", []))}
        if method == "POST" and rest == ["pickups"]:
            serials = body.get("serials", [])
            if any(s not in session.waiting for s in serials):
                return problem("unknown-message")
            self.picked_up(session.address, serials)
            return 204, None
        return problem("unknown-message", 404)

    def take(self, stmt: dict[int, Any], messages: list[dict[str, Any]]) -> list[dict[str, Any]]:
        """RP-5 §5.4, in its order: in the statement, decodes to its ID, from the address."""
        results = []
        for m in messages:
            text = m["id"]
            air = jws.unb64u(m.get("air", ""))
            cid = rp.id_bytes(text)
            if cid is None or cid not in stmt.get(6, []):
                results.append(refused(text, "not-authorized", "not in the session's statement"))
                continue
            try:
                canonical = rp.air_decode(air or b"")
            except ValueError:
                canonical = b""
            if rp.content_id(canonical) != cid:
                results.append(refused(text, "malformed", "the air form is not this message"))
                continue
            if cbor.decode(canonical).get(2) != stmt[1]:
                results.append(refused(text, "not-authorized", "not from the statement's address"))
                continue
            status = "duplicate" if cid in self.had else "accepted"
            self.had.add(cid)
            results.append({"id": text, "status": status})
        return results

    def picked_up(self, address: str, serials: list[int]) -> None:
        """RP-5 §5.5: mark them, and have every copy placed ahead dropped (§5.8)."""
        for w in self.mailboxes.get(address, []):
            if w.serial in serials and not w.picked:
                w.picked = True
                for items in self.copies.values():
                    if any(i["address"] == address and i.get("serial") == w.serial for i in items):
                        items.append({"kind": "drop", "address": address, "serial": w.serial})

    def reports(self, gateway: dict[int, Any], body: dict[str, Any]) -> Response:
        """RP-5 §5.6: each session checked at its own `started`, one answer each, in order."""
        out: list[dict[str, Any]] = []
        for s in body.get("sessions", []):
            stmt, failed = self.check_session(s, gateway[4], s["started"])
            if failed is not None:
                prob = failed[1]
                assert prob is not None
                out.append({"code": prob["code"], "name": prob["name"]})
                continue
            results = self.take(stmt, s.get("messages", []))
            self.picked_up(stmt[1], s.get("pickups", []))
            out.append({"results": results})
        return 200, {"sessions": out}

    def poll(self, gateway: dict[int, Any], query: str) -> Response:
        """RP-5 §5.8: the items for this gateway after the cursor. Cursors here are `c_<count>`."""
        args = dict(p.partition("=")[::2] for p in query.split("&") if p)
        after = args.get("after", "")
        start = int(after[2:]) if after.startswith("c_") else 0
        items = self.copies.get(gateway[4], [])
        return 200, {"cursor": f"c_{len(items)}", "items": items[start:]}

    def enroll(self, body: dict[str, Any]) -> Response:
        """RP-5 §6.1: a one-time code, and a signature that proves the new device holds its key."""
        code = self.codes.pop(body.get("code", ""), None)
        if code is None or (code["address"], code["operator"]) != (
            body.get("address"),
            body.get("operator"),
        ):
            return problem("not-authorized")
        device = jws.unb64u(body.get("device", ""))
        signature = jws.unb64u(body.get("signature", ""))
        if device is None or signature is None or len(device) != 32:
            return problem("malformed")
        signed = body["code"].encode("utf-8") + self.base_url.encode("utf-8")
        if not rp.verify(rp.CTX_ENROLL, signed, signature, device):
            return problem("signature-invalid")
        credential = self.issue(code["address"], code["operator"], device, code["flags"])
        return 200, {"credential": jws.b64u(credential)}

    def register(self, user: dict[int, Any], body: dict[str, Any]) -> Response:
        """RP-5 §6.4: a user registers a gateway they operate, and gets its credential."""
        record = dict(body.get("record", {}))
        if record.get("operator") != user[4]:
            return problem("not-authorized")
        device = jws.unb64u(body.get("device", ""))
        signature = jws.unb64u(body.get("signature", ""))
        if device is None or signature is None or len(device) != 32:
            return problem("malformed")
        signed = record["callsign"].encode("utf-8") + self.base_url.encode("utf-8")
        if not rp.verify(rp.CTX_GATEWAY, signed, signature, device):
            return problem("signature-invalid")
        record["updated"] = self.now
        self.gateways[record["callsign"]] = record
        flags = rp.FLAGS["send"] | rp.FLAGS["gateway"]
        credential = self.issue(user[3], record["callsign"], device, flags)
        return 201, {"credential": jws.b64u(credential)}
