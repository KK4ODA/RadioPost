# SPDX-License-Identifier: MIT OR Apache-2.0
"""RadioPost's objects, as the specifications define them, for making and checking test vectors.

This is a reference for the vectors, not a deployable implementation: it does what RP-1, RP-2,
RP-3 and RP-5 say, as plainly as possible, and nothing about networking, storage or time.
"""

from __future__ import annotations

import base64
import hashlib
import re
import unicodedata
import zlib
from typing import Any

from . import cbor, ed25519

# RP-0 §9.2: signature contexts.
CTX_CREDENTIAL = "RadioPost-credential-v1"
CTX_SESSION = "RadioPost-session-v1"
CTX_LOGIN = "RadioPost-api-login-v1"
CTX_ENROLL = "RadioPost-api-enroll-v1"
CTX_GATEWAY = "RadioPost-api-gateway-v1"
CTX_PLACEMENT = "RadioPost-placement-v1"  # RP-7 §4.2

# RP-1 §4.1: credential flags.
FLAGS = {"send": 1, "receive": 2, "role": 4, "gateway": 8}

# RP-2 §2: message keys.
MESSAGE_KEYS = {
    "v": 1,
    "from": 2,
    "to": 3,
    "cc": 4,
    "date": 5,
    "subject": 6,
    "body": 7,
    "files": 8,
    "priority": 9,
    "reply_to": 10,
    "flags": 11,
    "held": 12,
    "form": 13,
}

# RP-3 §4: PDU types.
PDU_TYPES = {
    "HELLO": 1,
    "WELCOME": 2,
    "AUTH": 3,
    "MSG": 4,
    "RECEIPT": 5,
    "REQUEST": 6,
    "DONE": 7,
    "ERROR": 8,
    "CREDENTIAL": 9,
    "NOTICE": 10,
}

# RP-0 §9.3: error codes, shared by RP-3 and RP-5.
ERRORS = {
    name: code
    for code, name in enumerate(
        [
            "unsupported-version",
            "malformed",
            "credential-invalid",
            "credential-expired",
            "credential-revoked",
            "credential-required",
            "unknown-post-office",
            "signature-invalid",
            "time-skew",
            "not-authorized",
            "too-large",
            "quota-exceeded",
            "refused-by-policy",
            "already-have",
            "unknown-message",
            "post-office-unreachable",
            "busy",
            "timeout",
            "internal",
        ],
        start=1,
    )
}

TIME_WINDOW_S = 600  # RP-1 §6.3
REPLAY_S = 1200  # RP-1 §6.3
FUTURE_ISSUE_S = 600  # RP-1 §5, step 4


# -- RP-0 ----------------------------------------------------------------------------------------


def kid(public: bytes) -> bytes:
    """RP-0 §5: a key's ID is the first 4 bytes of SHA-256 of the public key."""
    return hashlib.sha256(public).digest()[:4]


def signed_input(context: str, data: bytes) -> bytes:
    """RP-0 §5: context, one zero byte, then the signed bytes."""
    return context.encode("ascii") + b"\x00" + data


def sign(context: str, data: bytes, secret: bytes) -> bytes:
    return ed25519.sign(secret, signed_input(context, data))


def verify(context: str, data: bytes, signature: bytes, public: bytes) -> bool:
    return ed25519.verify(public, signed_input(context, data), signature)


def id_text(content_id: bytes) -> str:
    """RP-0 §6.3: base32, lowercase, RFC 4648 alphabet, no padding."""
    return base64.b32encode(content_id).decode("ascii").rstrip("=").lower()


def id_bytes(text: str) -> bytes | None:
    """A content ID from its text (RP-0 §6.3), or None when the text is not one."""
    if not re.fullmatch(r"[a-z2-7]{20}", text):
        return None
    raw = base64.b32decode(text.upper() + "====")
    return raw if id_text(raw) == text else None


def normalise(text: str) -> str:
    """RP-0 §4.3: NFC, LF line endings."""
    return unicodedata.normalize("NFC", text.replace("\r\n", "\n").replace("\r", "\n"))


# -- RP-2: messages ------------------------------------------------------------------------------


