# SPDX-License-Identifier: MIT OR Apache-2.0
"""Deterministic CBOR, as RP-0 §4.1 uses it (RFC 8949 §4.2.1, core deterministic encoding).

Only what RadioPost carries: unsigned and negative integers, byte strings, text strings, arrays,
maps, and true, false and null. No floats, no tags, no indefinite lengths. The decoder is strict:
it refuses anything that is not already in deterministic form, because a station that re-encoded a
message differently would compute a different content ID (RP-2 §3).
"""

from __future__ import annotations

import struct
from itertools import pairwise
from typing import Any

Value = Any


class NotCanonical(ValueError):
    """The bytes decode, but are not in deterministic form (or not RadioPost's subset)."""


def _head(major: int, n: int) -> bytes:
    if n < 24:
        return bytes([major << 5 | n])
    if n < 0x100:
        return bytes([major << 5 | 24, n])
    if n < 0x10000:
        return bytes([major << 5 | 25]) + struct.pack(">H", n)
    if n < 0x100000000:
        return bytes([major << 5 | 26]) + struct.pack(">I", n)
    if n < 0x10000000000000000:
        return bytes([major << 5 | 27]) + struct.pack(">Q", n)
    raise ValueError("integer out of CBOR's range")


def encode(value: Value) -> bytes:
    """Encode deterministically: shortest heads, definite lengths, map keys in byte order."""
    if value is True:
        return b"\xf5"
    if value is False:
        return b"\xf4"
    if value is None:
        return b"\xf6"
    if isinstance(value, int):
        return _head(0, value) if value >= 0 else _head(1, -1 - value)
    if isinstance(value, bytes | bytearray):
        return _head(2, len(value)) + bytes(value)
    if isinstance(value, str):
        raw = value.encode("utf-8")
        return _head(3, len(raw)) + raw
    if isinstance(value, list | tuple):
        return _head(4, len(value)) + b"".join(encode(v) for v in value)
    if isinstance(value, dict):
        items = sorted((encode(k), encode(v)) for k, v in value.items())
        for a, b in pairwise(items):
            if a[0] == b[0]:
                raise ValueError("duplicate map key")
        return _head(5, len(value)) + b"".join(k + v for k, v in items)
    raise TypeError(f"not a RadioPost CBOR value: {type(value).__name__}")


def decode(data: bytes) -> Value:
    """Decode one item that fills ``data`` exactly; NotCanonical if it is not deterministic."""
    value, end = _item(data, 0)
    if end != len(data):
        raise NotCanonical("bytes after the item")
    return value


def _length(data: bytes, pos: int, info: int) -> tuple[int, int]:
    if info < 24:
        return info, pos
    sizes = {24: 1, 25: 2, 26: 4, 27: 8}
    if info not in sizes:
        raise NotCanonical("indefinite length or reserved value")
    size = sizes[info]
    if pos + size > len(data):
        raise NotCanonical("truncated")
    n = int.from_bytes(data[pos : pos + size], "big")
    if _head(0, n)[0] & 0x1F != info:
        raise NotCanonical("a length or integer not in its shortest form")
    return n, pos + size


def _item(data: bytes, pos: int) -> tuple[Value, int]:
    if pos >= len(data):
        raise NotCanonical("truncated")
    major, info = data[pos] >> 5, data[pos] & 0x1F
    pos += 1
    if major == 7:
        simple = {20: False, 21: True, 22: None}
        if info not in simple:
            raise NotCanonical("floats and other simple values are not used")
        return simple[info], pos
    n, pos = _length(data, pos, info)
    if major == 0:
        return n, pos
    if major == 1:
        return -1 - n, pos
    if major in {2, 3}:
        if pos + n > len(data):
            raise NotCanonical("truncated")
        raw = data[pos : pos + n]
        if major == 2:
            return bytes(raw), pos + n
        try:
            return raw.decode("utf-8"), pos + n
        except UnicodeDecodeError as err:
            raise NotCanonical("text is not UTF-8") from err
    if major == 4:
        items = []
        for _ in range(n):
            item, pos = _item(data, pos)
            items.append(item)
        return items, pos
    if major == 5:
        out: dict[Value, Value] = {}
        previous = None
        for _ in range(n):
            start = pos
            key, pos = _item(data, pos)
            key_bytes = data[start:pos]
            if previous is not None and key_bytes <= previous:
                raise NotCanonical("map keys out of order, or repeated")
            previous = key_bytes
            if not isinstance(key, int | str):
                raise NotCanonical("map keys are integers or text")
            out[key], pos = _item(data, pos)
        return out, pos
    raise NotCanonical("tags are not used")


def diagnostic(value: Value) -> str:
    """RFC 8949 §8 diagnostic notation, for people reading the vectors."""
    if value is True:
        return "true"
    if value is False:
        return "false"
    if value is None:
        return "null"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, bytes):
        return f"h'{value.hex()}'"
    if isinstance(value, str):
        out = value.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")
        return f'"{out}"'
    if isinstance(value, list | tuple):
        return "[" + ", ".join(diagnostic(v) for v in value) + "]"
    if isinstance(value, dict):
        items = sorted(value.items(), key=lambda kv: encode(kv[0]))
        return "{" + ", ".join(f"{diagnostic(k)}: {diagnostic(v)}" for k, v in items) + "}"
    raise TypeError(type(value).__name__)


# RFC 8949 Appendix A: the examples inside RadioPost's subset (no floats, tags or indefinite
# lengths), as (value, encoding).
RFC8949_EXAMPLES: list[tuple[Value, str]] = [
    (0, "00"),
    (1, "01"),
    (10, "0a"),
    (23, "17"),
    (24, "1818"),
    (25, "1819"),
    (100, "1864"),
    (1000, "1903e8"),
    (1000000, "1a000f4240"),
    (1000000000000, "1b000000e8d4a51000"),
    (18446744073709551615, "1bffffffffffffffff"),
    (-1, "20"),
    (-10, "29"),
    (-100, "3863"),
    (-1000, "3903e7"),
    (-18446744073709551616, "3bffffffffffffffff"),
    (False, "f4"),
    (True, "f5"),
    (None, "f6"),
    (b"", "40"),
    (bytes.fromhex("01020304"), "4401020304"),
    ("", "60"),
    ("a", "6161"),
    ("IETF", "6449455446"),
    ('"\\', "62225c"),
    ("ü", "62c3bc"),
    ("水", "63e6b0b4"),
    ("\U00010151", "64f0908591"),
    ([], "80"),
    ([1, 2, 3], "83010203"),
    ([1, [2, 3], [4, 5]], "8301820203820405"),
    (list(range(1, 26)), "98190102030405060708090a0b0c0d0e0f101112131415161718181819"),
    ({}, "a0"),
    ({1: 2, 3: 4}, "a201020304"),
    ({"a": 1, "b": [2, 3]}, "a26161016162820203"),
    (["a", {"b": "c"}], "826161a161626163"),
    (
        {"a": "A", "b": "B", "c": "C", "d": "D", "e": "E"},
        "a56161614161626142616361436164614461656145",
    ),
]


def self_test() -> None:
    """Refuse to go on if this code disagrees with RFC 8949's own examples."""
    for value, encoding in RFC8949_EXAMPLES:
        raw = bytes.fromhex(encoding)
        if encode(value) != raw or decode(raw) != value:
            raise AssertionError(f"CBOR disagrees with RFC 8949 Appendix A on {encoding}")
