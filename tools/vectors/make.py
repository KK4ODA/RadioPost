# SPDX-License-Identifier: MIT OR Apache-2.0
"""Make RadioPost's test vectors in vectors/, or check them.

    python tools/vectors/make.py            # write vectors/*.json
    python tools/vectors/make.py --check    # rebuild in memory; fail if any file differs

Standard library only. Every key is derived from a published label (vectors/keys.json), every
"random" value from a published seed, so the files are the same on every machine. The one
exception is DEFLATE output (RP-2 §4), which can differ between zlib versions: those air forms are
marked ``"exact": false`` and checked by inflating them, not byte for byte.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).parent))

from rpref import cbor, ed25519, rp

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "vectors"

DAY = 86400
T_KEYS_FROM = 1767225600  # 2026-01-01T00:00:00Z
T_KEYS_UNTIL = 1830297600  # 2028-01-01T00:00:00Z
T_OLD_KEY_FROM = 1735689600  # 2025-01-01T00:00:00Z
T_ISSUED = 1793491200  # 2026-11-01T00:00:00Z
T_EXPIRES = T_ISSUED + 90 * DAY
T_SESSION = 1794235200  # 2026-11-09T14:40:00Z

PO_RADIOPOST = "https://po.radiopost.org/rp/v1"
PO_HAMPOST = "https://po.hampost.org/rp/v1"

WARNING = (
    "TEST KEYS ONLY. Their private halves are published here; never use them for anything but "
    "checking an implementation against these vectors."
)


# -- deterministic inputs ------------------------------------------------------------------------


def seed(label: str) -> bytes:
    """A test private key: SHA-256 of a published label."""
    return hashlib.sha256(b"RadioPost test vector key: " + label.encode()).digest()


def stream(label: str, n: int) -> bytes:
    """Published pseudo-random bytes: SHA-256 of the label and a counter, concatenated."""
    out = b""
    i = 0
    while len(out) < n:
        out += hashlib.sha256(f"RadioPost test bytes: {label}: {i}".encode()).digest()
        i += 1
    return out[:n]


KEY_LABELS = [
    ("po:radiopost.org", "Post office key of radiopost.org", T_KEYS_FROM, T_KEYS_UNTIL),
    ("po:radiopost.org:2025", "radiopost.org's previous key", T_OLD_KEY_FROM, T_KEYS_FROM),
    ("po:hampost.org", "Post office key of hampost.org", T_KEYS_FROM, T_KEYS_UNTIL),
    ("device:facundo", "Facundo's laptop: facundo@radiopost.org, KK4ODA", None, None),
    ("device:facundo-tablet", "Facundo's tablet, enrolled for receiving only", None, None),
    ("device:ana", "Ana's laptop: ana@hampost.org, N0CALL", None, None),
    ("device:shelter3-n3call", "Shelter 3's laptop, operator N3CALL", None, None),
    ("device:shelter3-n4call", "Shelter 3's laptop, operator N4CALL", None, None),
    ("device:gateway-n1call-10", "Gateway N1CALL-10 (credential from hampost.org)", None, None),
    ("device:gateway-kk4oda-1", "Gateway KK4ODA-1, being registered at radiopost.org", None, None),
]

SECRET = {label: seed(label) for label, *_ in KEY_LABELS}
PUBLIC = {label: ed25519.public_key(s) for label, s in SECRET.items()}


def po_keys() -> dict[str, list[dict[str, Any]]]:
    """What a verifier knows of each post office's keys."""
    out: dict[str, list[dict[str, Any]]] = {"radiopost.org": [], "hampost.org": []}
    for label, _, start, end in KEY_LABELS:
        if label.startswith("po:"):
            domain = label.split(":")[1]
            out[domain].append({"public": PUBLIC[label], "not_before": start, "not_after": end})
    return out


# -- helpers for writing -------------------------------------------------------------------------


def pdu_entry(name: str, pdu: dict[int, Any]) -> dict[str, Any]:
    raw = cbor.encode(pdu)
    return {
        "name": name,
        "diagnostic": cbor.diagnostic(pdu),
        "pdu_hex": raw.hex(),
        "frame_hex": rp.frame(pdu).hex(),
    }


def header(purpose: str, specs: dict[str, str]) -> dict[str, Any]:
    return {
        "radiopost_vectors": purpose,
        "format": 1,
        "specs": specs,
        "made_by": "tools/vectors/make.py",
    }


# -- keys.json -----------------------------------------------------------------------------------


def make_keys() -> dict[str, Any]:
    keys = []
    for label, what, start, end in KEY_LABELS:
        entry: dict[str, Any] = {
            "label": label,
            "what": what,
            "seed_hex": SECRET[label].hex(),
            "public_hex": PUBLIC[label].hex(),
            "kid_hex": rp.kid(PUBLIC[label]).hex(),
        }
        if start is not None:
            entry["not_before"], entry["not_after"] = start, end
        keys.append(entry)
    out = header("Test keys used by every other vector file", {"RP-0": "§5", "RP-1": "§2"})
    out["warning"] = WARNING
    out["how_derived"] = 'seed = SHA-256("RadioPost test vector key: " + label)'
    out["keys"] = keys
    return out


# -- cbor.json -----------------------------------------------------------------------------------


