# SPDX-License-Identifier: MIT OR Apache-2.0
"""The client's icon: an envelope under two radio arcs, on a graphite tile.

Writes the desktop application's icons (`app/src-tauri/icons/`: PNGs, a Windows ICO and a macOS
ICNS) and the
panel's favicon (`app/ui/icon.svg`) from one geometry. Standard library only. The PNGs are drawn at
512 pixels, two samples a side, and the smaller ones averaged down from it.

    python tools/figures/icons.py           # write them
    python tools/figures/icons.py --check   # fail if a file differs from what this draws

`--check` compares pixels, not bytes: zlib builds compress the same pixels differently.
"""

from __future__ import annotations

import math
import struct
import sys
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ICONS = ROOT / "app" / "src-tauri" / "icons"
FAVICON = ROOT / "app" / "ui" / "icon.svg"

TILE = (0x1B, 0x1F, 0x23)  # the panel's graphite key
INK = (0xE3, 0xE6, 0xE9)  # its light text on dark

# The geometry, in a unit square, y down.
TILE_BOX = (0.06, 0.94)
TILE_RADIUS = 0.2
STROKE = 0.05
ENVELOPE = (0.24, 0.44, 0.76, 0.78)  # left, top, right, bottom
FLAP_TIP = (0.5, 0.64)
ARC_CENTRE = (0.5, 0.44)
ARCS = (0.12, 0.22)  # radii
ARC_SPREAD = math.radians(42)  # each side of straight up


def segment_distance(px: float, py: float, a: tuple[float, float], b: tuple[float, float]) -> float:
    ax, ay = a
    bx, by = b
    dx, dy = bx - ax, by - ay
    t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    return math.hypot(px - (ax + t * dx), py - (ay + t * dy))


LEFT, TOP, RIGHT, BOTTOM = ENVELOPE
SEGMENTS = [
    ((LEFT, TOP), (RIGHT, TOP)),
    ((RIGHT, TOP), (RIGHT, BOTTOM)),
    ((RIGHT, BOTTOM), (LEFT, BOTTOM)),
    ((LEFT, BOTTOM), (LEFT, TOP)),
    ((LEFT, TOP), FLAP_TIP),
    (FLAP_TIP, (RIGHT, TOP)),
]


def in_tile(x: float, y: float) -> bool:
    lo, hi = TILE_BOX
    cx = min(max(x, lo + TILE_RADIUS), hi - TILE_RADIUS)
    cy = min(max(y, lo + TILE_RADIUS), hi - TILE_RADIUS)
    return math.hypot(x - cx, y - cy) <= TILE_RADIUS


def inked(x: float, y: float) -> bool:
    half = STROKE / 2
    if any(segment_distance(x, y, a, b) <= half for a, b in SEGMENTS):
        return True
    dx, dy = x - ARC_CENTRE[0], ARC_CENTRE[1] - y
    if dy <= 0 or abs(math.atan2(dx, dy)) > ARC_SPREAD:
        return False
    r = math.hypot(dx, dy)
    return any(abs(r - radius) <= half for radius in ARCS)


def render(size: int, samples: int) -> bytes:
    """RGBA pixels, row by row, each pixel the average of samples x samples points."""
    out = bytearray()
    n = samples * samples
    for row in range(size):
        for col in range(size):
            tile = ink = 0
            for i in range(samples):
                for j in range(samples):
                    x = (col + (j + 0.5) / samples) / size
                    y = (row + (i + 0.5) / samples) / size
                    if in_tile(x, y):
                        tile += 1
                        ink += inked(x, y)
            if tile == 0:
                out += b"\0\0\0\0"
                continue
            mix = [round(t + (k - t) * ink / tile) for t, k in zip(TILE, INK, strict=True)]
            out += bytes([*mix, round(255 * tile / n)])
    return bytes(out)


def shrink(pixels: bytes, size: int, factor: int) -> bytes:
    """Averages factor x factor blocks, weighting colour by alpha."""
    small = size // factor
    out = bytearray()
    for row in range(small):
        for col in range(small):
            acc = [0, 0, 0, 0]
            for i in range(factor):
                base = ((row * factor + i) * size + col * factor) * 4
                for j in range(factor):
                    r, g, b, a = pixels[base + 4 * j : base + 4 * j + 4]
                    acc[0] += r * a
                    acc[1] += g * a
                    acc[2] += b * a
                    acc[3] += a
            alpha = acc[3]
            colour = [round(c / alpha) if alpha else 0 for c in acc[:3]]
            out += bytes([*colour, round(alpha / (factor * factor))])
    return bytes(out)


def png(pixels: bytes, size: int) -> bytes:
    def chunk(kind: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
        )

    raw = b"".join(b"\0" + pixels[r * size * 4 : (r + 1) * size * 4] for r in range(size))
    header = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def png_pixels(data: bytes) -> bytes:
    """The pixels of a PNG this script wrote (8-bit RGBA, no filtering)."""
    pos, idat, size = 8, b"", 0
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        kind, body = data[pos + 4 : pos + 8], data[pos + 8 : pos + 8 + length]
        if kind == b"IHDR":
            size = struct.unpack(">I", body[:4])[0]
        elif kind == b"IDAT":
            idat += body
        pos += 12 + length
    raw = zlib.decompress(idat)
    stride = size * 4 + 1
    return b"".join(raw[r * stride + 1 : (r + 1) * stride] for r in range(size))


