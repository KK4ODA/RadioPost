# SPDX-License-Identifier: MIT OR Apache-2.0
"""P.526 against NTIA's Irregular Terrain Model on the same profiles (choosing a gateway, §18).

Reads what the client's `terrain_paths` example wrote (each path's profile exactly as P.526 read
it, and P.526's loss), runs ITM 1.2.2 point to point on each profile, and writes the comparison:

    uv run --with itmlogic==1.2 --with numpy python tools/terrain/itm.py OUT CSV

ITM is itmlogic 1.2 (MIT; Oughton, Russell, Johnson, Yardim, Kusuma), a Python port of ITM 1.2.2,
set up as NTIA's own `point_to_point` sets it up. Its `qlrpfl` is replaced by `qlrpfl` below: a
copy with two lines made NTIA's (the effective height of the far end read from the profile's last
point, `pfl[np + 2]`, where the port reads the one before; and `>` where the port has `>=`).

What is compared: ITM's median basic transmission loss less free space (time, location and
situation at 50 %), against P.526-16 § 4.5's diffraction loss. Both read the same ground
(εr 22, σ 0.003 S/m, as P.452 takes land), vertical polarization, and the same Earth: ITM's from
a surface refractivity of 301 N-units, about k = 4/3, and P.526's k = 4/3. ITM's climate is
continental temperate (5) everywhere, so the terrain is what differs.
"""

import csv
import json
import math
import statistics
import sys

from itmlogic.lrprop import lrprop
from itmlogic.preparatory_subroutines.dlthx import dlthx
from itmlogic.preparatory_subroutines.hzns import hzns
from itmlogic.preparatory_subroutines.qlrps import qlrps
from itmlogic.preparatory_subroutines.zlsq1 import zlsq1
from itmlogic.statistics.avar import avar

EPS, SGM, NS, CLIMATE, VERTICAL = 22.0, 0.003, 301.0, 5, 1


def qlrpfl(prop: dict) -> dict:
    """itmlogic 1.2's `qlrpfl` (MIT), with NTIA's index and comparison: see the module's text."""
    prop["dist"] = prop["pfl"][0] * prop["pfl"][1]
    np = prop["pfl"][0]
    prop["the"], prop["dl"] = hzns(prop["pfl"], prop["dist"], prop["hg"], prop["gme"])
    xl = {j: min(15 * prop["hg"][j], 0.1 * prop["dl"][j]) for j in range(2)}
    xl[1] = prop["dist"] - xl[1]
    prop["dh"] = dlthx(prop["pfl"], xl[0], xl[1])
    if prop["dl"][0] + prop["dl"][1] > 1.5 * prop["dist"]:
        za, zb = zlsq1(prop["pfl"], xl[0], xl[1])
        prop["he"] = [
            prop["hg"][0] + max(prop["pfl"][2] - za, 0),
            prop["hg"][1] + max(prop["pfl"][np + 2] - zb, 0),
        ]
        for j in range(2):
            prop["dl"][j] = math.sqrt(2 * prop["he"][j] / prop["gme"]) * math.exp(
                -0.07 * math.sqrt(prop["dh"] / max(prop["he"][j], 5))
            )
        q = prop["dl"][0] + prop["dl"][1]
        if q <= prop["dist"]:
            q = (prop["dist"] / q) ** 2
            for j in range(2):
                prop["he"][j] *= q
                prop["dl"][j] = math.sqrt(2 * prop["he"][j] / prop["gme"]) * math.exp(
                    -0.07 * math.sqrt(prop["dh"] / max(prop["he"][j], 5))
                )
        for j in range(2):
            q = math.sqrt(2 * prop["he"][j] / prop["gme"])
            prop["the"][j] = (0.65 * prop["dh"] * (q / prop["dl"][j] - 1) - 2 * prop["he"][j]) / q
    else:
        za, _ = zlsq1(prop["pfl"], xl[0], 0.9 * prop["dl"][0])
        _, zb = zlsq1(prop["pfl"], prop["dist"] - 0.9 * prop["dl"][1], xl[1])
        prop["he"] = [
            prop["hg"][0] + max(prop["pfl"][2] - za, 0),
            prop["hg"][1] + max(prop["pfl"][np + 2] - zb, 0),
        ]
    prop["mdp"] = -1
    prop["lvar"] = max(prop["lvar"], 3)
    if prop["mdvarx"] >= 0:
        prop["mdvar"] = prop["mdvarx"]
        prop["lvar"] = max(prop["lvar"], 4)
    if prop["klimx"] > 0:
        prop["klim"] = prop["klimx"]
        prop["lvar"] = 5
    return lrprop(0, prop)


