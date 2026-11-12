# SPDX-License-Identifier: MIT OR Apache-2.0
"""A post office's API (RP-5) as a reference: one domain, its mailboxes and sessions, in memory.

`PostOffice.handle` takes a request (method, path, JSON body, bearer token) at the post office's
clock and returns the status and JSON body RP-5 says. It does what RP-5, with RP-1 and RP-2,
requires, as plainly as possible, and nothing about storage, mail or real time.
"""

from __future__ import annotations

import re
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

from . import cbor, fwd, jws, mail, rp

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
CUT_OFF_S = 900  # RP-7 §4.1: no copies poll for this long, and a gateway is cut off
ENTRY = re.compile(r"(?:[^@\s]+)?@[a-z0-9-]+(?:\.[a-z0-9-]+)+")  # RP-5 §6.5, in lowercase
CODE_S = 7 * 86400  # an enrollment code's life (RP-5 §8: within days)
POSTCARD_S = 30 * 86400  # a code sent by post (RP-5 §7.3: at least 30 days)
APPLICATION_S = 30 * 86400  # a pending application is kept (RP-5 §6.7: at least 14 days)
DECIDED_S = 7 * 86400  # a decided one is answered (RP-5 §6.7: at least 7 days)
CODE_ALPHABET = "23456789ABCDEFGHJKLMNPQRSTUVWXYZ"  # read out by people: no 0, O, 1 or I
ACCOUNT = re.compile(r"[a-z0-9._-]{1,64}@[a-z0-9-]+(?:\.[a-z0-9-]+)+")  # RP-5 §6.7
CONTACT = re.compile(r"[^@\s]{1,64}@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+")
BASE_CALL = re.compile(r"[A-Z0-9]{1,10}")  # RP-0 §6.1's base
CALLSIGN = re.compile(
    r"(?:[A-Z0-9]{1,10}/)?[A-Z0-9]{1,10}(?:/[A-Z0-9]{1,10})?(?:-(?:1[0-5]|[0-9]))?"
)
GRANTABLE = {"send", "receive", "role"}  # RP-5 §7.3: an operator never grants `gateway`

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


def operates(callsign: str) -> Response:
    """RP-5 §7.1: refused because it would make someone an operator; `operates` names the call."""
    status, body = problem("refused-by-policy")
    assert body is not None
    return status, {**body, "operates": callsign}


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
    third_party: bool = False  # RP-6 §5.1: the sender is not shown to be licensed

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
            "held": bool(cbor.decode(rp.air_decode(self.air)).get(11, 0) & 2),
            "third_party": self.third_party,
        }


