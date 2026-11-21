# SPDX-License-Identifier: MIT OR Apache-2.0
"""Writes the land of the client's world map, ``app/ui/world.js``, from Natural Earth's land.

The map (the client's Gateways view; the gateway selection design, §16.2) is drawn with no
library and no tile server, so the land comes with the panel as one SVG path in the map's own
frame: x = longitude + 180, y = 90 - latitude, both in degrees, 360 by 180.

The source is Natural Earth's 1:110m land, version 4.1.0 (public domain), as world-atlas 2.0.2
redistributes it in TopoJSON (``land-110m.json``; ISC licence, written into the file). Get it
with ``npm pack world-atlas@2.0.2`` and ``tar -xzf world-atlas-2.0.2.tgz``, then:

    python tools/map/world.py package/land-110m.json

``--check`` compares what it would write with the file there and changes nothing. Standard
library only.
"""

from __future__ import annotations

import argparse
import hashlib
import itertools
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "app" / "ui" / "world.js"
SOURCE_SHA256 = "ead5f68119c49a9250902e7da303bcb209341bbb8fefe7369a439b48b704658a"
# a tenth of a degree, about 11 km: finer than the 1:110m data itself
STEP = 10
# world-atlas's licence, which asks that it go with every copy
ISC = """Copyright 2013-2019 Michael Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE."""


def arcs_of(topology: dict) -> list[list[tuple[float, float]]]:
    """Each arc as longitude and latitude, undoing the TopoJSON quantization and delta coding."""
    (sx, sy), (tx, ty) = topology["transform"]["scale"], topology["transform"]["translate"]
    arcs = []
    for arc in topology["arcs"]:
        x = y = 0
        points = []
        for dx, dy in arc:
            x += dx
            y += dy
            points.append((x * sx + tx, y * sy + ty))
        arcs.append(points)
    return arcs


def ring(indices: list[int], arcs: list[list[tuple[float, float]]]) -> list[tuple[float, float]]:
    """A ring from its arcs; a negative index is the arc ``~i`` reversed. Arcs share their ends."""
    points: list[tuple[float, float]] = []
    for i in indices:
        arc = arcs[i] if i >= 0 else arcs[~i][::-1]
        points.extend(arc if not points else arc[1:])
    return points


def polygons(topology: dict) -> list[list[list[tuple[float, float]]]]:
    arcs = arcs_of(topology)
    out = []
    for g in topology["objects"]["land"]["geometries"]:
        shapes = g["arcs"] if g["type"] == "MultiPolygon" else [g["arcs"]]
        out.extend([ring(r, arcs) for r in shape] for shape in shapes)
    return out


def across(ring: list[tuple[float, float]]) -> list[list[tuple[float, float]]]:
    """A ring as the flat map draws it. A ring that crosses the antimeridian (Chukotka, Fiji) has
    its longitudes made continuous, running past 180 degrees, and is drawn again 360 degrees over,
    so each copy shows one side and the SVG clips the rest: joined as given, its points either side
    of the line would be drawn straight across the map. A ring round a pole is closed along it."""
    flat = [ring[0]]
    for given, lat in ring[1:]:
        before = flat[-1][0]
        # the copy of this longitude nearest the one before: within 180 degrees of it
        lon = given - 360 * round((given - before) / 360)
        flat.append((lon, lat))
    if abs(flat[-1][0] - flat[0][0]) > 180:
        pole = -90.0 if sum(lat for _, lat in flat) < 0 else 90.0
        flat += [(flat[-1][0], pole), (flat[0][0], pole)]
    lons = [lon for lon, _ in flat]
    copies = [flat]
    if min(lons) < -180:
        copies.append([(lon + 360, lat) for lon, lat in flat])
    if max(lons) > 180:
        copies.append([(lon - 360, lat) for lon, lat in flat])
    return copies


def path_of(rings: list[list[tuple[float, float]]]) -> str:
    """The rings as one SVG path, each point a tenth of a degree, the first of a ring absolute and
    the rest relative to the one before, points that round to the one before left out."""
    parts = []
    for r in rings:
        q = [(round((lon + 180) * STEP), round((90 - lat) * STEP)) for lon, lat in r]
        kept = [q[0]]
        for p in q[1:]:
            if p != kept[-1]:
                kept.append(p)
        if len(kept) < 3:
            continue

        def n(v: int) -> str:
            text = f"{v / STEP:g}"
            return text.replace("0.", ".", 1) if text.startswith(("0.", "-0.")) else text

        moves = []
        for (x0, y0), (x1, y1) in itertools.pairwise(kept):
            dx, dy = n(x1 - x0), n(y1 - y0)
            moves.append(dx + ("" if dy.startswith("-") else " ") + dy)
        parts.append(f"M{n(kept[0][0])} {n(kept[0][1])}l" + " ".join(moves) + "z")
    return "".join(parts)


def text_of(source: Path) -> str:
    data = source.read_bytes()
    digest = hashlib.sha256(data).hexdigest()
    if digest != SOURCE_SHA256:
        sys.exit(f"{source} is not world-atlas 2.0.2's land-110m.json (SHA-256 {digest})")
    rings = [c for poly in polygons(json.loads(data)) for r in poly for c in across(r)]
    return (
        "// SPDX-License-Identifier: ISC\n"
        "// The land of the client's world map, written by tools/map/world.py: do not edit it.\n"
        "// One SVG path: x = longitude + 180, y = 90 - latitude, in degrees.\n"
        "//\n"
        "// Natural Earth's 1:110m land, version 4.1.0, which is in the public domain\n"
        "// (naturalearthdata.com), as world-atlas 2.0.2 redistributes it, under this notice:\n"
        "//\n"
        + "".join(f"// {line}".rstrip() + "\n" for line in ISC.splitlines())
        + f'export const LAND = "{path_of(rings)}";\n'
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("source", type=Path, help="world-atlas 2.0.2's land-110m.json")
    parser.add_argument("--check", action="store_true", help="compare, change nothing")
    args = parser.parse_args()
    text = text_of(args.source)
    if args.check:
        if not OUT.exists() or OUT.read_text(encoding="utf-8") != text:
            sys.exit(f"{OUT.relative_to(ROOT)} is not what {args.source} gives: run world.py")
        print(f"{OUT.relative_to(ROOT)} agrees with {args.source}")
        return
    OUT.write_text(text, encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)}: {len(text)} bytes")


if __name__ == "__main__":
    main()