def make_cbor() -> dict[str, Any]:
    valid_values: list[tuple[str, Any]] = [
        ("small integer", 23),
        ("one-byte integer", 24),
        ("two-byte integer", 1000),
        ("four-byte integer, a time", T_SESSION),
        ("negative integer", -1000),
        ("empty byte string", b""),
        ("12-byte content ID", bytes.fromhex("a3f9c27e15d0b84466e2c901")),
        ("text with non-ASCII", "Señal ☕"),
        ("booleans in an array", [True, False]),
        (
            "map keys in byte order: 1, 2, 10, 23, 24, 100",
            {100: 0, 24: 0, 23: 0, 10: 0, 2: 0, 1: 0},
        ),
        ("text map keys after integer keys", {"a": 1, 1: "a"}),
        ("shorter text keys first", {"bb": 0, "a": 0, "c": 0}),
        ("nested", {1: [1, [2, 3]], 2: {1: b"\x01"}}),
    ]
    valid = [
        {"what": what, "diagnostic": cbor.diagnostic(v), "hex": cbor.encode(v).hex()}
        for what, v in valid_values
    ]
    invalid = [
        ("1817", "23 in two bytes: not the shortest form"),
        ("190064", "100 in three bytes: not the shortest form"),
        ("5f4101ff", "an indefinite-length byte string"),
        ("9f01ff", "an indefinite-length array"),
        ("a2020101", "map keys out of order (2 before 1)"),
        ("a201010101", "a repeated map key"),
        ("f93c00", "a floating-point number (1.0, half precision)"),
        ("c11a6912b240", "a tag (1, a time)"),
        ("0101", "bytes after the item"),
        ("62c3", "text shorter than its length"),
        ("61ff", "text that is not UTF-8"),
    ]
    out = header(
        "Deterministic CBOR as RadioPost uses it",
        {"RP-0": "§4.1", "RFC 8949": "§4.2.1"},
    )
    out["rule"] = (
        "An encoder produces exactly `hex` from the value in `diagnostic`. A decoder refuses every "
        "`invalid` entry."
    )
    out["valid"] = valid
    out["invalid"] = [{"hex": h, "why": why} for h, why in invalid]
    return out


# -- messages ------------------------------------------------------------------------------------


ROSTER = "".join(
    f"{i:2d}. Family {name}: {n} people, {need}\n"
    for i, (name, n, need) in enumerate(
        [
            ("Alvarez", 4, "insulin needed by Thursday"),
            ("Brooks", 2, "no needs"),
            ("Chen", 5, "infant formula"),
            ("Diallo", 3, "no needs"),
            ("Evans", 1, "wheelchair, needs ground floor"),
            ("Fischer", 6, "no needs"),
            ("Garcia", 2, "oxygen concentrator, needs power"),
            ("Haddad", 4, "no needs"),
            ("Ivanova", 3, "pet dog"),
            ("Johnson", 2, "no needs"),
            ("Kowalski", 5, "no needs"),
            ("Lopez", 3, "prescription refill, blood pressure"),
            ("Mensah", 4, "no needs"),
            ("Nakamura", 2, "no needs"),
            ("Okafor", 7, "two children under three"),
            ("Patel", 3, "no needs"),
            ("Quinn", 1, "no needs"),
            ("Rossi", 4, "dietary: gluten free"),
            ("Silva", 2, "no needs"),
            ("Thompson", 3, "no needs"),
            ("Ueda", 2, "hearing impaired, needs written notices"),
            ("Vargas", 5, "no needs"),
            ("Williams", 2, "no needs"),
            ("Xu", 3, "no needs"),
            ("Young", 4, "no needs"),
            ("Zhou", 2, "no needs"),
        ],
        start=1,
    )
)


def messages() -> dict[str, dict[str, Any]]:
    """The messages of the vectors, by name, as named fields (RP-2 §2)."""
    m: dict[str, dict[str, Any]] = {}
    m["maria-419"] = {
        "v": 1,
        "from": "maria@example.com",
        "to": ["facundo@radiopost.org"],
        "date": T_SESSION - 3 * 3600,
        "subject": "Are you safe?",
        "body": (
            "Facundo,\n\nWe saw the news about the storm and cannot reach anyone by phone. "
            "Are you and the family safe? Is the house all right? Do you need anything we "
            "can send?\n\nWrite when you can, even one line.\n\nMaria\n"
        ),
    }
    m["eoc-420"] = {
        "v": 1,
        "from": "eoc@radiopost.org",
        "to": ["facundo@radiopost.org"],
        "cc": ["shelter3@radiopost.org"],
        "date": T_SESSION - 2 * 3600,
        "subject": "Shelter roster",
        "body": "Shelter 3 roster as of 12:00 local. Please confirm by radio.\n\n" + ROSTER,
    }
    m["maria-421"] = {
        "v": 1,
        "from": "maria@example.com",
        "to": ["facundo@radiopost.org"],
        "date": T_SESSION - 3600,
        "subject": "Photo",
        "body": "The kids made you a card.\n",
        "files": [{"name": "IMG_2231.jpg", "type": "image/jpeg", "data": stream("photo", 6000)}],
    }
    m["facundo-reply"] = {
        "v": 1,
        "from": "facundo@radiopost.org",
        "to": ["maria@example.com"],
        "date": T_SESSION - 600,
        "subject": "Re: Are you safe?",
        "body": "We are fine. Power is out, but the radio works.\n",
    }
    m["facundo-to-ana"] = {
        "v": 1,
        "from": "facundo@radiopost.org",
        "to": ["ana@hampost.org"],
        "date": T_SESSION - 300,
        "subject": "Checking in",
        "body": "Ana, I am on the road near Austin until Friday. All well here. 73, Facundo\n",
    }
    m["shelter3-priority"] = {
        "v": 1,
        "from": "shelter3@radiopost.org",
        "to": ["eoc@radiopost.org"],
        "date": T_SESSION - 1200,
        "subject": "Café closed; señal débil",
        "body": "Need 40 cots and water for 3 days. Señal débil después de las 18:00.\n",
        "priority": 1,
    }
    m["emergency-held"] = {
        "v": 1,
        "from": "eoc@radiopost.org",
        "to": ["shelter3@radiopost.org"],
        "cc": ["facundo@radiopost.org"],
        "date": T_SESSION - 900,
        "subject": "Evacuation map",
        "body": "Route 3 closed. Use the attached map.\n",
        "priority": 2,
        "flags": 2,
        "held": [
            {
                "name": "map.pdf",
                "type": "application/pdf",
                "size": 2_400_000,
                "ref": stream("held-ref", 8),
            }
        ],
    }
    m["experimental-key"] = {
        "v": 1,
        "from": "ana@hampost.org",
        "to": ["facundo@radiopost.org"],
        "date": T_SESSION - 60,
        "subject": "Test",
        "body": "",
        "64": "a key from 64 up, for experiments (RP-0 §4.1): kept and hashed",
    }
    m["facundo-reply"]["reply_to"] = rp.content_id(cbor.encode(rp.message_map(m["maria-419"])))
    return m


