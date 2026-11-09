# SPDX-License-Identifier: MIT OR Apache-2.0
"""Forwarding between gateways (RP-7) as a reference: session records, PROOF, placements.

Each function is one paragraph of RP-7, in its order, and returns an RP-0 §9.3 error name or "ok".
"""

from __future__ import annotations

import hashlib
from typing import Any

from . import cbor, rp

MAX_PATH = 8  # RP-7 §3.3
LOCAL_DAYS = 30  # RP-7 §4.4: how long a copy makes its address a local user
KEEP_DAYS = 30  # RP-7 §3.6, §7: how long a record is kept, and a message passed on remembered
HOLD_S = 3600  # RP-7 §6.2: how long records wait for a partner that reaches the post offices
SEGMENTS = {  # RP-7 §5.1: where automatic control may start a data transmission, hertz
    "US": [
        (3_585_000, 3_600_000),
        (7_100_000, 7_105_000),
        (10_140_000, 10_150_000),
        (14_095_000, 14_099_500),
        (14_100_500, 14_112_000),
        (18_105_000, 18_110_000),
        (21_090_000, 21_100_000),
        (24_925_000, 24_930_000),
        (28_120_000, 28_189_000),
        (50_100_000, 54_000_000),
    ],
}
PROOF = 11  # RP-3 §4


# -- §3: outward ---------------------------------------------------------------------------------


def record_id(statement: bytes) -> bytes:
    """RP-7 §3.1: the first 12 bytes of SHA-256 of the statement's encoding."""
    return hashlib.sha256(statement).digest()[:12]


def proof_pdu(record: dict[str, Any], *, hashed: bool = False) -> dict[int, Any]:
    """RP-7 §3.2: one session record, without its messages; the credential by its hash if held."""
    pdu: dict[int, Any] = {
        0: PROOF,
        2: record["statement"],
        3: record["signature"],
        4: record["started"],
        5: record["session"],
        6: list(record["path"]),
    }
    if hashed:
        pdu[9] = rp.credential_hash(record["credential"])
    else:
        pdu[1] = record["credential"]
    if record.get("pickups"):
        pdu[7] = list(record["pickups"])
    if record.get("local"):
        pdu[8] = list(record["local"])
    return pdu


def check_proof(
    pdu: dict[int, Any],
    caller: str,
    me: str,
    post_offices: dict[str, list[dict[str, Any]]],
    revoked: dict[str, dict[str, list[Any]]],
    *,
    held: dict[bytes, bytes] | None = None,
) -> tuple[str, dict[int, Any]]:
    """RP-7 §3.3: the proof as a statement relayed later, then the path.

    `held` maps the hashes of the credentials the partner holds to them; a credential that
    verifies joins it (§3.2). Returns the verdict and the statement (empty when it does not
    decode).
    """
    try:
        stmt = cbor.decode(pdu[2])
        signature, started, path = pdu[3], pdu[4], pdu[6]
    except (cbor.NotCanonical, KeyError):
        return "malformed", {}
    if (1 in pdu) == (9 in pdu):
        return "malformed", stmt
    held = {} if held is None else held
    credential = pdu[1] if 1 in pdu else held.get(pdu[9])
    if credential is None:
        return "credential-required", stmt
    need = (["send"] if stmt.get(6) else []) + (["receive"] if pdu.get(7) or pdu.get(8) else [])
    result, body = rp.verify_credential(credential, post_offices, revoked, started, need)
    if result != "ok" or body is None:
        return result, stmt
    held[rp.credential_hash(credential)] = credential
    if stmt.get(1) != body[3] or stmt.get(2) != body[4]:
        return "not-authorized", stmt
    if not rp.verify(rp.CTX_SESSION, pdu[2], signature, body[5]):
        return "signature-invalid", stmt
    if 4 in stmt and abs(stmt[4] - started) > rp.TIME_WINDOW_S:
        return "time-skew", stmt
    if not path or path[0] != stmt.get(3) or path[-1] != caller:
        return "refused-by-policy", stmt
    if len(path) > MAX_PATH or me in path:
        return "refused-by-policy", stmt
    return "ok", stmt


def check_forwarded(canonical: bytes, statements: list[dict[int, Any]]) -> str:
    """RP-7 §3.3: a forwarded message is in a record taken, and from that record's address."""
    cid = rp.content_id(canonical)
    sender = cbor.decode(canonical).get(2)
    for stmt in statements:
        if cid in stmt.get(6, []) and sender == stmt.get(1):
            return "ok"
    return "not-authorized"


