# SPDX-License-Identifier: MIT OR Apache-2.0
"""Aether's modem profile for choosing a gateway (design §10.3), from Aether's own measurements.

For each of Aether's two ladders (2300 Hz and 500 Hz) and each rung: its name, its net rate, and
the SNR (dB in 3 kHz) at which its frames fail one time in ten on each channel class (AWGN and
ITU-R F.1487's Good, Moderate and Poor), read where its measured frame-error curve crosses 10 %;
and the same for the tone floor's control frame, which carries a call and its answer.

    python tools/modems/aether_profile.py AETHER_CHECKOUT > app/client/src/selection/aether.json

The curves are Aether's `bench/baselines/` (`phy_fer_awgn14.csv`, `phy_fer.csv`,
`phy_fer_500.csv`, `tone_floor.csv`); the rungs, their rates and their AWGN thresholds are its
air-interface specification's tables (§4, §4.1), which the AWGN crossings found here are checked
against. Where a 2300 Hz OFDM rung was measured on AWGN only, its fading thresholds are its AWGN
threshold plus the penalty of the measured rungs either side (in AWGN threshold), interpolated,
and marked `filled`. Where a curve never fell to 10 % in the range measured, the threshold is above
its highest SNR: it is taken 1 dB above that, or the interpolation if higher, and marked `beyond`.
A filled threshold is never below that of an OFDM rung easier on AWGN: a harder rung does not need
less on the same channel. Standard library only.
"""

import csv
import json
import re
import subprocess
import sys
from pathlib import Path

CLASSES = ("awgn", "good", "moderate", "poor")
TARGET_FER = 0.1
# a crossing found here and the specification's AWGN column may differ by this, dB: the
# specification rounds to 0.1 and some of its points come from other runs of the same bench
AGREE_DB = 0.35


def crossing(points: list[tuple[float, float]]) -> float | None:
    """The SNR where the frame error rate falls through 10 %, by linear interpolation between the
    measured points either side: the last point above it before the curve stays at or below it."""
    pts = sorted(points)
    for i in range(len(pts) - 1, -1, -1):
        if pts[i][1] > TARGET_FER:
            if i == len(pts) - 1:
                return None  # never below 10 % in the range measured
            (s0, f0), (s1, f1) = pts[i], pts[i + 1]
            return s0 + (f0 - TARGET_FER) / (f0 - f1) * (s1 - s0)
    return pts[0][0]  # below 10 % at the lowest SNR measured: that SNR is an upper bound


def curves(path: Path, key: str) -> dict[tuple[str, str], list[tuple[float, float]]]:
    out: dict[tuple[str, str], list[tuple[float, float]]] = {}
    with path.open(encoding="utf-8") as f:
        for r in csv.DictReader(f):
            frames = int(r["frames"])
            fer = 1 - int(r["decoded"]) / frames if frames else 1.0
            out.setdefault((r[key], r["channel"]), []).append((float(r["snr_3k_db"]), fer))
    return out


def spec_table(text: str, block: str) -> list[dict]:
    """A generated table of the specification: rung, name, net rate and AWGN threshold."""
    m = re.search(rf"<!-- BEGIN:{block} -->\n(.*?)<!-- END:{block} -->", text, re.S)
    if not m:
        sys.exit(f"no {block} table in the specification")
    rows = []
    for line in m.group(1).splitlines()[2:]:
        cells = [c.strip() for c in line.strip("|").split("|")]
        name = cells[1].split(" (")[0]
        rows.append(
            {
                "rung": int(cells[0]),
                "name": name,
                "net_bps": int(cells[10]),
                "awgn_db": float(cells[11].replace("+", "")),
            }
        )
    return rows