def to_json(value: Any) -> Any:
    """Named message fields to JSON: bytes as hex, under a name ending in `_hex`."""
    if isinstance(value, dict):
        out = {}
        for k, v in value.items():
            if isinstance(v, bytes):
                out[f"{k}_hex"] = v.hex()
            else:
                out[k] = to_json(v)
        return out
    if isinstance(value, list):
        return [to_json(v) for v in value]
    return value


def file_map(f: dict[str, Any]) -> dict[int, Any]:
    keys = {"name": 1, "type": 2, "data": 3, "size": 3, "ref": 4}
    return {keys[k]: v for k, v in f.items()}


def cbor_message(fields: dict[str, Any]) -> dict[int | str, Any]:
    out = dict(fields)
    if "files" in out:
        out["files"] = [file_map(f) for f in out["files"]]
    if "held" in out:
        out["held"] = [file_map(f) for f in out["held"]]
    return rp.message_map(out)


def encoded(name: str) -> tuple[bytes, bytes]:
    """A message's canonical encoding and content ID."""
    canonical = cbor.encode(cbor_message(MESSAGES[name]))
    return canonical, rp.content_id(canonical)


MESSAGES = messages()


def make_messages() -> dict[str, Any]:
    entries = []
    for name, fields in MESSAGES.items():
        canonical, cid = encoded(name)
        air = [{"compression": 0, "hex": rp.air_form(canonical, 0).hex(), "exact": True}]
        deflated = rp.air_form(canonical, 1)
        if len(deflated) < len(canonical) + 1:
            air.append({"compression": 1, "hex": deflated.hex(), "exact": False})
        entries.append(
            {
                "name": name,
                "fields": to_json(fields),
                "diagnostic": cbor.diagnostic(cbor_message(fields)),
                "canonical_hex": canonical.hex(),
                "content_id_hex": cid.hex(),
                "content_id_text": rp.id_text(cid),
                "air": air,
            }
        )
    canonical, cid = encoded("facundo-reply")
    swapped = bytearray(canonical)
    # The map's first two entries (key 1 and key 2) exchanged: the same message, out of order.
    first = cbor.encode(1) + cbor.encode(1)
    second = cbor.encode(2) + cbor.encode(MESSAGES["facundo-reply"]["from"])
    assert canonical[1 : 1 + len(first) + len(second)] == first + second
    swapped[1 : 1 + len(first) + len(second)] = second + first
    invalid = [
        {
            "why": "map keys out of order: the same message, not in deterministic form",
            "air_hex": (b"\x00" + bytes(swapped)).hex(),
            "expect": "malformed",
        },
        {
            "why": "compression byte 9 is not defined",
            "air_hex": (b"\x09" + canonical).hex(),
            "expect": "malformed",
        },
        {
            "why": "DEFLATE stream with bytes after its end",
            "air_hex": (rp.air_form(canonical, 1) + b"\x00").hex(),
            "expect": "malformed",
        },
        {
            "why": "the `v` key is missing",
            "air_hex": (
                b"\x00" + cbor.encode({2: "a@b.c", 3: ["d@e.f"], 5: 0, 6: "", 7: ""})
            ).hex(),
            "expect": "malformed",
        },
    ]
    out = header("Messages: canonical encodings, content IDs, air forms", {"RP-2": "§2–§4"})
    out["rule"] = (
        "From `fields`, an encoder produces exactly `canonical_hex`; its content ID is "
        "`content_id_hex`. Every air form decodes to `canonical_hex`. An air form marked "
        "`exact: false` is one valid DEFLATE stream among many: check that it inflates, not that "
        "you produce the same bytes. Every `invalid` air form is refused as `malformed`."
    )
    out["fields_note"] = (
        "Field names are RP-2 §2's; a name ending in `_hex` holds bytes. `files` and `held` items "
        "use the names of RP-2 §2 (name, type, data or size, ref). A field named with a number is "
        "that CBOR key."
    )
    out["messages"] = entries
    out["invalid"] = invalid
    return out


# -- credentials ---------------------------------------------------------------------------------