def itm(heights: list[float], spacing_m: float, f_mhz: float, h1: float, h2: float) -> dict:
    """ITM point to point on one profile, as NTIA's `point_to_point` runs it at 50 % of time,
    locations and situations: the median loss, free space, the reference attenuation, the
    mode and ITM's own warning (0 when every input is in its range)."""
    pfl = [len(heights) - 1, spacing_m, *heights]
    np = pfl[0]
    # the system's elevation: the mean of the profile's middle, as NTIA takes it
    ja = int(3.0 + 0.1 * pfl[0])
    jb = np - ja + 6
    zsys = sum(pfl[i] for i in range(ja - 1, jb)) / (jb - ja + 1)
    prop = {"hg": [h1, h2], "klim": CLIMATE, "kwx": 0, "lvar": 5, "mdp": -1, "pfl": pfl}
    prop["wn"], prop["gme"], prop["ens"], prop["zgnd"] = qlrps(f_mhz, zsys, NS, VERTICAL, EPS, SGM)
    prop["klimx"], prop["mdvarx"] = CLIMATE, 12
    prop = qlrpfl(prop)
    fs = 32.45 + 20 * math.log10(f_mhz) + 20 * math.log10(prop["dist"] / 1000)
    excess, prop = avar(0.0, 0.0, 0.0, prop)
    beyond = prop["dist"] - prop["dla"]
    if int(beyond) < 0:
        mode = "line of sight"
    elif prop["dist"] <= prop["dlsa"] or prop["dist"] <= prop["dx"]:
        mode = "diffraction"
    else:
        mode = "troposcatter"
    return {
        "loss_db": excess + fs,
        "fs_db": fs,
        "aref_db": prop["aref"],
        "mode": mode,
        "outside": outside(prop),
    }


def outside(prop: dict) -> list[str]:
    """Where the path is outside the ranges ITM was made for, as NTIA's `lrprop` checks them
    (each end against its own smooth-Earth horizon: the port mixes the two ends in one check, and
    its warning is not used). ITM calls a path with any of these "probably invalid"."""
    found = []
    for j, end in enumerate(("your end", "the gateway's end")):
        if abs(prop["the"][j]) > 0.2:
            found.append(f"{end}: horizon {math.degrees(prop['the'][j]):.0f}° up or down")
        if prop["dl"][j] < 0.1 * prop["dls"][j]:
            found.append(
                f"{end}: horizon {prop['dl'][j] / 1000:.1f} km, under a tenth of a smooth Earth's"
            )
        if prop["dl"][j] > 3 * prop["dls"][j]:
            found.append(f"{end}: horizon over three times a smooth Earth's")
    if not 250 <= prop["ens"] <= 400:
        found.append(f"surface refractivity {prop['ens']:.0f} N-units at the path's elevation")
    if prop["dist"] < prop["dmin"]:
        found.append("too short for the antennas' difference in height")
    return found


def main() -> None:
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    with open(sys.argv[1], encoding="utf-8") as f:
        worked = json.load(f)
    rows = []
    for p in worked["paths"]:
        r = itm(p["h_m"], p["spacing_m"], p["f_mhz"], *p["agl_m"])
        rows.append(
            {
                "id": p["id"],
                "region": p["region"],
                "km": round(p["length_km"], 1),
                "mhz": p["f_mhz"],
                "agl_m": f"{p['agl_m'][0]:g}/{p['agl_m'][1]:g}",
                "class": p["class"],
                "p526_db": round(p["p526_db"], 1),
                "itm_db": round(r["loss_db"] - r["fs_db"], 1),
                "itm_aref_db": round(r["aref_db"], 1),
                "difference_db": round(p["p526_db"] - (r["loss_db"] - r["fs_db"]), 1),
                "itm_mode": r["mode"],
                "itm_outside": "; ".join(r["outside"]),
            }
        )
    d = [r["difference_db"] for r in rows]
    a = [r["p526_db"] for r in rows]
    b = [r["itm_db"] for r in rows]
    print(
        f"{len(rows)} paths; P.526 less ITM: mean {statistics.mean(d):.1f} dB, median "
        f"{statistics.median(d):.1f}, standard deviation {statistics.stdev(d):.1f}, "
        f"RMS {math.sqrt(statistics.mean(x * x for x in d)):.1f}, correlation "
        f"{statistics.correlation(a, b):.2f}"
    )
    held = [r for r in rows if not r["itm_outside"]]
    if len(held) > 1:
        dh = [r["difference_db"] for r in held]
        print(
            f"{len(held)} inside ITM's ranges: mean {statistics.mean(dh):+.1f} dB, median "
            f"{statistics.median(dh):+.1f}, sd {statistics.stdev(dh):.1f}, RMS "
            f"{math.sqrt(statistics.mean(x * x for x in dh)):.1f}"
        )
    for r in rows:
        r["itm_range"] = "outside" if r["itm_outside"] else "inside"
    for key in ("itm_range", "class", "itm_mode", "mhz", "region"):
        for value in sorted({r[key] for r in rows}, key=str):
            ds = [r["difference_db"] for r in rows if r[key] == value]
            spread = f", sd {statistics.stdev(ds):.1f}" if len(ds) > 1 else ""
            print(
                f"  {key} {value}: {len(ds)} paths, mean {statistics.mean(ds):+.1f}, "
                f"median {statistics.median(ds):+.1f}{spread}"
            )

    with open(sys.argv[2], "w", newline="", encoding="utf-8") as f:
        w = csv.DictWriter(f, fieldnames=list(rows[0]))
        w.writeheader()
        w.writerows(rows)


if __name__ == "__main__":
    main()
