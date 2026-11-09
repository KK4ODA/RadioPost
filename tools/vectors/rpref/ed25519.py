# SPDX-License-Identifier: MIT OR Apache-2.0
"""Ed25519 (RFC 8032), after the reference code in its §6.

Pure Python and slow (milliseconds a signature), which is fine for making and checking test
vectors and nothing else. It is checked against RFC 8032 §7.1's test vectors before any
RadioPost vector is made (``make.py`` refuses to run if it disagrees).
"""

from __future__ import annotations

import hashlib

P = 2**255 - 19
Q = 2**252 + 27742317777372353535851937790883648493

Point = tuple[int, int, int, int]


def _inv(x: int) -> int:
    return pow(x, P - 2, P)


D = -121665 * _inv(121666) % P
SQRT_M1 = pow(2, (P - 1) // 4, P)


def _sha512_modq(data: bytes) -> int:
    return int.from_bytes(hashlib.sha512(data).digest(), "little") % Q


def _add(a: Point, b: Point) -> Point:
    m = (a[1] - a[0]) * (b[1] - b[0]) % P
    n = (a[1] + a[0]) * (b[1] + b[0]) % P
    c = 2 * a[3] * b[3] * D % P
    d = 2 * a[2] * b[2] % P
    e, f, g, h = n - m, d - c, d + c, n + m
    return (e * f % P, g * h % P, f * g % P, e * h % P)


def _mul(s: int, point: Point) -> Point:
    out: Point = (0, 1, 1, 0)
    while s > 0:
        if s & 1:
            out = _add(out, point)
        point = _add(point, point)
        s >>= 1
    return out


def _equal(a: Point, b: Point) -> bool:
    return (a[0] * b[2] - b[0] * a[2]) % P == 0 and (a[1] * b[2] - b[1] * a[2]) % P == 0


def _recover_x(y: int, sign: int) -> int | None:
    if y >= P:
        return None
    x2 = (y * y - 1) * _inv(D * y * y + 1) % P
    if x2 == 0:
        return None if sign else 0
    x = pow(x2, (P + 3) // 8, P)
    if (x * x - x2) % P != 0:
        x = x * SQRT_M1 % P
    if (x * x - x2) % P != 0:
        return None
    if (x & 1) != sign:
        x = P - x
    return x


_GY = 4 * _inv(5) % P
_GX = _recover_x(_GY, 0)
assert _GX is not None
G: Point = (_GX, _GY, 1, _GX * _GY % P)


def _compress(point: Point) -> bytes:
    zinv = _inv(point[2])
    x, y = point[0] * zinv % P, point[1] * zinv % P
    return (y | (x & 1) << 255).to_bytes(32, "little")


def _decompress(raw: bytes) -> Point | None:
    if len(raw) != 32:
        return None
    y = int.from_bytes(raw, "little")
    sign = y >> 255
    y &= (1 << 255) - 1
    x = _recover_x(y, sign)
    return None if x is None else (x, y, 1, x * y % P)


def _expand(secret: bytes) -> tuple[int, bytes]:
    if len(secret) != 32:
        raise ValueError("an Ed25519 private key is 32 bytes")
    h = hashlib.sha512(secret).digest()
    a = int.from_bytes(h[:32], "little")
    a &= (1 << 254) - 8
    a |= 1 << 254
    return a, h[32:]


def public_key(secret: bytes) -> bytes:
    a, _ = _expand(secret)
    return _compress(_mul(a, G))


def sign(secret: bytes, message: bytes) -> bytes:
    a, prefix = _expand(secret)
    public = _compress(_mul(a, G))
    r = _sha512_modq(prefix + message)
    rs = _compress(_mul(r, G))
    h = _sha512_modq(rs + public + message)
    s = (r + h * a) % Q
    return rs + s.to_bytes(32, "little")


def verify(public: bytes, message: bytes, signature: bytes) -> bool:
    if len(public) != 32 or len(signature) != 64:
        return False
    a_point = _decompress(public)
    r_point = _decompress(signature[:32])
    if a_point is None or r_point is None:
        return False
    s = int.from_bytes(signature[32:], "little")
    if s >= Q:
        return False
    h = _sha512_modq(signature[:32] + public + message)
    return _equal(_mul(s, G), _add(r_point, _mul(h, a_point)))


# RFC 8032 §7.1, tests 1-3: (secret, public, message, signature).
RFC8032_TESTS = [
    (
        "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
        "",
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bac"
        "c61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
    ),
    (
        "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
        "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
        "72",
        "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e"
        "458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
    ),
    (
        "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
        "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
        "af82",
        "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290"
        "ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
    ),
]


def self_test() -> None:
    """Refuse to go on if this code disagrees with RFC 8032's own test vectors."""
    for secret, public, message, signature in RFC8032_TESTS:
        sk, pk = bytes.fromhex(secret), bytes.fromhex(public)
        msg, sig = bytes.fromhex(message), bytes.fromhex(signature)
        if public_key(sk) != pk or sign(sk, msg) != sig or not verify(pk, msg, sig):
            raise AssertionError("Ed25519 disagrees with RFC 8032 §7.1")
        if verify(pk, msg + b"x", sig):
            raise AssertionError("Ed25519 accepted a wrong message")