CREDENTIALS: dict[str, dict[str, Any]] = {
    "facundo": {
        "po": "po:radiopost.org",
        "device": "device:facundo",
        "serial": 1041,
        "address": "facundo@radiopost.org",
        "operator": "KK4ODA",
        "flags": ["send", "receive"],
    },
    "facundo-tablet": {
        "po": "po:radiopost.org",
        "device": "device:facundo-tablet",
        "serial": 1050,
        "address": "facundo@radiopost.org",
        "operator": "KK4ODA",
        "flags": ["receive"],
    },
    "ana": {
        "po": "po:hampost.org",
        "device": "device:ana",
        "serial": 2001,
        "address": "ana@hampost.org",
        "operator": "N0CALL",
        "flags": ["send", "receive"],
    },
    "shelter3-n3call": {
        "po": "po:radiopost.org",
        "device": "device:shelter3-n3call",
        "serial": 1077,
        "address": "shelter3@radiopost.org",
        "operator": "N3CALL",
        "flags": ["send", "receive", "role"],
    },
    "shelter3-n4call": {
        "po": "po:radiopost.org",
        "device": "device:shelter3-n4call",
        "serial": 1078,
        "address": "shelter3@radiopost.org",
        "operator": "N4CALL",
        "flags": ["send", "receive", "role"],
    },
    "gateway-n1call-10": {
        "po": "po:hampost.org",
        "device": "device:gateway-n1call-10",
        "serial": 2017,
        "address": "n1call@hampost.org",
        "operator": "N1CALL-10",
        "flags": ["send", "gateway"],
    },
    "signed-by-retired-key": {
        "po": "po:radiopost.org:2025",
        "device": "device:facundo",
        "serial": 999,
        "address": "facundo@radiopost.org",
        "operator": "KK4ODA",
        "flags": ["send", "receive"],
    },
    "issued-an-hour-ahead": {
        "po": "po:radiopost.org",
        "device": "device:facundo",
        "serial": 1090,
        "address": "facundo@radiopost.org",
        "operator": "KK4ODA",
        "flags": ["send", "receive"],
        "issued": T_SESSION + 3600,
    },
    "issued-ten-minutes-ahead": {
        "po": "po:radiopost.org",
        "device": "device:facundo",
        "serial": 1091,
        "address": "facundo@radiopost.org",
        "operator": "KK4ODA",
        "flags": ["send", "receive"],
        "issued": T_SESSION + 600,
    },
}


def credential_body(spec: dict[str, Any]) -> dict[int, Any]:
    issued = spec.get("issued", T_ISSUED)
    return {
        1: 1,
        2: spec["serial"],
        3: spec["address"],
        4: spec["operator"],
        5: PUBLIC[spec["device"]],
        6: issued,
        7: issued + 90 * DAY,
        8: rp.kid(PUBLIC[spec["po"]]),
        9: sum(rp.FLAGS[f] for f in spec["flags"]),
    }


def credential(name: str) -> bytes:
    spec = CREDENTIALS[name]
    return rp.make_credential(credential_body(spec), SECRET[spec["po"]])[2]


def make_credentials() -> dict[str, Any]:
    entries = []
    for name, spec in CREDENTIALS.items():
        body = credential_body(spec)
        payload, signature, cred = rp.make_credential(body, SECRET[spec["po"]])
        entries.append(
            {
                "name": name,
                "signed_by": spec["po"],
                "device": spec["device"],
                "body": {
                    "v": 1,
                    "serial": body[2],
                    "address": body[3],
                    "operator": body[4],
                    "device_hex": body[5].hex(),
                    "issued": body[6],
                    "expires": body[7],
                    "kid_hex": body[8].hex(),
                    "flags": body[9],
                },
                "payload_hex": payload.hex(),
                "signed_input_hex": rp.signed_input(rp.CTX_CREDENTIAL, payload).hex(),
                "signature_hex": signature.hex(),
                "credential_hex": cred.hex(),
                "credential_hash_hex": rp.credential_hash(cred).hex(),
            }
        )

    good = credential("facundo")
    outer = cbor.decode(good)
    tampered_payload = outer[0].replace(b"facundo@", b"facundp@")
    tampered = cbor.encode([tampered_payload, outer[1]])
    rad = rp.kid(PUBLIC["po:radiopost.org"])
    cases_in = [
        ("valid", "facundo", T_SESSION, ["send", "receive"], {}, None),
        ("one second before expiry", "facundo", T_EXPIRES - 1, ["send"], {}, None),
        ("at expiry", "facundo", T_EXPIRES, ["send"], {}, None),
        (
            "serial revoked",
            "facundo",
            T_SESSION,
            ["send"],
            {"radiopost.org": {"serials": [1041]}},
            None,
        ),
        (
            "post office key revoked",
            "facundo",
            T_SESSION,
            ["send"],
            {"radiopost.org": {"kids": [rad]}},
            None,
        ),
        ("address changed after signing", None, T_SESSION, ["send"], {}, tampered),
        ("post office not known to this verifier", "ana", T_SESSION, ["send"], {}, None),
        (
            "signed by a key outside its validity period",
            "signed-by-retired-key",
            T_SESSION,
            ["send"],
            {},
            None,
        ),
        (
            "issued an hour ahead of the verifier's clock",
            "issued-an-hour-ahead",
            T_SESSION,
            ["send"],
            {},
            None,
        ),
        (
            "issued ten minutes ahead: within the allowance",
            "issued-ten-minutes-ahead",
            T_SESSION,
            ["send"],
            {},
            None,
        ),
        ("receive-only device asked to send", "facundo-tablet", T_SESSION, ["send"], {}, None),
        ("receive-only device picking up", "facundo-tablet", T_SESSION, ["receive"], {}, None),
        ("role mailbox operator", "shelter3-n3call", T_SESSION, ["send", "receive"], {}, None),
        ("gateway acting as a gateway", "gateway-n1call-10", T_SESSION, ["gateway"], {}, None),
        (
            "a user's credential asked to act as a gateway",
            "facundo",
            T_SESSION,
            ["gateway"],
            {},
            None,
        ),
        (
            "not a two-item array",
            None,
            T_SESSION,
            ["send"],
            {},
            cbor.encode([outer[0], outer[1], b""]),
        ),
    ]
    cases = []
    for what, name, now, need, revoked, raw in cases_in:
        cred = raw if raw is not None else credential(name)
        known = po_keys()
        if what == "post office not known to this verifier":
            known = {"radiopost.org": known["radiopost.org"]}
        result, _ = rp.verify_credential(cred, known, revoked, now, need)
        cases.append(
            {
                "what": what,
                "credential": name if raw is None else None,
                "credential_hex": cred.hex() if raw is not None else None,
                "now": now,
                "need": need,
                "known_post_offices": sorted(known),
                "revoked": {
                    d: {
                        k: [x.hex() if isinstance(x, bytes) else x for x in v] for k, v in r.items()
                    }
                    for d, r in revoked.items()
                },
                "expect": result,
            }
        )
    out = header("Credentials and how they verify", {"RP-1": "§4–§5, §8", "RP-0": "§5, §9.3"})
    out["rule"] = (
        "From `body` and the signing key, an issuer produces exactly `payload_hex`, "
        "`signature_hex` and `credential_hex` (Ed25519 is deterministic). A verifier given a "
        "case's clock, needed flags, known post offices (keys and validity periods from keys.json) "
        "and revocations returns `expect`: `ok` or an RP-0 §9.3 error name, checking in RP-1 §5's "
        "order."
    )
    out["credentials"] = entries
    out["verification"] = cases
    return out


