# SPDX-License-Identifier: MIT OR Apache-2.0
"""Alerts (RP-8): a circle's key from its keyword, an alert's frame and tag, and what a station
that hears a broadcast makes of it.

As plainly as the specification says it, for the vectors: no clock, no store, no modem. The
receiver's situation (its callsigns, the circles it knows, the time, the tags it has heard) comes
in as arguments.
"""

from __future__ import annotations

import hashlib
import hmac
import re
import unicodedata
from typing import Any

from . import cbor

# RP-0 §9.2: the contexts of a circle's key, its ID and an alert's tag.
CTX_CIRCLE = "RadioPost-circle-v1"
CTX_CIRCLE_ID = "RadioPost-circle-id-v1"
CTX_ALERT = "RadioPost-alert-v1"

ITERATIONS = 600_000  # RP-8 §2
KEYWORD_MIN = 8  # characters, after normalising (RP-8 §2)
TEXT_MAX = 100  # bytes of UTF-8 (RP-8 §3)
WINDOW_S = 600  # RP-8 §4: an alert's time against the receiver's clock
REMEMBER_S = 2 * WINDOW_S  # RP-8 §4: how long a tag heard is kept

# RP-0 §9.5: broadcast types. RP-8 defines four.
ALERT, HEARD, SEEN, REPLY = 1, 2, 3, 4
TYPES = {ALERT: "alert", HEARD: "heard", SEEN: "seen", REPLY: "reply"}

# RP-8 §3: the frame's keys.
K_TYPE, K_CIRCLE, K_FROM, K_TO, K_TIME, K_TEXT, K_RE, K_TAG = range(8)

CALLSIGN = re.compile(
    r"(?:[A-Z0-9]{1,10}/)?[A-Z0-9]{1,10}(?:/[A-Z0-9]{1,10})?(?:-(?:1[0-5]|[0-9]))?"
)


def is_callsign(text: object) -> bool:
    """RP-0 §6.1, as it is sent: upper case, at most 20 characters."""
    return isinstance(text, str) and len(text) <= 20 and CALLSIGN.fullmatch(text) is not None


def base_callsign(callsign: str) -> str:
    """RP-0 §6.1."""
    best = ""
    for part in callsign.upper().split("-", 1)[0].split("/"):
        if len(part) >= len(best):
            best = part
    return best


# -- §2: the circle ------------------------------------------------------------------------------


def normalise_keyword(keyword: str) -> str:
    """RP-8 §2: NFC; the ASCII letters A to Z in lower case; every run of spaces and tabs one
    space, none at either end."""
    text = unicodedata.normalize("NFC", keyword)
    text = "".join(chr(ord(c) + 32) if "A" <= c <= "Z" else c for c in text)
    return re.sub(r"[ \t]+", " ", text).strip(" ")


def keyword_problem(keyword: str) -> str | None:
    """Why a program refuses a keyword, or None (RP-8 §2)."""
    if len(normalise_keyword(keyword)) < KEYWORD_MIN:
        return f"shorter than {KEYWORD_MIN} characters"
    return None


def circle_key(keyword: str) -> bytes:
    """RP-8 §2: PBKDF2-HMAC-SHA256 of the normalised keyword, the context as the salt."""
    password = normalise_keyword(keyword).encode("utf-8")
    return hashlib.pbkdf2_hmac("sha256", password, CTX_CIRCLE.encode("ascii"), ITERATIONS, 32)


def mac(key: bytes, context: str, data: bytes) -> bytes:
    """RP-0 §5: HMAC-SHA256 over the context, one zero byte, then the bytes."""
    return hmac.new(key, context.encode("ascii") + b"\x00" + data, hashlib.sha256).digest()


def circle_id(key: bytes) -> bytes:
    """RP-8 §2: the first 4 bytes of the key's MAC over its ID context and nothing more."""
    return mac(key, CTX_CIRCLE_ID, b"")[:4]


def check_text(cid: bytes) -> str:
    """RP-8 §2: the ID as operators read it to each other, in two groups of four hex digits."""
    h = cid.hex()
    return f"{h[:4]} {h[4:]}"


# -- §3: the frame -------------------------------------------------------------------------------