def message_map(fields: dict[str, Any]) -> dict[int | str, Any]:
    """Named fields (as the vectors write them) to the CBOR map RP-2 §2 defines."""
    out: dict[int | str, Any] = {}
    for name, value in fields.items():
        key = MESSAGE_KEYS.get(name)
        if key is None:
            key = int(name)  # an experimental key, written as its number
        out[key] = value
    return out


def content_id(canonical: bytes) -> bytes:
    """RP-2 §3."""
    return hashlib.sha256(canonical).digest()[:12]


def air_form(canonical: bytes, compression: int) -> bytes:
    """RP-2 §4. Compression 1 is raw DEFLATE; any valid stream is correct, this is one."""
    if compression == 0:
        return b"\x00" + canonical
    if compression == 1:
        packer = zlib.compressobj(9, zlib.DEFLATED, -15)
        return b"\x01" + packer.compress(canonical) + packer.flush()
    raise ValueError(f"compression {compression} is not made by this reference")


def air_decode(air: bytes) -> bytes:
    """An air form back to its canonical encoding; ValueError("malformed") when it is not one."""
    if not air:
        raise ValueError("malformed")
    if air[0] == 0:
        canonical = air[1:]
    elif air[0] == 1:
        unpacker = zlib.decompressobj(-15)
        try:
            canonical = unpacker.decompress(air[1:]) + unpacker.flush()
        except zlib.error as err:
            raise ValueError("malformed") from err
        if not unpacker.eof or unpacker.unused_data:
            raise ValueError("malformed")
    else:
        raise ValueError("malformed")
    try:
        message = cbor.decode(canonical)
    except cbor.NotCanonical as err:
        raise ValueError("malformed") from err
    if not isinstance(message, dict) or message.get(1) != 1:
        raise ValueError("malformed")
    return canonical


# -- RP-1: credentials ---------------------------------------------------------------------------


def make_credential(body: dict[int, Any], po_secret: bytes) -> tuple[bytes, bytes, bytes]:
    """Payload, signature and the credential (RP-1 §4.1)."""
    payload = cbor.encode(body)
    signature = sign(CTX_CREDENTIAL, payload, po_secret)
    return payload, signature, cbor.encode([payload, signature])


def credential_hash(credential: bytes) -> bytes:
    """RP-1 §4.3."""
    return hashlib.sha256(credential).digest()[:8]


def verify_credential(
    credential: bytes,
    post_offices: dict[str, list[dict[str, Any]]],
    revoked: dict[str, dict[str, list[Any]]],
    now: int,
    need: list[str],
) -> tuple[str, dict[int, Any] | None]:
    """RP-1 §5, in its order. Returns ("ok", body) or (the RP-0 §9.3 error name, body or None).

    ``post_offices`` maps a domain to its keys: each {"public": bytes, "not_before", "not_after"}.
    ``revoked`` maps a domain to {"serials": [...], "kids": [bytes, ...]}.
    """
    try:
        outer = cbor.decode(credential)
        if not (isinstance(outer, list) and len(outer) == 2):
            return "credential-invalid", None
        payload, signature = outer
        if not (isinstance(payload, bytes) and isinstance(signature, bytes)):
            return "credential-invalid", None
        body = cbor.decode(payload)
    except cbor.NotCanonical:
        return "credential-invalid", None
    if not isinstance(body, dict) or body.get(1) != 1:
        return "credential-invalid", None
    domain = str(body.get(3, "")).rpartition("@")[2].lower()
    keys = post_offices.get(domain)
    if not keys:
        return "unknown-post-office", body
    matching = [
        k
        for k in keys
        if kid(k["public"]) == body.get(8) and k["not_before"] <= body.get(6, -1) < k["not_after"]
    ]
    if not any(verify(CTX_CREDENTIAL, payload, signature, k["public"]) for k in matching):
        return "credential-invalid", body
    if now >= body[7]:
        return "credential-expired", body
    if body[6] > now + FUTURE_ISSUE_S:
        return "credential-invalid", body
    gone = revoked.get(domain, {})
    if body[2] in gone.get("serials", []) or body[8] in gone.get("kids", []):
        return "credential-revoked", body
    if any(not body[9] & FLAGS[f] for f in need):
        return "not-authorized", body
    return "ok", body