# -- statements ----------------------------------------------------------------------------------


NONCE = stream("session-01 nonce", 8)
CHALLENGE = stream("challenge", 16)


def statement_session() -> dict[int, Any]:
    ids = [encoded("facundo-reply")[1], encoded("facundo-to-ana")[1]]
    return rp.statement(
        "facundo@radiopost.org", "KK4ODA", "N1CALL-10", ids, time=T_SESSION, nonce=NONCE
    )


def make_statements() -> dict[str, Any]:
    ids = [encoded("facundo-reply")[1], encoded("facundo-to-ana")[1]]
    forms = {
        "time-form": (statement_session(), "device:facundo", "facundo"),
        "challenge-form": (
            rp.statement("facundo@radiopost.org", "KK4ODA", "N1CALL-10", ids, challenge=CHALLENGE),
            "device:facundo",
            "facundo",
        ),
        "pickup-only": (
            rp.statement(
                "shelter3@radiopost.org",
                "N3CALL",
                "N2CALL-5",
                [],
                time=T_SESSION,
                nonce=stream("shelter nonce", 8),
            ),
            "device:shelter3-n3call",
            "shelter3-n3call",
        ),
    }
    entries = []
    signed: dict[str, tuple[dict[int, Any], bytes, str]] = {}
    for name, (stmt, device, cred) in forms.items():
        raw = cbor.encode(stmt)
        sig = rp.sign(rp.CTX_SESSION, raw, SECRET[device])
        signed[name] = (stmt, sig, cred)
        entries.append(
            {
                "name": name,
                "credential": cred,
                "diagnostic": cbor.diagnostic(stmt),
                "statement_hex": raw.hex(),
                "signed_input_hex": rp.signed_input(rp.CTX_SESSION, raw).hex(),
                "signature_hex": sig.hex(),
            }
        )

    def body(cred: str) -> dict[int, Any]:
        return cbor.decode(cbor.decode(credential(cred))[0])

    stmt, sig, cred = signed["time-form"]
    fewer = dict(stmt)
    fewer[6] = stmt[6][:1]
    other_op = dict(stmt)
    other_op[2] = "N0CALL"
    c_stmt, c_sig, _ = signed["challenge-form"]
    p_stmt, p_sig, p_cred = signed["pickup-only"]
    cases_in = [
        (
            "valid, 41 s after the client's clock",
            stmt,
            sig,
            cred,
            "N1CALL-10",
            T_SESSION + 41,
            None,
            [],
        ),
        (
            "exactly 600 s apart: still inside",
            stmt,
            sig,
            cred,
            "N1CALL-10",
            T_SESSION + 600,
            None,
            [],
        ),
        (
            "601 s apart: sign a challenge instead",
            stmt,
            sig,
            cred,
            "N1CALL-10",
            T_SESSION + 601,
            None,
            [],
        ),
        (
            "nonce seen before: sign a challenge instead",
            stmt,
            sig,
            cred,
            "N1CALL-10",
            T_SESSION + 41,
            None,
            [("facundo@radiopost.org", NONCE)],
        ),
        (
            "verified by a gateway that was not the one called",
            stmt,
            sig,
            cred,
            "KK4ODA-1",
            T_SESSION + 41,
            None,
            [],
        ),
        (
            "a message removed from the list after signing",
            fewer,
            sig,
            cred,
            "N1CALL-10",
            T_SESSION + 41,
            None,
            [],
        ),
        (
            "operator not the credential's",
            other_op,
            sig,
            cred,
            "N1CALL-10",
            T_SESSION + 41,
            None,
            [],
        ),
        (
            "challenge form, the challenge issued",
            c_stmt,
            c_sig,
            cred,
            "N1CALL-10",
            T_SESSION + 900,
            CHALLENGE,
            [],
        ),
        (
            "challenge form, another challenge issued",
            c_stmt,
            c_sig,
            cred,
            "N1CALL-10",
            T_SESSION + 900,
            stream("another challenge", 16),
            [],
        ),
        (
            "pickup only, no messages offered",
            p_stmt,
            p_sig,
            p_cred,
            "N2CALL-5",
            T_SESSION,
            None,
            [],
        ),
    ]
    cases = []
    for what, s, sg, c, me, now, challenge, seen in cases_in:
        result = rp.verify_statement(s, sg, body(c), me=me, now=now, challenge=challenge, seen=seen)
        cases.append(
            {
                "what": what,
                "statement_hex": cbor.encode(s).hex(),
                "signature_hex": sg.hex(),
                "credential": c,
                "verifier": me,
                "now": now,
                "challenge_hex": challenge.hex() if challenge else None,
                "seen": [[a, n.hex()] for a, n in seen],
                "expect": result,
            }
        )
    out = header("Session statements and how they verify", {"RP-1": "§6", "RP-3": "§5.1, §5.3"})
    out["rule"] = (
        "A client produces exactly `statement_hex` and `signature_hex`. A verifier given a "
        "case's statement, signature, credential (already verified), its own callsign, its clock, "
        "the challenge it issued and the (address, nonce) pairs it has seen returns `expect`, "
        "checking in RP-1 §6.3's order."
    )
    out["statements"] = entries
    out["verification"] = cases
    return out