def frame_map(
    kind: int,
    circle: bytes,
    sender: str,
    time: int,
    *,
    to: str | None = None,
    text: str | None = None,
    re_tag: bytes | None = None,
) -> dict[int, Any]:
    """The frame's map without its tag."""
    m: dict[int, Any] = {K_TYPE: kind, K_CIRCLE: circle, K_FROM: sender, K_TIME: time}
    if to is not None:
        m[K_TO] = to
    if text is not None:
        m[K_TEXT] = unicodedata.normalize("NFC", text)
    if re_tag is not None:
        m[K_RE] = re_tag
    return m


def tag_of(key: bytes, untagged: dict[int, Any]) -> bytes:
    """RP-8 §3: the first 8 bytes of the MAC over the map's encoding without key 7."""
    return mac(key, CTX_ALERT, cbor.encode(untagged))[:8]


def make(key: bytes, untagged: dict[int, Any]) -> bytes:
    """The frame: the map with its tag, deterministically encoded."""
    return cbor.encode({**untagged, K_TAG: tag_of(key, untagged)})


# -- §4: hearing one -----------------------------------------------------------------------------


def shape_problem(m: dict[Any, Any]) -> str | None:
    """RP-8 §3's rules on the keys, or None when the map keeps them."""
    kind = m[K_TYPE]
    circle, sender, time = m.get(K_CIRCLE), m.get(K_FROM), m.get(K_TIME)
    if not (isinstance(circle, bytes) and len(circle) == 4):
        return "circle: 4 bytes"
    if not is_callsign(sender):
        return "from: a callsign"
    if not (isinstance(time, int) and time >= 0):
        return "time: Unix seconds"
    to = m.get(K_TO)
    if to is not None and not is_callsign(to):
        return "to: a callsign"
    if to is None and kind != ALERT:
        return "to: only an alert may leave it out"
    text = m.get(K_TEXT)
    if kind in {ALERT, REPLY}:
        if not isinstance(text, str):
            return "text: needed"
        raw = text.encode("utf-8")
        if not 1 <= len(raw) <= TEXT_MAX:
            return f"text: 1 to {TEXT_MAX} bytes"
        if unicodedata.normalize("NFC", text) != text:
            return "text: NFC"
        if any(ord(c) < 0x20 or 0x7F <= ord(c) < 0xA0 for c in text):
            return "text: no control characters"
    elif text is not None:
        return "text: only an alert or a reply"
    re_tag = m.get(K_RE)
    if kind == ALERT:
        if re_tag is not None:
            return "re: not on an alert"
    elif not (isinstance(re_tag, bytes) and len(re_tag) == 8):
        return "re: 8 bytes"
    tag = m.get(K_TAG)
    if not (isinstance(tag, bytes) and len(tag) == 8):
        return "tag: 8 bytes"
    return None


def hear(
    data: bytes,
    mine: list[str],
    circles: dict[bytes, bytes],
    now: int,
    heard: set[bytes],
) -> tuple[str, dict[int, Any] | None]:
    """RP-8 §4: what a station makes of a broadcast's data, checked in the specification's order.

    `mine` are the station's callsigns, `circles` its keys by circle ID, `heard` the tags it has
    heard within the last REMEMBER_S. Returns the verdict and, past the tag, the map.
    """
    try:
        m = cbor.decode(data)
    except (ValueError, IndexError):
        return "not an alert", None
    if not isinstance(m, dict) or m.get(K_TYPE) not in TYPES:
        return "not an alert", None
    if shape_problem(m):
        return "malformed", None
    key = circles.get(m[K_CIRCLE])
    if key is None:
        return "unknown circle", None
    untagged = {k: v for k, v in m.items() if k != K_TAG}
    if not hmac.compare_digest(tag_of(key, untagged), m[K_TAG]):
        return "the tag does not check", None
    if abs(now - m[K_TIME]) > WINDOW_S:
        return "out of time", None
    to = m.get(K_TO)
    bases = {base_callsign(c) for c in mine}
    if to is not None and base_callsign(to) not in bases:
        return "for another station", m
    if m[K_TAG] in heard:
        return "heard again", m
    kind = m[K_TYPE]
    if kind == ALERT:
        return ("an alert for the circle" if to is None else "an alert for this station"), m
    return TYPES[kind], m


def answer(
    key: bytes, alert: dict[int, Any], kind: int, me: str, now: int, *, text: str | None = None
) -> bytes:
    """RP-8 §5: a receipt (`heard`, `seen`) or a reply to an alert, from `me`."""
    return make(
        key,
        frame_map(kind, alert[K_CIRCLE], me, now, to=alert[K_FROM], text=text, re_tag=alert[K_TAG]),
    )
