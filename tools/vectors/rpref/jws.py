# SPDX-License-Identifier: MIT OR Apache-2.0
"""JWS compact serialisation with EdDSA (RFC 7515, RFC 8037), as RP-0 §5 and RP-5 §1 use it.

A post office signs its public documents (RP-5 §4) this way. The signature is plain JWS, over the
JWS signing input, with no RadioPost context: that input is base64url text and a dot, so it never
holds the zero byte that every RadioPost context ends with (RP-0 §5), and it cannot be taken for
any other RadioPost signature.
"""

from __future__ import annotations

import base64
import json
import re
from typing import Any

from . import ed25519, rp

BASE64URL = re.compile(r"[A-Za-z0-9_-]*")


def b64u(data: bytes) -> str:
    """Base64url without padding (RFC 4648 §5; RP-0 §4.2)."""
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode("ascii")


def unb64u(text: str) -> bytes | None:
    """The bytes of base64url without padding, or None."""
    if not BASE64URL.fullmatch(text) or len(text) % 4 == 1:
        return None
    return base64.urlsafe_b64decode(text + "=" * (-len(text) % 4))


def compact(value: Any) -> bytes:
    """One JSON spelling: no spaces, members in the order given, UTF-8. Any spelling is valid."""
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")


def sign(payload: bytes, secret: bytes, public: bytes, header: dict[str, Any] | None = None) -> str:
    """A JWS over ``payload``; the header names the key by its ID (RP-5 §1)."""
    protected = header if header is not None else {"alg": "EdDSA", "kid": rp.kid(public).hex()}
    signing_input = b64u(compact(protected)) + "." + b64u(payload)
    return signing_input + "." + b64u(ed25519.sign(secret, signing_input.encode("ascii")))


def verify(jws: str, keys: dict[str, bytes]) -> tuple[str, Any]:
    """("ok", the payload as JSON) or (an RP-0 §9.3 error name, None).

    ``keys`` maps a key ID in hex to a public key: the keys the verifier accepts (RP-1 §7.2).
    """
    parts = jws.split(".")
    if len(parts) != 3:
        return "malformed", None
    raw = [unb64u(p) for p in parts]
    if any(r is None for r in raw):
        return "malformed", None
    header_bytes, payload, signature = raw
    assert header_bytes is not None and payload is not None and signature is not None
    try:
        header = json.loads(header_bytes)
        document = json.loads(payload)
    except ValueError:
        return "malformed", None
    if not isinstance(header, dict) or not isinstance(document, dict):
        return "malformed", None
    if header.get("alg") != "EdDSA" or "crit" in header:
        return "signature-invalid", None
    public = keys.get(header.get("kid", ""))
    signing_input = (parts[0] + "." + parts[1]).encode("ascii")
    if public is None or not ed25519.verify(public, signing_input, signature):
        return "signature-invalid", None
    return "ok", document


def accepted_keys(dns: dict[str, bytes], info: str | None) -> dict[str, bytes]:
    """RP-1 §7.2: the keys in DNS, and those an info document signed by a DNS key lists."""
    keys = dict(dns)
    if info is not None:
        result, document = verify(info, dns)
        if result == "ok":
            for k in document.get("keys", []):
                public = unb64u(k.get("public", ""))
                if public is not None and len(public) == 32 and rp.kid(public).hex() == k["kid"]:
                    keys[k["kid"]] = public
    return keys