# -- the session ---------------------------------------------------------------------------------


SESSION_ID = stream("session-01 id", 8)
WAITING = [(419, "maria-419"), (420, "eoc-420"), (421, "maria-421")]
OUTGOING = ["facundo-reply", "facundo-to-ana"]
AUTO = 1000


def air0(name: str) -> bytes:
    return rp.air_form(encoded(name)[0], 0)


def session_pdus() -> list[tuple[str, list[tuple[str, dict[int, Any]]]]]:
    """The five transmissions of RP-3 §12, each as (who, [(name, pdu), ...])."""
    stmt = statement_session()
    signature = rp.sign(rp.CTX_SESSION, cbor.encode(stmt), SECRET["device:facundo"])
    offers = [[encoded(n)[1], len(air0(n)), 0] for n in OUTGOING]
    hello = {
        0: 1,
        1: [1, 1],
        2: 0,
        3: "facundo@radiopost.org",
        4: "KK4ODA",
        5: "N1CALL-10",
        6: T_SESSION,
        7: NONCE,
        8: offers,
        9: 418,
        10: AUTO,
        12: {1: [0, 1, 2], 2: 4096},
        14: rp.credential_hash(credential("facundo")),
        15: signature,
    }
    assert rp.statement_from_hello(hello) == stmt
    waiting = [
        [
            serial,
            encoded(name)[1],
            MESSAGES[name]["from"],
            MESSAGES[name]["subject"][:40],
            len(air0(name)),
            MESSAGES[name].get("priority", 0),
            0,
        ]
        for serial, name in WAITING
    ]
    welcome = {
        0: 2,
        1: 1,
        2: SESSION_ID,
        3: 0,
        5: [[encoded(n)[1], 0] for n in OUTGOING],
        6: [],
        7: waiting,
        8: 0,
        9: {1: [0, 1], 2: 4096},
        10: T_SESSION + 41,
        11: True,
        12: True,
    }

    def msg(name: str, serial: int | None = None) -> dict[int, Any]:
        air = air0(name)
        out = {0: 4, 1: encoded(name)[1], 2: 0, 3: len(air), 4: air}
        if serial is not None:
            out[5] = serial
        return out

    auto = [(s, n) for s, n in WAITING if len(air0(n)) <= AUTO]
    assert [s for s, _ in auto] == [419], "the example sends #419 alone unasked"
    return [
        ("client", [("HELLO", hello)]),
        ("gateway", [("WELCOME", welcome)] + [("MSG", msg(n, s)) for s, n in auto]),
        (
            "client",
            [("MSG", msg(n)) for n in OUTGOING]
            + [("RECEIPT", {0: 5, 1: [encoded("maria-419")[1]]}), ("REQUEST", {0: 6, 1: [420]})],
        ),
        (
            "gateway",
            [
                ("RECEIPT", {0: 5, 1: [encoded(n)[1] for n in OUTGOING]}),
                ("MSG", msg("eoc-420", 420)),
                ("DONE", {0: 7}),
            ],
        ),
        ("client", [("RECEIPT", {0: 5, 1: [encoded("eoc-420")[1]]}), ("DONE", {0: 7})]),
    ]


def make_session() -> dict[str, Any]:
    transmissions = []
    streams = {"client": b"", "gateway": b""}
    for n, (who, pdus) in enumerate(session_pdus(), start=1):
        data = b"".join(rp.frame(p) for _, p in pdus)
        streams[who] += data
        transmissions.append(
            {
                "number": n,
                "from": who,
                "pdus": [pdu_entry(name, p) for name, p in pdus],
                "bytes": len(data),
                "bytes_hex": data.hex(),
            }
        )
    out = header("A whole session, byte for byte", {"RP-3": "§3, §5, §12", "RP-1": "§6"})
    out["story"] = (
        "Facundo (facundo@radiopost.org, KK4ODA) calls gateway N1CALL-10, which holds his "
        "credential from an earlier visit and is online. He sends two messages and picks up one "
        "unasked (#419, under his `auto` of 1 000 bytes) and one he asks for (#420); #421 stays "
        "waiting. The client hangs up after transmission 5 (RP-3 §8)."
    )
    out["rule"] = (
        "Each transmission's `bytes_hex` is its PDUs' frames in order; the two `streams` are what "
        "each station writes to the link. A gateway or client playing either side must accept the "
        "other side's bytes and, given the same inputs, produce its own side's bytes exactly. The "
        "messages travel uncompressed (compression 0) so that every byte is reproducible; a real "
        "sender would compress where that is shorter (RP-2 §4), which changes only the MSG data."
    )
    out["context"] = {
        "client_credential": "facundo (credentials.json)",
        "gateway": "N1CALL-10, holding the client's credential, online",
        "statement": "time-form (statements.json)",
        "waiting": [
            {
                "serial": s,
                "message": n,
                "content_id_hex": encoded(n)[1].hex(),
                "air_bytes": len(air0(n)),
            }
            for s, n in WAITING
        ],
        "outgoing": [
            {"message": n, "content_id_hex": encoded(n)[1].hex(), "air_bytes": len(air0(n))}
            for n in OUTGOING
        ],
        "session_id_hex": SESSION_ID.hex(),
        "client_clock": T_SESSION,
        "gateway_clock": T_SESSION + 41,
    }
    out["transmissions"] = transmissions
    out["streams"] = {k: v.hex() for k, v in streams.items()}
    out["after"] = {
        "gateway_reports_picked_up": [419, 420],
        "gateway_hands_in": [encoded(n)[1].hex() for n in OUTGOING],
        "still_waiting": [421],
    }
    return out


