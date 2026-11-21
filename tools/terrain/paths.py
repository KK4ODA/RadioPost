# SPDX-License-Identifier: MIT OR Apache-2.0
"""The real VHF paths Phase 2 is measured on (choosing a gateway, §18).

Fifty paths, ten in each of five regions of different ground, drawn with a fixed seed so the set
is the same each time: their ends, frequency and antenna heights. The script writes them to
PATHS and fetches the Copernicus GLO-90 tiles they cross into TILES/tif, where the client's
`terrain_paths` example converts them and works each path out with P.526.

    python tools/terrain/paths.py TILES PATHS
    cargo run --release -p radiopost-client --example terrain_paths -- TILES PATHS OUT

Standard library only. The tiles are about 5 MB each, ten of them, fewer where the sea is.
"""

import json
import math
import random
import sys
import urllib.request
from pathlib import Path

SEED = 20261121
# each region two tiles, 1° of latitude by 2° of longitude: (south, west) of the western tile
REGIONS = {
    "Georgia piedmont": (33, -85),
    "Southern Appalachians": (35, -84),
    "Colorado Front Range": (39, -106),
    "Tampa Bay, flat and coastal": (27, -83),
    "Puget Sound, water and hills": (47, -123),
}
PER_REGION = 10
BUCKET = "https://copernicus-dem-90m.s3.amazonaws.com"


def km(a: tuple[float, float], b: tuple[float, float]) -> float:
    """Great-circle distance on a 6 371 km sphere, as the client measures it."""
    la1, lo1, la2, lo2 = map(math.radians, (*a, *b))
    h = (
        math.sin((la2 - la1) / 2) ** 2
        + math.cos(la1) * math.cos(la2) * math.sin((lo2 - lo1) / 2) ** 2
    )
    return 2 * 6371 * math.asin(math.sqrt(h))


# the lengths drawn, km, and how many of each region's ten: short paths are mostly in line of
# sight, long ones mostly beyond the horizon, so each of a model's regimes is in the set
BANDS = ((2, 10, 3), (10, 30, 4), (30, 80, 3))


def ahead(at: tuple[float, float], bearing: float, d_km: float) -> tuple[float, float]:
    """The point `d_km` from `at` along `bearing` (degrees from north) on the same sphere."""
    la, lo, b, r = math.radians(at[0]), math.radians(at[1]), math.radians(bearing), d_km / 6371
    la2 = math.asin(math.sin(la) * math.cos(r) + math.cos(la) * math.sin(r) * math.cos(b))
    lo2 = lo + math.atan2(
        math.sin(b) * math.sin(r) * math.cos(la), math.cos(r) - math.sin(la) * math.sin(la2)
    )
    return math.degrees(la2), math.degrees(lo2)


def draw() -> list[dict]:
    """The paths: in each region, ends 0.05° or more inside its tiles, at lengths drawn evenly in
    log from each band of BANDS; 145 MHz for seven of each ten and 440 MHz for three; your antenna
    2, 5 or 10 m up, the gateway's 15 to 45 m, as gateways are put up."""
    rng = random.Random(SEED)
    paths = []
    for name, (south, west) in REGIONS.items():
        n = 0
        for low, high, count in BANDS:
            drawn = 0
            while drawn < count:
                start = (
                    rng.uniform(south + 0.05, south + 0.95),
                    rng.uniform(west + 0.05, west + 1.95),
                )
                length = math.exp(rng.uniform(math.log(low), math.log(high)))
                end = ahead(start, rng.uniform(0, 360), length)
                inside = (
                    south + 0.05 <= end[0] <= south + 0.95 and west + 0.05 <= end[1] <= west + 1.95
                )
                if not inside:
                    continue
                drawn += 1
                n += 1
                paths.append(
                    {
                        "id": f"{name.split()[0].lower().strip(',')}-{n:02}",
                        "region": name,
                        "from": [round(start[0], 5), round(start[1], 5)],
                        "to": [round(end[0], 5), round(end[1], 5)],
                        "f_mhz": 440.1 if n in {3, 6, 9} else 145.05,
                        "agl_m": [rng.choice([2, 5, 10]), rng.choice([15, 20, 30, 45])],
                    }
                )
    return paths


def key(lat: int, lon: int) -> str:
    ns, ew = ("N" if lat >= 0 else "S"), ("E" if lon >= 0 else "W")
    return f"Copernicus_DSM_COG_30_{ns}{abs(lat):02}_00_{ew}{abs(lon):03}_00_DEM"


def fetch(tiles: Path) -> None:
    """Each region's two tiles, unless already there; one the bucket's list does not have is the
    sea. The ends are 0.05° inside them, so no profile reads a neighbour's edge."""
    folder = tiles / "tif"
    folder.mkdir(parents=True, exist_ok=True)
    with urllib.request.urlopen(f"{BUCKET}/tileList.txt", timeout=60) as r:
        listed = set(r.read().decode().split())
    wanted = {(south, lon) for south, west in REGIONS.values() for lon in (west, west + 1)}
    for lat, lon in sorted(wanted):
        name = key(lat, lon)
        out = folder / f"{name}.tif"
        if out.exists() or name not in listed:
            continue
        print(f"fetching {name}", file=sys.stderr)
        with urllib.request.urlopen(f"{BUCKET}/{name}/{name}.tif", timeout=300) as r:
            data = r.read()
        if data[:4] not in {b"II*\0", b"MM\0*"}:
            sys.exit(f"{name}: not a TIFF")
        out.write_bytes(data)


def main() -> None:
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    tiles, paths = Path(sys.argv[1]), Path(sys.argv[2])
    drawn = draw()
    paths.write_text(json.dumps({"seed": SEED, "paths": drawn}, indent=1) + "\n", encoding="utf-8")
    print(f"{len(drawn)} paths written to {paths}", file=sys.stderr)
    fetch(tiles)


if __name__ == "__main__":
    main()