def ladder(rows: list[dict], measured: dict, air: str) -> list[dict]:
    rungs = []
    for row in rows:
        thresholds, filled, beyond = {}, [], {}
        for c in CLASSES:
            points = measured.get((row["name"], c))
            t = crossing(points) if points else None
            if t is None:
                filled.append(c)
                if points:
                    beyond[c] = max(snr for snr, _ in points)
            thresholds[c] = None if t is None else round(t, 1)
        if thresholds["awgn"] is None:
            sys.exit(f"{air} {row['name']}: no AWGN curve")
        if abs(thresholds["awgn"] - row["awgn_db"]) > AGREE_DB:
            sys.exit(
                f"{air} {row['name']}: AWGN {thresholds['awgn']} dB here, "
                f"{row['awgn_db']} in the specification"
            )
        rungs.append(
            {
                "rung": row["rung"],
                "name": row["name"],
                "net_bps": row["net_bps"],
                "threshold_db": thresholds,
                "filled": filled,
                "beyond": beyond,
            }
        )
    # a fading threshold not measured: AWGN plus the penalty of the measured rungs either side
    for r in rungs:
        for c in r["filled"]:
            known = [
                (o["threshold_db"]["awgn"], o["threshold_db"][c] - o["threshold_db"]["awgn"])
                for o in rungs
                if c not in o["filled"]
                and o["name"].split("-")[0] != "tone"
                and not o["name"].startswith("tone")
            ]
            a = r["threshold_db"]["awgn"]
            below = max((k for k in known if k[0] <= a), default=None)
            above = min((k for k in known if k[0] >= a), default=None)
            if below and above and above[0] > below[0]:
                pen = below[1] + (a - below[0]) / (above[0] - below[0]) * (above[1] - below[1])
            else:
                pen = (below or above)[1]
            estimate = a + pen
            if c in r["beyond"]:
                estimate = max(estimate, r["beyond"][c] + 1.0)
            r["threshold_db"][c] = round(estimate, 1)
    # a harder OFDM rung needs no less than an easier one on the same channel
    ofdm = sorted(
        (r for r in rungs if not r["name"].startswith("tone")),
        key=lambda r: r["threshold_db"]["awgn"],
    )
    for c in CLASSES[1:]:
        highest = float("-inf")
        for r in ofdm:
            if c in r["filled"] and c not in r["beyond"]:
                r["threshold_db"][c] = round(max(r["threshold_db"][c], highest), 1)
            highest = max(highest, r["threshold_db"][c])
    for r in rungs:
        r["beyond"] = sorted(r.pop("beyond"))
        r["filled"] = [c for c in r["filled"] if c not in r["beyond"]]
    return rungs


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    root = Path(sys.argv[1])
    base = root / "bench" / "baselines"
    spec = (root / "docs" / "spec" / "air-interface.md").read_text(encoding="utf-8")
    commit = subprocess.run(
        ["git", "-C", str(root), "rev-parse", "HEAD"], capture_output=True, text=True, check=True
    ).stdout.strip()
    ofdm_wide = curves(base / "phy_fer.csv", "mode_name")
    ofdm_wide.update(curves(base / "phy_fer_awgn14.csv", "mode_name"))
    ofdm_narrow = curves(base / "phy_fer_500.csv", "mode_name")
    tones = curves(base / "tone_floor.csv", "frame")
    control = {}
    for c in CLASSES:
        t = crossing(tones["tone-control", c])
        control[c] = round(t, 1) if t is not None else None
    profile = {
        "modem": "aether",
        "source": {
            "repository": "https://github.com/KK4ODA/aether-hf",
            "commit": commit,
            "files": [
                "docs/spec/air-interface.md",
                "bench/baselines/phy_fer_awgn14.csv",
                "bench/baselines/phy_fer.csv",
                "bench/baselines/phy_fer_500.csv",
                "bench/baselines/tone_floor.csv",
            ],
        },
        "snr_in": "dB in 3 kHz, where a rung's frames fail one time in ten",
        "control_db": control,
        "airs": {
            "2300": ladder(spec_table(spec, "modes"), {**ofdm_wide, **tones}, "2300 Hz"),
            "500": ladder(spec_table(spec, "modes500"), {**ofdm_narrow, **tones}, "500 Hz"),
        },
    }
    json.dump(profile, sys.stdout, indent=1)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