# -- other PDUs ----------------------------------------------------------------------------------


def make_pdus() -> dict[str, Any]:
    session = {name: pdu for _, pdus in session_pdus() for name, pdu in pdus}
    hello_full = dict(session["HELLO"])
    del hello_full[14]
    hello_full[13] = credential("facundo")
    unsigned = dict(session["HELLO"])
    del unsigned[15]
    c_stmt = rp.statement(
        "facundo@radiopost.org",
        "KK4ODA",
        "N1CALL-10",
        [o[0] for o in session["HELLO"][8]],
        challenge=CHALLENGE,
    )
    photo = air0("maria-421")
    cut = 4000
    examples = [
        ("HELLO, signed, credential by hash", session["HELLO"]),
        ("HELLO, signed, with the credential", hello_full),
        ("HELLO, unsigned (the client's clock is wrong)", unsigned),
        ("WELCOME, ready", session["WELCOME"]),
        (
            "WELCOME, sign this challenge",
            {0: 2, 1: 1, 2: SESSION_ID, 3: 1, 4: CHALLENGE, 10: T_SESSION + 41, 11: True, 12: True},
        ),
        (
            "WELCOME, send your credential",
            {0: 2, 1: 1, 2: SESSION_ID, 3: 2, 10: T_SESSION + 41, 11: True, 12: False},
        ),
        ("AUTH", {0: 3, 1: rp.sign(rp.CTX_SESSION, cbor.encode(c_stmt), SECRET["device:facundo"])}),
        (
            "MSG, first piece of a 6 kB message",
            {0: 4, 1: encoded("maria-421")[1], 2: 0, 3: len(photo), 4: photo[:cut], 5: 421},
        ),
        (
            "MSG, second piece",
            {0: 4, 1: encoded("maria-421")[1], 2: cut, 3: len(photo), 4: photo[cut:], 5: 421},
        ),
        (
            "RECEIPT with a partial and a failure",
            {
                0: 5,
                1: [encoded("facundo-reply")[1]],
                2: [[encoded("maria-421")[1], cut]],
                3: [[encoded("facundo-to-ana")[1], 2]],
            },
        ),
        (
            "REQUEST: send, held files, more",
            {0: 6, 1: [420], 2: [[421, stream("held-ref", 8)]], 3: True},
        ),
        ("DONE", {0: 7}),
        ("ERROR, fatal", {0: 8, 1: 5, 2: "Your credential was revoked by radiopost.org.", 4: True}),
        ("ERROR about one message, not fatal", {0: 8, 1: 11, 3: encoded("maria-421")[1], 4: False}),
        ("CREDENTIAL", {0: 9, 1: credential("facundo")}),
        ("NOTICE", {0: 10, 1: "Internet down here since 14:10; your mail waits in my queue."}),
    ]
    invalid = [
        {
            "why": "a PDU of an unknown type (99)",
            "frame_hex": rp.frame({0: 99}).hex(),
            "expect": "malformed",
        },
        {
            "why": "a PDU that is not deterministic CBOR",
            "frame_hex": "0005a2020100" + "07",
            "expect": "malformed",
        },
        {"why": "a frame of length 0", "frame_hex": "0000", "expect": "malformed"},
    ]
    out = header("One example of each PDU", {"RP-3": "§2, §4, §5"})
    out["rule"] = (
        "From `diagnostic`, an encoder produces exactly `pdu_hex`, framed as `frame_hex` "
        "(two bytes of length, big-endian). Every `invalid` frame is answered with ERROR "
        "`malformed` (RP-3 §2)."
    )
    out["pdus"] = [pdu_entry(name, p) for name, p in examples]
    out["invalid"] = invalid
    return out


# -- dns.json and api.json -----------------------------------------------------------------------


def make_dns() -> dict[str, Any]:
    pub = PUBLIC["po:radiopost.org"]
    b64 = base64.b64encode(pub).decode()
    kid = rp.kid(pub).hex()
    texts = [
        f"v=RP1; t=api; url={PO_RADIOPOST}",
        f"v=RP1; t=key; kid={kid}; p={b64}",
        f"v=RP1;t=key;kid={kid};p={b64}",
        f"  v=RP1 ; t=key ; kid={kid} ; p={b64} ;",
        f"v=RP1; t=key; kid={kid}; p={b64}; note=rotated 2026",
        f"v=RP2; t=key; kid={kid}; p={b64}",
        f"v=RP1; t=key; kid=00000000; p={b64}",
        f"v=RP1; t=key; kid={kid}; p={base64.b64encode(pub[:31]).decode()}",
        f"v=RP1; t=key; kid={kid}; kid={kid}; p={b64}",
        "v=RP1; t=api; url=http://po.radiopost.org/rp/v1",
        f"v=RP1; kid={kid}; p={b64}",
        "v=spf1 mx -all",
    ]
    cases = [{"txt": t, "result": rp.parse_txt(t)} for t in texts]
    out = header("`_radiopost` TXT records: which are read and which ignored", {"RP-1": "§7.1"})
    out["rule"] = (
        "Parsing `txt` gives `result`: the record's tags, or null when it must be ignored."
    )
    out["records"] = cases
    return out


