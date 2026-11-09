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
import email.utils
import hashlib
import json
import sys
from collections.abc import Callable
from datetime import datetime, timedelta, timezone
from email.header import Header
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).parent))

from rpref import cbor, ed25519, fwd, jws, mail, po, rp, rpmi

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
T_ACCOUNT = T_SESSION + 2 * DAY  # api-account.json: renewal and the accept list
T_WINDOW = 1794492900  # session-04: N1CALL-10's window opens, 2026-11-12T14:15:00Z (:15 hourly)
T_CUT = T_WINDOW - 6 * 3600  # Atlanta loses the Internet

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
        "device:gateway-n2call-5",
        "Gateway N2CALL-5, the county EOC in Atlanta, credential from radiopost.org",
        None,
        None,
    ),
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


def confirm(
    cases: list[dict[str, Any]],
    wanted: dict[str, Any],
    verdict: Callable[[dict[str, Any]], Any] = lambda c: c["expect"],
    by: str = "what",
) -> None:
    """Each case's verdict as the specifications state it, written out next to its name.

    The reference is then tested, not trusted: a wrong reference fails here, where otherwise it
    would quietly write its own mistake into the vectors as the expected answer.
    """
    got = {c[by]: verdict(c) for c in cases}
    unmatched = set(got) ^ set(wanted)
    assert not unmatched, f"cases and written verdicts do not match: {sorted(unmatched)}"
    for what, want in wanted.items():
        assert got[what] == want, f"{what}: the reference says {got[what]!r}, the spec {want!r}"