def ico(images: list[tuple[int, bytes]]) -> bytes:
    """A Windows icon holding PNG images (Windows Vista and later)."""
    head = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries, blobs = b"", b""
    for size, data in images:
        side = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", side, side, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
        blobs += data
    return head + entries + blobs


def ico_pixels(data: bytes) -> list[bytes]:
    (count,) = struct.unpack("<H", data[4:6])
    out = []
    for k in range(count):
        length, offset = struct.unpack("<II", data[6 + 16 * k + 8 : 6 + 16 * k + 16])
        out.append(png_pixels(data[offset : offset + length]))
    return out


# ICNS entry types holding PNG data, by size (Apple's icon family; macOS 10.7 and later).
ICNS_TYPES = {32: b"icp5", 128: b"ic07", 256: b"ic08", 512: b"ic09"}


def icns(images: list[tuple[int, bytes]]) -> bytes:
    """A macOS icon family holding PNG images."""
    body = b"".join(
        ICNS_TYPES[size] + struct.pack(">I", 8 + len(data)) + data for size, data in images
    )
    return b"icns" + struct.pack(">I", 8 + len(body)) + body


def icns_pixels(data: bytes) -> list[bytes]:
    out, pos = [], 8
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos + 4 : pos + 8])
        out.append(png_pixels(data[pos + 8 : pos + length]))
        pos += length
    return out


def svg() -> str:
    def p(v: float) -> str:
        return f"{v * 64:g}"

    lo, hi = TILE_BOX
    side = hi - lo
    flap = f"M{p(LEFT)} {p(TOP)}L{p(FLAP_TIP[0])} {p(FLAP_TIP[1])}L{p(RIGHT)} {p(TOP)}"
    arcs = []
    for r in ARCS:
        dx, dy = r * math.sin(ARC_SPREAD), r * math.cos(ARC_SPREAD)
        cx, cy = ARC_CENTRE
        arcs.append(
            f'<path d="M{p(cx - dx)} {p(cy - dy)}A{p(r)} {p(r)} 0 0 1 {p(cx + dx)} {p(cy - dy)}"/>'
        )
    ink = "#{:02x}{:02x}{:02x}".format(*INK)
    tile = "#{:02x}{:02x}{:02x}".format(*TILE)
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">\n'
        "<!-- SPDX-License-Identifier: CC-BY-4.0. Drawn by tools/figures/icons.py. -->\n"
        f'<rect x="{p(lo)}" y="{p(lo)}" width="{p(side)}" height="{p(side)}" '
        f'rx="{p(TILE_RADIUS)}" fill="{tile}"/>\n'
        f'<g fill="none" stroke="{ink}" stroke-width="{p(STROKE)}" stroke-linecap="round" '
        'stroke-linejoin="round">\n'
        f'<rect x="{p(LEFT)}" y="{p(TOP)}" width="{p(RIGHT - LEFT)}" height="{p(BOTTOM - TOP)}"/>\n'
        f'<path d="{flap}"/>\n' + "\n".join(arcs) + "\n</g>\n</svg>\n"
    )


def main() -> int:
    check = "--check" in sys.argv[1:]
    big = render(512, 2)
    sizes = {512: big, 256: shrink(big, 512, 2), 128: shrink(big, 512, 4), 32: shrink(big, 512, 16)}
    pngs = {
        "icon.png": 512,
        "128x128@2x.png": 256,
        "128x128.png": 128,
        "32x32.png": 32,
    }
    wrong = []
    ICONS.mkdir(parents=True, exist_ok=True)
    for name, size in pngs.items():
        path = ICONS / name
        if check:
            if not path.exists() or png_pixels(path.read_bytes()) != sizes[size]:
                wrong.append(path)
        else:
            path.write_bytes(png(sizes[size], size))
    icon = ico([(s, png(sizes[s], s)) for s in (32, 128, 256)])
    path = ICONS / "icon.ico"
    if check:
        if not path.exists() or ico_pixels(path.read_bytes()) != [sizes[s] for s in (32, 128, 256)]:
            wrong.append(path)
    else:
        path.write_bytes(icon)
    family = (32, 128, 256, 512)
    path = ICONS / "icon.icns"
    if check:
        if not path.exists() or icns_pixels(path.read_bytes()) != [sizes[s] for s in family]:
            wrong.append(path)
    else:
        path.write_bytes(icns([(s, png(sizes[s], s)) for s in family]))
    if check:
        if not FAVICON.exists() or FAVICON.read_text() != svg():
            wrong.append(FAVICON)
    else:
        FAVICON.write_text(svg())
    for path in wrong:
        print(f"{path.relative_to(ROOT)} is not what icons.py draws", file=sys.stderr)
    if not check:
        print(f"wrote {len(pngs) + 2} icons in {ICONS.relative_to(ROOT)} and {FAVICON.name}")
    return 1 if wrong else 0


if __name__ == "__main__":
    sys.exit(main())