def make_api() -> dict[str, Any]:
    nonce = stream("login nonce", 32)
    login_in = nonce + PO_RADIOPOST.encode()
    code = "7Q4F-AZ9K"
    enroll_in = code.encode() + PO_RADIOPOST.encode()
    reg_in = b"KK4ODA-1" + PO_RADIOPOST.encode()
    out = header("Post office API signatures", {"RP-5": "§3.2, §6.1, §6.4", "RP-0": "§9.2"})
    out["rule"] = (
        "Each `signature_hex` is the device key's Ed25519 signature over `signed_input_hex`."
    )
    out["login"] = {
        "base_url": PO_RADIOPOST,
        "credential": "gateway-n1call-10",
        "credential_hex": credential("gateway-n1call-10").hex(),
        "device": "device:gateway-n1call-10",
        "nonce_hex": nonce.hex(),
        "signed_input_hex": rp.signed_input(rp.CTX_LOGIN, login_in).hex(),
        "signature_hex": rp.sign(rp.CTX_LOGIN, login_in, SECRET["device:gateway-n1call-10"]).hex(),
    }
    out["enroll"] = {
        "base_url": PO_RADIOPOST,
        "address": "facundo@radiopost.org",
        "operator": "KK4ODA",
        "code": code,
        "device": "device:facundo-tablet",
        "device_public_hex": PUBLIC["device:facundo-tablet"].hex(),
        "signed_input_hex": rp.signed_input(rp.CTX_ENROLL, enroll_in).hex(),
        "signature_hex": rp.sign(rp.CTX_ENROLL, enroll_in, SECRET["device:facundo-tablet"]).hex(),
    }
    out["register_gateway"] = {
        "base_url": PO_RADIOPOST,
        "callsign": "KK4ODA-1",
        "device": "device:gateway-kk4oda-1",
        "device_public_hex": PUBLIC["device:gateway-kk4oda-1"].hex(),
        "signed_input_hex": rp.signed_input(rp.CTX_GATEWAY, reg_in).hex(),
        "signature_hex": rp.sign(rp.CTX_GATEWAY, reg_in, SECRET["device:gateway-kk4oda-1"]).hex(),
    }
    return out


# -- writing and checking ------------------------------------------------------------------------


FILES = {
    "keys.json": make_keys,
    "cbor.json": make_cbor,
    "messages.json": make_messages,
    "credentials.json": make_credentials,
    "statements.json": make_statements,
    "pdus.json": make_pdus,
    "session-01.json": make_session,
    "dns.json": make_dns,
    "api.json": make_api,
}


def dump(data: dict[str, Any]) -> str:
    return json.dumps(data, indent=2, ensure_ascii=False) + "\n"


def inexact_to_canonical(node: Any) -> Any:
    """For comparing: replace each inexact air form by what it inflates to."""
    if isinstance(node, dict):
        if node.get("exact") is False and "hex" in node:
            return {**node, "hex": rp.air_decode(bytes.fromhex(node["hex"])).hex()}
        return {k: inexact_to_canonical(v) for k, v in node.items()}
    if isinstance(node, list):
        return [inexact_to_canonical(v) for v in node]
    return node


def self_check(name: str, data: dict[str, Any]) -> None:
    """Check the vectors against the reference's own decoders and verifiers."""
    if name == "cbor.json":
        for v in data["valid"]:
            raw = bytes.fromhex(v["hex"])
            assert cbor.encode(cbor.decode(raw)) == raw
        for v in data["invalid"]:
            try:
                cbor.decode(bytes.fromhex(v["hex"]))
            except cbor.NotCanonical:
                continue
            raise AssertionError(f"accepted invalid CBOR {v['hex']}")
    if name == "messages.json":
        for m in data["messages"]:
            for air in m["air"]:
                assert rp.air_decode(bytes.fromhex(air["hex"])).hex() == m["canonical_hex"]
            assert rp.content_id(bytes.fromhex(m["canonical_hex"])).hex() == m["content_id_hex"]
        for bad in data["invalid"]:
            try:
                rp.air_decode(bytes.fromhex(bad["air_hex"]))
            except ValueError:
                continue
            raise AssertionError(f"accepted an invalid air form: {bad['why']}")
    if name == "session-01.json":
        for t in data["transmissions"]:
            raw = bytes.fromhex(t["bytes_hex"])
            pos = 0
            for p in t["pdus"]:
                n = int.from_bytes(raw[pos : pos + 2], "big")
                assert raw[pos + 2 : pos + 2 + n].hex() == p["pdu_hex"]
                cbor.decode(raw[pos + 2 : pos + 2 + n])
                pos += 2 + n
            assert pos == len(raw)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--check", action="store_true", help="fail if a file differs")
    args = parser.parse_args()
    ed25519.self_test()
    cbor.self_test()
    OUT.mkdir(exist_ok=True)
    failed = []
    for name, make in FILES.items():
        data = make()
        self_check(name, data)
        path = OUT / name
        if args.check:
            if not path.exists():
                failed.append(f"{name}: missing")
                continue
            stored = json.loads(path.read_text(encoding="utf-8"))
            if inexact_to_canonical(stored) != inexact_to_canonical(data):
                failed.append(f"{name}: differs from what make.py produces")
            else:
                self_check(name, stored)
        else:
            path.write_text(dump(data), encoding="utf-8")
            print(f"wrote vectors/{name} ({len(dump(data)):,} bytes)")
    if failed:
        print("\n".join(failed), file=sys.stderr)
        return 1
    if args.check:
        print(f"{len(FILES)} vector files match the reference")
    return 0


if __name__ == "__main__":
    sys.exit(main())