def answer_of(e: dict[str, Any]) -> str:
    """An API answer in a few words: the status, and the error's name if it is one."""
    body = e["response"].get("body") or {}
    status = str(e["response"]["status"])
    return f"{status} {body['name']}" if "name" in body and "status" in body else status


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
    m["facundo-to-jose"] = {
        "v": 1,
        "from": "facundo@radiopost.org",
        "to": ["Jose@example.net"],
        "date": T_ACCOUNT - 600,
        "subject": "Generator",
        "body": "José, can you bring the generator to Shelter 3 tomorrow? 73, Facundo\n",
    }
    m["shelter3-to-ana"] = {
        "v": 1,
        "from": "shelter3@radiopost.org",
        "to": ["ana@hampost.org"],
        "date": T_WINDOW - 4 * 3600 - 600,
        "subject": "Water",
        "body": "Ana, Shelter 3 is out of drinking water for 120 people. Can your club help? "
        "N3CALL for Shelter 3\n",
        "priority": 1,
    }
    m["ana-to-shelter3"] = {
        "v": 1,
        "from": "ana@hampost.org",
        "to": ["shelter3@radiopost.org"],
        "date": T_WINDOW - 3 * 3600 - 300,
        "subject": "Water truck",
        "body": "Shelter 3, our club can bring a 2 000 litre water truck on Thursday. Tell us "
        "where to park. 73, Ana\n",
    }
    m["shelter3-to-facundo"] = {
        "v": 1,
        "from": "shelter3@radiopost.org",
        "to": ["facundo@radiopost.org"],
        "date": T_WINDOW + 900,
        "subject": "Spark plug",
        "body": "Facundo, the shelter's generator needs a spark plug. Can you bring one tomorrow? "
        "N3CALL for Shelter 3\n",
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
    "gateway-n2call-5": {
        "po": "po:radiopost.org",
        "device": "device:gateway-n2call-5",
        "serial": 1061,
        "address": "eoc@radiopost.org",
        "operator": "N2CALL-5",
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
        9: sum(rp.FLAGS[f] for f in spec["flags"]),
    }


def credential(name: str) -> bytes:
    spec = CREDENTIALS[name]
    return rp.make_credential(credential_body(spec), SECRET[spec["po"]])[3]


def make_credentials() -> dict[str, Any]:
    entries = []
    for name, spec in CREDENTIALS.items():
        body = credential_body(spec)
        payload, protected, signature, cred = rp.make_credential(body, SECRET[spec["po"]])
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
                    "flags": body[9],
                },
                "kid_hex": rp.kid(PUBLIC[spec["po"]]).hex(),
                "payload_hex": payload.hex(),
                "protected_hex": protected.hex(),
                "to_be_signed_hex": rp.to_be_signed(rp.CTX_CREDENTIAL, protected, payload).hex(),
                "signature_hex": signature.hex(),
                "credential_hex": cred.hex(),
                "credential_hash_hex": rp.credential_hash(cred).hex(),
            }
        )

    good = credential("facundo")
    outer = cbor.decode(good)
    tampered = cbor.encode([outer[0], {}, outer[2].replace(b"facundo@", b"facundp@"), outer[3]])
    rad_secret = SECRET["po:radiopost.org"]
    rad_kid = rp.kid(PUBLIC["po:radiopost.org"])

    def resigned(header: dict[int, Any], unprotected: dict[int, Any]) -> bytes:
        """Facundo's payload signed properly by radiopost.org, under another header."""
        protected = cbor.encode(header)
        signed = rp.to_be_signed(rp.CTX_CREDENTIAL, protected, outer[2])
        return cbor.encode([protected, unprotected, outer[2], ed25519.sign(rad_secret, signed)])

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
            "not a COSE_Sign1: three items",
            None,
            T_SESSION,
            ["send"],
            {},
            cbor.encode([outer[0], outer[2], outer[3]]),
        ),
        (
            "signed and valid, but EdDSA (-8) in place of Ed25519 (-19)",
            None,
            T_SESSION,
            ["send"],
            {},
            resigned({1: -8, 4: rad_kid}, {}),
        ),
        (
            "signed and valid, but a header left unprotected",
            None,
            T_SESSION,
            ["send"],
            {},
            resigned({1: rp.COSE_ED25519, 4: rad_kid}, {3: 0}),
        ),
        (
            "header naming radiopost.org's previous key, not the signer",
            None,
            T_SESSION,
            ["send"],
            {},
            resigned({1: rp.COSE_ED25519, 4: rp.kid(PUBLIC["po:radiopost.org:2025"])}, {}),
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
    confirm(  # RP-1 §5, step by step
        cases,
        {
            "valid": "ok",
            "one second before expiry": "ok",
            "at expiry": "credential-expired",
            "serial revoked": "credential-revoked",
            "post office key revoked": "credential-revoked",
            "address changed after signing": "credential-invalid",
            "post office not known to this verifier": "unknown-post-office",
            "signed by a key outside its validity period": "credential-invalid",
            "issued an hour ahead of the verifier's clock": "credential-invalid",
            "issued ten minutes ahead: within the allowance": "ok",
            "receive-only device asked to send": "not-authorized",
            "receive-only device picking up": "ok",
            "role mailbox operator": "ok",
            "gateway acting as a gateway": "ok",
            "a user's credential asked to act as a gateway": "not-authorized",
            "not a COSE_Sign1: three items": "credential-invalid",
            "signed and valid, but EdDSA (-8) in place of Ed25519 (-19)": "credential-invalid",
            "signed and valid, but a header left unprotected": "credential-invalid",
            "header naming radiopost.org's previous key, not the signer": "credential-invalid",
        },
    )
    out = header("Credentials and how they verify", {"RP-1": "§4–§5, §8", "RP-0": "§5, §9.3"})
    out["rule"] = (
        "From `body` and the signing key (`kid_hex`), an issuer produces exactly `payload_hex`, "
        "`protected_hex`, the COSE `to_be_signed_hex`, `signature_hex` and `credential_hex`, an "
        "untagged COSE_Sign1 (Ed25519 is deterministic). A verifier given a "
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
        return rp.credential_body(credential(cred))

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
    confirm(  # RP-1 §6.3 in its order; §6.5 for the station called
        cases,
        {
            "valid, 41 s after the client's clock": "ok",
            "exactly 600 s apart: still inside": "ok",
            "601 s apart: sign a challenge instead": "time-skew",
            "nonce seen before: sign a challenge instead": "time-skew",
            "verified by a gateway that was not the one called": "not-authorized",
            "a message removed from the list after signing": "signature-invalid",
            "operator not the credential's": "not-authorized",
            "challenge form, the challenge issued": "ok",
            "challenge form, another challenge issued": "signature-invalid",
            "pickup only, no messages offered": "ok",
            "called station, checked by the caller with the nonce it sent": "ok",
            "called station, its clock two hours ahead: the clocks are not compared": "ok",
            "called station, the nonce of another session": "signature-invalid",
            "called station, checked by a caller it was not made for": "not-authorized",
        },
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
    role: int = 0,
    auto: int = AUTO,
) -> dict[int, Any]:
    """A signed HELLO (RP-3 §5.1) from the client holding credential ``who``."""
    spec = CREDENTIALS[who]
    ids = [encoded(n)[1] for n in offers]
    stmt = rp.statement(spec["address"], spec["operator"], called, ids, time=clock, nonce=nonce)
    hello: dict[int, Any] = {
        0: 1,
        1: [1, 1],
        2: role,
        3: spec["address"],
        4: spec["operator"],
        5: called,
        6: clock,
        7: nonce,
        8: [offer(n) for n in offers],
        9: cursor,
        10: auto,
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


def rpmi_answers(steps: list[dict[str, Any]]) -> dict[str, str]:
    """Each request with an `id`, and what came of it: its answer, and for a call its end."""
    out = {}
    for i, step in enumerate(steps):
        m = step.get("message")
        if step.get("to") != "adapter" or m is None or "id" not in m:
            continue
        mine = [
            t["message"]
            for t in steps[i + 1 :]
            if t.get("to") == "host"
            and (t["station"], t["host"]) == (step["station"], step["host"])
        ]
        first = next(x for x in mine if x.get("id") == m["id"] and x["op"] in RPMI_ANSWERS)
        said = first["code"] if first["op"] == "error" else first["op"]
        if m["op"] == "connect" and said == "ok":
            end = next(
                x for x in mine if x.get("id") == m["id"] and x["op"] in {"connected", "failed"}
            )
            said += (
                ", then connected" if end["op"] == "connected" else f", then failed {end['code']}"
            )
        out[f"{step['station']} {step['host']} {m['op']} {m['id']}"] = said
    return out


RPMI_ANSWERS = {"ok", "error", "hello", "status"}


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
    session, errors, shared, broadcast = (
        rpmi_session(),
        rpmi_errors(),
        rpmi_shared(),
        rpmi_broadcast(),
    )
    gateway_start = {
        "N1CALL-10 gateway hello 1": "hello",
        "N1CALL-10 gateway callsigns 2": "ok",
        "N1CALL-10 gateway listen 3": "ok",
    }
    client_start = {"KK4ODA client hello 1": "hello", "KK4ODA client callsigns 2": "ok"}
    for steps, wanted in [
        (
            session,
            gateway_start
            | client_start
            | {"KK4ODA client connect 3": "ok, then connected", "KK4ODA client disconnect 4": "ok"},
        ),
        (
            errors,
            gateway_start
            | {
                "KK4ODA client connect 1": "refused",  # before hello (RP-4 §3)
                "KK4ODA client hello 2": "unsupported-version",
                "KK4ODA client hello 3": "hello",
                "KK4ODA client listen 4": "refused",  # no callsigns yet (§4)
                "KK4ODA client connect 5": "refused",
                "KK4ODA client callsigns 6": "ok",
                "KK4ODA client connect 7": "not-authorized",  # `as` not its own
                "KK4ODA client connect 8": "unsupported",  # a bandwidth the modem lacks
                "KK4ODA client tune 9": "unsupported",
                "KK4ODA client send 10": "not-connected",
                "KK4ODA client abort 11": "not-connected",  # nothing to abort
                "KK4ODA client connect 12": "ok, then failed aborted",
                "KK4ODA client abort 13": "ok",
                "KK4ODA client connect 14": "ok, then failed timeout",
                "KK4ODA client connect 15": "ok, then connected",
                "KK4ODA client connect 16": "busy",  # max_links is 1
                "KK4ODA client abort 17": "ok",
            },
        ),
        (
            shared,
            {  # RP-4 §1.1
                "N1CALL radiopost hello 1": "hello",
                "N1CALL radiopost callsigns 2": "ok",
                "N1CALL radiopost listen 3": "ok",
                "N1CALL other hello 1": "hello",
                "N1CALL other callsigns 2": "claimed",
                "N1CALL other callsigns 3": "ok",
                "N3CALL client hello 1": "hello",
                "N3CALL client callsigns 2": "ok",
                "N3CALL client connect 3": "ok, then failed timeout",
                "N1CALL other listen 4": "ok",
                "N3CALL client connect 4": "ok, then connected",
                "N1CALL radiopost status 4": "status",
                "N1CALL radiopost send 5": "not-connected",
                "N1CALL radiopost connect 6": "busy",
                "N3CALL client disconnect 5": "ok",
                "N3CALL client connect 6": "ok, then failed timeout",
            },
        ),
        (broadcast, gateway_start | client_start | {"KK4ODA client broadcast 3": "ok"}),
    ]:
        got = rpmi_answers(steps)
        assert got == wanted, {
            k: (got.get(k), wanted.get(k))
            for k in got.keys() | wanted.keys()
            if got.get(k) != wanted.get(k)
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
            "steps": session,
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
            "steps": errors,
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
            "steps": shared,
        },
        {
            "name": "a broadcast",
            "story": "The client sends a broadcast; the gateway's host hears it with its source.",
            "steps": broadcast,
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
    confirm(  # RP-4 §2: (the answer's op, its code, its id)
        cases,
        {
            "a string id, an unknown member": ("status", None, "s1"),
            "not JSON": ("error", "malformed", None),
            "not a JSON object": ("error", "malformed", None),
            "no op": ("error", "malformed", 4),
            "an op that is not a string": ("error", "malformed", 5),
            "an id that is neither a string nor an integer": ("error", "malformed", None),
            "an id that is a boolean": ("error", "malformed", None),
            "base64url, not standard base64": ("error", "malformed", 8),
            "base64 without its padding": ("error", "malformed", 9),
            "an op this adapter does not know": ("error", "unsupported", 10),
            "good base64, but no such link": ("error", "not-connected", 11),
        },
        lambda c: (c["answers"][0]["op"], c["answers"][0].get("code"), c["answers"][0].get("id")),
    )
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
    confirm(  # RP-0 §5 and RP-1 §7.2
        cases,
        {
            "info, signed by the key in DNS": "ok",
            "revocations, signed by the new key that the info document lists": "ok",
            "revocations, checked with the DNS key alone": "signature-invalid",
            "post-offices, its payload changed after signing": "signature-invalid",
            "gateways, `alg` none and no signature": "signature-invalid",
            "an info document signed by a key DNS does not list, naming itself; then a "
            "document signed by that key": "signature-invalid",
            "two parts, not three": "malformed",
            "a header that is not JSON": "malformed",
        },
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
        return self.login_with(at, who, credential(cred), CREDENTIALS[cred]["device"])

    def login_with(self, at: int, who: str, cred: bytes, device: str) -> str:
        """RP-5 §3: a challenge, then a token for this credential (empty if refused)."""
        nonce = self.call(at, who, "POST", "/auth/challenge")["nonce"]
        raw = jws.unb64u(nonce)
        assert raw is not None
        signature = rp.sign(rp.CTX_LOGIN, raw + PO_RADIOPOST.encode(), SECRET[device])
        body = {"credential": jws.b64u(cred), "nonce": nonce, "signature": jws.b64u(signature)}
        return self.call(at, who, "POST", "/auth/token", body).get("token", "")


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
    """RP-5 §5.2's body, from a HELLO the gateway received at `started` (session-01's)."""
    stmt = rp.statement_from_hello(hello)
    return {
        "statement": jws.b64u(cbor.encode(stmt)),
        "signature": jws.b64u(hello[15]),
        "credential": jws.b64u(credential(who)),
        "started": started,
        "gateway_session": SESSION_ID.hex(),
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
        "attach_bytes": ATTACH_BYTES,
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


HANDED_IN = (
    "messages handed in: accepted; the same again, a duplicate; an air form that is not its "
    "message; a message from someone else; a message not in the statement. Then a pickup and a "
    "fetch of a serial the session did not list."
)


def answers(c: dict[str, Any]) -> list[str]:
    """A case's API answers in a few words, and a handed-in message's result after its status."""
    out = []
    for e in c["exchanges"]:
        if "request" not in e:
            continue
        said = answer_of(e)
        for r in (e["response"].get("body") or {}).get("results", []):
            said += f"; {r['status']}" + (f" {r['name']}" if "name" in r else "")
        out.append(said)
    return out


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
            "what": HANDED_IN,
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
    story, cases, report = api_story(), api_cases(), api_report()
    confirm(  # RP-5 §1, §2, §5.2–§5.5
        cases,
        {
            "session-01, opened 40 s after the client's clock": ["201"],
            "the post office's clock 601 s after `started`": ["422 time-skew"],
            "`started` 601 s after the statement's time": ["422 time-skew"],
            "the same statement twice: a replay": ["201", "422 time-skew"],
            "a message removed from the statement after signing": ["422 signature-invalid"],
            "the user's credential revoked": ["422 credential-revoked"],
            "carried by a gateway the statement does not name": ["403 not-authorized"],
            "no token": ["401 not-authorized"],
            "a user's token on a gateway endpoint": ["403 not-authorized"],
            HANDED_IN: [
                "201",
                "200; accepted; duplicate; refused malformed; refused not-authorized; "
                "refused not-authorized",
                "404 unknown-message",
                "404 unknown-message",
            ],
        },
        answers,
    )
    failed = [
        (e["who"], e["request"]["path"], answer_of(e))
        for e in story
        if "request" in e and e["response"]["status"] >= 300
    ]
    assert failed == [("KK4ODA-1", "/copies?after=c_3&wait=60", "401 not-authorized")]
    reported = [e for e in report if "request" in e][-1]["response"]["body"]["sessions"]
    assert [s.get("name") or [r["status"] for r in s["results"]] for s in reported] == [
        ["accepted", "accepted"],
        "signature-invalid",
    ]  # RP-5 §5.6: one answer a session; a statement that fails takes nothing
    out["story"] = story
    out["cases"] = cases
    out["report"] = report
    return out


# -- session-04.json and forwarding.json --------------------------------------------------------


FWD_SHELTER_STARTED = T_WINDOW - 4 * 3600 + 35  # N2CALL-5's clock when Shelter 3's HELLO came
FWD_FACUNDO_STARTED = T_WINDOW - 3 * 3600 - 1800 + 28
FWD_CLOCK = T_WINDOW + 20  # N2CALL-5's clock when it calls
FWD_PARTNER_CLOCK = FWD_CLOCK + 33  # N1CALL-10's
FWD_AUTO = 65536  # what N2CALL-5 takes unasked: everything
# Aether's 2 300 Hz signal occupies 240-2 760 Hz of audio by the wider reading of §97.3(a)(8)
# (measured: aether-hf core/aetherd/data/occupancy.json); on a USB dial, add the dial.
AETHER_2300_HZ = (240, 2760)
AETHER_500_HZ = (1143, 1857)
WINDOW = {
    "channel": {
        "dial_hz": 7101000,
        "mode": "USB",
        "modem": "aether",
        "bandwidth_hz": 2300,
        "occupied_hz": [7101000 + AETHER_2300_HZ[0], 7101000 + AETHER_2300_HZ[1]],
    },
    "every_s": 3600,
    "at_s": 900,
    "length_s": 600,
}
ROUTE = {"users_at": ["N2CALL-5", "KK4ODA-1"], "via": "N1CALL-10"}


def offline_record(
    cred: str, names: list[str], at: int, label: str, pickups: list[int]
) -> dict[str, Any]:
    """A session N2CALL-5 carried while cut off (RP-7 §3.1), checked there as RP-1 §6.3 says."""
    spec = CREDENTIALS[cred]
    ids = [encoded(n)[1] for n in names]
    stmt = rp.statement(
        spec["address"],
        spec["operator"],
        "N2CALL-5",
        ids,
        time=at - 30,
        nonce=stream(f"{label} nonce", 8),
    )
    raw = cbor.encode(stmt)
    signature = rp.sign(rp.CTX_SESSION, raw, SECRET[spec["device"]])
    need = (["send"] if names else []) + ["receive"]
    result, body = rp.verify_credential(credential(cred), po_keys(), {}, at, need)
    assert result == "ok" and body is not None
    assert rp.verify_statement(stmt, signature, body, me="N2CALL-5", now=at) == "ok"
    return {
        "credential": credential(cred),
        "statement": raw,
        "signature": signature,
        "started": at,
        "session": stream(f"{label} id", 8),
        "messages": names,
        "pickups": pickups,
        "path": ["N2CALL-5"],
    }


def offline_records() -> list[dict[str, Any]]:
    """Shelter 3 sends its message to Ana; Facundo picks up #421 from N2CALL-5's copy."""
    return [
        offline_record(
            "shelter3-n3call", ["shelter3-to-ana"], FWD_SHELTER_STARTED, "fwd shelter", []
        ),
        offline_record("facundo", [], FWD_FACUNDO_STARTED, "fwd facundo", [421]),
    ]


def cut_off_office() -> tuple[po.PostOffice, Exchanges]:
    """radiopost.org on the day Atlanta loses the Internet (RP-7 §4.1)."""
    office = post_office()
    office.gateways["N2CALL-5"] = {
        "callsign": "N2CALL-5",
        "operator": "N2CALL",
        "copies": {"users": 50, "days": 7, "max_air_bytes": 16384},
    }
    office.gateways["N1CALL-10"] = {
        "callsign": "N1CALL-10",
        "operator": "N1CALL",
        "forwarding": [WINDOW],
    }
    office.routes = [ROUTE]
    office.settings["facundo@radiopost.org"] = {"usual_gateways": ["KK4ODA-1", "N2CALL-5"]}
    office.settings["shelter3@radiopost.org"] = {"usual_gateways": ["N2CALL-5"]}
    return office, Exchanges(office)


def deliver(
    office: po.PostOffice, ex: Exchanges, address: str, serial: int, name: str, at: int
) -> None:
    f = MESSAGES[name]
    office.now = at
    ex.note(f"Mail arrives for {address}: #{serial}, {f['subject']!r}.")
    office.deliver(
        address,
        serial,
        air0(name),
        sender=f["from"],
        subject=f["subject"],
        priority=f.get("priority", 0),
    )


def forwarding_office() -> tuple[Exchanges, dict[str, str], dict[str, Any]]:
    """radiopost.org until N1CALL-10 has reported session-04's records: the exchanges, each
    gateway's copies cursor, and the forward item it placed."""
    office, ex = cut_off_office()
    deliver(office, ex, "facundo@radiopost.org", 421, "maria-421", MESSAGES["maria-421"]["date"])
    ex.note("N2CALL-5 polls for copies, as it does all the time, and takes the copy of #421.")
    n2 = ex.login(T_CUT - 120, "N2CALL-5", "gateway-n2call-5")
    copies = ex.call(T_CUT - 60, "N2CALL-5", "GET", "/copies?after=&wait=60", None, n2)
    ex.call(T_CUT - 59, "N2CALL-5", "POST", "/copies/ack", {"cursor": copies["cursor"]}, n2)
    ex.note(
        "Atlanta loses the Internet. N2CALL-5 polls no more; 15 minutes on, radiopost.org counts "
        "it cut off (RP-7 §4.1). Shelter 3 and Facundo call N2CALL-5, which checks them offline "
        "and keeps their session records (RP-7 §3.1)."
    )
    deliver(
        office,
        ex,
        "shelter3@radiopost.org",
        501,
        "ana-to-shelter3",
        MESSAGES["ana-to-shelter3"]["date"] + 600,
    )
    ex.note(
        "Shelter 3 named N2CALL-5 as usual, and N2CALL-5 is cut off: the route sends the mail to "
        "N1CALL-10, as a forward item with a placement signed by radiopost.org (RP-7 §4.2)."
    )
    n1 = ex.login(T_WINDOW - 700, "N1CALL-10", "gateway-n1call-10")
    polled = ex.call(T_WINDOW - 600, "N1CALL-10", "GET", "/copies?after=&wait=60", None, n1)
    ex.call(T_WINDOW - 599, "N1CALL-10", "POST", "/copies/ack", {"cursor": polled["cursor"]}, n1)
    item = next(i for i in polled["items"] if i["kind"] == "forward")
    ex.note(
        "In N1CALL-10's window N2CALL-5 calls it (session-04.json): the two records go out, the "
        "forward item comes in. N1CALL-10 reaches radiopost.org and reports the records with "
        "their path (RP-5 §5.6, RP-7 §3.4)."
    )
    sessions = []
    for record in offline_records():
        stmt = cbor.decode(record["statement"])
        sessions.append(
            {
                "statement": jws.b64u(record["statement"]),
                "signature": jws.b64u(record["signature"]),
                "credential": jws.b64u(record["credential"]),
                "started": record["started"],
                "gateway_session": record["session"].hex(),
                "messages": handed_in(record["messages"])["messages"],
                "pickups": record["pickups"],
                "path": [*record["path"], "N1CALL-10"],
            }
        )
        assert stmt[3] == "N2CALL-5"
    n1 = ex.login(FWD_PARTNER_CLOCK + 280, "N1CALL-10", "gateway-n1call-10")
    ex.call(FWD_PARTNER_CLOCK + 300, "N1CALL-10", "POST", "/reports", {"sessions": sessions}, n1)
    ex.note("A report whose path does not end at the reporting gateway is refused (RP-5 §5.6).")
    wrong = sessions[1] | {"path": ["N2CALL-5", "KK4ODA-1"]}
    ex.call(FWD_PARTNER_CLOCK + 310, "N1CALL-10", "POST", "/reports", {"sessions": [wrong]}, n1)
    return ex, {"N2CALL-5": copies["cursor"], "N1CALL-10": polled["cursor"]}, item


def forwarding_story() -> tuple[list[dict[str, Any]], dict[str, Any]]:
    """The post office's side, before and after session-04, and the forward item it placed."""
    ex, cursors, item = forwarding_office()
    ex.note(
        "Atlanta's Internet returns. N2CALL-5's poll ends the forwarding: radiopost.org has the "
        "forward item at N1CALL-10 dropped (RP-7 §4.1). N2CALL-5 gets the copy of #501 it already "
        "holds by radio; the copy of #421, picked up, is dropped."
    )
    back = T_WINDOW + 2 * 3600
    n2 = ex.login(back, "N2CALL-5", "gateway-n2call-5")
    after = cursors["N2CALL-5"]
    ex.call(back + 5, "N2CALL-5", "GET", f"/copies?after={after}&wait=60", None, n2)
    n1 = ex.login(back + 300, "N1CALL-10", "gateway-n1call-10")
    after = cursors["N1CALL-10"]
    ex.call(back + 305, "N1CALL-10", "GET", f"/copies?after={after}&wait=60", None, n1)
    return ex.log, item


def forwarding_pdus() -> list[Transmission]:
    """Session-04 (RP-7 §2): N2CALL-5, cut off, calls N1CALL-10 in its window."""
    records = offline_records()
    offers = [n for r in records for n in r["messages"]]
    hello = hello_pdu(
        "gateway-n2call-5",
        "N1CALL-10",
        FWD_CLOCK,
        stream("session-04 nonce", 8),
        offers=offers,
        cursor=0,
        full_credential=True,
        role=1,
        auto=FWD_AUTO,
    )
    result, body = rp.verify_credential(
        credential("gateway-n2call-5"), po_keys(), {}, FWD_PARTNER_CLOCK, ["gateway", "send"]
    )
    assert result == "ok" and body is not None
    stmt = rp.statement_from_hello(hello)
    assert rp.verify_statement(stmt, hello[15], body, me="N1CALL-10", now=FWD_PARTNER_CLOCK) == "ok"
    _, item = forwarding_story()
    placement = jws.unb64u(item["placement"])
    assert placement is not None
    welcome = welcome_pdu(
        stream("session-04 id", 8),
        FWD_PARTNER_CLOCK,
        accept=[[encoded(n)[1], 0] for n in offers],
        decline=[],
        waiting=[waiting_entry(1, "ana-to-shelter3")],
        caps=GATEWAY_CAPS,
        online=True,
        cached=True,
    )
    inward = [{**p, 6: placement} for p in msg_pdus("ana-to-shelter3", serial=1)]
    proofs = [fwd.proof_pdu(r) for r in records]
    return [
        ("caller", [("HELLO", hello)]),
        ("partner", [("WELCOME", welcome)] + [("MSG", p) for p in inward] + [("DONE", DONE)]),
        (
            "caller",
            [("PROOF", p) for p in proofs]
            + [("MSG", p) for n in offers for p in msg_pdus(n)]
            + [("RECEIPT", receipt(["ana-to-shelter3"])), ("DONE", DONE)],
        ),
        (
            "partner",
            [
                (
                    "RECEIPT",
                    {
                        0: 5,
                        1: [encoded(n)[1] for n in offers],
                        4: [fwd.record_id(r["statement"]) for r in records],
                    },
                )
            ],
        ),
    ]


def make_forwarding_session() -> dict[str, Any]:
    transmissions, streams = write_transmissions(forwarding_pdus())
    out = header(
        "A forwarding session between gateways, byte for byte",
        {"RP-7": "§2, §3, §4", "RP-3": "§5"},
    )
    out["story"] = (
        "Atlanta has lost the Internet. N2CALL-5 (the county EOC, cut off) calls N1CALL-10 in "
        "Austin in N1CALL-10's window, at :15 past the hour, as a client calls a gateway (role 1). "
        "It offers the message Shelter 3 left with it and sends two PROOF PDUs: Shelter 3's "
        "session record and Facundo's, a pickup only. N1CALL-10 sends the forward item it holds "
        "for N2CALL-5, Ana's message to Shelter 3, with radiopost.org's placement. N1CALL-10 "
        "receipts the message and both records; N2CALL-5 hangs up after transmission 4."
    )
    out["rule"] = (
        "As in session-01.json. The partner checks each PROOF as RP-7 §3.3 says (with `caller` "
        "N2CALL-5) before taking the messages it covers, and lists the record IDs it took in "
        "RECEIPT key 4. The caller checks each MSG key 6 placement (RP-7 §4.3) before holding the "
        "message as a copy for the address the placement names."
    )
    out["context"] = {
        "caller": "N2CALL-5, credential gateway-n2call-5 (credentials.json), cut off",
        "partner": "N1CALL-10, credential gateway-n1call-10, online",
        "window": WINDOW,
        "caller_clock": FWD_CLOCK,
        "partner_clock": FWD_PARTNER_CLOCK,
        "records": "forwarding.json",
        "forward_item": "forwarding.json, post_office: the item N1CALL-10 polled",
    }
    out["transmissions"] = transmissions
    out["streams"] = streams
    out["after"] = {
        "partner_reports": (
            "forwarding.json, post_office: POST /reports with path [N2CALL-5, N1CALL-10]"
        ),
        "caller_holds_as_copy": {"address": "shelter3@radiopost.org", "serial": 501},
        "caller_lets_go": ["shelter3-to-ana", "both records"],
    }
    return out


def spec_segments(country: str) -> list[tuple[int, int]]:
    """RP-7 §5.1's table for `country`, read from the specification itself."""
    text = (ROOT / "spec" / "rp-0007-forwarding.md").read_text(encoding="utf-8")
    names = {"US": "**United States**"}
    part = text.split(names[country], 1)[1].split("\n\n", 2)[1]
    rows = [r.split("|")[2:4] for r in part.splitlines()[2:]]
    return [(int(a.replace(" ", "")), int(b.replace(" ", ""))) for a, b in rows]


def make_forwarding() -> dict[str, Any]:
    story, item = forwarding_story()
    placement = jws.unb64u(item["placement"])
    assert placement is not None
    protected, _, payload, signature = cbor.decode(placement)
    cid = encoded("ana-to-shelter3")[1]
    keys = po_keys()

    def placed(changes: dict[int, Any]) -> bytes:
        body = cbor.decode(payload) | changes
        return fwd.make_placement(body, SECRET["po:radiopost.org"])

    bent = bytearray(placement)
    bent[-1] ^= 1
    misnamed_header = rp.protected_header(rp.kid(PUBLIC["po:radiopost.org:2025"]))
    misnamed_signed = rp.to_be_signed(rp.CTX_PLACEMENT, misnamed_header, payload)
    misnamed = cbor.encode(
        [misnamed_header, {}, payload, ed25519.sign(SECRET["po:radiopost.org"], misnamed_signed)]
    )
    placement_cases_in = [
        ("as placed, checked by N2CALL-5 in session-04", placement, cid, "N2CALL-5", FWD_CLOCK),
        (
            "signed by a key radiopost.org does not have",
            fwd.make_placement(cbor.decode(payload), SECRET["po:hampost.org"]),
            cid,
            "N2CALL-5",
            FWD_CLOCK,
        ),
        ("its signature changed", bytes(bent), cid, "N2CALL-5", FWD_CLOCK),
        ("its header naming another key of radiopost.org", misnamed, cid, "N2CALL-5", FWD_CLOCK),
        ("for another gateway", placed({5: "KK4ODA-1"}), cid, "N2CALL-5", FWD_CLOCK),
        ("for another message", placement, encoded("maria-421")[1], "N2CALL-5", FWD_CLOCK),
        ("past its `expires`", placement, cid, "N2CALL-5", cbor.decode(payload)[6]),
        ("not a placement", cbor.encode([payload]), cid, "N2CALL-5", FWD_CLOCK),
    ]
    placement_cases = []
    for what, pl, c, me, now in placement_cases_in:
        result, _ = fwd.check_placement(pl, c, me, now, keys)
        placement_cases.append(
            {
                "what": what,
                "placement_hex": pl.hex(),
                "content_id_hex": c.hex(),
                "me": me,
                "now": now,
                "expect": result,
            }
        )
    confirm(  # RP-7 §4.3, in its order
        placement_cases,
        {
            "as placed, checked by N2CALL-5 in session-04": "ok",
            "signed by a key radiopost.org does not have": "signature-invalid",
            "its signature changed": "signature-invalid",
            "its header naming another key of radiopost.org": "signature-invalid",
            "for another gateway": "not-authorized",
            "for another message": "not-authorized",
            "past its `expires`": "refused-by-policy",
            "not a placement": "malformed",
        },
    )
    records = offline_records()
    entries = []
    for r in records:
        pdu = fwd.proof_pdu(r)
        entries.append(
            {
                "messages": r["messages"],
                "record_id_hex": fwd.record_id(r["statement"]).hex(),
                "proof": pdu_entry("PROOF", pdu),
            }
        )
    good = fwd.proof_pdu(records[0])
    loop = good | {6: ["N2CALL-5", "N1CALL-10", "N2CALL-5"]}
    far = good | {6: ["N2CALL-5", *[f"N{i}CALL-1" for i in range(4, 11)], "N2CALL-5"]}
    changed = dict(good)
    stmt = cbor.decode(good[2])
    stmt[6] = []
    changed[2] = cbor.encode(stmt)
    hashed = fwd.proof_pdu(records[0], hashed=True)
    kept: dict[bytes, bytes] = {}
    assert fwd.check_proof(good, "N2CALL-5", "N1CALL-10", keys, {}, held=kept)[0] == "ok"
    assert list(kept.values()) == [records[0]["credential"]]
    proof_cases_in = [
        ("Shelter 3's record, as N2CALL-5 sends it", good, "N2CALL-5", []),
        ("Facundo's record, a pickup only", fwd.proof_pdu(records[1]), "N2CALL-5", []),
        (
            "a path that does not start at the gateway the user called",
            good | {6: ["KK4ODA-1", "N2CALL-5"]},
            "N2CALL-5",
            [],
        ),
        ("a path that does not end at the caller", good | {6: ["N2CALL-5"]}, "KK4ODA-1", []),
        ("a path through the partner already: a loop", loop, "N2CALL-5", []),
        ("a path of nine gateways", far, "N2CALL-5", []),
        ("the statement changed after signing", changed, "N2CALL-5", []),
        (
            "Facundo's record by his credential's hash, which N1CALL-10 holds",
            fwd.proof_pdu(records[1], hashed=True),
            "N2CALL-5",
            [records[1]["credential"]],
        ),
        (
            "Shelter 3's record by hash, after a PROOF with its credential",
            hashed,
            "N2CALL-5",
            list(kept.values()),
        ),
        ("Shelter 3's record by a hash N1CALL-10 does not hold", hashed, "N2CALL-5", []),
        ("both the credential and its hash", good | {9: hashed[9]}, "N2CALL-5", []),
    ]
    proof_cases = []
    for what, pdu, caller, held in proof_cases_in:
        holds = {rp.credential_hash(c): c for c in held}
        result, _ = fwd.check_proof(pdu, caller, "N1CALL-10", keys, {}, held=holds)
        proof_cases.append(
            {
                "what": what,
                "proof_hex": cbor.encode(pdu).hex(),
                "caller": caller,
                "me": "N1CALL-10",
                "held_hex": [c.hex() for c in held],
                "expect": result,
            }
        )
    confirm(  # RP-7 §3.3
        proof_cases,
        {
            "Shelter 3's record, as N2CALL-5 sends it": "ok",
            "Facundo's record, a pickup only": "ok",
            "a path that does not start at the gateway the user called": "refused-by-policy",
            "a path that does not end at the caller": "refused-by-policy",
            "a path through the partner already: a loop": "refused-by-policy",
            "a path of nine gateways": "refused-by-policy",
            "the statement changed after signing": "signature-invalid",
            "Facundo's record by his credential's hash, which N1CALL-10 holds": "ok",
            "Shelter 3's record by hash, after a PROOF with its credential": "ok",
            "Shelter 3's record by a hash N1CALL-10 does not hold": "credential-required",
            "both the credential and its hash": "malformed",
        },
    )
    assert spec_segments("US") == fwd.SEGMENTS["US"]

    def usb(dial: int, audio: tuple[int, int] = AETHER_2300_HZ) -> list[int]:
        return [dial + audio[0], dial + audio[1]]

    window_in = [
        ("N1CALL-10's window: 7.101 MHz, Aether 2 300 Hz", WINDOW["channel"]["occupied_hz"], "US"),
        ("7.103 MHz: the signal runs past 7.105", usb(7103000), "US"),
        ("3.590 MHz, Aether 500 Hz", usb(3590000, AETHER_500_HZ), "US"),
        ("14.0975 MHz: across the beacons at 14.100", usb(14097500), "US"),
        ("14.101 MHz, above the beacons", usb(14101000), "US"),
        ("60 m, 5.357 MHz: no automatic control", usb(5357000), "US"),
        ("6 m, 50.690 MHz", usb(50690000), "US"),
        ("6 m, 50.050 MHz: CW only below 50.1", usb(50050000), "US"),
        ("exactly 7.100 to 7.105 MHz: the ends count", [7100000, 7105000], "US"),
        ("one hertz past 3.600 MHz", [3597600, 3600001], "US"),
        ("7.101 MHz from a country with no table here", WINDOW["channel"]["occupied_hz"], "CA"),
    ]
    window_cases = [
        {
            "what": what,
            "occupied_hz": occupied,
            "country": country,
            "expect": fwd.window_allowed(occupied, country),
        }
        for what, occupied, country in window_in
    ]
    confirm(  # RP-7 §5, §5.1: inside one segment of the station's own table, ends included
        window_cases,
        {
            "N1CALL-10's window: 7.101 MHz, Aether 2 300 Hz": True,
            "7.103 MHz: the signal runs past 7.105": False,
            "3.590 MHz, Aether 500 Hz": True,
            "14.0975 MHz: across the beacons at 14.100": False,
            "14.101 MHz, above the beacons": True,
            "60 m, 5.357 MHz: no automatic control": False,
            "6 m, 50.690 MHz": True,
            "6 m, 50.050 MHz: CW only below 50.1": False,
            "exactly 7.100 to 7.105 MHz: the ends count": True,
            "one hertz past 3.600 MHz": False,
            "7.101 MHz from a country with no table here": False,
        },
    )
    now = T_WINDOW + 1860  # 14:46, in N4CALL-2's window
    n4 = {"every_s": 3600, "at_s": 2700, "length_s": 600}

    def w4(opens_in: int) -> dict[str, Any]:
        return {"every_s": 14400, "at_s": (now + opens_in) % 14400, "length_s": 1200}

    partner_in = [
        (
            "N1CALL-10, which said `online` true",
            "N1CALL-10",
            T_WINDOW + 60,
            {"N1CALL-10": (WINDOW, True), "N4CALL-2": (n4, False)},
        ),
        (
            "N4CALL-2, cut off; N1CALL-10, online, opens in 29 minutes",
            "N4CALL-2",
            now,
            {"N1CALL-10": (WINDOW, True), "N4CALL-2": (n4, False)},
        ),
        (
            "N4CALL-2, cut off; N1CALL-10 cut off too",
            "N4CALL-2",
            now,
            {"N1CALL-10": (WINDOW, False), "N4CALL-2": (n4, False)},
        ),
        (
            "N4CALL-2, never called; N1CALL-10, online, opens in 29 minutes",
            "N4CALL-2",
            now,
            {"N1CALL-10": (WINDOW, True), "N4CALL-2": (n4, None)},
        ),
        (
            "N4CALL-2, cut off; N1CALL-10 never called",
            "N4CALL-2",
            now,
            {"N1CALL-10": (WINDOW, None), "N4CALL-2": (n4, False)},
        ),
        (
            "N4CALL-2, cut off; W4CALL-1, online, opens in exactly an hour",
            "N4CALL-2",
            now,
            {"N4CALL-2": (n4, False), "W4CALL-1": (w4(3600), True)},
        ),
        (
            "N4CALL-2, cut off; W4CALL-1, online, opens in 61 minutes",
            "N4CALL-2",
            now,
            {"N4CALL-2": (n4, False), "W4CALL-1": (w4(3660), True)},
        ),
        (
            "N4CALL-2, cut off; W4CALL-1, online, opened 5 minutes ago",
            "N4CALL-2",
            now,
            {"N4CALL-2": (n4, False), "W4CALL-1": (w4(-300), True)},
        ),
    ]
    partner_cases = []
    for what, calling, at, known in partner_in:
        partners = {
            cs: {"window": w} | ({} if online is None else {"online": online})
            for cs, (w, online) in known.items()
        }
        assert fwd.next_window(partners[calling]["window"], at) <= at  # its window is open
        partner_cases.append(
            {
                "what": what,
                "calling": calling,
                "now": at,
                "partners": partners,
                "expect": fwd.records_to(calling, partners, at),
            }
        )
    confirm(  # RP-7 §6.2: records wait an hour at most for a partner that reaches the Internet
        partner_cases,
        {
            "N1CALL-10, which said `online` true": True,
            "N4CALL-2, cut off; N1CALL-10, online, opens in 29 minutes": False,
            "N4CALL-2, cut off; N1CALL-10 cut off too": True,
            "N4CALL-2, never called; N1CALL-10, online, opens in 29 minutes": True,
            "N4CALL-2, cut off; N1CALL-10 never called": True,
            "N4CALL-2, cut off; W4CALL-1, online, opens in exactly an hour": False,
            "N4CALL-2, cut off; W4CALL-1, online, opens in 61 minutes": True,
            "N4CALL-2, cut off; W4CALL-1, online, opened 5 minutes ago": False,
        },
    )
    out = header(
        "Forwarding: placements, session records, and the post office's side",
        {"RP-7": "§3, §4, §5, §6", "RP-5": "§5.6, §5.8"},
    )
    out["rule"] = (
        "A placement made from `placement_body` with radiopost.org's key is exactly "
        "`placement_hex`; a gateway checking a placement case with `me` and `now`, and the post "
        "office keys of keys.json, returns `expect` (RP-7 §4.3). A record's ID is SHA-256 of its "
        "statement, cut to 12 bytes (RP-7 §3.1). A partner (`me`) holding the credentials "
        "`held_hex` and checking a PROOF case from `caller` returns `expect` (RP-7 §3.2, §3.3). "
        "A station of `country` may publish or call in a window whose signal occupies "
        "`occupied_hz` when `expect` is true (RP-7 §5, §5.1). "
        "A cut-off gateway in `calling`'s window at `now`, knowing `partners`, sends it its "
        "records when `expect` is true (RP-7 §5, §6.2). The post office's exchanges read as in "
        "api-exchanges.json."
    )
    out["context"] = {
        "cut_off_at": T_CUT,
        "cut_off_after_s": 900,
        "route": ROUTE,
        "window": WINDOW,
        "gateways": {
            "N2CALL-5": "eoc@radiopost.org's gateway, holds copies, cut off",
            "N1CALL-10": "online, takes calls from partners in its window",
        },
        "usual_gateways": {
            "facundo@radiopost.org": ["KK4ODA-1", "N2CALL-5"],
            "shelter3@radiopost.org": ["N2CALL-5"],
        },
    }
    out["placement"] = {
        "for": "ana-to-shelter3, #501 for shelter3@radiopost.org, at N2CALL-5",
        "placement_body": cbor.diagnostic(cbor.decode(payload)),
        "payload_hex": payload.hex(),
        "protected_hex": protected.hex(),
        "to_be_signed_hex": rp.to_be_signed(rp.CTX_PLACEMENT, protected, payload).hex(),
        "signature_hex": signature.hex(),
        "placement_hex": placement.hex(),
    }
    out["placement_cases"] = placement_cases
    out["records"] = entries
    out["proof_cases"] = proof_cases
    out["window_cases"] = window_cases
    out["segments"] = {
        "US": [list(seg) for seg in fwd.SEGMENTS["US"]],
        "rule": "47 CFR §97.221(b), §97.305(c); RP-7 §5.1",
    }
    out["partner_cases"] = partner_cases
    out["post_office"] = story
    return out


# -- session-05.json and local-delivery.json ----------------------------------------------------


LOCAL_A = T_WINDOW + 1200  # 14:35, after session-04: Shelter 3 calls N2CALL-5 (client's clock)
LOCAL_B = T_WINDOW + 2400  # 14:55: Facundo calls it
LOCAL_WINDOW = T_WINDOW + 3600  # 15:15: N1CALL-10's next window, which takes both records
LOCAL_NOTICE = (
    "No Internet here since 08:15 UTC. Your mail waits in my queue; mail for this gateway's own "
    "users is handed over here."
)
LOCAL_NAME = "shelter3-to-facundo"


def record_from(
    hello: dict[int, Any], welcome: dict[int, Any], started: int, **parts: Any
) -> dict[str, Any]:
    """The session record a cut-off gateway keeps for a session it carried (RP-7 §3.1)."""
    return {
        "credential": hello.get(13) or credential(parts.pop("who")),
        "statement": cbor.encode(rp.statement_from_hello(hello)),
        "signature": hello[15],
        "started": started,
        "session": welcome[2],
        "messages": parts.get("messages", []),
        "pickups": parts.get("pickups", []),
        "local": parts.get("local", []),
        "path": ["N2CALL-5"],
    }


def local_sessions() -> tuple[list[tuple[str, list[Transmission], int, int]], list[dict[str, Any]]]:
    """Two sessions at N2CALL-5, cut off, and the records it keeps of them (RP-7 §4.4)."""
    notice = {0: 10, 1: LOCAL_NOTICE}
    cid = encoded(LOCAL_NAME)[1]
    hello_a = hello_pdu(
        "shelter3-n3call",
        "N2CALL-5",
        LOCAL_A,
        stream("session-05a nonce", 8),
        offers=[LOCAL_NAME],
        cursor=0,
        full_credential=False,
    )
    gateway_accepts(hello_a, "shelter3-n3call", "N2CALL-5", LOCAL_A + 31)
    welcome_a = welcome_pdu(
        stream("session-05a id", 8),
        LOCAL_A + 31,
        accept=[[cid, 0]],
        decline=[],
        waiting=[waiting_entry(501, "ana-to-shelter3", 2)],
        caps=GATEWAY_CAPS,
        online=False,
        cached=True,
    )
    assert len(air0("ana-to-shelter3")) <= hello_a[10]  # under `auto`: sent unasked
    a = [
        ("client", [("HELLO", hello_a)]),
        (
            "gateway",
            [("WELCOME", welcome_a), ("NOTICE", notice)]
            + [("MSG", p) for p in msg_pdus("ana-to-shelter3", serial=501)]
            + [("DONE", DONE)],
        ),
        (
            "client",
            [("MSG", p) for p in msg_pdus(LOCAL_NAME)]
            + [("RECEIPT", receipt(["ana-to-shelter3"])), ("DONE", DONE)],
        ),
        ("gateway", [("RECEIPT", receipt([LOCAL_NAME]))]),
    ]
    hello_b = hello_pdu(
        "facundo",
        "N2CALL-5",
        LOCAL_B,
        stream("session-05b nonce", 8),
        offers=[],
        cursor=421,
        full_credential=False,
        auto=0,
    )
    gateway_accepts(hello_b, "facundo", "N2CALL-5", LOCAL_B + 26)
    assert hello_b[9] == max(offline_records()[1]["pickups"])  # #421, at 10:45 (session-04)
    welcome_b = welcome_pdu(
        stream("session-05b id", 8),
        LOCAL_B + 26,
        accept=[],
        decline=[],
        waiting=[waiting_entry(0, LOCAL_NAME, 4)],
        caps=GATEWAY_CAPS,
        online=False,
        cached=True,
    )
    b = [
        ("client", [("HELLO", hello_b)]),
        ("gateway", [("WELCOME", welcome_b), ("NOTICE", notice), ("DONE", DONE)]),
        ("client", [("REQUEST", {0: 6, 4: [cid]})]),
        ("gateway", [("MSG", p) for p in msg_pdus(LOCAL_NAME)] + [("DONE", DONE)]),
        ("client", [("RECEIPT", receipt([LOCAL_NAME])), ("DONE", DONE)]),
    ]
    records = [
        record_from(
            hello_a,
            welcome_a,
            LOCAL_A + 31,
            who="shelter3-n3call",
            messages=[LOCAL_NAME],
            pickups=[501],
        ),
        record_from(hello_b, welcome_b, LOCAL_B + 26, who="facundo", local=[cid]),
    ]
    sessions = [("a", a, LOCAL_A, LOCAL_A + 31), ("b", b, LOCAL_B, LOCAL_B + 26)]
    return sessions, records


def make_local_session() -> dict[str, Any]:
    sessions, _ = local_sessions()
    stories = {
        "a": "14:35, twenty minutes after session-04. Shelter 3 (N3CALL) calls N2CALL-5, which "
        "has no Internet and says so in a NOTICE. Ana's message, #501, which came in by "
        "session-04, waits as a copy and goes unasked. Shelter 3 hands over a message for "
        "Facundo and receipts #501; N2CALL-5 receipts the message and keeps it both in Shelter "
        "3's session record, with the pickup of #501, and for Facundo, a local user (it holds "
        "copies for him).",
        "b": "14:55. Facundo calls N2CALL-5 with `auto` 0: list only. He picked up #421 at "
        "10:45 (session-04's record), so only Shelter 3's message waits: serial 0, flag `local`. "
        "He asks for it by content ID (REQUEST key 4), takes it, and hangs up. N2CALL-5 adds the "
        "message's content ID to his record's local pickups.",
    }
    out = header(
        "Local delivery at a cut-off gateway, byte for byte",
        {"RP-7": "§4.4", "RP-3": "§5.2, §5.4, §5.6"},
    )
    out["story"] = (
        "Atlanta has been without the Internet since 08:15; session-04 ran at 14:15. Two users "
        "of N2CALL-5 exchange a message through it before their post office hears of it (RP-7 "
        "§4.4)."
    )
    out["rule"] = (
        "As in session-01.json, for each session. A local message is listed with serial 0 and "
        "flag 4, asked for by content ID, and sent without a serial."
    )
    out["context"] = {
        "gateway": "N2CALL-5, cut off; it holds copies for facundo@radiopost.org, so he is a local "
        "user",
        "records": "local-delivery.json",
    }
    out["sessions"] = []
    for label, seq, client, gateway in sessions:
        transmissions, streams = write_transmissions(seq)
        out["sessions"].append(
            {
                "session": label,
                "story": stories[label],
                "client_clock": client,
                "gateway_clock": gateway,
                "transmissions": transmissions,
                "streams": streams,
            }
        )
    return out


def local_report(records: list[dict[str, Any]]) -> tuple[Exchanges, str, int, str]:
    """radiopost.org until N1CALL-10 reports N2CALL-5's records of session-05 (RP-7 §4.4).

    It goes on from forwarding.json's story after session-04's report, and returns N1CALL-10's
    copies cursor from before that window.
    """
    ex, cursors, _ = forwarding_office()
    ex.log = []
    ex.note(
        "As in forwarding.json until N1CALL-10 has reported session-04's records: Facundo has "
        "picked up #421, and Ana's #501 waits at N2CALL-5 for Shelter 3. N2CALL-5 is still cut "
        "off. Shelter 3 and Facundo exchange a message through it (session-05.json). In "
        "N1CALL-10's next window N2CALL-5 forwards both records, each credential by its hash, "
        "since N1CALL-10 took both in session-04 (RP-7 §3.2); N1CALL-10 reports them."
    )
    window = LOCAL_WINDOW
    n1 = ex.login(window + 200, "N1CALL-10", "gateway-n1call-10")
    reported = []
    for r in records:
        reported.append(
            {
                "statement": jws.b64u(r["statement"]),
                "signature": jws.b64u(r["signature"]),
                "credential": jws.b64u(r["credential"]),
                "started": r["started"],
                "gateway_session": r["session"].hex(),
                "messages": handed_in(r["messages"])["messages"],
                "pickups": r["pickups"],
                "local_pickups": [rp.id_text(c) for c in r["local"]],
                "path": ["N2CALL-5", "N1CALL-10"],
            }
        )
    ex.call(window + 220, "N1CALL-10", "POST", "/reports", {"sessions": reported}, n1)
    return ex, n1, window, cursors["N1CALL-10"]


def make_local_delivery() -> dict[str, Any]:
    _, records = local_sessions()
    now = LOCAL_A
    seen = {
        "facundo@radiopost.org": T_CUT - 60,  # the copy of #421, polled a minute before the cut
        "shelter3@radiopost.org": FWD_CLOCK,  # the copy of #501, in session-04
        "ana@hampost.org": now - 29 * DAY,
        "kq4sax@radiopost.org": now - 31 * DAY,
    }
    local = fwd.local_users(seen, now)

    def message(to: list[str], cc: list[str] | None = None) -> bytes:
        fields: dict[str, Any] = {
            "v": 1,
            "from": "shelter3@radiopost.org",
            "to": to,
            "date": now,
            "subject": "Test",
            "body": "",
        }
        if cc:
            fields["cc"] = cc
        return cbor.encode(rp.message_map(fields))

    cases_in = [
        ("to Facundo, a copy here a minute before the cut", message(["facundo@radiopost.org"])),
        ("to Maria, never a copy here", message(["maria@example.com"])),
        ("to Maria, cc Facundo", message(["maria@example.com"], ["facundo@radiopost.org"])),
        ("to Ana, of another post office, a copy here 29 days ago", message(["ana@hampost.org"])),
        ("to KQ4SAX, his last copy here 31 days ago", message(["kq4sax@radiopost.org"])),
        (
            "to Facundo and Shelter 3, a copy here this afternoon",
            message(["facundo@radiopost.org", "shelter3@radiopost.org"]),
        ),
    ]
    cases = [
        {"what": w, "message_hex": m.hex(), "expect": fwd.local_recipients(m, local)}
        for w, m in cases_in
    ]
    confirm(  # RP-7 §4.4: a local user had a copy here in the last 30 days
        cases,
        {
            "to Facundo, a copy here a minute before the cut": ["facundo@radiopost.org"],
            "to Maria, never a copy here": [],
            "to Maria, cc Facundo": ["facundo@radiopost.org"],
            "to Ana, of another post office, a copy here 29 days ago": ["ana@hampost.org"],
            "to KQ4SAX, his last copy here 31 days ago": [],
            "to Facundo and Shelter 3, a copy here this afternoon": [
                "facundo@radiopost.org",
                "shelter3@radiopost.org",
            ],
        },
    )
    entries = []
    held = {rp.credential_hash(r["credential"]): r["credential"] for r in offline_records()}
    for r in records:
        pdu = fwd.proof_pdu(r, hashed=True)  # N1CALL-10 took both credentials in session-04
        assert fwd.check_proof(pdu, "N2CALL-5", "N1CALL-10", po_keys(), {}, held=held)[0] == "ok"
        entries.append(
            {
                "messages": r["messages"],
                "local_pickups": [c.hex() for c in r["local"]],
                "record_id_hex": fwd.record_id(r["statement"]).hex(),
                "proof": pdu_entry("PROOF", pdu),
            }
        )

    ex, n1, window, cursor = local_report(records)
    ex.note(
        "Shelter 3's message reaches Facundo's mailbox as #422, and is marked picked up at once: "
        "he took it at N2CALL-5. The forward item placed for it at N1CALL-10 is dropped, so "
        "N1CALL-10's next poll carries both, and drops it; #501, which Shelter 3 picked up, is "
        "dropped too."
    )
    ex.call(window + 230, "N1CALL-10", "GET", f"/copies?after={cursor}&wait=60", None, n1)
    ex.note("Facundo calls N1CALL-10 later: neither #421 nor #422 is offered; he has both.")
    body = signed_session("facundo", "N1CALL-10", [], window + 900, "local facundo")
    opened = ex.call(window + 960, "N1CALL-10", "POST", "/sessions", body, n1)
    assert opened["waiting"] == []
    out = header(
        "Local delivery: who is local, the records, and the post office's side",
        {"RP-7": "§3.2, §4.4", "RP-5": "§5.6"},
    )
    out["rule"] = (
        "A gateway that received copies for `copies_seen`'s addresses at those times counts as "
        "local, at `now`, those seen in the last 30 days; given a case's message it keeps the "
        "message for `expect`, its local recipients (RP-7 §4.4). Each record's PROOF is exact, "
        "its credential by hash (key 9: N1CALL-10 holds both), Shelter 3's with its pickup of "
        "#501 and Facundo's with his local pickup (key 8). The post office's exchanges read as "
        "in api-exchanges.json."
    )
    out["context"] = {"gateway": "N2CALL-5", "now": now, "copies_seen": seen}
    out["cases"] = cases
    out["records"] = entries
    out["post_office"] = ex.log
    return out


# -- queued.json ---------------------------------------------------------------------------------


def make_queued() -> dict[str, Any]:
    """RP-7 §3.6: what the sender's client offers again, a cut-off gateway's answer, the view."""
    _, records = local_sessions()
    ex, _, window, _ = local_report(records)
    cid = encoded(LOCAL_NAME)[1]
    other = encoded("maria-421")[1]
    took = LOCAL_A
    cut = {"N4CALL-2": True}
    offer_in = [
        ("N1CALL-10, never called before; 3 days on", "N1CALL-10", {}, 3 * DAY),
        ("N2CALL-5, which queued it, cut off then; 3 days on", "N2CALL-5", {}, 3 * DAY),
        ("N2CALL-5, cut off then; 30 days on", "N2CALL-5", {}, 30 * DAY),
        ("N2CALL-5, online when last called; 3 days on", "N2CALL-5", {"N2CALL-5": False}, 3 * DAY),
        ("N4CALL-2, cut off when last called; 3 days on", "N4CALL-2", cut, 3 * DAY),
        ("N4CALL-2, cut off when last called; 31 days on", "N4CALL-2", cut, 31 * DAY),
        ("N4CALL-2, never called; 3 days on", "N4CALL-2", {}, 3 * DAY),
    ]
    offer_cases = []
    for what, calling, last, later in offer_in:
        then = {"N2CALL-5": True, **last}
        offer_cases.append(
            {
                "what": what,
                "queued_at": took,
                "calling": calling,
                "cut_off_when_last_called": then,
                "now": took + later,
                "expect": fwd.offer_again(took, calling, then, took + later),
            }
        )
    confirm(  # RP-7 §3.6: not to a gateway cut off when last called, until 30 days have passed
        offer_cases,
        {
            "N1CALL-10, never called before; 3 days on": True,
            "N2CALL-5, which queued it, cut off then; 3 days on": False,
            "N2CALL-5, cut off then; 30 days on": True,
            "N2CALL-5, online when last called; 3 days on": True,
            "N4CALL-2, cut off when last called; 3 days on": False,
            "N4CALL-2, cut off when last called; 31 days on": True,
            "N4CALL-2, never called; 3 days on": True,
        },
    )
    now = took + 40 * DAY
    answer_in = [
        ("in its queue", {cid}, {}),
        ("passed on to N1CALL-10 10 days ago", set(), {cid: now - 10 * DAY}),
        ("passed on 30 days ago", set(), {cid: now - 30 * DAY}),
        ("its record dropped unsent", set(), {}),
        ("another message in its queue", {other}, {}),
    ]
    answer_cases = [
        {
            "what": what,
            "offer_hex": cid.hex(),
            "queue_hex": sorted(c.hex() for c in queue),
            "passed_on": {c.hex(): t for c, t in passed.items()},
            "now": now,
            "expect": fwd.answer_offer(cid, queue, passed, now),
        }
        for what, queue, passed in answer_in
    ]
    confirm(  # RP-7 §3.6: `already-have` while queued, and for 30 days after passing it on
        answer_cases,
        {
            "in its queue": "already-have",
            "passed on to N1CALL-10 10 days ago": "already-have",
            "passed on 30 days ago": "accept",
            "its record dropped unsent": "accept",
            "another message in its queue": "accept",
        },
    )
    view_in = [
        ("receipted by N2CALL-5 an hour ago", False, 3600),
        ("declined `already-have` by N1CALL-10, online", True, 3 * DAY),
        ("unconfirmed after 29 days", False, 29 * DAY),
        ("unconfirmed after 30 days", False, 30 * DAY),
    ]
    view_cases = [
        {
            "what": what,
            "queued_at": took,
            "confirmed": confirmed,
            "now": took + later,
            "expect": fwd.sender_view(took, confirmed, took + later),
        }
        for what, confirmed, later in view_in
    ]
    confirm(  # RP-7 §3.6: sent once confirmed; after 30 days, perhaps dropped
        view_cases,
        {
            "receipted by N2CALL-5 an hour ago": "queued",
            "declined `already-have` by N1CALL-10, online": "sent",
            "unconfirmed after 29 days": "queued",
            "unconfirmed after 30 days": "may have been dropped",
        },
    )

    ex.log = []
    ex.note(
        "As in local-delivery.json, N1CALL-10 has reported Shelter 3's record: radiopost.org holds "
        "its message. Shelter 3's client still shows it queued at N2CALL-5, and offers it again."
    )
    at = window + 1800
    started = at + 20
    hello = hello_pdu(
        "shelter3-n3call",
        "N1CALL-10",
        at,
        stream("queued nonce", 8),
        offers=[LOCAL_NAME],
        cursor=0,
        full_credential=True,
    )
    gateway_accepts(hello, "shelter3-n3call", "N1CALL-10", started)
    sid = stream("queued id", 8)
    n1 = ex.login(started, "N1CALL-10", "gateway-n1call-10")
    request = {
        "statement": jws.b64u(cbor.encode(rp.statement_from_hello(hello))),
        "signature": jws.b64u(hello[15]),
        "credential": jws.b64u(credential("shelter3-n3call")),
        "started": started,
        "gateway_session": sid.hex(),
        "cursor": 0,
    }
    opened = ex.call(started, "N1CALL-10", "POST", "/sessions", request, n1)
    assert opened["accept"] == [] and opened["waiting"] == []
    assert opened["decline"] == [{"id": rp.id_text(cid), "code": 14, "name": "already-have"}]
    ex.call(started + 30, "N1CALL-10", "DELETE", f"/sessions/{opened['session']}", None, n1)
    welcome = welcome_pdu(
        sid,
        started,
        accept=[],
        decline=[[cid, rp.ERRORS["already-have"]]],
        waiting=[],
        caps=GATEWAY_CAPS,
        online=True,
        cached=True,
    )
    seq = [
        ("client", [("HELLO", hello)]),
        ("gateway", [("WELCOME", welcome), ("DONE", DONE)]),
        ("client", [("DONE", DONE)]),
    ]
    transmissions, streams = write_transmissions(seq)
    out = header(
        "Mail queued at a cut-off gateway: offered again, answered, shown",
        {"RP-7": "§3.6", "RP-3": "§5.2, §5.5", "RP-5": "§5.2"},
    )
    out["rule"] = (
        "A client offers a queued message again to the gateway it calls as `offer_cases` say, a "
        "cut-off gateway answers an offer as `answer_cases` say, and the client shows the message "
        "as `view_cases` say (RP-7 §3.6). The session is as in session-01.json, and the post "
        "office's exchanges as in api-exchanges.json."
    )
    out["context"] = {
        "message": LOCAL_NAME,
        "content_id_hex": cid.hex(),
        "queued_at": "N2CALL-5, cut off, at `queued_at` (session-05.json, session a)",
    }
    out["offer_cases"] = offer_cases
    out["answer_cases"] = answer_cases
    out["view_cases"] = view_cases
    out["session"] = {
        "story": "Shelter 3 reaches N1CALL-10, online, and offers its queued message again. The "
        "post office already holds it, so N1CALL-10 declines it `already-have`, and Shelter 3's "
        "client shows it sent. The offer cost its entries in HELLO and WELCOME.",
        "client_clock": at,
        "gateway_clock": started,
        "transmissions": transmissions,
        "streams": streams,
    }
    out["post_office"] = ex.log
    return out


# -- api-account.json ----------------------------------------------------------------------------


def signed_session(
    cred: str, gateway: str, names: list[str], at: int, label: str
) -> dict[str, Any]:
    """RP-5 §5.2's body for a session in which `cred`'s holder offers `names` through `gateway`."""
    spec = CREDENTIALS[cred]
    ids = [encoded(n)[1] for n in names]
    stmt = rp.statement(
        spec["address"], spec["operator"], gateway, ids, time=at, nonce=stream(f"{label} nonce", 8)
    )
    raw = cbor.encode(stmt)
    return {
        "statement": jws.b64u(raw),
        "signature": jws.b64u(rp.sign(rp.CTX_SESSION, raw, SECRET[spec["device"]])),
        "credential": jws.b64u(credential(cred)),
        "started": at + 30,
        "gateway_session": stream(f"{label} id", 8).hex(),
        "cursor": 0,
    }


def account_renewal() -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """RP-5 §6.2 and RP-3 §5.8: renewing over the Internet and by radio."""
    office = post_office()
    office.next_serial = 1101
    ex = Exchanges(office)
    at = T_ACCOUNT
    ex.note("Facundo renews his laptop's credential: same account, operator, key and flags.")
    token = ex.login(at, "Facundo's laptop", "facundo")
    renewed = jws.unb64u(
        ex.call(at + 5, "Facundo's laptop", "POST", "/credentials", {}, token)["credential"]
    )
    assert renewed is not None
    ex.note("He logs in with the new credential; the old one still works until it expires.")
    ex.login_with(at + 60, "Facundo's laptop", renewed, "device:facundo")
    ex.login(at + 90, "Facundo's laptop", "facundo")
    ex.note("A gateway's credential expires too: KK4ODA-1 renews its own.")
    kk = ex.login(at + 120, "KK4ODA-1", "gateway-kk4oda-1")
    ex.call(at + 125, "KK4ODA-1", "POST", "/credentials", {}, kk)
    story = ex.log

    cases = []
    expired_office = post_office()
    ex = Exchanges(expired_office)
    ex.login(T_EXPIRES, "Facundo's laptop", "facundo")
    cases.append(
        {
            "what": "at its expiry a credential cannot log in, so cannot renew: the device enrolls "
            "again with a new code",
            "exchanges": ex.log,
        }
    )
    revoked_office = post_office()
    revoked_office.revoked = {
        "radiopost.org": {"serials": [CREDENTIALS["facundo"]["serial"]], "kids": []}
    }
    ex = Exchanges(revoked_office)
    ex.login(at, "Facundo's laptop", "facundo")
    cases.append({"what": "a revoked credential cannot log in", "exchanges": ex.log})

    radio_office = post_office()
    radio_office.next_serial = 1101
    deliver_waiting(radio_office, None)
    ex = Exchanges(radio_office)
    hello = dict(session_pdus()[0][1][0][1])
    hello[12] = {**hello[12], 3: True}
    assert rp.statement_from_hello(hello) == statement_session()
    ex.note(
        "Session-01 again, with `renew` in HELLO's caps (RP-3 §5.1). The caps are not in the "
        "statement, so the signature is the same."
    )
    ex.log.append({"rp3": pdu_entry("HELLO", hello)})
    gw = ex.login(T_SESSION - 600, "N1CALL-10", "gateway-n1call-10")
    body = session_request(hello, T_SESSION + 40) | {"renew": True}
    opened = ex.call(T_SESSION + 40, "N1CALL-10", "POST", "/sessions", body, gw)
    by_radio = jws.unb64u(opened["credential"])
    assert by_radio is not None
    ex.note("The gateway hands the new credential to Facundo in the session (RP-3 §5.8).")
    ex.log.append({"rp3": pdu_entry("CREDENTIAL", {0: 9, 1: by_radio})})
    cases.append({"what": "renewal by radio, asked for in HELLO", "exchanges": ex.log})
    return story, cases


def account_accept_list() -> list[dict[str, Any]]:
    """RP-5 §6.5 and RP-6 §2.2: reading and changing a list, a shared one, and the door."""
    office = post_office()
    office.accept = {
        "facundo@radiopost.org": list(FACUNDO_ACCEPT),
        "shelter3@radiopost.org": ["@redcross.example"],
    }
    ex = Exchanges(office)
    at = T_ACCOUNT

    def door(when: int, sender: str) -> None:
        raw = door_email(sender)
        ctx = {
            "accounts": dict(office.accept),
            "known_post_offices": ["radiopost.org", "hampost.org"],
            "max_email_bytes": 10485760,
            "dmarc": "pass",
            "dkim_pass": [],
            "sent_message_ids": {},
        }
        result = mail.door({"mail_from": sender, "rcpt_to": "facundo@radiopost.org"}, raw, ctx)
        ex.log.append(
            {
                "at": when,
                "smtp": {
                    "mail_from": sender,
                    "rcpt_to": "facundo@radiopost.org",
                    "email": raw.decode("utf-8"),
                    "dmarc": "pass",
                    "dkim_pass": [],
                    "expect": result,
                },
            }
        )

    ex.note("Facundo reads his list, then changes it.")
    me = ex.login(at, "Facundo's laptop", "facundo")
    ex.call(at + 1, "Facundo's laptop", "GET", "/accept-list", None, me)
    ex.call(
        at + 2,
        "Facundo's laptop",
        "POST",
        "/accept-list",
        {
            "add": ["Ana.Lopez@Example.ORG", "@weather.example", "maria@example.com"],
            "remove": ["@REDCROSS.example"],
        },
        me,
    )
    ex.call(at + 3, "Facundo's laptop", "POST", "/accept-list", {"add": ["not an address"]}, me)
    ex.note("Shelter 3's list is one, shared by its two operators.")
    n3 = ex.login(at + 10, "Shelter 3 (N3CALL)", "shelter3-n3call")
    ex.call(at + 11, "Shelter 3 (N3CALL)", "POST", "/accept-list", {"add": ["@fema.example"]}, n3)
    n4 = ex.login(at + 12, "Shelter 3 (N4CALL)", "shelter3-n4call")
    ex.call(at + 13, "Shelter 3 (N4CALL)", "GET", "/accept-list", None, n4)
    ex.note("José is on no list: his mail is refused at the door.")
    door(at + 20, "jose@example.net")
    ex.note("Facundo writes to José by radio. When his message is accepted, José joins his list.")
    gw = ex.login(at + 30, "N1CALL-10", "gateway-n1call-10")
    sid = ex.call(
        at + 60,
        "N1CALL-10",
        "POST",
        "/sessions",
        signed_session("facundo", "N1CALL-10", ["facundo-to-jose"], at + 30, "account jose"),
        gw,
    )["session"]
    ex.call(
        at + 90,
        "N1CALL-10",
        "POST",
        f"/sessions/{sid}/messages",
        handed_in(["facundo-to-jose"]),
        gw,
    )
    ex.call(at + 91, "N1CALL-10", "DELETE", f"/sessions/{sid}", None, gw)
    ex.call(at + 100, "Facundo's laptop", "GET", "/accept-list", None, me)
    ex.note("José's answer now passes the door.")
    door(at + 200, "jose@example.net")
    ex.note("Shelter 3 turns `accept_on_send` off: what it sends adds no one.")
    settings = {"usual_gateways": [], "auto_bytes": AUTO, "accept_on_send": False}
    ex.call(at + 300, "Shelter 3 (N3CALL)", "PUT", "/settings", settings, n3)
    sid = ex.call(
        at + 360,
        "N1CALL-10",
        "POST",
        "/sessions",
        signed_session(
            "shelter3-n3call", "N1CALL-10", ["shelter3-priority"], at + 330, "account shelter"
        ),
        gw,
    )["session"]
    ex.call(
        at + 390,
        "N1CALL-10",
        "POST",
        f"/sessions/{sid}/messages",
        handed_in(["shelter3-priority"]),
        gw,
    )
    ex.call(at + 400, "Shelter 3 (N4CALL)", "GET", "/accept-list", None, n4)
    return ex.log


def make_api_account() -> dict[str, Any]:
    story, cases = account_renewal()
    out = header(
        "Post office API: renewal and the accept list",
        {"RP-5": "§5.2, §6.2, §6.3, §6.5", "RP-6": "§2.2", "RP-3": "§5.1, §5.8"},
    )
    out["rule"] = (
        "As in api-exchanges.json: each exchange is a request to the post office at its clock and "
        "its answer, compared as JSON values. A renewed credential is new bytes from the post "
        "office's key (`po:radiopost.org`), for the same account, operator, device key and flags, "
        "with the next serial and issued at the request's time. An `smtp` entry is the door's "
        "answer (email-door.json) for the accept list as it stands at that moment. An `rp3` entry "
        "is a PDU on the air."
    )
    out["context"] = {
        "post_office": "radiopost.org",
        "next_serial": 1101,
        "accept_lists_at_start": {
            "facundo@radiopost.org": FACUNDO_ACCEPT,
            "shelter3@radiopost.org": ["@redcross.example"],
        },
    }
    accept_list = account_accept_list()
    assert all(e["response"]["status"] < 300 for e in story if "request" in e)
    confirm(  # RP-5 §3.2, §6.2
        cases,
        {
            "at its expiry a credential cannot log in, so cannot renew: the device enrolls "
            "again with a new code": ["200", "422 credential-expired"],
            "a revoked credential cannot log in": ["200", "422 credential-revoked"],
            "renewal by radio, asked for in HELLO": ["200", "200", "201"],
        },
        answers,
    )
    seen: list[Any] = []
    for e in accept_list:
        if e.get("request", {}).get("path") == "/accept-list":
            body = e["response"]["body"]
            seen.append(body["entries"] if "entries" in body else answer_of(e))
        if "smtp" in e:
            seen.append((e["smtp"]["expect"]["rcpt"], e["smtp"]["expect"]["data"]))
    facundo = ["maria@example.com", "ana.lopez@example.org", "@weather.example"]
    shelter = ["@redcross.example", "@fema.example"]
    assert seen == [  # RP-5 §6.5, RP-6 §2.2
        ["maria@example.com", "@redcross.example"],
        facundo,  # lowercase, a duplicate not added, a domain removed in other capitals
        "400 malformed",
        shelter,  # N3CALL adds,
        shelter,  # and N4CALL sees the same list
        ("550 5.7.1", None),  # José on no list
        [*facundo, "jose@example.net"],  # written to, so added, in lowercase
        ("250 2.1.5", "250 2.0.0"),
        shelter,  # accept_on_send off: nothing added
    ]
    out["renewal"] = story
    out["renewal_cases"] = cases
    out["accept_list"] = accept_list
    return out


# -- email-door.json, email-in.json, email-out.json --------------------------------------------


def crlf(text: str) -> bytes:
    return text.replace("\n", "\r\n").encode("utf-8")


def mail_date(seconds: int, hours: int) -> str:
    """An RFC 5322 date as a sender in another time zone writes it."""
    zone = timezone(timedelta(hours=hours))
    return email.utils.format_datetime(datetime.fromtimestamp(seconds, zone))


MARIA_ID = "<CAF.maria.419@mail.example.com>"
MARIA_421_ID = "<CAF.maria.421@mail.example.com>"
FACUNDO_ACCEPT = ["maria@example.com", "@redcross.example"]
ATTACH_BYTES = 16384  # Facundo's setting (RP-5 §6.3)
MAX_AIR_BYTES = 65536  # radiopost.org's limit (RP-5 §4.1)


def maria_email(name: str) -> bytes:
    """Maria's mail as her provider sends it; it becomes `name` exactly (messages.json)."""
    f = MESSAGES[name]
    head = (
        "From: Maria Alvarez <maria@example.com>\n"
        "To: Facundo <facundo@radiopost.org>\n"
        f"Subject: {f['subject']}\n"
        f"Date: {mail_date(f['date'], -6)}\n"
        f"Message-ID: {MARIA_ID if name == 'maria-419' else MARIA_421_ID}\n"
        "MIME-Version: 1.0\n"
    )
    if "files" not in f:
        return crlf(
            head + 'Content-Type: text/plain; charset="UTF-8"\n'
            "Content-Transfer-Encoding: 7bit\n\n" + f["body"]
        )
    photo = f["files"][0]
    b64 = base64.b64encode(photo["data"]).decode()
    lines = "\n".join(b64[i : i + 76] for i in range(0, len(b64), 76))
    return crlf(
        head + 'Content-Type: multipart/mixed; boundary="b1"\n\n'
        "--b1\n"
        'Content-Type: text/plain; charset="UTF-8"\n\n' + f["body"] + "--b1\n"
        f"Content-Type: {photo['type']}\n"
        f'Content-Disposition: attachment; filename="{photo["name"]}"\n'
        "Content-Transfer-Encoding: base64\n\n" + lines + "\n--b1--\n"
    )


def facundo_reply_id() -> str:
    return f"<{rp.id_text(encoded('facundo-reply')[1])}@radiopost.org>"


def inbound_emails() -> list[tuple[str, str, bytes, int, dict[str, Any]]]:
    """(name, what it shows, the email, its serial, extra context) for §2.4."""
    subject = "Re: Are you safe? ¡Qué alivio!"
    quoted = crlf(
        "From: Maria Alvarez <maria@example.com>\n"
        "To: Facundo <facundo@radiopost.org>\n"
        f"Subject: {Header(subject, 'utf-8').encode()}\n"
        f"Date: {mail_date(T_SESSION + 1800, -6)}\n"
        f"Message-ID: <CAF.maria.422@mail.example.com>\n"
        f"In-Reply-To: {facundo_reply_id()}\n"
        f"References: {MARIA_ID} {facundo_reply_id()}\n"
        "MIME-Version: 1.0\n"
        'Content-Type: text/plain; charset="UTF-8"\n'
        "Content-Transfer-Encoding: quoted-printable\n\n"
        "=C2=A1Que=CC=81 alivio! So glad to hear it. Tell us if you need anything.\n\n"
        "Maria\n\n"
        "On Mon, 9 Nov 2026 at 08:30, Facundo <facundo@radiopost.org>\n"
        "wrote:\n"
        "> We are fine. Power is out, but the radio works.\n"
        ">\n"
        "> --=20\n"
        "> Sent by amateur radio through RadioPost. Please reply in plain text, under 20 kB.\n"
        "> Amateur radio carries no business content.\n"
    )
    outlook = crlf(
        "From: Jorge Ruiz <jorge@redcross.example>\n"
        "To: Facundo <facundo@RadioPost.org>, EOC <eoc@radiopost.org>\n"
        "Cc: Shelter 3 <shelter3@RADIOPOST.ORG>\n"
        "Subject: RE: Shelter list\n"
        f"Date: {mail_date(T_SESSION + 3600, -6)}\n"
        "Message-ID: <BY5PR.jorge.1@redcross.example>\n"
        "MIME-Version: 1.0\n"
        'Content-Type: text/plain; charset="us-ascii"\n\n'
        "Facundo, the list is in the next mail. Shelters 3 and 7 have room.\n\n"
        "Jorge\n\n"
        "________________________________\n"
        "From: Facundo <facundo@radiopost.org>\n"
        "Sent: Monday, November 9, 2026 9:00 AM\n"
        "To: Jorge Ruiz <jorge@redcross.example>\n"
        "Subject: Shelter list\n\n"
        "Can you send me the shelter list?\n"
    )
    alternative = crlf(
        "From: Red Cross Texas <updates@redcross.example>\n"
        "To: facundo@radiopost.org\n"
        "Subject: Shelter update\n"
        "Date: sometime Tuesday\n"
        "MIME-Version: 1.0\n"
        'Content-Type: multipart/alternative; boundary="alt"\n\n'
        "--alt\n"
        'Content-Type: text/plain; charset="utf-8"\n\n'
        "Shelters in Bastrop County are open. Water at the high school.\n"
        "--alt\n"
        'Content-Type: text/html; charset="utf-8"\n\n'
        "<p>Shelters in <b>Bastrop County</b> are open.</p>\n"
        "--alt--\n"
    )
    html_only = crlf(
        "From: Red Cross Texas <updates@redcross.example>\n"
        "To: facundo@radiopost.org\n"
        "Subject: Where to get water\n"
        f"Date: {mail_date(T_SESSION + 7200, -6)}\n"
        "MIME-Version: 1.0\n"
        'Content-Type: text/html; charset="utf-8"\n\n'
        "<html><head><style>p {color: red}</style></head><body>\n"
        '<p>Water &amp; ice at <a href="https://example.org/water">these places</a>.</p>\n'
        '<p>Fuel: see <a href="https://example.org/fuel">the fuel map</a>.</p>\n'
        "</body></html>\n"
    )
    pdf = stream("policy pdf", 40000)
    b64 = base64.b64encode(pdf).decode()
    held = crlf(
        "From: Maria Alvarez <maria@example.com>\n"
        "To: Facundo <facundo@radiopost.org>\n"
        "Subject: Insurance papers\n"
        f"Date: {mail_date(T_SESSION + 9000, -6)}\n"
        "MIME-Version: 1.0\n"
        'Content-Type: multipart/mixed; boundary="b2"\n\n'
        "--b2\n"
        'Content-Type: text/plain; charset="UTF-8"\n\n'
        "Here are the insurance papers, in case you need them.\n"
        "--b2\n"
        "Content-Type: application/pdf\n"
        'Content-Disposition: attachment; filename="policy.pdf"\n'
        "Content-Transfer-Encoding: base64\n\n"
        + "\n".join(b64[i : i + 76] for i in range(0, len(b64), 76))
        + "\n--b2--\n"
    )
    return [
        (
            "maria-419",
            "Maria's mail: plain text, a Date in her time zone",
            maria_email("maria-419"),
            419,
            {},
        ),
        (
            "maria-421",
            "a photo under Facundo's `attach_bytes`, carried in `files`",
            maria_email("maria-421"),
            421,
            {},
        ),
        (
            "maria-reply",
            "a reply: an encoded subject, quoted-printable text in decomposed form, the quotation "
            "under an `On … wrote:` of two lines trimmed, `reply_to` from `In-Reply-To`",
            quoted,
            422,
            {},
        ),
        (
            "jorge-outlook",
            "an Outlook reply: the history under the rule and `From:`/`Sent:` block trimmed; "
            "display names dropped, domains in lowercase",
            outlook,
            423,
            {},
        ),
        (
            "redcross-alternative",
            "multipart/alternative: the plain part is used; an unreadable Date gives the arrival",
            alternative,
            424,
            {},
        ),
        (
            "redcross-html",
            "HTML only: one rendering as text, the links listed at the end",
            html_only,
            425,
            {},
        ),
        (
            "maria-held",
            "a 40 kB attachment over Facundo's `attach_bytes`: held at the post office",
            held,
            426,
            {"held_refs": [stream("held ref 426", 8)]},
        ),
    ]


def inbound_context(extra: dict[str, Any]) -> dict[str, Any]:
    return {
        "arrival": T_SESSION + 10800,
        "attach_bytes": ATTACH_BYTES,
        "max_air_bytes": MAX_AIR_BYTES,
        "sent_message_ids": {facundo_reply_id(): encoded("facundo-reply")[1]},
        **extra,
    }


def json_fields(fields: dict[str, Any]) -> dict[str, Any]:
    out = to_json(fields)
    if "reply_to_hex" in out:
        out["reply_to_hex"] = fields["reply_to"].hex()
    return out


def dsn() -> bytes:
    """A delivery failure report (RFC 3464) for Facundo's reply to Maria."""
    return crlf(
        "From: Mail Delivery System <MAILER-DAEMON@mx.example.com>\n"
        "To: facundo@radiopost.org\n"
        "Subject: Undelivered Mail Returned to Sender\n"
        f"Date: {mail_date(T_SESSION + 400, 0)}\n"
        "MIME-Version: 1.0\n"
        'Content-Type: multipart/report; report-type=delivery-status; boundary="r"\n\n'
        "--r\n"
        "Content-Type: text/plain\n\n"
        "Your message could not be delivered.\n"
        "--r\n"
        "Content-Type: message/delivery-status\n\n"
        "Reporting-MTA: dns; mx.example.com\n\n"
        "Final-Recipient: rfc822; maria@example.com\n"
        "Action: failed\n"
        "Status: 5.2.2\n"
        "Diagnostic-Code: smtp; 552 5.2.2 Mailbox full\n"
        "--r\n"
        "Content-Type: text/rfc822-headers\n\n"
        "From: Facundo <facundo@radiopost.org>\n"
        "To: maria@example.com\n"
        "Subject: Re: Are you safe?\n"
        f"Message-ID: {facundo_reply_id()}\n"
        "--r--\n"
    )


def make_email_in() -> dict[str, Any]:
    cases = []
    for name, what, raw, serial, extra in inbound_emails():
        fields, from_html = mail.to_message(raw, inbound_context(extra))
        canonical = mail.encode_fields(fields)
        if name in MESSAGES:
            assert canonical == encoded(name)[0], f"{name} does not convert to messages.json's"
        cases.append(
            {
                "name": name,
                "what": what,
                "email": raw.decode("utf-8"),
                "serial": serial,
                "context": {k: v.hex() if isinstance(v, bytes) else v for k, v in extra.items()}
                | ({"held_refs": [r.hex() for r in extra["held_refs"]]} if extra else {}),
                "message": json_fields(fields),
                "canonical_hex": canonical.hex(),
                "content_id_text": rp.id_text(rp.content_id(canonical)),
                "exact": not from_html,
            }
        )
    bounce = mail.bounce(
        dsn(),
        {
            "domain": "radiopost.org",
            "sender": "facundo@radiopost.org",
            "arrival": T_SESSION + 405,
            "sent_message_ids": {facundo_reply_id(): encoded("facundo-reply")[1]},
        },
    )
    bounce_canonical = mail.encode_fields(bounce)
    between = []
    sent_headers, sent = outbound()["facundo-to-ana"]
    del sent_headers
    for what, raw in between_emails(sent):
        part = mail.find_part(raw)
        assert part is not None
        result, canonical = mail.check_part(part, "ana@hampost.org", po_keys(), {})
        used = "part" if canonical is not None else "email"
        if canonical is None:
            fields, _ = mail.to_message(
                raw,
                {
                    "arrival": T_SESSION + 120,
                    "attach_bytes": ATTACH_BYTES,
                    "max_air_bytes": MAX_AIR_BYTES,
                },
            )
            canonical = mail.encode_fields(fields)
        between.append(
            {
                "what": what,
                "email": raw.decode("utf-8"),
                "recipient": "ana@hampost.org",
                "part_check": result,
                "message_from": used,
                "canonical_hex": canonical.hex(),
                "content_id_text": rp.id_text(rp.content_id(canonical)),
            }
        )
    assert between[0]["canonical_hex"] == encoded("facundo-to-ana")[0].hex()
    confirm(  # RP-6 §4: the part used only when every check passes, recipient included
        between,
        {
            "the part as sent: its message is used, priority and content ID kept": ("ok", "part"),
            "the part carries another message of the same session, not addressed to Ana": (
                "not-authorized",
                "email",
            ),
            "the statement's signature changed": ("signature-invalid", "email"),
            "the part's message changed: its content ID is not in the statement": (
                "not-authorized",
                "email",
            ),
        },
        lambda c: (c["part_check"], c["message_from"]),
    )
    arrival = T_SESSION + 10800

    def made(c: dict[str, Any]) -> tuple[int, bool, bool, str]:
        m = c["message"]
        date = "arrival" if m["date"] == arrival else "header"
        return (m.get("flags", 0), "files" in m, "reply_to_hex" in m, date)

    confirm(  # RP-6 §2.4: (flags, files carried, reply_to set, where the date came from)
        cases,
        {
            "maria-419": (0, False, False, "header"),
            "maria-421": (0, True, False, "header"),  # 6 122 bytes: within attach_bytes
            "maria-reply": (1, False, True, "header"),  # trimmed; In-Reply-To one of ours
            "jorge-outlook": (1, False, False, "header"),
            "redcross-alternative": (0, False, False, "arrival"),  # Date unreadable
            "redcross-html": (0, False, False, "header"),
            "maria-held": (2, False, False, "header"),  # 40 kB over attach_bytes: held
        },
        made,
        by="name",
    )
    named = {c["name"]: c["message"] for c in cases}
    assert named["maria-reply"]["body"] == (
        "¡Qué alivio! So glad to hear it. Tell us if you need anything.\n\nMaria\n"
    )  # quoted-printable decoded, NFC, the quotation after `On … wrote:` cut (step 4)
    assert named["jorge-outlook"]["body"] == (
        "Facundo, the list is in the next mail. Shelters 3 and 7 have room.\n\nJorge\n"
    )
    assert named["jorge-outlook"]["to"] == ["facundo@radiopost.org", "eoc@radiopost.org"]
    assert named["jorge-outlook"]["cc"] == ["shelter3@radiopost.org"]  # domains in lowercase
    held = named["maria-held"]["held"][0]
    assert (held["name"], held["type"], held["size"]) == ("policy.pdf", "application/pdf", 40000)
    assert bounce["from"] == "postmaster@radiopost.org"  # RP-6 §3.3
    assert bounce["subject"] == "Not delivered: Re: Are you safe?"
    assert bounce["body"] == "maria@example.com: 552 5.2.2 Mailbox full\n"
    assert bounce["reply_to"] == encoded("facundo-reply")[1]
    out = header("Email in: what an email becomes", {"RP-6": "§2.4, §3.3, §4"})
    out["rule"] = (
        "A post office given `email` (and, for a case, its `context`) makes `message`: exactly "
        "`canonical_hex` and its content ID where `exact` is true. Where it is false the text "
        "came from HTML, and another rendering is as good (RP-6 §2.4 step 3); every other field "
        "must still match. The message is stored under `serial`. In `between`, the receiving post "
        "office checks the `application/radiopost-message` part (RP-6 §4) for `recipient`: "
        "`part_check` ok means it uses the part's message, anything else that it converts the "
        "email instead."
    )
    out["context"] = {
        "post_office": "radiopost.org",
        "recipient": "facundo@radiopost.org",
        "arrival": T_SESSION + 10800,
        "attach_bytes": ATTACH_BYTES,
        "max_air_bytes": MAX_AIR_BYTES,
        "sent_by_this_post_office": {facundo_reply_id(): encoded("facundo-reply")[1].hex()},
    }
    out["emails"] = cases
    out["bounce"] = {
        "what": "a delivery failure report for Facundo's reply to Maria (RP-6 §3.3)",
        "email": dsn().decode("utf-8"),
        "arrival": T_SESSION + 405,
        "message": json_fields(bounce),
        "canonical_hex": bounce_canonical.hex(),
        "content_id_text": rp.id_text(rp.content_id(bounce_canonical)),
    }
    out["between"] = between
    return out


def proof_of(name: str) -> dict[str, Any]:
    """Session-01's proof for one of its messages (RP-1 §6.4), as RP-6 §4's part carries it."""
    hello = session_pdus()[0][1][0][1]
    return {
        "air": air0(name),
        "credential": credential("facundo"),
        "statement": cbor.encode(rp.statement_from_hello(hello)),
        "signature": hello[15],
        "started": T_SESSION + 40,
    }


def outbound() -> dict[str, tuple[dict[str, str], bytes]]:
    """§3.1, §3.2: Facundo's two messages of session-01, and Shelter 3's priority message."""
    session01 = {
        "operator": "KK4ODA",
        "gateways": ["N1CALL-10"],
        "started": T_SESSION + 40,
        "session": SESSION_ID.hex(),
    }
    facundo = {"display_name": "Facundo", "radio_path": session01}
    return {
        "facundo-reply": mail.from_message(
            MESSAGES["facundo-reply"],
            facundo | {"message_ids": {encoded("maria-419")[1]: MARIA_ID}},
        ),
        "facundo-to-ana": mail.from_message(
            MESSAGES["facundo-to-ana"], facundo | {"proof": proof_of("facundo-to-ana")}
        ),
        "shelter3-priority": mail.from_message(
            MESSAGES["shelter3-priority"],
            {
                "display_name": "Shelter 3",
                "radio_path": {
                    "operator": "N3CALL",
                    "gateways": ["N2CALL-5"],
                    "started": T_SESSION - 1140,
                    "session": stream("shelter session id", 8).hex(),
                },
            },
        ),
    }


def between_emails(sent: bytes) -> list[tuple[str, bytes]]:
    """The email of facundo-to-ana as it reaches hampost.org, and three that must not pass."""
    good = mail.find_part(sent)
    assert good is not None
    head, _, _ = sent.partition(b"application/radiopost-message")

    def with_part(proof: dict[str, Any]) -> bytes:
        new = mail.base64_lines(mail.part_bytes(proof)).encode()
        old = mail.base64_lines(good).encode()
        assert sent.count(old) == 1
        return sent.replace(old, new)

    swapped = proof_of("facundo-to-ana") | {"air": air0("facundo-reply")}
    bent_sig = bytearray(proof_of("facundo-to-ana")["signature"])
    bent_sig[0] ^= 1
    changed_text = MESSAGES["facundo-to-ana"] | {"body": "Ana, send money. Facundo\n"}
    changed = proof_of("facundo-to-ana") | {"air": rp.air_form(mail.encode_fields(changed_text), 0)}
    del head
    return [
        ("the part as sent: its message is used, priority and content ID kept", sent),
        (
            "the part carries another message of the same session, not addressed to Ana",
            with_part(swapped),
        ),
        (
            "the statement's signature changed",
            with_part(proof_of("facundo-to-ana") | {"signature": bytes(bent_sig)}),
        ),
        ("the part's message changed: its content ID is not in the statement", with_part(changed)),
    ]


def make_email_out() -> dict[str, Any]:
    cases = []
    for name, (headers, raw) in outbound().items():
        cases.append(
            {
                "message": name,
                "headers": headers,
                "email": raw.decode("utf-8"),
                "part_hex": (mail.find_part(raw) or b"").hex() or None,
            }
        )
    out = header("Email out: what a message sent by radio becomes", {"RP-6": "§3.1, §3.2, §4"})
    out["rule"] = (
        "A post office given the named message (messages.json) and the `context` makes an email "
        "whose headers have the values in `headers`, compared after unfolding and RFC 2047 "
        "decoding, and whose decoded text is the message's body and the footer. `email` is one "
        "spelling of it, before DKIM signing: header order, folding and the MIME boundary are "
        "the writer's. `part_hex`, where present, is the `application/radiopost-message` part's "
        "CBOR, exact (RP-6 §4)."
    )
    out["context"] = {
        "facundo-reply": "display name Facundo; session-01 (KK4ODA via N1CALL-10, started "
        f"{T_SESSION + 40}, gateway session {SESSION_ID.hex()}); #419 came as {MARIA_ID}",
        "facundo-to-ana": "as facundo-reply; every recipient (ana@hampost.org) at a RadioPost "
        "post office, so the part is added, with session-01's proof",
        "shelter3-priority": "display name Shelter 3; N3CALL via N2CALL-5, started "
        f"{T_SESSION - 1140}, gateway session {stream('shelter session id', 8).hex()}; "
        "eoc@radiopost.org is in the same post office, so no part",
    }
    out["emails"] = cases
    return out


DOOR_CONTEXT = {
    "accounts": {
        "facundo@radiopost.org": FACUNDO_ACCEPT,
        "shelter3@radiopost.org": ["@redcross.example"],
    },
    "known_post_offices": ["radiopost.org", "hampost.org"],
    "max_email_bytes": 10485760,
}


def door_email(sender: str, extra: str = "", body: str = "Hello.\n") -> bytes:
    return crlf(
        f"From: {sender}\nTo: facundo@radiopost.org\nSubject: Test\n"
        f"Date: {mail_date(T_SESSION, 0)}\nMIME-Version: 1.0\n{extra}"
        + ("" if "Content-Type" in extra else 'Content-Type: text/plain; charset="utf-8"\n')
        + "\n"
        + body
    )


def make_email_door() -> dict[str, Any]:
    maria = door_email("Maria Alvarez <maria@example.com>")
    pgp = door_email(
        "maria@example.com",
        body="-----BEGIN PGP MESSAGE-----\nhQEMA1x\n-----END PGP MESSAGE-----\n",
    )
    smime_enc = door_email(
        "maria@example.com",
        'Content-Type: application/pkcs7-mime; smime-type=enveloped-data; name="smime.p7m"\n'
        "Content-Transfer-Encoding: base64\n",
        "MIAGCSqGSIb3DQEHA6CAMIACAQAxggE=\n",
    )
    smime_signed = door_email(
        "maria@example.com",
        'Content-Type: multipart/signed; protocol="application/pkcs7-signature"; '
        'micalg=sha-256; boundary="s"\n',
        "--s\nContent-Type: text/plain\n\nSigned, not encrypted.\n--s\n"
        'Content-Type: application/pkcs7-signature; name="smime.p7s"\n'
        "Content-Transfer-Encoding: base64\n\nMIAGCSqGSIb3DQEHAqCAMIACAQEx\n--s--\n",
    )
    encrypted = door_email(
        "maria@example.com",
        'Content-Type: multipart/encrypted; protocol="application/pgp-encrypted"; boundary="e"\n',
        "--e\nContent-Type: application/pgp-encrypted\n\nVersion: 1\n--e\n"
        "Content-Type: application/octet-stream\n\nhQEMA1x\n--e--\n",
    )
    exe = door_email(
        "maria@example.com",
        'Content-Type: multipart/mixed; boundary="x"\n',
        "--x\nContent-Type: text/plain\n\nSee the program.\n--x\n"
        "Content-Type: application/octet-stream\n"
        'Content-Disposition: attachment; filename="Photos.EXE"\n'
        "Content-Transfer-Encoding: base64\n\nTVqQAAMAAAAEAAAA\n--x--\n",
    )
    report = dsn()
    spam = door_email("MAILER-DAEMON@mx.example.com")
    sent = {facundo_reply_id(): encoded("facundo-reply")[1].hex()}
    base = {"dmarc": "pass", "dkim_pass": []}
    cases_in: list[tuple[str, str, str, bytes, dict[str, Any]]] = [
        (
            "Maria, on Facundo's list, DMARC pass",
            "maria@example.com",
            "facundo@radiopost.org",
            maria,
            base,
        ),
        (
            "an address that does not exist",
            "maria@example.com",
            "nobody@radiopost.org",
            maria,
            base,
        ),
        (
            "a sender not on the list",
            "stranger@example.net",
            "facundo@radiopost.org",
            door_email("stranger@example.net"),
            base,
        ),
        (
            "on the list with other capitals",
            "Maria@EXAMPLE.com",
            "facundo@radiopost.org",
            door_email("Maria@EXAMPLE.com"),
            base,
        ),
        (
            "a domain on the list (`@redcross.example`)",
            "jorge@redcross.example",
            "facundo@radiopost.org",
            door_email("jorge@redcross.example"),
            base,
        ),
        (
            "a subdomain of a listed domain is not that domain",
            "x@mail.redcross.example",
            "facundo@radiopost.org",
            door_email("x@mail.redcross.example"),
            base,
        ),
        (
            "a user of a known RadioPost post office, DKIM pass for its domain",
            "ana@hampost.org",
            "facundo@radiopost.org",
            door_email("Ana <ana@hampost.org>"),
            {"dmarc": "pass", "dkim_pass": ["hampost.org"]},
        ),
        (
            "claims a known post office's domain, but no DKIM signature passes for it",
            "ana@hampost.org",
            "facundo@radiopost.org",
            door_email("Ana <ana@hampost.org>"),
            {"dmarc": "none", "dkim_pass": []},
        ),
        (
            "envelope sender on the list, header sender not",
            "maria@example.com",
            "facundo@radiopost.org",
            door_email("someone@example.net"),
            base,
        ),
        (
            "DMARC fails for the `From:` domain",
            "maria@example.com",
            "facundo@radiopost.org",
            maria,
            {"dmarc": "fail", "dkim_pass": []},
        ),
        (
            "over the post office's `max_email_bytes` (here 100)",
            "maria@example.com",
            "facundo@radiopost.org",
            maria,
            base | {"max_email_bytes": 100},
        ),
        ("inline PGP", "maria@example.com", "facundo@radiopost.org", pgp, base),
        (
            "PGP/MIME, multipart/encrypted",
            "maria@example.com",
            "facundo@radiopost.org",
            encrypted,
            base,
        ),
        ("S/MIME enveloped-data", "maria@example.com", "facundo@radiopost.org", smime_enc, base),
        (
            "S/MIME signed, not encrypted",
            "maria@example.com",
            "facundo@radiopost.org",
            smime_signed,
            base,
        ),
        ("an executable attachment", "maria@example.com", "facundo@radiopost.org", exe, base),
        (
            "a delivery report about a message this post office sent, null sender",
            "",
            "facundo@radiopost.org",
            report,
            {"dmarc": "none", "dkim_pass": []},
        ),
        (
            "null sender, not a delivery report",
            "",
            "facundo@radiopost.org",
            spam,
            {"dmarc": "none", "dkim_pass": []},
        ),
    ]
    cases = []
    for what, sender, rcpt, raw, verdicts in cases_in:
        ctx = DOOR_CONTEXT | verdicts | {"sent_message_ids": sent}
        replies = mail.door({"mail_from": sender, "rcpt_to": rcpt}, raw, ctx)
        case = {
            "what": what,
            "mail_from": sender,
            "rcpt_to": rcpt,
            "email": raw.decode("utf-8"),
            "dmarc": verdicts["dmarc"],
            "dkim_pass": verdicts["dkim_pass"],
        }
        if "max_email_bytes" in verdicts:
            case["max_email_bytes"] = verdicts["max_email_bytes"]
        cases.append(case | {"expect": replies})
    taken, at_data = ("250 2.1.5", "250 2.0.0"), "250 2.1.5"
    confirm(  # RP-6 §2.1, in the table's order; §2.2 for the list
        cases,
        {
            "Maria, on Facundo's list, DMARC pass": taken,
            "an address that does not exist": ("550 5.1.1", None),
            "a sender not on the list": ("550 5.7.1", None),
            "on the list with other capitals": taken,
            "a domain on the list (`@redcross.example`)": taken,
            "a subdomain of a listed domain is not that domain": ("550 5.7.1", None),
            "a user of a known RadioPost post office, DKIM pass for its domain": taken,
            "claims a known post office's domain, but no DKIM signature passes for it": (
                at_data,
                "550 5.7.1",
            ),
            "envelope sender on the list, header sender not": (at_data, "550 5.7.1"),
            "DMARC fails for the `From:` domain": (at_data, "550 5.7.1"),
            "over the post office's `max_email_bytes` (here 100)": (at_data, "552 5.3.4"),
            "inline PGP": (at_data, "554 5.7.1"),
            "PGP/MIME, multipart/encrypted": (at_data, "554 5.7.1"),
            "S/MIME enveloped-data": (at_data, "554 5.7.1"),
            "S/MIME signed, not encrypted": taken,
            "an executable attachment": (at_data, "554 5.7.1"),
            "a delivery report about a message this post office sent, null sender": taken,
            "null sender, not a delivery report": (at_data, "550 5.7.1"),
        },
        lambda c: (c["expect"]["rcpt"], c["expect"]["data"]),
    )
    out = header("Email at the door: taken or refused", {"RP-6": "§2.1, §2.2"})
    out["rule"] = (
        "A post office with `context`'s accounts, accept lists and known post offices, given the "
        "envelope, the email and the mail server's DMARC and DKIM verdicts, answers `RCPT TO` and "
        "then `DATA` with `expect` (code and enhanced status; the text is its own). `data` is "
        "null when the recipient was refused. A case's `max_email_bytes` replaces the context's."
    )
    out["context"] = DOOR_CONTEXT | {"sent_by_this_post_office": sent}
    out["cases"] = cases
    return out


# -- dns.json and api.json -----------------------------------------------------------------------


def make_dns() -> dict[str, Any]:
    pub = PUBLIC["po:radiopost.org"]
    b64 = base64.b64encode(pub).decode()
    kid = rp.kid(pub).hex()
    texts = [
        ("an API record", f"v=RP1; t=api; url={PO_RADIOPOST}"),
        ("a key record", f"v=RP1; t=key; kid={kid}; p={b64}"),
        ("no spaces", f"v=RP1;t=key;kid={kid};p={b64}"),
        ("spaces around everything, a `;` at the end", f"  v=RP1 ; t=key ; kid={kid} ; p={b64} ;"),
        ("an unknown tag", f"v=RP1; t=key; kid={kid}; p={b64}; note=rotated 2026"),
        ("another version", f"v=RP2; t=key; kid={kid}; p={b64}"),
        ("a `kid` that is not the key's", f"v=RP1; t=key; kid=00000000; p={b64}"),
        (
            "a key of 31 bytes",
            f"v=RP1; t=key; kid={kid}; p={base64.b64encode(pub[:31]).decode()}",
        ),
        ("a tag twice", f"v=RP1; t=key; kid={kid}; kid={kid}; p={b64}"),
        ("an API URL that is not https", "v=RP1; t=api; url=http://po.radiopost.org/rp/v1"),
        ("no `t`", f"v=RP1; kid={kid}; p={b64}"),
        ("not RadioPost's: an SPF record", "v=spf1 mx -all"),
    ]
    cases = [{"what": w, "txt": t, "result": rp.parse_txt(t)} for w, t in texts]
    confirm(  # RP-1 §7.1: unknown tags are ignored; these records are ignored whole
        cases,
        {
            "an API record": "read",
            "a key record": "read",
            "no spaces": "read",
            "spaces around everything, a `;` at the end": "read",
            "an unknown tag": "read",
            "another version": "ignored",
            "a `kid` that is not the key's": "ignored",
            "a key of 31 bytes": "ignored",
            "a tag twice": "ignored",
            "an API URL that is not https": "ignored",
            "no `t`": "ignored",
            "not RadioPost's: an SPF record": "ignored",
        },
        lambda c: "read" if c["result"] else "ignored",
    )
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
    "session-04.json": make_forwarding_session,
    "session-05.json": make_local_session,
    "rpmi-lines.json": make_rpmi_lines,
    "rpmi-conversations.json": make_rpmi_conversations,
    "dns.json": make_dns,
    "api.json": make_api,
    "jws.json": make_jws,
    "email-door.json": make_email_door,
    "email-in.json": make_email_in,
    "email-out.json": make_email_out,
    "api-exchanges.json": make_api_exchanges,
    "api-account.json": make_api_account,
    "forwarding.json": make_forwarding,
    "local-delivery.json": make_local_delivery,
    "queued.json": make_queued,
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
    if name == "pdus.json":
        for pdu in data["pdus"]:
            assert rp.unframe(bytes.fromhex(pdu["frame_hex"])) == cbor.decode(
                bytes.fromhex(pdu["pdu_hex"])
            )
        for bad in data["invalid"]:
            try:
                rp.unframe(bytes.fromhex(bad["frame_hex"]))
            except ValueError as err:
                assert str(err) == bad["expect"]
                continue
            raise AssertionError(f"took a frame RP-3 §2 refuses: {bad['why']}")
    if name == "rpmi-conversations.json":
        check_rpmi(data)
    if name == "jws.json":
        check_jws(data)
    if name == "api-exchanges.json":
        check_api(data)
    if name == "email-out.json":
        check_email_out(data)
    if name == "api-account.json":
        check_account(data)
    if name == "session-04.json":
        check_forwarding_session(data)
    if name == "credentials.json":
        check_cose(data["credentials"], rp.CTX_CREDENTIAL)
    if name == "forwarding.json":
        check_forwarding(data)
    if name == "session-05.json":
        check_local_session(data)
    if name == "local-delivery.json":
        check_local_delivery(data)
    if name == "queued.json":
        check_queued(data)


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


def check_forwarding_session(data: dict[str, Any]) -> None:
    """From the bytes alone: the caller, every PROOF, every message, the receipt, the placement."""
    t = [pdus_of(x) for x in data["transmissions"]]
    hello, welcome = t[0][0], t[1][0]
    caller, me = hello[4], hello[5]
    now = data["context"]["partner_clock"]
    need = ["gateway"] + (["send"] if hello[8] else [])
    result, body = rp.verify_credential(hello[13], po_keys(), {}, now, need)
    assert result == "ok" and body is not None and hello[2] == 1
    stmt = rp.statement_from_hello(hello)
    assert rp.verify_statement(stmt, hello[15], body, me=me, now=now) == "ok"
    taken, ids = [], []
    for pdu in t[2]:
        if pdu[0] == fwd.PROOF:
            result, record = fwd.check_proof(pdu, caller, me, po_keys(), {})
            assert result == "ok"
            taken.append(record)
            ids.append(fwd.record_id(pdu[2]))
        if pdu[0] == rp.PDU_TYPES["MSG"]:
            assert fwd.check_forwarded(rp.air_decode(pdu[4]), taken) == "ok"
    assert t[3][0][4] == ids
    for pdu in t[1]:
        if pdu[0] == rp.PDU_TYPES["MSG"]:
            cid = rp.content_id(rp.air_decode(pdu[4]))
            now = data["context"]["caller_clock"]
            assert fwd.check_placement(pdu[6], cid, caller, now, po_keys())[0] == "ok"
            assert welcome[7][0][1] == cid


def bstr(raw: bytes) -> bytes:
    """A CBOR byte string's head and bytes, written out by hand (RFC 8949 §3.1)."""
    n = len(raw)
    if n < 24:
        return bytes([0x40 + n]) + raw
    if n < 256:
        return bytes([0x58, n]) + raw
    return bytes([0x59]) + n.to_bytes(2, "big") + raw


def check_cose(objects: list[dict[str, Any]], context: str) -> None:
    """RP-0 §5 built by hand: the header, COSE's Sig_structure, the object, the signature."""
    for o in objects:
        kid = bytes.fromhex(o["kid_hex"]) if "kid_hex" in o else None
        protected = bytes.fromhex(o["protected_hex"])
        assert protected[:5] == bytes([0xA2, 0x01, 0x32, 0x04, 0x44]) and len(protected) == 9
        assert kid is None or protected[5:] == kid
        payload = bytes.fromhex(o["payload_hex"])
        signed = b"\x84\x6aSignature1" + bstr(protected) + bstr(context.encode()) + bstr(payload)
        assert signed.hex() == o["to_be_signed_hex"]
        signature = bytes.fromhex(o["signature_hex"])
        whole = b"\x84" + bstr(protected) + b"\xa0" + bstr(payload) + bstr(signature)
        assert whole.hex() == o.get("credential_hex", o.get("placement_hex"))
        keys = [k["public"] for ks in po_keys().values() for k in ks]
        assert any(ed25519.verify(k, signed, signature) for k in keys)


def check_forwarding(data: dict[str, Any]) -> None:
    """The placement is the forward item's; each record's ID is its statement's hash."""
    check_cose([data["placement"]], rp.CTX_PLACEMENT)
    items = [
        i
        for e in data["post_office"]
        for i in (e.get("response", {}).get("body") or {}).get("items", [])
        if i["kind"] == "forward"
    ]
    assert jws.unb64u(items[0]["placement"]) == bytes.fromhex(data["placement"]["placement_hex"])
    for r in data["records"]:
        pdu = cbor.decode(bytes.fromhex(r["proof"]["pdu_hex"]))
        assert hashlib.sha256(pdu[2]).digest()[:12].hex() == r["record_id_hex"]
    reports = [e for e in data["post_office"] if e.get("request", {}).get("path") == "/reports"]
    refused = reports[1]["response"]["body"]["sessions"][0]
    assert refused == {"code": rp.ERRORS["not-authorized"], "name": "not-authorized"}
    for sent, r in zip(reports[0]["request"]["body"]["sessions"], data["records"], strict=True):
        pdu = cbor.decode(bytes.fromhex(r["proof"]["pdu_hex"]))
        assert jws.unb64u(sent["statement"]) == pdu[2] and sent["path"] == [*pdu[6], "N1CALL-10"]
    assert [tuple(seg) for seg in data["segments"]["US"]] == spec_segments("US")  # RP-7 §5.1
    for case in data["window_cases"]:
        low, high = case["occupied_hz"]
        table = data["segments"].get(case["country"], [])
        assert case["expect"] == any(a <= low and high <= b for a, b in table)
    for case in data["partner_cases"]:  # RP-7 §5: each is in the window of the partner called
        w = case["partners"][case["calling"]]["window"]
        assert (case["now"] - w["at_s"]) % w["every_s"] < w["length_s"]
    for case in data["proof_cases"]:  # RP-7 §3.2: a hash names a credential the partner holds
        pdu = cbor.decode(bytes.fromhex(case["proof_hex"]))
        if 9 in pdu and case["expect"] == "ok":
            held = [hashlib.sha256(bytes.fromhex(c)).digest()[:8] for c in case["held_hex"]]
            assert 1 not in pdu and pdu[9] in held


def check_local_session(data: dict[str, Any]) -> None:
    """From the bytes alone: the message handed in is listed `local`, asked for and taken."""
    a, b = ([pdus_of(t) for t in s["transmissions"]] for s in data["sessions"])
    handed = [p for p in a[2] if p[0] == rp.PDU_TYPES["MSG"]]
    assert len(handed) == 1 and a[3][0][1] == [handed[0][1]]
    copies = [w for w in a[1][0][7] if w[6] & 2]  # a copy served offline, sent unasked
    served = [p for p in a[1] if p[0] == rp.PDU_TYPES["MSG"]]
    assert [(p.get(5), p[1]) for p in served] == [(w[0], w[1]) for w in copies]
    receipts = [p[1] for p in a[2] if p[0] == rp.PDU_TYPES["RECEIPT"]]
    assert receipts == [[w[1] for w in copies]]
    cid = rp.content_id(rp.air_decode(handed[0][4]))
    hello, welcome = b[0][0], b[1][0]
    assert hello[10] == 0 and not welcome[11]
    assert hello[9] not in [w[0] for w in welcome[7]]  # RP-3 §6.2: the cursor's is picked up
    local = [w for w in welcome[7] if w[6] & 4]
    assert [w[0] for w in local] == [0] and local[0][1] == cid
    assert all(w[0] > 0 for w in welcome[7] if not w[6] & 4)
    assert b[2] == [{0: rp.PDU_TYPES["REQUEST"], 4: [cid]}]
    sent = [p for p in b[3] if p[0] == rp.PDU_TYPES["MSG"]]
    assert len(sent) == 1 and 5 not in sent[0] and sent[0][1] == cid
    message = cbor.decode(rp.air_decode(sent[0][4]))
    assert hello[3] in message.get(3, []) + message.get(4, [])
    assert b[4][0][1] == [cid]


def check_local_delivery(data: dict[str, Any]) -> None:
    """The report carries each PROOF's local pickups; the post office drops what was taken."""
    for r in data["records"]:
        pdu = cbor.decode(bytes.fromhex(r["proof"]["pdu_hex"]))
        assert hashlib.sha256(pdu[2]).digest()[:12].hex() == r["record_id_hex"]
        assert [c.hex() for c in pdu.get(8, [])] == r["local_pickups"]
    log = [e for e in data["post_office"] if "request" in e]
    report = next(e for e in log if e["request"]["path"] == "/reports")
    taken = set()
    for sent, r in zip(report["request"]["body"]["sessions"], data["records"], strict=True):
        assert [rp.id_bytes(t).hex() for t in sent["local_pickups"]] == r["local_pickups"]
        taken.update(sent["local_pickups"])
        pdu = cbor.decode(bytes.fromhex(r["proof"]["pdu_hex"]))  # RP-7 §3.2: by its hash
        credential = jws.unb64u(sent["credential"])
        assert credential is not None and 1 not in pdu
        assert pdu[9] == hashlib.sha256(credential).digest()[:8]
        assert pdu.get(7, []) == sent["pickups"]
    after = log[log.index(report) + 1 :]
    items = [i for e in after for i in (e["response"].get("body") or {}).get("items", [])]
    placed = {i["serial"] for i in items if i["kind"] != "drop" and i["id"] in taken}
    assert placed and placed <= {i["serial"] for i in items if i["kind"] == "drop"}
    opened = next(e for e in after if e["request"]["path"] == "/sessions")
    assert not placed & {w["serial"] for w in opened["response"]["body"]["waiting"]}


def check_queued(data: dict[str, Any]) -> None:
    """From the bytes alone: the signed offer, declined `already-have` as the post office said."""
    session = data["session"]
    for t in session["transmissions"]:
        check_transmission(t)
    hello, welcome = (pdus_of(t)[0] for t in session["transmissions"][:2])
    now = session["gateway_clock"]
    result, body = rp.verify_credential(hello[13], po_keys(), {}, now, ["send", "receive"])
    assert result == "ok" and body is not None
    stmt = rp.statement_from_hello(hello)
    assert rp.verify_statement(stmt, hello[15], body, me=hello[5], now=now) == "ok"
    offered = [o[0] for o in hello[8]]
    assert offered == [bytes.fromhex(data["context"]["content_id_hex"])]
    assert welcome[11] is True and welcome[5] == [] and welcome[7] == []
    assert welcome[6] == [[c, rp.ERRORS["already-have"]] for c in offered]
    opened = next(e for e in data["post_office"] if e.get("request", {}).get("path") == "/sessions")
    assert jws.unb64u(opened["request"]["body"]["statement"]) == cbor.encode(stmt)
    declined = [[rp.id_bytes(d["id"]), d["code"]] for d in opened["response"]["body"]["decline"]]
    assert declined == welcome[6]


def check_account(data: dict[str, Any]) -> None:
    """Renewed credentials keep all but serial and dates; the door follows the list as read."""
    logs = [data["renewal"], data["accept_list"]] + [c["exchanges"] for c in data["renewal_cases"]]
    for log in logs:
        holder: dict[str, bytes] = {}  # who -> the credential their token came from
        given: list[bytes] = []
        for e in log:
            req, res = e.get("request", {}), e.get("response", {})
            body = res.get("body") or {}
            if req.get("path") == "/auth/token" and res.get("status") == 200:
                raw = jws.unb64u(req["body"]["credential"])
                assert raw is not None
                holder[e["who"]] = raw
            for key in ("credential",):
                if key in body and req.get("path") in {"/credentials", "/sessions"}:
                    new = jws.unb64u(body[key])
                    assert new is not None
                    old = holder[e["who"]] if req["path"] == "/credentials" else None
                    if old is None:
                        user = jws.unb64u(req["body"]["credential"])
                        assert user is not None
                        old = user
                    result, nb = rp.verify_credential(new, po_keys(), {}, e["at"], [])
                    assert result == "ok" and nb is not None
                    ob = rp.credential_body(old)
                    assert [nb[k] for k in (3, 4, 5, 9)] == [ob[k] for k in (3, 4, 5, 9)]
                    assert nb[2] != ob[2] and nb[6] == e["at"]
                    given.append(new)
            if "rp3" in e and e["rp3"]["name"] == "CREDENTIAL":
                pdu = cbor.decode(bytes.fromhex(e["rp3"]["pdu_hex"]))
                assert pdu[1] == given[-1]
            if req.get("path") == "/accept-list" and res.get("status") == 200:
                entries = body["entries"]
                assert entries == [x.lower() for x in entries] and len(set(entries)) == len(entries)
    last: list[str] = []
    for e in data["accept_list"]:
        if (
            e.get("who") == "Facundo's laptop"
            and e.get("request", {}).get("path") == "/accept-list"
        ) and e["response"]["status"] == 200:
            last = e["response"]["body"]["entries"]
        if "smtp" in e:
            smtp = e["smtp"]
            ctx = {
                "accounts": {smtp["rcpt_to"]: last},
                "known_post_offices": ["radiopost.org", "hampost.org"],
                "max_email_bytes": 10485760,
                "dmarc": smtp["dmarc"],
                "dkim_pass": smtp["dkim_pass"],
                "sent_message_ids": {},
            }
            envelope = {"mail_from": smtp["mail_from"], "rcpt_to": smtp["rcpt_to"]}
            assert mail.door(envelope, smtp["email"].encode(), ctx) == smtp["expect"]


def check_email_out(data: dict[str, Any]) -> None:
    """Each email, read back by a mail library, gives its message's values and text (RP-6 §3)."""
    for case in data["emails"]:
        fields = MESSAGES[case["message"]]
        parsed = mail.parse(case["email"].encode("utf-8"))
        assert str(parsed["Subject"]) == fields["subject"]
        assert parsed["To"].addresses[0].addr_spec == fields["to"][0]
        assert parsed["From"].addresses[0].addr_spec == fields["from"]
        when = email.utils.parsedate_to_datetime(str(parsed["Date"]))
        assert int(when.timestamp()) == fields["date"]
        cid = rp.id_text(encoded(case["message"])[1])
        assert str(parsed["Message-ID"]) == f"<{cid}@{mail.domain_of(fields['from'])}>"
        body = parsed.get_body(preferencelist=("plain",))
        assert body is not None
        assert body.get_content().replace("\r\n", "\n") == fields["body"] + mail.FOOTER
        if case["part_hex"]:
            assert mail.find_part(case["email"].encode("utf-8")) == bytes.fromhex(case["part_hex"])


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