@dataclass
class Session:
    sid: str
    gateway: str
    address: str
    statement: dict[int, Any]
    waiting: list[int]
    expires: int
    gateway_session: str  # RP-3's session ID, for the Radio-Path header (RP-6 §3.2)


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
    accept: dict[str, list[str]] = field(default_factory=dict)  # accept lists (RP-5 §6.5)
    routes: list[dict[str, Any]] = field(default_factory=list)  # radio routes (RP-7 §6)
    last_poll: dict[str, int] = field(default_factory=dict)  # gateway -> its last copies poll
    forwarded: dict[str, list[tuple[str, str, int]]] = field(default_factory=dict)  # RP-7 §4.1
    picked_locally: dict[str, set[bytes]] = field(default_factory=dict)  # RP-7 §4.4, not yet here
    gateways: dict[str, dict[str, Any]] = field(default_factory=dict)
    copies: dict[str, list[dict[str, Any]]] = field(default_factory=dict)
    codes: dict[str, dict[str, Any]] = field(default_factory=dict)
    challenges: dict[bytes, int] = field(default_factory=dict)
    tokens: dict[str, dict[str, Any]] = field(default_factory=dict)
    sessions: dict[str, Session] = field(default_factory=dict)
    replay: dict[tuple[str, bytes], int] = field(default_factory=dict)
    held: dict[bytes, bytes] = field(default_factory=dict)  # RP-6 §2.5: attachments by ref
    max_air_bytes: int = 65536  # RP-5 §4.1
    issued: int = 0  # how many random values drawn, so each draw is new
    next_serial: int = 1  # the serial of the next credential this post office issues
    operators: list[str] = field(default_factory=list)  # RP-5 §7.1; none, no operator endpoints
    applications_open: bool = False  # RP-5 §4.1 `applications`
    applications: dict[str, dict[str, Any]] = field(default_factory=dict)  # RP-5 §6.7, by ID
    accounts: set[str] = field(default_factory=set)
    names: dict[str, str] = field(default_factory=dict)  # display names (RP-6 §3.1)
    credentials: dict[int, dict[str, Any]] = field(default_factory=dict)  # issued, by serial
    serials_of: dict[bytes, list[int]] = field(default_factory=dict)  # by device key (RP-5 §6.2)

    # -- what happens outside the API --------------------------------------------------------------

    def fresh(self, what: str, n: int) -> bytes:
        self.issued += 1
        return self.draw(f"post office {what} {self.issued}", n)

    def deliver(
        self,
        address: str,
        serial: int,
        air: bytes,
        *,
        sender: str,
        subject: str,
        priority: int,
        third_party: bool,
    ) -> None:
        """Mail arrives for a user (RP-6); copies go to the user's usual gateways (§5.8)."""
        waiting = Waiting(serial, air, sender, subject, priority, third_party=third_party)
        self.mailboxes.setdefault(address, []).append(waiting)
        for callsign in self.settings.get(address, {}).get("usual_gateways", []):
            record = self.gateways.get(callsign)
            if record is None or not record.get("copies"):
                continue
            item = self._copy_item(address, waiting, record)
            self.copies.setdefault(callsign, []).append(item)
            if self._cut_off(callsign):
                self._place_forward(callsign, address, waiting, item)

    def _copy_item(self, address: str, waiting: Waiting, record: dict[str, Any]) -> dict[str, Any]:
        item = {"kind": "copy", "address": address, **waiting.entry()}
        del item["held"]
        if len(waiting.air) <= record["copies"]["max_air_bytes"]:
            item["air"] = jws.b64u(waiting.air)
        item["expires"] = self.now + record["copies"]["days"] * 86400
        return item

    def _cut_off(self, callsign: str) -> bool:
        """RP-7 §4.1: a gateway that has polled before, but not for 15 minutes."""
        last = self.last_poll.get(callsign)
        return last is not None and self.now - last >= CUT_OFF_S

    def _place_forward(
        self, cut: str, address: str, waiting: Waiting, copy: dict[str, Any]
    ) -> None:
        """RP-7 §4.1, §4.2: a forward item, with its signed placement, at each route's `via`."""
        for route in self.routes:
            if cut not in route["users_at"]:
                continue
            body = {
                1: 1,
                2: address,
                3: waiting.serial,
                4: waiting.cid,
                5: cut,
                6: copy["expires"],
            }
            placement = fwd.make_placement(body, self.secret)
            item = {**copy, "kind": "forward", "gateway": cut, "placement": jws.b64u(placement)}
            self.copies.setdefault(route["via"], []).append(item)
            self.forwarded.setdefault(cut, []).append((route["via"], address, waiting.serial))

    def _add_code(
        self, code: str, address: str, operator: str, flags: int, life: int = CODE_S
    ) -> None:
        """An enrollment code (RP-5 §6.1); the account exists from now, for mail to arrive."""
        self.accounts.add(address)
        self.codes[code] = {
            "address": address,
            "operator": operator,
            "flags": flags,
            "expires": self.now + life,
        }

    def _new_code(self) -> str:
        """Eight characters from a 32-letter alphabet, in two groups: `7Q4F-AZ9K`."""
        raw = self.fresh("code", 8)
        chars = "".join(CODE_ALPHABET[b % len(CODE_ALPHABET)] for b in raw)
        return f"{chars[:4]}-{chars[4:]}"

    def issue(self, address: str, operator: str, device: bytes, flags: int) -> bytes:
        serial = self.next_serial
        self.next_serial += 1
        self.accounts.add(address)
        self.serials_of.setdefault(device, []).append(serial)
        self.credentials[serial] = {
            "address": address,
            "operator": operator,
            "device": device,
            "flags": flags,
            "expires": self.now + CREDENTIAL_S,
        }
        body = {
            1: 1,
            2: serial,
            3: address,
            4: operator,
            5: device,
            6: self.now,
            7: self.now + CREDENTIAL_S,
            9: flags,
        }
        return rp.make_credential(body, self.secret)[3]

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
        if (method, route) == ("POST", "/applications"):
            return self._apply(body or {})
        if method == "GET" and len(parts) == 2 and parts[0] == "applications":
            return self._application(parts[1])
        if parts[0] == "office" and not self.operators:
            return problem("unknown-message", 404)  # RP-5 §7.1: not offered
        user = self.authorise(token)
        if isinstance(user, tuple):
            return user
        if parts[0] == "office":
            if not self._is_operator(user):
                return problem("not-authorized")
            return self._office(method, parts[1:], body or {}, (user[3], user[4]))
        is_gateway = bool(user[9] & rp.FLAGS["gateway"])
        gateway_routes = {"sessions", "reports", "copies"}
        anyone = {"/credentials"}  # gateways renew their credentials too (§6.2)
        if route not in anyone and (parts[0] in gateway_routes) != is_gateway:
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
        if (method, route) == ("POST", "/credentials"):
            return 200, {"credential": jws.b64u(self.renewed(user))}
        if (method, route) == ("POST", "/direct"):
            return self.direct(user, body or {})
        if route == "/accept-list":
            return self.accept_list(user[3], method, body or {})
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
        answer = {
            "token": token,
            "expires": expires,
            "address": cbody[3],
            "operator": cbody[4],
            "flags": cbody[9],
        }
        if self.operators and self._is_operator(cbody):
            answer["office"] = True  # RP-5 §3.2
        return 200, answer

    def _is_operator(self, cbody: dict[int, Any]) -> bool:
        """RP-5 §7.1: a credential this post office issued, to one of its operators, no gateway."""
        return (
            cbody[3].rpartition("@")[2] in self.domains
            and cbody[4] in self.operators
            and not cbody[9] & rp.FLAGS["gateway"]
        )

    def check_session(
        self, body: dict[str, Any], gateway: str, clock: int
    ) -> tuple[dict[int, Any], dict[int, Any], Response | None]:
        """RP-5 §5.2, §5.6: the credential (at `clock`) and the statement (RP-1 §5, §6.3).

        Returns the statement, the credential's body and None, or empty maps and the problem to
        answer with.
        """
        raw = [jws.unb64u(body.get(k, "")) for k in ("statement", "signature", "credential")]
        statement_raw, signature, credential = raw
        if statement_raw is None or signature is None or credential is None:
            return {}, {}, problem("malformed")
        try:
            stmt = cbor.decode(statement_raw)
        except cbor.NotCanonical:
            return {}, {}, problem("malformed")
        started = body["started"]
        need = ["receive"] + (["send"] if stmt.get(6) else [])
        result, cbody = rp.verify_credential(credential, self.known, self.revoked, clock, need)
        if result != "ok" or cbody is None:
            return {}, {}, problem(result)
        seen = [k for k, t in self.replay.items() if started - t < rp.REPLAY_S]
        # RP-5 §5.2: the gateway issued a challenge-form statement's challenge and checked it
        # (RP-1 §6.3 step 6); the post office takes it as issued, once.
        challenge = stmt.get(7)
        if challenge is not None and (stmt[1], challenge) in seen:
            challenge = None
        result = rp.verify_statement(
            stmt, signature, cbody, me=gateway, now=started, challenge=challenge, seen=seen
        )
        if result != "ok":
            return {}, {}, problem(result)
        return stmt, cbody, None

    def open_session(self, gateway: dict[int, Any], body: dict[str, Any]) -> Response:
        stmt, cbody, failed = self.check_session(body, gateway[4], self.now)
        if failed is not None:
            return failed
        if abs(body["started"] - self.now) > rp.TIME_WINDOW_S:
            return problem("time-skew")
        if 5 in stmt or 7 in stmt:
            self.replay[stmt[1], stmt.get(5, stmt.get(7))] = body["started"]
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
            sid,
            gateway[4],
            stmt[1],
            stmt,
            [w.serial for w in waiting],
            self.now + SESSION_S,
            body.get("gateway_session", ""),
        )
        self.sessions[sid] = session
        answer = {
            "session": sid,
            "expires": session.expires,
            "waiting": [w.entry() for w in waiting],
            "more": 0,
            "accept": accept,
            "decline": decline,
        }
        if body.get("renew"):
            answer["credential"] = jws.b64u(self.renewed(cbody))
        return 201, answer

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
        if method == "POST" and rest == ["files"]:
            return self._files(session, mailbox, body)
        return problem("unknown-message", 404)

    def _files(
        self, session: Session, mailbox: dict[int, Waiting], body: dict[str, Any]
    ) -> Response:
        """RP-5 §5.3, RP-6 §2.5: held attachments asked for, sent in new messages."""
        asked = body.get("files")
        if not isinstance(asked, list) or not asked:
            return problem("malformed")
        by_message: dict[int, list[dict[int, Any]]] = {}  # serial -> held entries, in order
        for entry in asked:
            serial = entry.get("serial") if isinstance(entry, dict) else None
            ref = jws.unb64u(entry.get("ref", "")) if isinstance(entry, dict) else None
            if not isinstance(serial, int) or isinstance(serial, bool) or not ref:
                return problem("malformed")
            if serial not in mailbox:  # picked up or not: the user reads the list once it came
                return problem("unknown-message")
            original = cbor.decode(rp.air_decode(mailbox[serial].air))
            listed = next((h for h in original.get(12, []) if h.get(4) == ref), None)
            if listed is None or ref not in self.held:
                return problem("unknown-message")
            chosen = by_message.setdefault(serial, [])
            if listed not in chosen:
                chosen.append(listed)
        made: list[tuple[Waiting, bytes]] = []
        for serial, entries in by_message.items():
            original_waiting = mailbox[serial]
            original = cbor.decode(rp.air_decode(original_waiting.air))
            group: list[dict[int, Any]] = []
            for listed in entries:
                if self._held_air(session.address, original, [*group, listed]) is not None:
                    group.append(listed)
                    continue
                if not group:
                    return problem("too-large", 413)
                made.append((original_waiting, self._held_air(session.address, original, group)))
                group = [listed]
                if self._held_air(session.address, original, group) is None:
                    return problem("too-large", 413)
            made.append((original_waiting, self._held_air(session.address, original, group)))
        for original_waiting, air in made:
            serial = max((w.serial for w in self.mailboxes.get(session.address, [])), default=0) + 1
            message = cbor.decode(rp.air_decode(air))
            self.deliver(
                session.address,
                serial,
                air,
                sender=message[2],
                subject=message[6],
                priority=message.get(9, 0),
                third_party=original_waiting.third_party,
            )
        return 202, None

    def _held_air(
        self, address: str, original: dict[int, Any], listed: list[dict[int, Any]]
    ) -> bytes | None:
        """RP-6 §2.5: the message bringing these held attachments, or None when its air form is
        over `max_air_bytes`."""
        files = [{"name": h[1], "type": h[2], "data": self.held[h[4]]} for h in listed]
        fields: dict[str, Any] = {
            "v": 1,
            "from": "postmaster@" + address.rpartition("@")[2],
            "to": [address],
            "date": self.now,
            "subject": "Attachments: " + original.get(6, ""),
            "body": "".join(f"{f['name']}, {len(f['data'])} bytes\n" for f in files),
            "files": files,
            "reply_to": rp.content_id(cbor.encode(original)),
        }
        if original.get(9):
            fields["priority"] = original[9]
        air = rp.air_form(mail.encode_fields(fields), 0)
        return air if len(air) <= self.max_air_bytes else None

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
            if status == "accepted":
                self.written_to(stmt[1], cbor.decode(canonical))
                self._deliver_here(canonical)
            results.append({"id": text, "status": status})
        return results

    def _deliver_here(self, canonical: bytes, has_it: str = "") -> None:
        """Mail between this post office's own accounts goes to their mailboxes at once."""
        message = cbor.decode(canonical)
        air = rp.air_form(canonical, 0)
        for address in message.get(3, []) + message.get(4, []):
            if address.rpartition("@")[2] not in self.domains or address == has_it:
                continue
            serial = max((w.serial for w in self.mailboxes.get(address, [])), default=0) + 1
            fields = {"sender": message[2], "subject": message[6], "priority": message.get(9, 0)}
            self.deliver(address, serial, air, **fields, third_party=False)  # a user's: licensed
            self._picked_locally_here(address)

    def _picked_locally_here(self, address: str) -> None:
        """RP-5 §5.6 `local_pickups`: mark what the user already picked up at a gateway."""
        cids = self.picked_locally.get(address, set())
        serials = [w.serial for w in self.mailboxes.get(address, []) if w.cid in cids]
        self.picked_up(address, serials)

    def direct(self, user: dict[int, Any], body: dict[str, Any]) -> Response:
        """RP-5 §6.6: the user's station-to-station sessions (RP-3 §10), checked, then filed."""
        out: list[dict[str, Any]] = []
        for s in body.get("sessions", []):
            checked = self._check_direct(user, s)
            if isinstance(checked, str):
                out.append({"code": rp.ERRORS[checked], "name": checked})
                continue
            mine, theirs = checked
            results = [self._sent_direct(mine, theirs, m) for m in s.get("sent", [])]
            results += [self._received_direct(mine, theirs, m) for m in s.get("received", [])]
            out.append({"results": results})
        return 200, {"sessions": out}

    def _check_direct(
        self, user: dict[int, Any], s: dict[str, Any]
    ) -> str | tuple[dict[int, Any], dict[int, Any]]:
        """RP-5 §6.6's checks, in order: the error name, or the two statements."""
        started, role = s.get("started", 0), s.get("role")
        proofs = self._direct_proofs(s)
        if proofs is None or role not in {"caller", "called"}:
            return "malformed"
        need = (["send"] if s.get("sent") else []) + (["receive"] if s.get("received") else [])
        bodies = []
        for (credential, _, _), flags in zip(
            proofs, [need, ["send"] if s.get("received") else []], strict=True
        ):
            result, cbody = rp.verify_credential(
                credential, self.known, self.revoked, started, flags
            )
            if result != "ok" or cbody is None:
                return result
            bodies.append(cbody)
        if bodies[0][3] != user[3] or user[3].rpartition("@")[2] not in self.domains:
            return "not-authorized"
        caller, called = (0, 1) if role == "caller" else (1, 0)
        (_, cstmt, csig), (_, dstmt, dsig) = proofs[caller], proofs[called]
        result = rp.verify_statement(cstmt, csig, bodies[caller], me=bodies[called][4], now=started)
        if result == "ok":
            result = rp.verify_statement(
                dstmt,
                dsig,
                bodies[called],
                me=bodies[caller][4],
                now=started,
                sent_nonce=cstmt.get(5),
            )
        if result != "ok":
            return result
        return proofs[0][1], proofs[1][1]

    @staticmethod
    def _direct_proofs(s: dict[str, Any]) -> list[tuple[bytes, dict[int, Any], bytes]] | None:
        """Each side's credential, statement and signature, decoded; None if one does not."""
        proofs = []
        for side in ("mine", "theirs"):
            p = s.get(side, {})
            raw = [jws.unb64u(p.get(k, "")) for k in ("credential", "statement", "signature")]
            if raw[0] is None or raw[1] is None or raw[2] is None:
                return None
            try:
                proofs.append((raw[0], cbor.decode(raw[1]), raw[2]))
            except cbor.NotCanonical:
                return None
        return proofs

    @staticmethod
    def _direct_message(stmt: dict[int, Any], m: dict[str, Any]) -> tuple[str, bytes] | None:
        """The message's text ID and canonical encoding, if its statement lists it and it is it."""
        cid = rp.id_bytes(m.get("id", ""))
        try:
            canonical = rp.air_decode(jws.unb64u(m.get("air", "")) or b"")
        except ValueError:
            return None
        if cid is None or cid not in stmt.get(6, []) or rp.content_id(canonical) != cid:
            return None
        if cbor.decode(canonical).get(2) != stmt[1]:
            return None
        return m["id"], canonical

    def _sent_direct(
        self, mine: dict[int, Any], theirs: dict[int, Any], m: dict[str, Any]
    ) -> dict[str, Any]:
        """Sent by the user: taken as mail it sent, sent on to all but the station that has it."""
        found = self._direct_message(mine, m)
        if found is None:
            return refused(m.get("id", ""), "not-authorized", "not this user's, in its statement")
        text, canonical = found
        cid = rp.content_id(canonical)
        if cid in self.had:
            return {"id": text, "status": "duplicate"}
        self.had.add(cid)
        message = cbor.decode(canonical)
        self.written_to(mine[1], message)
        self._deliver_here(canonical, has_it=theirs[1])
        others = [a for a in message.get(3, []) + message.get(4, []) if a != theirs[1]]
        return {"id": text, "status": "accepted", "sent_on": others}

    def _received_direct(
        self, mine: dict[int, Any], theirs: dict[int, Any], m: dict[str, Any]
    ) -> dict[str, Any]:
        """Received by the user: filed in its mailbox, picked up already."""
        found = self._direct_message(theirs, m)
        if found is None:
            return refused(m.get("id", ""), "not-authorized", "not the other station's, signed")
        text, canonical = found
        message = cbor.decode(canonical)
        if mine[1] not in message.get(3, []) + message.get(4, []):
            return refused(text, "not-authorized", "not addressed to this user")
        box = self.mailboxes.setdefault(mine[1], [])
        if any(w.cid == rp.content_id(canonical) for w in box):
            return {"id": text, "status": "duplicate"}
        serial = max((w.serial for w in box), default=0) + 1
        air = rp.air_form(canonical, 0)
        waiting = Waiting(serial, air, message[2], message[6], message.get(9, 0), picked=True)
        box.append(waiting)
        return {"id": text, "status": "accepted", "serial": serial}

    def written_to(self, address: str, message: dict[int, Any]) -> None:
        """RP-5 §6.5: the recipients of mail a user sent join the user's accept list."""
        if not self.settings.get(address, {}).get("accept_on_send", True):
            return
        entries = self.accept.setdefault(address, [])
        for recipient in message.get(3, []) + message.get(4, []):
            if not mail.on_list(recipient, entries):
                entries.append(recipient.lower())

    def renewed(self, held: dict[int, Any]) -> bytes:
        """RP-5 §6.2: a new credential for the same account, operator, device key and flags."""
        return self.issue(held[3], held[4], held[5], held[9])

    def accept_list(self, address: str, method: str, body: dict[str, Any]) -> Response:
        """RP-5 §6.5: one list for the account, shared by every operator of a role mailbox."""
        entries = self.accept.setdefault(address, [])
        if method == "POST":
            given = [*body.get("add", []), *body.get("remove", [])]
            if not all(isinstance(e, str) and ENTRY.fullmatch(e.lower()) for e in given):
                return problem("malformed")
            add = [e.lower() for e in body.get("add", [])]
            remove = [e.lower() for e in body.get("remove", [])]
            entries.extend(e for i, e in enumerate(add) if e not in entries and e not in add[:i])
            entries[:] = [e for e in entries if e not in remove]
        return 200, {"entries": list(entries)}

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
            path = s.get("path", [])
            called = path[0] if path else gateway[4]
            if path and path[-1] != gateway[4]:
                prob = problem("not-authorized")[1]
                assert prob is not None
                out.append({"code": prob["code"], "name": prob["name"]})
                continue
            stmt, _, failed = self.check_session(s, called, s["started"])
            if failed is not None:
                prob = failed[1]
                assert prob is not None
                out.append({"code": prob["code"], "name": prob["name"]})
                continue
            results = self.take(stmt, s.get("messages", []))
            self.picked_up(stmt[1], s.get("pickups", []))
            local = {rp.id_bytes(t) for t in s.get("local_pickups", [])} - {None}
            self.picked_locally.setdefault(stmt[1], set()).update(local)  # type: ignore[arg-type]
            self._picked_locally_here(stmt[1])
            out.append({"results": results})
        return 200, {"sessions": out}

    def poll(self, gateway: dict[int, Any], query: str) -> Response:
        """RP-5 §5.8: the items for this gateway after the cursor. Cursors here are `c_<count>`.

        A poll from a gateway that was cut off ends its forwarding (RP-7 §4.1).
        """
        callsign = gateway[4]
        for via, address, serial in self.forwarded.pop(callsign, []):
            drop = {"kind": "drop", "address": address, "serial": serial}
            self.copies.setdefault(via, []).append(drop)
        self.last_poll[callsign] = self.now
        args = dict(p.partition("=")[::2] for p in query.split("&") if p)
        after = args.get("after", "")
        start = int(after[2:]) if after.startswith("c_") else 0
        items = self.copies.get(gateway[4], [])
        return 200, {"cursor": f"c_{len(items)}", "items": items[start:]}

    def enroll(self, body: dict[str, Any]) -> Response:
        """RP-5 §6.1: a one-time code, and a signature that proves the new device holds its key."""
        code = self.codes.get(body.get("code", ""))
        if (
            code is None
            or code.get("expires", self.now + 1) <= self.now
            or (code["address"], code["operator"]) != (body.get("address"), body.get("operator"))
        ):
            return problem("not-authorized")
        device = jws.unb64u(body.get("device", ""))
        signature = jws.unb64u(body.get("signature", ""))
        if device is None or signature is None or len(device) != 32:
            return problem("malformed")
        signed = body["code"].encode("utf-8") + self.base_url.encode("utf-8")
        if not rp.verify(rp.CTX_ENROLL, signed, signature, device):
            return problem("signature-invalid")
        del self.codes[body["code"]]  # used: only by an enrollment that succeeds
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

    # -- applications (RP-5 §6.7) -----------------------------------------------------------------

    def _alive(self, app: dict[str, Any]) -> bool:
        """A pending application is kept 30 days; a decided one answered 7 days on."""
        until = (
            app["received"] + APPLICATION_S if "decided" not in app else app["decided"] + DECIDED_S
        )
        return self.now < until

    def _apply(self, body: dict[str, Any]) -> Response:
        """RP-5 §6.7: checked in its order; a pending application waits for the operators."""
        address, operator = body.get("address"), body.get("operator")
        name, contact = body.get("name"), body.get("contact")
        device = jws.unb64u(body.get("device", "")) if isinstance(body.get("device"), str) else None
        signature = (
            jws.unb64u(body.get("signature", ""))
            if isinstance(body.get("signature"), str)
            else None
        )
        if not (
            isinstance(address, str)
            and ACCOUNT.fullmatch(address)
            and isinstance(operator, str)
            and BASE_CALL.fullmatch(operator)
            and isinstance(name, str)
            and 1 <= len(name) <= 64
            and name.strip()
            and isinstance(contact, str)
            and len(contact) <= 254
            and CONTACT.fullmatch(contact)
            and device is not None
            and len(device) == 32
            and signature is not None
        ):
            return problem("malformed")
        if not self.applications_open or address.rpartition("@")[2] not in self.domains:
            return problem("refused-by-policy")
        if operator in self.operators:
            return operates(operator)  # RP-5 §7.1: nobody becomes an operator by applying
        pending = any(
            a["address"] == address and a["status"] == "pending" and self._alive(a)
            for a in self.applications.values()
        )
        if address in self.accounts or pending:
            return problem("already-have")
        signed = f"{address}\n{operator}\n{self.base_url}".encode()
        if not rp.verify(rp.CTX_APPLY, signed, signature, device):
            return problem("signature-invalid")
        aid = "ap_" + jws.b64u(self.fresh("application", 16))
        self.applications[aid] = {
            "application": aid,
            "status": "pending",
            "address": address,
            "operator": operator,
            "name": name,
            "contact": contact,
            "device": jws.b64u(device),
            "received": self.now,
        }
        return 202, {
            "application": aid,
            "status": "pending",
            "expires": self.now + APPLICATION_S,
        }

    def _application(self, aid: str) -> Response:
        """RP-5 §6.7: what the applicant may see of it."""
        app = self.applications.get(aid)
        if app is None or not self._alive(app):
            return problem("unknown-message", 404)
        answer: dict[str, Any] = {"application": aid, "status": app["status"]}
        if app["status"] == "pending":
            answer["expires"] = app["received"] + APPLICATION_S
        if app["status"] == "approved":
            answer["credential"] = app["credential"]
        if app["status"] == "declined":
            answer["text"] = app["text"]
        return 200, answer

    # -- the operator endpoints (RP-5 §7) ---------------------------------------------------------

    def _office(
        self, method: str, rest: list[str], body: dict[str, Any], asking: tuple[str, str]
    ) -> Response:
        """RP-5 §7; `asking` is the token's account and callsign."""
        if (method, rest) == ("GET", ["status"]):
            return 200, {
                "accounts": len(self.accounts),
                "applications": sum(
                    a["status"] == "pending" and self._alive(a) for a in self.applications.values()
                ),
                "codes": sum(
                    c.get("expires", self.now + 1) > self.now for c in self.codes.values()
                ),
                "sessions": sum(s.expires > self.now for s in self.sessions.values()),
            }
        if (method, rest) == ("GET", ["applications"]):
            alive = [a for a in self.applications.values() if self._alive(a)]
            return 200, {"applications": [self._entry(a) for a in alive]}
        if method == "POST" and len(rest) == 2 and rest[0] == "applications":
            return self._decide(rest[1], body)
        if (method, rest) == ("GET", ["accounts"]):
            return 200, {"accounts": [self._account(a) for a in sorted(self.accounts)]}
        if (method, rest) == ("POST", ["codes"]):
            return self._make_code(body, asking)
        if (method, rest) == ("POST", ["revoke"]):
            device = (
                jws.unb64u(body.get("device", "")) if isinstance(body.get("device"), str) else None
            )
            if device is None or len(device) != 32:
                return problem("malformed")
            return 200, {"revoked": self._revoke_device(device)}
        return problem("unknown-message", 404)

    @staticmethod
    def _entry(app: dict[str, Any]) -> dict[str, Any]:
        entry = {k: v for k, v in app.items() if k != "credential"}
        if app["status"] == "pending":
            entry["expires"] = app["received"] + APPLICATION_S
        return entry

    @staticmethod
    def _flags(given: Any) -> int | None:
        """RP-5 §7.3: `send`, `receive`, `role` by name; none given, `send` and `receive`."""
        if given is None:
            return rp.FLAGS["send"] | rp.FLAGS["receive"]
        if not isinstance(given, list) or not all(f in GRANTABLE for f in given):
            return None
        return sum(rp.FLAGS[f] for f in set(given))

    def _decide(self, aid: str, body: dict[str, Any]) -> Response:
        app = self.applications.get(aid)
        if app is None or not self._alive(app):
            return problem("unknown-message", 404)
        decision = body.get("decision")
        flags = self._flags(body.get("flags")) if decision == "approve" else 0
        text = body.get("text")
        if (
            decision not in {"approve", "decline", "postcard"}
            or flags is None
            or (decision == "decline" and not (isinstance(text, str) and text))
        ):
            return problem("malformed")
        if app["status"] != "pending" or (
            decision != "decline" and app["address"] in self.accounts
        ):
            return problem("already-have")
        if decision != "decline" and app["operator"] in self.operators:
            return operates(app["operator"])  # RP-5 §7.3: its callsign became an operator's since
        app["decided"] = self.now
        if decision == "approve":
            device = jws.unb64u(app["device"])
            assert device is not None
            credential = self.issue(app["address"], app["operator"], device, flags)
            self.names[app["address"]] = app["name"]
            app.update(status="approved", flags=flags, credential=jws.b64u(credential))
        elif decision == "decline":
            app.update(status="declined", text=text)
        else:
            code = self._new_code()
            both = rp.FLAGS["send"] | rp.FLAGS["receive"]
            self._add_code(code, app["address"], app["operator"], both, POSTCARD_S)
            self.names[app["address"]] = app["name"]
            app.update(status="postcard", code=code, code_expires=self.now + POSTCARD_S)
        return 200, self._entry(app)

    def _account(self, address: str) -> dict[str, Any]:
        """RP-5 §7.4: the account, its waiting mail and its credentials not yet expired."""
        revoked = set(self.revoked.get(self.domains[0], {}).get("serials", []))
        creds = [
            {
                "serial": serial,
                "operator": c["operator"],
                "device": jws.b64u(c["device"]),
                "flags": c["flags"],
                "expires": c["expires"],
                "revoked": serial in revoked,
            }
            for serial, c in sorted(self.credentials.items())
            if c["address"] == address and c["expires"] > self.now
        ]
        entry: dict[str, Any] = {
            "address": address,
            "waiting": sum(not w.picked for w in self.mailboxes.get(address, [])),
            "credentials": creds,
        }
        if address in self.names:
            entry["name"] = self.names[address]
        return entry

    def _make_code(self, body: dict[str, Any], asking: tuple[str, str]) -> Response:
        """RP-5 §7.5: a code for a new account or a new device of one."""
        address, operator, name = body.get("address"), body.get("operator"), body.get("name")
        flags = self._flags(body.get("flags"))
        confirmed = body.get("operates", False)
        if not (
            isinstance(address, str)
            and ENTRY.fullmatch(address)
            and not address.startswith("@")
            and address.rpartition("@")[2] in self.domains
            and isinstance(operator, str)
            and len(operator) <= 20
            and CALLSIGN.fullmatch(operator)
            and flags is not None
            and (name is None or (isinstance(name, str) and 1 <= len(name) <= 64 and name.strip()))
            and isinstance(confirmed, bool)
        ):
            return problem("malformed")
        gives = operator in self.operators  # RP-5 §7.1: whoever enrolls with it is an operator
        if gives and not confirmed and (address, operator) != asking:
            return operates(operator)
        code = self._new_code()
        self._add_code(code, address, operator, flags)
        if name is not None:
            self.names[address] = name
        answer: dict[str, Any] = {
            "code": code,
            "address": address,
            "operator": operator,
            "flags": flags,
            "expires": self.now + CODE_S,
        }
        if gives:
            answer["operates"] = True
        return 200, answer

    def _revoke_device(self, device: bytes) -> list[int]:
        """RP-1 §8: every serial issued for the key, and the tokens they gave."""
        serials = self.serials_of.get(device, [])
        listed = self.revoked.setdefault(self.domains[0], {"serials": [], "kids": []})
        listed["serials"].extend(s for s in serials if s not in listed["serials"])
        self.tokens = {k: v for k, v in self.tokens.items() if v["body"][2] not in serials}
        return list(serials)
