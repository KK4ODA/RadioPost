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

from rpref import cbor, ed25519, jws, po, rp, rpmi

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "vectors"

DAY = 86400
T_KEYS_FROM = 1767225600  # 2026-01-01T00:00:00Z
T_KEYS_UNTIL = 1830297600  # 2028-01-01T00:00:00Z
T_OLD_KEY_FROM = 1735689600  # 2025-01-01T00:00:00Z
T_ISSUED = 1793491200  # 2026-11-01T00:00:00Z
T_EXPIRES = T_ISSUED + 90 * DAY
T_SESSION = 1794235200  # 2026-11-09T14:40:00Z
T_STATION = T_SESSION + 4 * 3600  # session-03: station to station, 18:40Z
T_RESUME = T_SESSION + 6 * 3600  # session-02: the first of three, 20:40Z

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
    (
        "po:radiopost.org:new",
        "radiopost.org's new key, in its info document before DNS has it",
        T_SESSION - 2 * DAY,
        T_KEYS_UNTIL + 365 * DAY,
    ),
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


ROADS = [
    ("I-35", "Round Rock to Georgetown"),
    ("I-35", "Kyle to San Marcos"),
    ("US-290", "Dripping Springs to Johnson City"),
    ("US-290", "Manor to Elgin"),
    ("US-183", "Cedar Park to Leander"),
    ("US-183", "Lockhart to Luling"),
    ("SH-71", "Bastrop to Smithville"),
    ("SH-71", "Spicewood to Marble Falls"),
    ("SH-130", "Pflugerville to Hutto"),
    ("SH-21", "Bastrop to Paige"),
    ("US-79", "Taylor to Thrall"),
    ("FM 969", "Webberville to Utley"),
    ("SH-95", "Elgin to Taylor"),
    ("FM 1626", "Buda to Manchaca"),
    ("SH-29", "Liberty Hill to Bertram"),
    ("US-281", "Marble Falls to Burnet"),
]
CONDITIONS = [
    "open, water over the shoulder in two places, drive slowly",
    "closed at the river crossing, detour signed by the county",
    "one lane open each way, trees down, crews on site",
    "open, no damage seen, fuel at the station on the east side",
    "closed, bridge being inspected, ask again after 18:00",
    "open to high-clearance vehicles only, mud on the low stretch",
]
REPORT = "Road report, Central Texas, as driven this morning. Times are local.\n\n" + (
    "".join(
        f"{k + 1:2d}. {ROADS[k % len(ROADS)][0]} {ROADS[k % len(ROADS)][1]} "
        f"({'out' if k < len(ROADS) else 'back'}, {7 + (k * 11) // 60}:{(k * 11) % 60:02d}): "
        f"{CONDITIONS[(k * 5) % len(CONDITIONS)]}.\n"
        for k in range(2 * len(ROADS))
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
    m["facundo-report"] = {
        "v": 1,
        "from": "facundo@radiopost.org",
        "to": ["eoc@radiopost.org"],
        "date": T_SESSION + 6 * 3600 - 900,
        "subject": "Road report, Central Texas",
        "body": REPORT,
    }
    m["facundo-direct"] = {
        "v": 1,
        "from": "facundo@radiopost.org",
        "to": ["ana@hampost.org"],
        "date": T_STATION - 300,
        "subject": "Direct",
        "body": "Ana, no gateway hears me from here, so I am calling you direct. All well; on the "
        "road until Friday. 73, Facundo\n",
    }
    m["facundo-to-eoc"] = {
        "v": 1,
        "from": "facundo@radiopost.org",
        "to": ["eoc@radiopost.org"],
        "date": T_STATION - 600,
        "subject": "Fuel",
        "body": "Fuel at the Bastrop station is down to a quarter tank. 73, Facundo\n",
    }
    m["ana-to-facundo"] = {
        "v": 1,
        "from": "ana@hampost.org",
        "to": ["facundo@radiopost.org"],
        "date": T_STATION - 3600,
        "subject": "Net tonight",
        "body": "Facundo, the county net meets at 20:00 local on 3.590 MHz. If you cannot make "
        "it, send me your check-in and I will read it. 73, Ana\n",
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
    "gateway-kk4oda-1": {
        "po": "po:radiopost.org",
        "device": "device:gateway-kk4oda-1",
        "serial": 1051,
        "address": "facundo@radiopost.org",
        "operator": "KK4ODA-1",
        "flags": ["send", "gateway"],
        "issued": T_ISSUED + 7200,
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
        "called-station": (called_statement(STATION_CLOCK), "device:ana", "ana"),
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
    cs_stmt, cs_sig, cs_cred = signed["called-station"]
    ahead = called_statement(T_STATION + 7200)
    ahead_sig = rp.sign(rp.CTX_SESSION, cbor.encode(ahead), SECRET["device:ana"])
    called_cases = [
        (
            "called station, checked by the caller with the nonce it sent",
            cs_stmt,
            cs_sig,
            cs_cred,
            "KK4ODA",
            T_STATION,
            None,
            [],
            STATION_NONCE,
        ),
        (
            "called station, its clock two hours ahead: the clocks are not compared",
            ahead,
            ahead_sig,
            cs_cred,
            "KK4ODA",
            T_STATION,
            None,
            [],
            STATION_NONCE,
        ),
        (
            "called station, the nonce of another session",
            cs_stmt,
            cs_sig,
            cs_cred,
            "KK4ODA",
            T_STATION,
            None,
            [],
            stream("another nonce", 8),
        ),
        (
            "called station, checked by a caller it was not made for",
            cs_stmt,
            cs_sig,
            cs_cred,
            "N3CALL",
            T_STATION,
            None,
            [],
            STATION_NONCE,
        ),
    ]
    cases = []
    for what, s, sg, c, me, now, challenge, seen, sent in [
        (*case, None) for case in cases_in
    ] + called_cases:
        result = rp.verify_statement(
            s, sg, body(c), me=me, now=now, challenge=challenge, seen=seen, sent_nonce=sent
        )
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
                "sent_nonce_hex": sent.hex() if sent else None,
                "expect": result,
            }
        )
    out = header(
        "Session statements and how they verify", {"RP-1": "§6", "RP-3": "§5.1, §5.3, §10"}
    )
    out["rule"] = (
        "A client produces exactly `statement_hex` and `signature_hex`. A verifier given a "
        "case's statement, signature, credential (already verified), its own callsign, its clock, "
        "the challenge it issued and the (address, nonce) pairs it has seen returns `expect`, "
        "checking in RP-1 §6.3's order. A case with `sent_nonce_hex` is a caller checking the "
        "station it called (RP-1 §6.5): steps 1 to 4, then the nonce in place of the clock."
    )
    out["statements"] = entries
    out["verification"] = cases
    return out


# -- sessions: shared parts ---------------------------------------------------------------------


AUTO = 1000
CLIENT_CAPS = {1: [0, 1, 2], 2: 4096}
GATEWAY_CAPS = {1: [0, 1], 2: 4096}
PIECE = 960  # data bytes in a MSG piece in session-02: every PDU fits 1 024 bytes (RP-3 §2)


def air0(name: str) -> bytes:
    return rp.air_form(encoded(name)[0], 0)


def subject40(text: str) -> str:
    """RP-3 §5.2: the subject cut to at most 40 bytes of UTF-8, on a character boundary."""
    return text.encode("utf-8")[:40].decode("utf-8", errors="ignore")


def offer(name: str) -> list[Any]:
    return [encoded(name)[1], len(air0(name)), MESSAGES[name].get("priority", 0)]


def waiting_entry(serial: int, name: str, flags: int = 0) -> list[Any]:
    fields = MESSAGES[name]
    return [
        serial,
        encoded(name)[1],
        fields["from"],
        subject40(fields["subject"]),
        len(air0(name)),
        fields.get("priority", 0),
        flags,
    ]


def hello_pdu(
    who: str,
    called: str,
    clock: int,
    nonce: bytes,
    *,
    offers: list[str],
    cursor: int,
    full_credential: bool,
    partials: list[list[Any]] | None = None,
) -> dict[int, Any]:
    """A signed HELLO (RP-3 §5.1) from the client holding credential ``who``."""
    spec = CREDENTIALS[who]
    ids = [encoded(n)[1] for n in offers]
    stmt = rp.statement(spec["address"], spec["operator"], called, ids, time=clock, nonce=nonce)
    hello: dict[int, Any] = {
        0: 1,
        1: [1, 1],
        2: 0,
        3: spec["address"],
        4: spec["operator"],
        5: called,
        6: clock,
        7: nonce,
        8: [offer(n) for n in offers],
        9: cursor,
        10: AUTO,
        12: CLIENT_CAPS,
        15: rp.sign(rp.CTX_SESSION, cbor.encode(stmt), SECRET[spec["device"]]),
    }
    if partials:
        hello[11] = partials
    if full_credential:
        hello[13] = credential(who)
    else:
        hello[14] = rp.credential_hash(credential(who))
    assert rp.statement_from_hello(hello) == stmt
    return hello


def welcome_pdu(
    session_id: bytes,
    clock: int,
    *,
    accept: list[list[Any]],
    decline: list[list[Any]],
    waiting: list[list[Any]],
    caps: dict[int, Any],
    online: bool,
    cached: bool,
) -> dict[int, Any]:
    """A WELCOME with status `ready` (RP-3 §5.2)."""
    return {
        0: 2,
        1: 1,
        2: session_id,
        3: 0,
        5: accept,
        6: decline,
        7: waiting,
        8: 0,
        9: caps,
        10: clock,
        11: online,
        12: cached,
    }


def msg_pdus(
    name: str, start: int = 0, serial: int | None = None, piece: int | None = None
) -> list[dict[int, Any]]:
    """A message's air form from ``start`` on, as MSG PDUs of ``piece`` data bytes (RP-3 §5.4)."""
    air = air0(name)
    size = piece or len(air)
    out = []
    for offset in range(start, len(air), size):
        pdu: dict[int, Any] = {
            0: 4,
            1: encoded(name)[1],
            2: offset,
            3: len(air),
            4: air[offset : offset + size],
        }
        if serial is not None:
            pdu[5] = serial
        out.append(pdu)
    return out


def receipt(names: list[str]) -> dict[int, Any]:
    return {0: 5, 1: [encoded(n)[1] for n in names]}


DONE: dict[int, Any] = {0: 7}

Transmission = tuple[str, list[tuple[str, dict[int, Any]]]]


def write_transmissions(
    seq: list[Transmission], cut: tuple[int, int, int] | None = None
) -> tuple[list[dict[str, Any]], dict[str, str]]:
    """Each transmission's frames; ``cut`` = (transmission, PDU in flight, its bytes received).

    A broken transmission was written whole; the other station received only its first
    ``received_bytes``, kept the PDUs complete in them and dropped the rest (RP-3 §7).
    """
    transmissions = []
    streams: dict[str, bytes] = {}
    for number, (who, pdus) in enumerate(seq, start=1):
        frames = [rp.frame(p) for _, p in pdus]
        data = b"".join(frames)
        streams[who] = streams.get(who, b"") + data
        entry: dict[str, Any] = {
            "number": number,
            "from": who,
            "pdus": [pdu_entry(name, p) for name, p in pdus],
            "bytes": len(data),
            "bytes_hex": data.hex(),
        }
        if cut is not None and cut[0] == number:
            _, in_flight, partly = cut
            assert 0 < partly < len(frames[in_flight])
            entry["broken"] = {
                "received_bytes": sum(len(f) for f in frames[:in_flight]) + partly,
                "complete_pdus": in_flight,
                "explain": (
                    f"The link broke {partly} bytes into PDU {in_flight + 1} "
                    f"({pdus[in_flight][0]}). The receiver keeps the {in_flight} complete PDUs "
                    "before it and drops the partial one; nothing after it arrived."
                ),
            }
        transmissions.append(entry)
    assert cut is None or cut[0] == len(seq), "the link breaks in the last transmission"
    return transmissions, {k: v.hex() for k, v in streams.items()}


def gateway_accepts(hello: dict[int, Any], who: str, me: str, now: int) -> None:
    """Check the reference's own HELLO the way a gateway would (RP-1 §5, §6.3)."""
    result, body = rp.verify_credential(credential(who), po_keys(), {}, now, ["send", "receive"])
    assert result == "ok" and body is not None
    stmt = rp.statement_from_hello(hello)
    assert rp.verify_statement(stmt, hello[15], body, me=me, now=now) == "ok"


def message_list(names: list[str], serials: dict[str, int] | None = None) -> list[dict[str, Any]]:
    out = []
    for n in names:
        entry: dict[str, Any] = {"message": n}
        if serials and n in serials:
            entry["serial"] = serials[n]
        entry["content_id_hex"] = encoded(n)[1].hex()
        entry["air_bytes"] = len(air0(n))
        out.append(entry)
    return out


# -- session-01.json -----------------------------------------------------------------------------


SESSION_ID = stream("session-01 id", 8)
WAITING = [(419, "maria-419"), (420, "eoc-420"), (421, "maria-421")]
OUTGOING = ["facundo-reply", "facundo-to-ana"]


def session_pdus() -> list[Transmission]:
    """The five transmissions of RP-3 §12, each as (who, [(name, pdu), ...])."""
    hello = hello_pdu(
        "facundo",
        "N1CALL-10",
        T_SESSION,
        NONCE,
        offers=OUTGOING,
        cursor=418,
        full_credential=False,
    )
    assert rp.statement_from_hello(hello) == statement_session()
    gateway_accepts(hello, "facundo", "N1CALL-10", T_SESSION + 41)
    welcome = welcome_pdu(
        SESSION_ID,
        T_SESSION + 41,
        accept=[[encoded(n)[1], 0] for n in OUTGOING],
        decline=[],
        waiting=[waiting_entry(s, n) for s, n in WAITING],
        caps=GATEWAY_CAPS,
        online=True,
        cached=True,
    )
    auto = [(s, n) for s, n in WAITING if len(air0(n)) <= AUTO]
    assert [s for s, _ in auto] == [419], "the example sends #419 alone unasked"
    return [
        ("client", [("HELLO", hello)]),
        (
            "gateway",
            [("WELCOME", welcome)] + [("MSG", p) for s, n in auto for p in msg_pdus(n, serial=s)],
        ),
        (
            "client",
            [("MSG", p) for n in OUTGOING for p in msg_pdus(n)]
            + [("RECEIPT", receipt(["maria-419"])), ("REQUEST", {0: 6, 1: [420]})],
        ),
        (
            "gateway",
            [("RECEIPT", receipt(OUTGOING))]
            + [("MSG", p) for p in msg_pdus("eoc-420", serial=420)]
            + [("DONE", DONE)],
        ),
        ("client", [("RECEIPT", receipt(["eoc-420"])), ("DONE", DONE)]),
    ]


def make_session() -> dict[str, Any]:
    transmissions, streams = write_transmissions(session_pdus())
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
    out["streams"] = streams
    out["after"] = {
        "gateway_reports_picked_up": [419, 420],
        "gateway_hands_in": [encoded(n)[1].hex() for n in OUTGOING],
        "still_waiting": [421],
    }
    return out


# -- session-02.json: resumed --------------------------------------------------------------------


REPORT_NAME = "facundo-report"
PHOTO_NAME = "maria-421"
PHOTO_SERIAL = 421


def resumed_sessions() -> list[dict[str, Any]]:
    """Three sessions: an upload broken and resumed, then a download broken and resumed."""
    report_pieces = msg_pdus(REPORT_NAME, piece=PIECE)
    photo_pieces = msg_pdus(PHOTO_NAME, serial=PHOTO_SERIAL, piece=PIECE)
    waiting = [waiting_entry(PHOTO_SERIAL, PHOTO_NAME)]
    request = {0: 6, 1: [PHOTO_SERIAL]}
    report_id = encoded(REPORT_NAME)[1]
    photo_id = encoded(PHOTO_NAME)[1]

    # a: the upload breaks in the third piece; the gateway keeps the first two.
    a_clock, a_gw = T_RESUME, T_RESUME + 38
    a_hello = hello_pdu(
        "facundo",
        "N1CALL-10",
        a_clock,
        stream("session-02a nonce", 8),
        offers=[REPORT_NAME],
        cursor=420,
        full_credential=False,
    )
    gateway_accepts(a_hello, "facundo", "N1CALL-10", a_gw)
    a_welcome = welcome_pdu(
        stream("session-02a id", 8),
        a_gw,
        accept=[[report_id, 0]],
        decline=[],
        waiting=waiting,
        caps=GATEWAY_CAPS,
        online=True,
        cached=True,
    )
    a_break = 2  # the link breaks in the third MSG
    a_held = report_pieces[a_break][2]
    a = [
        ("client", [("HELLO", a_hello)]),
        ("gateway", [("WELCOME", a_welcome), ("DONE", DONE)]),
        ("client", [("MSG", p) for p in report_pieces] + [("REQUEST", request)]),
    ]

    # b: the same gateway resumes the upload at the offset it holds; the download breaks.
    b_clock, b_gw = T_RESUME + 1200, T_RESUME + 1200 + 29
    b_hello = hello_pdu(
        "facundo",
        "N1CALL-10",
        b_clock,
        stream("session-02b nonce", 8),
        offers=[REPORT_NAME],
        cursor=420,
        full_credential=False,
    )
    gateway_accepts(b_hello, "facundo", "N1CALL-10", b_gw)
    b_welcome = welcome_pdu(
        stream("session-02b id", 8),
        b_gw,
        accept=[[report_id, a_held]],
        decline=[],
        waiting=waiting,
        caps=GATEWAY_CAPS,
        online=True,
        cached=True,
    )
    b_break = 5  # the link breaks in the sixth piece of the photo
    b_held = photo_pieces[b_break][2]
    b = [
        ("client", [("HELLO", b_hello)]),
        ("gateway", [("WELCOME", b_welcome), ("DONE", DONE)]),
        (
            "client",
            [("MSG", p) for p in msg_pdus(REPORT_NAME, a_held, piece=PIECE)]
            + [("REQUEST", request)],
        ),
        (
            "gateway",
            [("RECEIPT", receipt([REPORT_NAME]))]
            + [("MSG", p) for p in photo_pieces]
            + [("DONE", DONE)],
        ),
    ]

    # c: another gateway, the next morning, sends the rest of the photo unasked.
    c_clock, c_gw = T_RESUME + 64800, T_RESUME + 64800 + 52
    c_hello = hello_pdu(
        "facundo",
        "N2CALL-5",
        c_clock,
        stream("session-02c nonce", 8),
        offers=[],
        cursor=420,
        full_credential=True,
        partials=[[photo_id, b_held]],
    )
    gateway_accepts(c_hello, "facundo", "N2CALL-5", c_gw)
    c_welcome = welcome_pdu(
        stream("session-02c id", 8),
        c_gw,
        accept=[],
        decline=[],
        waiting=waiting,
        caps=GATEWAY_CAPS,
        online=True,
        cached=True,
    )
    c = [
        ("client", [("HELLO", c_hello)]),
        (
            "gateway",
            [("WELCOME", c_welcome)]
            + [("MSG", p) for p in msg_pdus(PHOTO_NAME, b_held, PHOTO_SERIAL, PIECE)]
            + [("DONE", DONE)],
        ),
        ("client", [("RECEIPT", receipt([PHOTO_NAME])), ("DONE", DONE)]),
    ]

    def pieces_of(name: str, held: int) -> dict[str, Any]:
        return {"message": name, "content_id_hex": encoded(name)[1].hex(), "bytes_held": held}

    sessions = []
    for label, seq, cut, gateway, clocks, story, after in [
        (
            "a",
            a,
            (3, a_break, 500),
            "N1CALL-10, holding the client's credential, online",
            (a_clock, a_gw),
            "Facundo offers his road report (4 pieces) and asks for the photo, #421. The link "
            "breaks in his third piece: the REQUEST never arrives.",
            {
                "gateway_holds_for_24_hours": [pieces_of(REPORT_NAME, a_held)],
                "client_outbox": [REPORT_NAME],
                "still_waiting": [PHOTO_SERIAL],
            },
        ),
        (
            "b",
            b,
            (4, b_break + 1, 500),
            "N1CALL-10 again, twenty minutes later; it still holds the report's first pieces",
            (b_clock, b_gw),
            f"The gateway accepts the report at offset {a_held}, the bytes it holds, and Facundo "
            "sends the rest. The gateway receipts it and sends the photo he asked for; the link "
            "breaks in the photo's sixth piece. The report is delivered; the photo is not.",
            {
                "gateway_hands_in": [report_id.hex()],
                "client_holds": [pieces_of(PHOTO_NAME, b_held)],
                "still_waiting": [PHOTO_SERIAL],
            },
        ),
        (
            "c",
            c,
            None,
            "N2CALL-5, a gateway Facundo has not called before, online, the next morning",
            (c_clock, c_gw),
            f"Facundo lists the photo under `partials` with the {b_held} bytes he holds and sends "
            "his credential in full (a first visit). The photo counts as asked for: the gateway "
            "sends the rest of it after WELCOME, unasked though it is over his `auto` (RP-3 §7), "
            "from the post office's air form, the same bytes N1CALL-10 sent.",
            {"gateway_reports_picked_up": [PHOTO_SERIAL], "still_waiting": []},
        ),
    ]:
        transmissions, streams = write_transmissions(seq, cut)
        hello, welcome = seq[0][1][0][1], seq[1][1][0][1]
        sessions.append(
            {
                "session": label,
                "story": story,
                "gateway": gateway,
                "client_clock": clocks[0],
                "gateway_clock": clocks[1],
                "session_id_hex": welcome[2].hex(),
                "nonce_hex": hello[7].hex(),
                "transmissions": transmissions,
                "streams": streams,
                "after": after,
            }
        )
    return sessions


def make_resumed() -> dict[str, Any]:
    out = header("Resumed sessions, byte for byte", {"RP-3": "§5, §7", "RP-1": "§6"})
    out["story"] = (
        "Three sessions of Facundo's (facundo@radiopost.org, KK4ODA). In the first, the link "
        "breaks while he is sending his road report; in the second, through the same gateway, "
        "he finishes it, and the link breaks while the gateway sends him the photo; in the "
        "third, through another gateway, he gets the rest of the photo."
    )
    out["rule"] = (
        "As in session-01.json, for each session. A transmission with `broken` was written whole "
        "but received only up to `received_bytes`: the receiver keeps the complete PDUs before "
        "the break and drops the partial one. Given what each station holds after a session "
        "(`after`), the next session's bytes follow. Pieces carry at most 960 data bytes, an "
        "input like the clocks: any piece size is correct (RP-3 §5.4). The messages travel "
        "uncompressed, as in session-01.json; a resumed air form must be the same bytes as the "
        "one begun (RP-3 §7)."
    )
    out["context"] = {
        "client_credential": "facundo (credentials.json)",
        "piece_data_bytes": PIECE,
        "auto": AUTO,
        "cursor": 420,
        "messages": message_list([REPORT_NAME, PHOTO_NAME], {PHOTO_NAME: PHOTO_SERIAL}),
    }
    out["sessions"] = resumed_sessions()
    return out


# -- session-03.json: station to station ---------------------------------------------------------


STATION_NONCE = stream("session-03 nonce", 8)
STATION_ID = stream("session-03 id", 8)
STATION_CLOCK = T_STATION + 47  # Ana's clock
STATION_OFFERS = ["facundo-direct", "facundo-to-eoc"]
STATION_WAITING = [(1, "ana-to-facundo")]


def called_statement(clock: int) -> dict[int, Any]:
    """Ana's statement as the station called (RP-1 §6.5), with Facundo's nonce."""
    ids = [encoded(n)[1] for _, n in STATION_WAITING]
    return rp.statement("ana@hampost.org", "N0CALL", "KK4ODA", ids, time=clock, nonce=STATION_NONCE)


def station_pdus() -> list[Transmission]:
    """Facundo calls Ana's client directly (RP-3 §10)."""
    hello = hello_pdu(
        "facundo",
        "N0CALL",
        T_STATION,
        STATION_NONCE,
        offers=STATION_OFFERS,
        cursor=0,
        full_credential=True,
    )
    gateway_accepts(hello, "facundo", "N0CALL", STATION_CLOCK)
    stmt = called_statement(STATION_CLOCK)
    welcome = welcome_pdu(
        STATION_ID,
        STATION_CLOCK,
        accept=[[encoded("facundo-direct")[1], 0]],
        decline=[[encoded("facundo-to-eoc")[1], rp.ERRORS["refused-by-policy"]]],
        waiting=[waiting_entry(s, n) for s, n in STATION_WAITING],
        caps=CLIENT_CAPS,
        online=False,
        cached=False,
    )
    welcome[13] = credential("ana")
    welcome[15] = rp.sign(rp.CTX_SESSION, cbor.encode(stmt), SECRET["device:ana"])
    # The caller checks the station it reached before taking anything from it.
    assert rp.statement_from_welcome(hello, welcome) == stmt
    result, body = rp.verify_credential(welcome[13], po_keys(), {}, T_STATION, ["send", "receive"])
    assert result == "ok" and body is not None
    assert (
        rp.verify_statement(
            stmt, welcome[15], body, me="KK4ODA", now=T_STATION, sent_nonce=STATION_NONCE
        )
        == "ok"
    )
    auto = [(s, n) for s, n in STATION_WAITING if len(air0(n)) <= AUTO]
    return [
        ("caller", [("HELLO", hello)]),
        (
            "called",
            [("WELCOME", welcome)]
            + [("MSG", p) for s, n in auto for p in msg_pdus(n, serial=s)]
            + [("DONE", DONE)],
        ),
        (
            "caller",
            [("MSG", p) for p in msg_pdus("facundo-direct")]
            + [("RECEIPT", receipt([n for _, n in auto])), ("DONE", DONE)],
        ),
        ("called", [("RECEIPT", receipt(["facundo-direct"]))]),
    ]


def make_station() -> dict[str, Any]:
    transmissions, streams = write_transmissions(station_pdus())
    out = header(
        "A station-to-station session, byte for byte", {"RP-3": "§5, §10", "RP-1": "§6.3, §6.5"}
    )
    out["story"] = (
        "No gateway hears Facundo (facundo@radiopost.org, KK4ODA), so he calls Ana's client "
        "(ana@hampost.org, N0CALL) directly. Ana's client checks his credential and statement "
        "with the post office keys it holds, takes the message addressed to her and refuses the "
        "one for eoc@radiopost.org (`refused-by-policy`), which stays in his outbox. It proves "
        "itself in its WELCOME with its credential and a statement over his nonce, and sends him "
        "the one message Ana wrote him. Facundo hangs up after transmission 4."
    )
    out["rule"] = (
        "As in session-01.json. Before taking any message from the station it called, the caller "
        "rebuilds that station's statement from HELLO and WELCOME (RP-1 §6.5: address and "
        "operator from WELCOME's credential, `gateway` = HELLO `operator`, `time` = WELCOME "
        "`time`, `nonce` = HELLO `nonce`, the content IDs of `waiting`) and verifies it against "
        "the nonce it sent, not its clock."
    )
    out["context"] = {
        "caller_credential": "facundo (credentials.json)",
        "called_credential": "ana (credentials.json)",
        "both_hold_keys_of": ["radiopost.org", "hampost.org"],
        "called_statement": "called-station (statements.json)",
        "outgoing": message_list(STATION_OFFERS),
        "waiting": message_list(
            [n for _, n in STATION_WAITING], {n: s for s, n in STATION_WAITING}
        ),
        "session_id_hex": STATION_ID.hex(),
        "caller_clock": T_STATION,
        "called_clock": STATION_CLOCK,
    }
    out["transmissions"] = transmissions
    out["streams"] = streams
    out["after"] = {
        "caller_holds_with_proof": [
            {
                "message": "ana-to-facundo",
                "proof": "Ana's credential, her statement (`called-station`) and its signature",
            }
        ],
        "called_holds_with_proof": [
            {
                "message": "facundo-direct",
                "proof": "Facundo's credential, his statement (from HELLO) and its signature",
            }
        ],
        "caller_outbox": ["facundo-to-eoc"],
    }
    return out


# -- other PDUs ----------------------------------------------------------------------------------


def make_pdus() -> dict[str, Any]:
    session = {name: pdu for _, pdus in session_pdus() for name, pdu in pdus}
    station = {name: pdu for _, pdus in station_pdus() for name, pdu in pdus}
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
        ("WELCOME from a station called directly, with its proof", station["WELCOME"]),
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


# -- rpmi-lines.json and rpmi-conversations.json ------------------------------------------------


RPMI_MODEM = "rpref simulated modem 1"
RPMI_FRAME = 128  # the most bytes one `received` carries on the simulated modem


def rpmi_adapter(world: rpmi.World, station: str, prefix: str, *, shared: bool) -> rpmi.Adapter:
    caps = {
        "bandwidths": [500, 2300],
        "max_links": 1,
        "delivery": True,
        "broadcast": True,
        "quality": False,
        "busy": True,
        "regulatory": False,
        "max_queue": 65536,
        "shared": shared,
    }
    return rpmi.Adapter(
        world, station, modem=RPMI_MODEM, caps=caps, prefix=prefix, frame_bytes=RPMI_FRAME
    )


def rpmi_start(
    adapter: rpmi.Adapter, host: str, app: str, callsigns: list[str], *, listen: bool
) -> None:
    """A host opens its connection, says hello, names its callsigns and maybe listens."""
    adapter.open(host)
    adapter.request(host, {"op": "hello", "id": 1, "version": [1, 1], "app": app})
    adapter.request(host, {"op": "callsigns", "id": 2, "callsigns": callsigns})
    if listen:
        adapter.request(host, {"op": "listen", "id": 3, "on": True})


def link_of(adapter: rpmi.Adapter, host: str) -> str:
    return next(e.link for e in adapter.ends if e.host == host)


def rpmi_session() -> list[dict[str, Any]]:
    """Session-01 (RP-3 §12) carried over RPMI on both stations."""
    w = rpmi.World()
    client = rpmi_adapter(w, "KK4ODA", "c", shared=False)
    gateway = rpmi_adapter(w, "N1CALL-10", "g", shared=False)
    rpmi_start(gateway, "gateway", "RadioPost gateway 0.1.0", ["N1CALL-10"], listen=True)
    rpmi_start(client, "client", "RadioPost client 0.1.0", ["KK4ODA"], listen=False)
    client.request("client", {"op": "connect", "id": 3, "remote": "N1CALL-10", "bandwidth": 2300})
    adapters = {"client": client, "gateway": gateway}
    links = {who: link_of(a, who) for who, a in adapters.items()}
    for n, (who, pdus) in enumerate(session_pdus(), start=1):
        w.note(f"RP-3 transmission {n}: {', '.join(name for name, _ in pdus)}")
        data = b"".join(rp.frame(p) for _, p in pdus)
        adapters[who].request(
            who,
            {"op": "send", "link": links[who], "data": rpmi.encode_data(data), "ref": f"t{n}"},
        )
    w.note("Both stations have sent DONE and hold receipts: the caller hangs up (RP-3 §8).")
    client.request("client", {"op": "disconnect", "id": 4, "link": links["client"]})
    return w.log


def rpmi_errors() -> list[dict[str, Any]]:
    """Every error a host meets on one station, and the call that fails each way."""
    w = rpmi.World()
    client = rpmi_adapter(w, "KK4ODA", "c", shared=False)
    gateway = rpmi_adapter(w, "N1CALL-10", "g", shared=False)
    rpmi_start(gateway, "gateway", "RadioPost gateway 0.1.0", ["N1CALL-10"], listen=True)
    app = "RadioPost client 0.1.0"
    client.open("client")
    client.request("client", {"op": "connect", "id": 1, "remote": "N1CALL-10"})
    client.request("client", {"op": "hello", "id": 2, "version": [2, 3], "app": app})
    client.open("client")
    for request in [
        {"op": "hello", "id": 3, "version": [1, 2], "app": app},
        {"op": "listen", "id": 4, "on": True},
        {"op": "connect", "id": 5, "remote": "N1CALL-10"},
        {"op": "callsigns", "id": 6, "callsigns": ["KK4ODA"]},
        {"op": "connect", "id": 7, "remote": "N1CALL-10", "as": "W1AW"},
        {"op": "connect", "id": 8, "remote": "N1CALL-10", "bandwidth": 2750},
        {"op": "tune", "id": 9},
        {"op": "send", "id": 10, "link": "c7", "data": rpmi.encode_data(b"\x00")},
        {"op": "abort", "id": 11},
        {"op": "connect", "id": 12, "remote": "N9ZZZ"},
        {"op": "abort", "id": 13},
        {"op": "connect", "id": 14, "remote": "N9ZZZ"},
    ]:
        client.request("client", request)
    w.note("Nobody answers N9ZZZ: the modem gives up on the call.")
    w.give_up()
    client.request("client", {"op": "connect", "id": 15, "remote": "N1CALL-10"})
    client.request("client", {"op": "connect", "id": 16, "remote": "N2CALL-5"})
    client.request("client", {"op": "abort", "id": 17, "link": link_of(client, "client")})
    return w.log


def rpmi_shared() -> list[dict[str, Any]]:
    """A gateway's modem shared by RadioPost and another program (RP-4 §1.1)."""
    w = rpmi.World()
    gateway = rpmi_adapter(w, "N1CALL", "g", shared=True)
    caller = rpmi_adapter(w, "N3CALL", "c", shared=False)
    rpmi_start(gateway, "radiopost", "RadioPost gateway 0.1.0", ["N1CALL-10"], listen=True)
    gateway.open("other")
    gateway.request("other", {"op": "hello", "id": 1, "version": [1, 1], "app": "Other mail 2.3"})
    gateway.request("other", {"op": "callsigns", "id": 2, "callsigns": ["N1CALL-10"]})
    gateway.request("other", {"op": "callsigns", "id": 3, "callsigns": ["N1CALL-1"]})
    rpmi_start(caller, "client", "Other mail client 2.3", ["N3CALL"], listen=False)
    caller.request("client", {"op": "connect", "id": 3, "remote": "N1CALL-1"})
    w.note(
        "N1CALL-1 is the other program's, and it is not listening; RadioPost listens, but only "
        "for its own N1CALL-10. Nobody answers."
    )
    w.give_up()
    gateway.request("other", {"op": "listen", "id": 4, "on": True})
    caller.request("client", {"op": "connect", "id": 4, "remote": "N1CALL-1"})
    other_link = link_of(gateway, "other")
    gateway.request("radiopost", {"op": "status", "id": 4})
    gateway.request(
        "radiopost",
        {"op": "send", "id": 5, "link": other_link, "data": rpmi.encode_data(b"not mine")},
    )
    gateway.request("radiopost", {"op": "connect", "id": 6, "remote": "KK4ODA"})
    caller.request("client", {"op": "disconnect", "id": 5, "link": link_of(caller, "client")})
    gateway.close("other")
    caller.request("client", {"op": "connect", "id": 6, "remote": "N1CALL-1"})
    w.note("The other program has gone, and N1CALL-1 with it: nobody answers.")
    w.give_up()
    return w.log


def rpmi_broadcast() -> list[dict[str, Any]]:
    w = rpmi.World()
    client = rpmi_adapter(w, "KK4ODA", "c", shared=False)
    gateway = rpmi_adapter(w, "N1CALL-10", "g", shared=False)
    rpmi_start(gateway, "gateway", "RadioPost gateway 0.1.0", ["N1CALL-10"], listen=True)
    rpmi_start(client, "client", "RadioPost client 0.1.0", ["KK4ODA"], listen=False)
    client.request(
        "client",
        {
            "op": "broadcast",
            "id": 3,
            "data": rpmi.encode_data(b"KK4ODA: on the road near Austin, all well"),
            "ref": "b1",
        },
    )
    return w.log


def make_rpmi_conversations() -> dict[str, Any]:
    out = header("Conversations between hosts and adapters", {"RP-4": "§1–§7"})
    out["rule"] = (
        "Each step is one line on one station's connection between a host and its adapter: "
        "`to: adapter` from the host, `to: host` from the adapter, or a connection opening or "
        "closing. Messages compare as JSON values, not as text; `text` is for people and need not "
        "match. An adapter given the host lines answers with the adapter lines, except where the "
        "modem decides: how `received` splits the bytes (here at most 128 a line) and the time "
        "between events. A host given the adapter lines sends the host lines. Notes are for "
        "people."
    )
    out["modem"] = {
        "name": RPMI_MODEM,
        "received_bytes_at_most": RPMI_FRAME,
        "link_ids": "each adapter's own: a letter for the station and a count from 1",
    }
    out["conversations"] = [
        {
            "name": "session-01 over RPMI",
            "story": (
                "The client KK4ODA calls the gateway N1CALL-10 and the two carry session-01.json "
                "between them: one `send` a transmission, with a `ref`. Each `received` carries "
                "part of the other station's stream; a host joins them, since `received` keeps no "
                "`send` boundaries. The bytes received are session-01.json's streams."
            ),
            "steps": rpmi_session(),
        },
        {
            "name": "errors",
            "story": (
                "A client meets every error in turn: a request before `hello`; no version in "
                "common (the adapter closes, and the host connects again); `listen` and `connect` "
                "before `callsigns`; an `as` that is not its own; a bandwidth the modem lacks; an "
                "unknown op; a link that is not its; an `abort` with nothing to abort. Then two "
                "calls to a station nobody answers for, one aborted and one given up, a call that "
                "connects, a second call while that link is up, and the link aborted."
            ),
            "steps": rpmi_errors(),
        },
        {
            "name": "a shared modem",
            "story": (
                "The gateway station N1CALL shares its modem between RadioPost (N1CALL-10) and "
                "another mail program (N1CALL-1). The other program cannot take N1CALL-10, and "
                "N1CALL-1 is a different callsign. A call to N1CALL-1 is answered only once that "
                "program listens. While its link is up, RadioPost hears only `busy`: its status "
                "lists no links, the other program's link is `not-connected` to it, and its own "
                "call is refused `busy`. When the other program's connection closes, its "
                "callsign goes with it."
            ),
            "steps": rpmi_shared(),
        },
        {
            "name": "a broadcast",
            "story": "The client sends a broadcast; the gateway's host hears it with its source.",
            "steps": rpmi_broadcast(),
        },
    ]
    return out


RPMI_LINES = [
    ('{ "op" : "status", "id" : "s1", "colour" : "blue" }', "a string id, an unknown member"),
    ("hello", "not JSON"),
    ('["op", "status"]', "not a JSON object"),
    ('{"id": 4}', "no op"),
    ('{"op": 5, "id": 5}', "an op that is not a string"),
    ('{"op": "status", "id": 1.5}', "an id that is neither a string nor an integer"),
    ('{"op": "status", "id": true}', "an id that is a boolean"),
    ('{"op": "send", "id": 8, "link": "c1", "data": "-_8="}', "base64url, not standard base64"),
    ('{"op": "send", "id": 9, "link": "c1", "data": "AQ"}', "base64 without its padding"),
    ('{"op": "tune", "id": 10}', "an op this adapter does not know"),
    ('{"op": "send", "id": 11, "link": "c1", "data": "AQ=="}', "good base64, but no such link"),
]


def make_rpmi_lines() -> dict[str, Any]:
    w = rpmi.World()
    adapter = rpmi_adapter(w, "KK4ODA", "c", shared=False)
    rpmi_start(adapter, "client", "RadioPost client 0.1.0", ["KK4ODA"], listen=False)
    cases = []
    for line, what in RPMI_LINES:
        before = len(w.log)
        adapter.request_line("client", line)
        answers = [e["message"] for e in w.log[before + 1 :]]
        cases.append({"what": what, "line": line, "answers": answers})
    out = header("RPMI lines: read, or answered with an error", {"RP-4": "§2, §4, §7"})
    out["rule"] = (
        "An adapter that has said `hello` to a host with the callsign KK4ODA, and has no links, "
        "reads each `line` (one line of JSON Lines, without its LF) and answers with `answers`. "
        "Messages compare as JSON values; `text` need not match."
    )
    out["lines"] = cases
    return out


# -- jws.json ------------------------------------------------------------------------------------


def key_entry(label: str) -> dict[str, Any]:
    """A key as an info document lists it (RP-1 §7.2)."""
    _, _, start, end = next(k for k in KEY_LABELS if k[0] == label)
    return {
        "kid": rp.kid(PUBLIC[label]).hex(),
        "public": jws.b64u(PUBLIC[label]),
        "not_before": start,
        "not_after": end,
    }


GATEWAY_RECORD = {
    "callsign": "KK4ODA-1",
    "operator": "KK4ODA",
    "grid": "EM73",
    "place": "Atlanta, GA",
    "channels": [
        {"dial_hz": 3590000, "mode": "USB", "modem": "aether", "bandwidth_hz": 2300},
        {"dial_hz": 7101000, "mode": "USB", "modem": "aether", "bandwidth_hz": 2300},
    ],
    "hours": "always",
    "internet": True,
    "copies": {"users": 50, "days": 7, "max_air_bytes": 16384},
}
T_DOCUMENTS = T_SESSION - DAY


def documents() -> list[tuple[str, str, dict[str, Any], str]]:
    """RP-5 §4's documents: (name, path, payload, the key that signs it)."""
    info = {
        "version": 1,
        "domains": ["radiopost.org"],
        "operator": "KK4ODA",
        "contact": "postmaster@radiopost.org",
        "country": "US",
        "keys": [key_entry("po:radiopost.org"), key_entry("po:radiopost.org:new")],
        "limits": {"max_air_bytes": 65536, "max_email_bytes": 10485760},
        "gateways": "any",
        "mail": {"jmap": "https://po.radiopost.org/jmap/session", "imap": "po.radiopost.org:993"},
        "updated": T_DOCUMENTS,
    }
    revocations = {
        "issued": T_DOCUMENTS,
        "next": T_DOCUMENTS + 21600,
        "serials": [1090],
        "kids": [],
    }
    hampost = PUBLIC["po:hampost.org"]
    post_offices = {
        "issued": T_DOCUMENTS,
        "post_offices": [
            {
                "domain": "hampost.org",
                "api": PO_HAMPOST,
                "keys": [{"kid": rp.kid(hampost).hex(), "public": jws.b64u(hampost)}],
            }
        ],
    }
    gateways = {
        "issued": T_DOCUMENTS,
        "gateways": [{**GATEWAY_RECORD, "updated": T_ISSUED + 7200}],
    }
    return [
        ("info", "/info", info, "po:radiopost.org"),
        ("revocations", "/revocations", revocations, "po:radiopost.org:new"),
        ("post-offices", "/post-offices", post_offices, "po:radiopost.org"),
        ("gateways", "/gateways", gateways, "po:radiopost.org"),
    ]


def signed_documents() -> dict[str, str]:
    return {
        name: jws.sign(jws.compact(payload), SECRET[key], PUBLIC[key])
        for name, _, payload, key in documents()
    }


def make_jws() -> dict[str, Any]:
    signed = signed_documents()
    dns_label = "po:radiopost.org"
    dns = {rp.kid(PUBLIC[dns_label]).hex(): PUBLIC[dns_label]}
    entries = []
    for name, path, payload, key in documents():
        token = signed[name]
        entries.append(
            {
                "name": name,
                "path": path,
                "signed_by": key,
                "payload": payload,
                "header_b64url": token.split(".")[0],
                "payload_b64url": token.split(".")[1],
                "jws": token,
            }
        )
    retired = "po:radiopost.org:2025"
    forged_info = jws.sign(
        jws.compact({"version": 1, "domains": ["radiopost.org"], "keys": [key_entry(retired)]}),
        SECRET[retired],
        PUBLIC[retired],
    )
    by_retired = jws.sign(
        jws.compact(
            {"issued": T_DOCUMENTS, "next": T_DOCUMENTS + 21600, "serials": [], "kids": []}
        ),
        SECRET[retired],
        PUBLIC[retired],
    )
    head, body, sig = signed["post-offices"].split(".")
    changed = dict(documents()[2][2])
    changed["post_offices"] = []
    altered = f"{head}.{jws.b64u(jws.compact(changed))}.{sig}"
    none_header = jws.b64u(jws.compact({"alg": "none", "kid": next(iter(dns))}))
    unsigned = f"{none_header}.{signed['gateways'].split('.')[1]}."
    cases_in = [
        ("info, signed by the key in DNS", signed["info"], None),
        (
            "revocations, signed by the new key that the info document lists",
            signed["revocations"],
            signed["info"],
        ),
        ("revocations, checked with the DNS key alone", signed["revocations"], None),
        ("post-offices, its payload changed after signing", altered, signed["info"]),
        ("gateways, `alg` none and no signature", unsigned, signed["info"]),
        (
            "an info document signed by a key DNS does not list, naming itself; then a "
            "document signed by that key",
            by_retired,
            forged_info,
        ),
        ("two parts, not three", ".".join(signed["info"].split(".")[:2]), None),
        ("a header that is not JSON", jws.b64u(b"alg=EdDSA") + "." + body + "." + sig, None),
    ]
    cases = []
    for what, token, info in cases_in:
        result, _ = jws.verify(token, jws.accepted_keys(dns, info))
        cases.append(
            {
                "what": what,
                "jws": token,
                "dns_kids": list(dns),
                "info_jws": info,
                "expect": result,
            }
        )
    out = header(
        "Signed documents (JWS) and how they verify",
        {"RP-0": "§5", "RP-1": "§7.2", "RP-5": "§1, §4"},
    )
    out["rule"] = (
        "From `header_b64url` and `payload_b64url` (the exact bytes signed), a signer with the "
        "key named produces exactly `jws`: Ed25519 over the ASCII of header, a dot and payload, "
        "with no RadioPost context (RP-0 §5). Another JSON spelling of the same payload is "
        "another valid JWS. A verifier holds the keys `dns_kids` names (keys.json) and, when "
        "`info_jws` is given, adds the keys an info document signed by one of them lists "
        "(RP-1 §7.2); it then returns `expect`."
    )
    out["documents"] = entries
    out["verification"] = cases
    return out


# -- api-exchanges.json --------------------------------------------------------------------------


class Exchanges:
    """Requests to a reference post office, and its answers, in order."""

    def __init__(self, office: po.PostOffice) -> None:
        self.po = office
        self.log: list[dict[str, Any]] = []

    def note(self, text: str) -> None:
        self.log.append({"note": text})

    def call(
        self,
        at: int,
        who: str,
        method: str,
        path: str,
        body: dict[str, Any] | None = None,
        token: str | None = None,
    ) -> dict[str, Any]:
        self.po.now = at
        status, answer = self.po.handle(method, path, body, token)
        request: dict[str, Any] = {"method": method, "path": path}
        if token is not None:
            request["authorization"] = f"Bearer {token}"
        if body is not None:
            request["body"] = body
        response: dict[str, Any] = {"status": status}
        if answer is not None:
            response["body"] = answer
        self.log.append({"at": at, "who": who, "request": request, "response": response})
        return answer or {}

    def login(self, at: int, who: str, cred: str) -> str:
        nonce = self.call(at, who, "POST", "/auth/challenge")["nonce"]
        raw = jws.unb64u(nonce)
        assert raw is not None
        signature = rp.sign(
            rp.CTX_LOGIN, raw + PO_RADIOPOST.encode(), SECRET[CREDENTIALS[cred]["device"]]
        )
        body = {
            "credential": jws.b64u(credential(cred)),
            "nonce": nonce,
            "signature": jws.b64u(signature),
        }
        return self.call(at, who, "POST", "/auth/token", body)["token"]


def post_office() -> po.PostOffice:
    """radiopost.org as the vectors find it: Facundo's account, his tablet's enrollment code."""
    office = po.PostOffice(
        domains=["radiopost.org"],
        base_url=PO_RADIOPOST,
        secret=SECRET["po:radiopost.org"],
        public=PUBLIC["po:radiopost.org"],
        known=po_keys(),
        draw=stream,
    )
    office.next_serial = 1050
    office.codes["7Q4F-AZ9K"] = {
        "address": "facundo@radiopost.org",
        "operator": "KK4ODA",
        "flags": rp.FLAGS["receive"],
    }
    return office


def deliver_waiting(office: po.PostOffice, ex: Exchanges | None) -> None:
    """Mail for Facundo arrives (RP-6), each message at its date."""
    for serial, name in WAITING:
        fields = MESSAGES[name]
        office.now = fields["date"]
        if ex is not None:
            ex.note(f"Mail arrives for facundo@radiopost.org: #{serial}, {fields['subject']!r}.")
        office.deliver(
            "facundo@radiopost.org",
            serial,
            air0(name),
            sender=fields["from"],
            subject=fields["subject"],
            priority=fields.get("priority", 0),
        )


def session_request(hello: dict[int, Any], started: int, who: str = "facundo") -> dict[str, Any]:
    """RP-5 §5.2's body, from a HELLO the gateway received at `started`."""
    stmt = rp.statement_from_hello(hello)
    return {
        "statement": jws.b64u(cbor.encode(stmt)),
        "signature": jws.b64u(hello[15]),
        "credential": jws.b64u(credential(who)),
        "started": started,
        "cursor": hello[9],
    }


def handed_in(names: list[str]) -> dict[str, Any]:
    return {
        "messages": [{"id": rp.id_text(encoded(n)[1]), "air": jws.b64u(air0(n))} for n in names]
    }


def api_story() -> list[dict[str, Any]]:
    """Enrollment, a gateway registered, copies placed ahead, session-01 at the post office."""
    office = post_office()
    ex = Exchanges(office)
    enroll_code = "7Q4F-AZ9K"
    tablet = "device:facundo-tablet"
    signed = enroll_code.encode() + PO_RADIOPOST.encode()
    ex.note("Facundo enrolls his tablet with the code the post office gave him.")
    ex.call(
        T_ISSUED,
        "Facundo's tablet",
        "POST",
        "/enroll",
        {
            "address": "facundo@radiopost.org",
            "operator": "KK4ODA",
            "code": enroll_code,
            "device": jws.b64u(PUBLIC[tablet]),
            "signature": jws.b64u(rp.sign(rp.CTX_ENROLL, signed, SECRET[tablet])),
        },
    )
    at = T_ISSUED + 7200
    ex.note("From his laptop he names his usual gateways, and registers his own, KK4ODA-1.")
    user = ex.login(at - 60, "Facundo's laptop", "facundo")
    settings = {
        "usual_gateways": ["KK4ODA-1", "N2CALL-5"],
        "auto_bytes": AUTO,
        "radio_skips_read": True,
    }
    ex.call(at - 30, "Facundo's laptop", "PUT", "/settings", settings, user)
    gateway = "device:gateway-kk4oda-1"
    reg = GATEWAY_RECORD["callsign"].encode() + PO_RADIOPOST.encode()
    ex.call(
        at,
        "Facundo's laptop",
        "POST",
        "/gateways",
        {
            "record": GATEWAY_RECORD,
            "device": jws.b64u(PUBLIC[gateway]),
            "signature": jws.b64u(rp.sign(rp.CTX_GATEWAY, reg, SECRET[gateway])),
        },
        user,
    )
    deliver_waiting(office, ex)
    ex.note("KK4ODA-1 fetches the copies placed ahead for it, and acknowledges them.")
    kk = ex.login(T_SESSION - 3600, "KK4ODA-1", "gateway-kk4oda-1")
    copies = ex.call(T_SESSION - 3590, "KK4ODA-1", "GET", "/copies?after=&wait=60", None, kk)
    ex.call(T_SESSION - 3589, "KK4ODA-1", "POST", "/copies/ack", {"cursor": copies["cursor"]}, kk)
    ex.note("Facundo calls N1CALL-10 (session-01.json). The gateway carries his session here.")
    gw = ex.login(T_SESSION - 600, "N1CALL-10", "gateway-n1call-10")
    hello = session_pdus()[0][1][0][1]
    opened = ex.call(
        T_SESSION + 40, "N1CALL-10", "POST", "/sessions", session_request(hello, T_SESSION + 40), gw
    )
    sid = opened["session"]
    ex.call(T_SESSION + 40, "N1CALL-10", "GET", f"/sessions/{sid}/messages/419", None, gw)
    ex.note("Transmission 3 brings Facundo's two messages and his receipt for #419.")
    ex.call(
        T_SESSION + 95, "N1CALL-10", "POST", f"/sessions/{sid}/messages", handed_in(OUTGOING), gw
    )
    ex.call(T_SESSION + 95, "N1CALL-10", "POST", f"/sessions/{sid}/pickups", {"serials": [419]}, gw)
    ex.call(T_SESSION + 95, "N1CALL-10", "GET", f"/sessions/{sid}/messages/420", None, gw)
    ex.note("Transmission 5 brings his receipt for #420; then he hangs up.")
    ex.call(
        T_SESSION + 160, "N1CALL-10", "POST", f"/sessions/{sid}/pickups", {"serials": [420]}, gw
    )
    ex.call(T_SESSION + 161, "N1CALL-10", "DELETE", f"/sessions/{sid}", None, gw)
    ex.note(
        "KK4ODA-1 polls again. Its token ran out at the end of its hour: the post office answers "
        "401, and the gateway logs in again. The new poll drops the copies of the two messages "
        "picked up."
    )
    poll = f"/copies?after={copies['cursor']}&wait=60"
    ex.call(T_SESSION + 300, "KK4ODA-1", "GET", poll, None, kk)
    kk = ex.login(T_SESSION + 301, "KK4ODA-1", "gateway-kk4oda-1")
    ex.call(T_SESSION + 302, "KK4ODA-1", "GET", poll, None, kk)
    return ex.log


def case_office() -> tuple[po.PostOffice, Exchanges, dict[str, str]]:
    """A post office with Facundo's mail waiting, and tokens for N1CALL-10, KK4ODA-1, Facundo."""
    office = post_office()
    deliver_waiting(office, None)
    quiet = Exchanges(office)
    tokens = {
        "N1CALL-10": quiet.login(T_SESSION - 600, "N1CALL-10", "gateway-n1call-10"),
        "KK4ODA-1": quiet.login(T_SESSION - 600, "KK4ODA-1", "gateway-kk4oda-1"),
        "facundo": quiet.login(T_SESSION - 600, "Facundo's laptop", "facundo"),
    }
    return office, Exchanges(office), tokens


def api_cases() -> list[dict[str, Any]]:
    hello = session_pdus()[0][1][0][1]
    good = session_request(hello, T_SESSION + 40)
    fewer = dict(good)
    stmt = rp.statement_from_hello(hello)
    stmt[6] = stmt[6][:1]
    fewer["statement"] = jws.b64u(cbor.encode(stmt))
    cases = []

    def case(what: str, steps: list[tuple[Any, ...]], revoked: list[int] | None = None) -> None:
        office, ex, tokens = case_office()
        if revoked:
            office.revoked = {"radiopost.org": {"serials": revoked, "kids": []}}
        for at, who, method, path, body in steps:
            ex.call(at, who, method, path, body, tokens.get(who))
        cases.append({"what": what, "exchanges": ex.log})

    gw = "N1CALL-10"
    case(
        "session-01, opened 40 s after the client's clock",
        [(T_SESSION + 40, gw, "POST", "/sessions", good)],
    )
    case(
        "the post office's clock 601 s after `started`",
        [(T_SESSION + 641, gw, "POST", "/sessions", good)],
    )
    late = dict(good, started=T_SESSION + 601)
    case(
        "`started` 601 s after the statement's time",
        [(T_SESSION + 601, gw, "POST", "/sessions", late)],
    )
    case(
        "the same statement twice: a replay",
        [
            (T_SESSION + 40, gw, "POST", "/sessions", good),
            (T_SESSION + 70, gw, "POST", "/sessions", good),
        ],
    )
    case(
        "a message removed from the statement after signing",
        [(T_SESSION + 40, gw, "POST", "/sessions", fewer)],
    )
    case(
        "the user's credential revoked",
        [(T_SESSION + 40, gw, "POST", "/sessions", good)],
        revoked=[CREDENTIALS["facundo"]["serial"]],
    )
    case(
        "carried by a gateway the statement does not name",
        [(T_SESSION + 40, "KK4ODA-1", "POST", "/sessions", good)],
    )
    case("no token", [(T_SESSION + 40, "nobody", "POST", "/sessions", good)])
    case(
        "a user's token on a gateway endpoint",
        [(T_SESSION + 40, "facundo", "POST", "/sessions", good)],
    )

    # Handing in: a statement that offers a message from someone else, to show every refusal.
    mixed_ids = [encoded(n)[1] for n in ["facundo-reply", "facundo-to-ana", "maria-419"]]
    nonce = stream("api cases nonce", 8)
    mixed = rp.statement(
        "facundo@radiopost.org", "KK4ODA", "N1CALL-10", mixed_ids, time=T_SESSION, nonce=nonce
    )
    mixed_body = dict(
        good,
        statement=jws.b64u(cbor.encode(mixed)),
        signature=jws.b64u(rp.sign(rp.CTX_SESSION, cbor.encode(mixed), SECRET["device:facundo"])),
    )
    _, ex, tokens = case_office()
    sid = ex.call(T_SESSION + 40, gw, "POST", "/sessions", mixed_body, tokens[gw])["session"]
    bent = bytearray(air0("facundo-reply"))
    bent[-1] ^= 1
    items = handed_in(["facundo-to-ana", "facundo-to-ana"])["messages"] + [
        {"id": rp.id_text(encoded("facundo-reply")[1]), "air": jws.b64u(bytes(bent))},
        handed_in(["maria-419"])["messages"][0],
        handed_in(["eoc-420"])["messages"][0],
    ]
    ex.call(
        T_SESSION + 95, gw, "POST", f"/sessions/{sid}/messages", {"messages": items}, tokens[gw]
    )
    ex.call(
        T_SESSION + 96, gw, "POST", f"/sessions/{sid}/pickups", {"serials": [419, 999]}, tokens[gw]
    )
    ex.call(T_SESSION + 97, gw, "GET", f"/sessions/{sid}/messages/999", None, tokens[gw])
    cases.append(
        {
            "what": (
                "messages handed in: accepted; the same again, a duplicate; an air form that "
                "is not its message; a message from someone else; a message not in the "
                "statement. Then a pickup and a fetch of a serial the session did not list."
            ),
            "exchanges": ex.log,
        }
    )
    return cases


def api_report() -> list[dict[str, Any]]:
    """RP-5 §5.6: an offline gateway reports a day later; one statement fails."""
    _, ex, _ = case_office()
    hello = session_pdus()[0][1][0][1]
    good = session_request(hello, T_SESSION + 40)
    stmt = rp.statement_from_hello(hello)
    stmt[6] = stmt[6][:1]
    bad = dict(good, statement=jws.b64u(cbor.encode(stmt)))
    ex.note(
        "N1CALL-10 had no Internet during session-01: it checked Facundo offline (RP-1 §5, §6.3), "
        "queued his two messages and served no mail. A day later it reports, with a second, "
        "made-up session whose statement no longer matches its signature."
    )
    token = ex.login(T_SESSION + DAY - 10, "N1CALL-10", "gateway-n1call-10")
    ex.call(
        T_SESSION + DAY,
        "N1CALL-10",
        "POST",
        "/reports",
        {
            "sessions": [
                {**good, "messages": handed_in(OUTGOING)["messages"], "pickups": []},
                {**bad, "messages": [], "pickups": []},
            ]
        },
        token,
    )
    return ex.log


def make_api_exchanges() -> dict[str, Any]:
    out = header(
        "Post office API: requests and answers",
        {"RP-5": "§1–§6", "RP-1": "§5, §6.3", "RP-3": "§12"},
    )
    out["rule"] = (
        "Each exchange is one HTTPS request to the post office at its clock `at`, with its path "
        "under the base URL, its bearer token and its JSON body, and the answer: status and "
        "body. A post office given the same requests in order answers the same, and a gateway "
        "or client sends the requests shown. JSON compares as values, not text; a problem's "
        "`title` and a result's `text` are for people and need not match. Tokens, session IDs, "
        "nonces and cursors are this post office's random or opaque choices: use the ones the "
        "answers give. Air forms are those of the session vectors (compression 0)."
    )
    out["context"] = {
        "post_office": "radiopost.org",
        "base_url": PO_RADIOPOST,
        "signs_credentials_with": "po:radiopost.org",
        "knows_keys_of": ["radiopost.org", "hampost.org"],
        "enrollment_code": {
            "code": "7Q4F-AZ9K",
            "address": "facundo@radiopost.org",
            "operator": "KK4ODA",
            "flags": rp.FLAGS["receive"],
            "serial": 1050,
        },
        "next_serial_after_it": 1051,
        "randomness": "post office values are stream('post office <what> <n>', bytes)",
    }
    out["story"] = api_story()
    out["cases"] = api_cases()
    out["report"] = api_report()
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
    "session-02.json": make_resumed,
    "session-03.json": make_station,
    "rpmi-lines.json": make_rpmi_lines,
    "rpmi-conversations.json": make_rpmi_conversations,
    "dns.json": make_dns,
    "api.json": make_api,
    "jws.json": make_jws,
    "api-exchanges.json": make_api_exchanges,
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
    if name.startswith("session-"):
        for session in data.get("sessions", [data]):
            for t in session["transmissions"]:
                check_transmission(t)
    if name == "session-02.json":
        check_resumed(data)
    if name == "session-03.json":
        check_station(data)
    if name == "rpmi-conversations.json":
        check_rpmi(data)
    if name == "jws.json":
        check_jws(data)
    if name == "api-exchanges.json":
        check_api(data)


def check_rpmi(data: dict[str, Any]) -> None:
    """Each request is a good line and is answered; session-01's bytes cross intact (RP-4)."""
    answers = {"ok", "error", "hello", "status"}
    for conversation in data["conversations"]:
        steps = conversation["steps"]
        for i, step in enumerate(steps):
            m = step.get("message")
            if step.get("to") != "adapter" or m is None:
                continue
            assert rpmi.parse_line(rpmi.line_of(m)) == (m, None)
            if "id" not in m:
                continue
            later = [
                t["message"]
                for t in steps[i + 1 :]
                if t.get("to") == "host"
                and (t["station"], t["host"]) == (step["station"], step["host"])
            ]
            first = next(x for x in later if x.get("id") == m["id"] and x["op"] in answers)
            if m["op"] == "connect" and first["op"] == "ok":
                assert any(
                    x.get("id") == m["id"] and x["op"] in {"connected", "failed"} for x in later
                )
    sent = {"client": b"", "gateway": b""}
    got = {"client": b"", "gateway": b""}
    for step in data["conversations"][0]["steps"]:
        m = step.get("message") or {}
        if m.get("op") in {"send", "received"}:
            data_bytes = rpmi.decode_data(m["data"])
            assert data_bytes is not None
            (sent if m["op"] == "send" else got)[step["host"]] += data_bytes
    streams = write_transmissions(session_pdus())[1]
    for who, other in (("client", "gateway"), ("gateway", "client")):
        assert sent[who].hex() == got[other].hex() == streams[who]


def check_jws(data: dict[str, Any]) -> None:
    """Each document verifies, and each case gives its verdict, from the file alone."""
    dns_label = "po:radiopost.org"
    dns = {rp.kid(PUBLIC[dns_label]).hex(): PUBLIC[dns_label]}
    info = next(d["jws"] for d in data["documents"] if d["name"] == "info")
    keys = jws.accepted_keys(dns, info)
    for d in data["documents"]:
        assert jws.verify(d["jws"], keys) == ("ok", d["payload"])
    for case in data["verification"]:
        given = {k: PUBLIC[label] for label in SECRET for k in [rp.kid(PUBLIC[label]).hex()]}
        held = {k: given[k] for k in case["dns_kids"]}
        assert (
            jws.verify(case["jws"], jws.accepted_keys(held, case["info_jws"]))[0] == case["expect"]
        )


def check_api(data: dict[str, Any]) -> None:
    """Statuses fit their codes; credentials, lists and air forms agree with the other files."""
    logs = [data["story"], data["report"]] + [c["exchanges"] for c in data["cases"]]
    for log in logs:
        for e in log:
            body = e.get("response", {}).get("body") or {}
            if "name" in body and "status" in body:
                assert body["status"] == e["response"]["status"]
                assert body["code"] == rp.ERRORS[body["name"]]
                if body["status"] == 401:
                    assert body["name"] == "not-authorized"
                else:
                    assert body["status"] == po.HTTP_STATUS[body["name"]]
    by_path = {}
    for e in data["story"]:
        if "request" in e:
            by_path.setdefault(e["request"]["method"] + " " + e["request"]["path"], []).append(e)
    tablet = by_path["POST /enroll"][0]["response"]["body"]["credential"]
    assert jws.unb64u(tablet) == credential("facundo-tablet")
    kk = by_path["POST /gateways"][0]["response"]["body"]["credential"]
    assert jws.unb64u(kk) == credential("gateway-kk4oda-1")
    opened = by_path["POST /sessions"][0]["response"]["body"]
    welcome = session_pdus()[1][1][0][1]
    as_welcome = [
        [
            w["serial"],
            rp.id_bytes(w["id"]),
            w["from"],
            subject40(w["subject"]),
            w["size"],
            w["priority"],
            1 if w["held"] else 0,
        ]
        for w in opened["waiting"]
    ]
    assert as_welcome == welcome[7]
    assert [[rp.id_bytes(a["id"]), a["offset"]] for a in opened["accept"]] == welcome[5]
    sent = {
        p[5]: p[4] for _, pdus in session_pdus() for name, p in pdus if name == "MSG" and 5 in p
    }
    for key, entries in by_path.items():
        if key.startswith("GET /sessions/"):
            body = entries[0]["response"]["body"]
            assert jws.unb64u(body["air"]) == sent[body["serial"]]
    for e in by_path["GET /copies?after=&wait=60"]:
        for item in e["response"]["body"]["items"]:
            assert jws.unb64u(item["air"]) == air0(dict((s, n) for s, n in WAITING)[item["serial"]])


def pdus_of(t: dict[str, Any]) -> list[dict[int, Any]]:
    return [cbor.decode(bytes.fromhex(p["pdu_hex"])) for p in t["pdus"]]


def check_transmission(t: dict[str, Any]) -> None:
    """The frames join to `bytes_hex`, each decodes, and a break falls inside the PDU it names."""
    raw = bytes.fromhex(t["bytes_hex"])
    pos = 0
    ends = []
    for p in t["pdus"]:
        n = int.from_bytes(raw[pos : pos + 2], "big")
        assert raw[pos + 2 : pos + 2 + n].hex() == p["pdu_hex"]
        cbor.decode(raw[pos + 2 : pos + 2 + n])
        pos += 2 + n
        ends.append(pos)
    assert pos == len(raw) == t["bytes"]
    if "broken" in t:
        k = t["broken"]["complete_pdus"]
        assert (ends[k - 1] if k else 0) < t["broken"]["received_bytes"] < ends[k]


def check_resumed(data: dict[str, Any]) -> None:
    """Every piece received, across the sessions, joins into air forms that check (RP-3 §7)."""
    held: dict[bytes, bytearray] = {}
    for session in data["sessions"]:
        for t in session["transmissions"]:
            pdus = pdus_of(t)
            if "broken" in t:
                pdus = pdus[: t["broken"]["complete_pdus"]]
            for p in pdus:
                if p[0] == rp.PDU_TYPES["MSG"]:
                    got = held.setdefault(p[1], bytearray())
                    assert p[2] == len(got), "pieces in order, without gaps or repeats"
                    got += p[4]
    for cid, air in held.items():
        assert rp.content_id(rp.air_decode(bytes(air))) == cid


def check_station(data: dict[str, Any]) -> None:
    """The caller verifies the station it called from the bytes alone (RP-1 §6.5)."""
    hello = pdus_of(data["transmissions"][0])[0]
    welcome = pdus_of(data["transmissions"][1])[0]
    now = data["context"]["caller_clock"]
    result, body = rp.verify_credential(welcome[13], po_keys(), {}, now, ["send", "receive"])
    assert result == "ok" and body is not None
    stmt = rp.statement_from_welcome(hello, welcome)
    assert (
        rp.verify_statement(stmt, welcome[15], body, me=hello[4], now=now, sent_nonce=hello[7])
        == "ok"
    )


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