# -- §3.6: the sender's side -------------------------------------------------------------------


def offer_again(queued_at: int, calling: str, cut_off_then: dict[str, bool], now: int) -> bool:
    """RP-7 §3.6: does the client offer a queued message again to the gateway it calls?

    `cut_off_then` holds, for each gateway the client has called, whether its last `WELCOME`
    said `online` false.
    """
    return now - queued_at >= KEEP_DAYS * 86400 or not cut_off_then.get(calling)


def answer_offer(cid: bytes, queue: set[bytes], passed_on: dict[bytes, int], now: int) -> str:
    """RP-7 §3.6: a cut-off gateway's answer to an offer, `accept` or `already-have`."""
    if cid in queue or now - passed_on.get(cid, now - KEEP_DAYS * 86400) < KEEP_DAYS * 86400:
        return "already-have"
    return "accept"


def sender_view(queued_at: int, confirmed: bool, now: int) -> str:
    """RP-7 §3.6: what the client shows of a message it handed to a cut-off gateway."""
    if confirmed:
        return "sent"
    return "queued" if now - queued_at < KEEP_DAYS * 86400 else "may have been dropped"


# -- §4: inward ----------------------------------------------------------------------------------


def make_placement(body: dict[int, Any], secret: bytes) -> bytes:
    """RP-7 §4.2: a COSE_Sign1 by the post office (RP-0 §5)."""
    return rp.cose_sign1(rp.CTX_PLACEMENT, cbor.encode(body), secret)[2]


def check_placement(
    placement: bytes,
    cid: bytes,
    me: str,
    now: int,
    post_offices: dict[str, list[dict[str, Any]]],
) -> tuple[str, dict[int, Any] | None]:
    """RP-7 §4.3: the cut-off gateway's checks, in order."""
    opened = rp.cose_open(placement)
    if opened is None:
        return "malformed", None
    try:
        body = cbor.decode(opened[2])
    except cbor.NotCanonical:
        return "malformed", None
    if not isinstance(body, dict) or body.get(1) != 1:
        return "malformed", None
    domain = str(body.get(2, "")).rpartition("@")[2].lower()
    keys = [k["public"] for k in post_offices.get(domain, []) if rp.kid(k["public"]) == opened[0]]
    if not any(rp.cose_verify(rp.CTX_PLACEMENT, opened, k) for k in keys):
        return "signature-invalid", body
    if body.get(4) != cid or body.get(5) != me:
        return "not-authorized", body
    if now >= body.get(6, 0):
        return "refused-by-policy", body
    return "ok", body


# -- §4.4: local delivery -----------------------------------------------------------------------


def local_users(copies_seen: dict[str, int], now: int) -> set[str]:
    """The addresses a gateway received a copy for in the last 30 days (RP-7 §4.4)."""
    return {a for a, t in copies_seen.items() if now - t < LOCAL_DAYS * 86400}


def local_recipients(canonical: bytes, local: set[str]) -> list[str]:
    """The message's `to` and `cc` that are local users, in the message's order."""
    message = cbor.decode(canonical)
    return [a for a in message.get(3, []) + message.get(4, []) if a in local]


# -- §5, §6.2: windows and partners --------------------------------------------------------------


def next_window(window: dict[str, Any], now: int) -> int:
    """RP-7 §5: when the window open at `now` opened, or else when the next one opens."""
    every, at = window["every_s"], window["at_s"]
    start = (now - at) // every * every + at
    return start if now < start + window["length_s"] else start + every


def window_allowed(occupied: list[int], country: str) -> bool:
    """RP-7 §5: the signal lies inside one segment of the country's table, ends included."""
    low, high = occupied
    return low <= high and any(a <= low and high <= b for a, b in SEGMENTS.get(country, []))


def records_to(partner: str, partners: dict[str, dict[str, Any]], now: int) -> bool:
    """RP-7 §6.2: in `partner`'s window, does a cut-off gateway send it its records?

    `partners` maps each partner to its `window` and, once called, the `online` of its last
    `WELCOME`.
    """
    if partners[partner].get("online", True):
        return True
    return not any(
        p != partner and s.get("online") and next_window(s["window"], now) - now <= HOLD_S
        for p, s in partners.items()
    )