# -- RP-1: session statements --------------------------------------------------------------------


def statement(
    address: str,
    operator: str,
    gateway: str,
    messages: list[bytes],
    *,
    time: int | None = None,
    nonce: bytes | None = None,
    challenge: bytes | None = None,
) -> dict[int, Any]:
    """RP-1 §6.1, in time form (time and nonce) or challenge form (challenge)."""
    out: dict[int, Any] = {1: address, 2: operator, 3: gateway, 6: list(messages)}
    if challenge is None:
        out[4], out[5] = time, nonce
    else:
        out[7] = challenge
    return out


def verify_statement(
    stmt: dict[int, Any],
    signature: bytes,
    body: dict[int, Any],
    *,
    me: str,
    now: int,
    challenge: bytes | None = None,
    seen: list[tuple[str, bytes]] = (),  # type: ignore[assignment]
    sent_nonce: bytes | None = None,
) -> str:
    """RP-1 §6.3 steps 2 to 6, once the credential (step 1) has passed. ``body`` is its body.

    With ``sent_nonce``, the statement is a called station's (RP-1 §6.5), checked by the caller:
    its nonce must be the one the caller sent, and the clocks are not compared.
    """
    if stmt.get(1) != body[3] or stmt.get(2) != body[4]:
        return "not-authorized"
    if stmt.get(3) != me:
        return "not-authorized"
    if not verify(CTX_SESSION, cbor.encode(stmt), signature, body[5]):
        return "signature-invalid"
    if sent_nonce is not None:
        return "ok" if 7 not in stmt and stmt.get(5) == sent_nonce else "signature-invalid"
    if 7 in stmt:
        return "ok" if stmt[7] == challenge else "signature-invalid"
    if abs(stmt[4] - now) > TIME_WINDOW_S or (stmt[1], stmt[5]) in seen:
        return "time-skew"
    return "ok"


# -- RP-3: PDUs ----------------------------------------------------------------------------------


def frame(pdu: dict[int, Any]) -> bytes:
    """RP-3 §2: two bytes of length, big-endian, then the PDU."""
    raw = cbor.encode(pdu)
    if not 0 < len(raw) < 0x10000:
        raise ValueError("a PDU is 1 to 65535 bytes")
    return len(raw).to_bytes(2, "big") + raw


def statement_from_hello(hello: dict[int, Any]) -> dict[int, Any]:
    """RP-3 §5.1: the statement a called station rebuilds from HELLO."""
    return statement(
        hello[3], hello[4], hello[5], [o[0] for o in hello[8]], time=hello[6], nonce=hello[7]
    )


def statement_from_welcome(hello: dict[int, Any], welcome: dict[int, Any]) -> dict[int, Any]:
    """RP-1 §6.5, RP-3 §10: the statement a caller rebuilds from a called station's WELCOME."""
    body = cbor.decode(cbor.decode(welcome[13])[0])
    return statement(
        body[3], body[4], hello[4], [w[1] for w in welcome[7]], time=welcome[10], nonce=hello[7]
    )


# -- RP-1 §7.1: DNS records ----------------------------------------------------------------------


def parse_txt(txt: str) -> dict[str, str] | None:
    """One `_radiopost` TXT record, or None when it must be ignored."""
    tags: dict[str, str] = {}
    for raw in txt.split(";"):
        part = raw.strip()
        if not part:
            continue
        name, eq, value = part.partition("=")
        name, value = name.strip(), value.strip()
        if not eq or not name or name in tags:
            return None
        tags[name] = value
    if tags.get("v") != "RP1":
        return None
    if tags.get("t") == "api":
        return tags if re.fullmatch(r"https://[^\s]+", tags.get("url", "")) else None
    if tags.get("t") == "key":
        try:
            public = base64.b64decode(tags.get("p", ""), validate=True)
        except ValueError:
            return None
        if len(public) != 32 or tags.get("kid") != kid(public).hex():
            return None
        return tags
    return None
