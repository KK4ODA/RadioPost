# RadioPost: VHF paths, P.526 against ITM (a report)

| | |
|---|---|
| **Status** | Measured |
| **Version** | 1.0.0 |
| **Updated** | 2026-11-21 |
| **Editor** | KK4ODA |
| **Licence** | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| **Follows** | [Choosing a gateway](04-gateway-selection.md), §8.5 and §18, Phase 2 |

Phase 2 of choosing a gateway works out a VHF path's diffraction loss by ITU-R P.526-16 § 4.5,
written from the Recommendation's text because no reference implementation was found. The design
asks for it to be compared with NTIA's Irregular Terrain Model on 50 real paths, run on the same
profiles, and the differences reported, and for 100 candidates to be judged in under a second from
tiles already held. This is that report.

**In short.** Where ITM holds, the two agree: P.526 gives 1.4 dB more loss than ITM on average,
with a spread of 4.9 dB, on the 21 paths inside ITM's ranges. On the other 29, ITM itself calls its
result "probably invalid", nearly always because an obstruction stands very close to an antenna
(trees or buildings beside a low antenna, in a surface model). There P.526 gives 8.5 dB less loss
than ITM, and neither model is checked. Judging 100 gateways took 15 to 42 ms.

---

## 1. The paths

`tools/terrain/paths.py` draws the paths with a fixed seed, ten in each of five regions of
different ground, each region two of the Copernicus DEM's tiles of a degree square:

| Region | Tiles | Ground |
|---|---|---|
| Georgia piedmont | N33W085, N33W084 | Rolling hills, towns and woods |
| Southern Appalachians | N35W084, N35W083 | Mountains, deep valleys |
| Colorado Front Range | N39W106, N39W105 | High plains against the Rockies |
| Tampa Bay | N27W083, N27W082 | Flat, built up, water |
| Puget Sound | N47W123, N47W122 | Water, hills, forest |

In each region three paths are 2 to 10 km long, four 10 to 30 km and three 30 to 80 km, drawn
evenly in log within each band, so that paths in line of sight and beyond the horizon are both in
the set. (A first draw of lengths uniform from 5 to 90 km gave 50 paths all blocked, and was
replaced before anything was compared.) Seven of each ten are at 145.05 MHz and three at
440.1 MHz. Your antenna is 2, 5 or 10 m up and the gateway's 15, 20, 30 or 45 m, as gateways are
put up. Both ends stay 0.05° inside the region's tiles.

The profiles are read from GLO-90, the 90 m data the client fetches first, by the client's own code
(the `terrain_paths` example, which calls `radioprop-terrain` as the client does). A profile is
sampled at the data's own spacing, about 90 m, and written out exactly as P.526 read it, so ITM is
run on the same heights. GLO-90 is a surface model: trees and buildings count as ground. Of the
50 paths, P.526's geometry calls 11 partly obstructed (the ground inside 60 % of the first Fresnel
zone, not above the direct ray) and 39 blocked; none is clear.

## 2. The two models

**P.526** is the client's: `radioprop-terrain` (hf-predict), § 4.5's delta-Bullington construction
with § 3.2's spherical-Earth loss, k = 4/3, land (εr 22, σ 0.003 S/m), vertical polarization. Its
diffraction loss is compared.

**ITM** is version 1.2.2 point to point, as NTIA's `point_to_point` runs it, at 50 % of time,
locations and situations: the median basic transmission loss less free space. It is `itmlogic`
1.2 (MIT; a Python port of ITM 1.2.2 by Oughton, Russell, Johnson, Yardim and Kusuma) driven by
`tools/terrain/itm.py`, with the same ground, vertical polarization, a surface refractivity of
301 N-units (about k = 4/3) and the continental temperate climate everywhere, so that the terrain
is what differs. Reading the port against NTIA's code (the ITMDLL lineage as SPLAT! 1.4.2 keeps it,
read only) found two faults, which the tool corrects:

* In `qlrpfl`, on a path whose horizons are far, the port takes the far end's ground from the
  profile's second-to-last point (`pfl[np + 1]`); NTIA takes the last (`pfl[np + 2]`). The tool
  carries a copy of the function with NTIA's index (and NTIA's `>` where the port has `>=`). On
  these paths the port's index would move three results, by 0.2 to 0.8 dB.
* In `lrprop`, the port's range check compares one end's horizon with the other end's
  smooth-Earth horizon. Only ITM's warning depends on it, so the tool works out where each path is
  outside ITM's ranges itself, NTIA's way, and says why.

On flat ground at 9.3 km with antennas 5 and 10 m up at 145 MHz, ITM gives 29.9 dB more than free
space, and the two-ray plane-earth loss 29.7 dB: the reference is sound where the answer is known.

## 3. What they give

P.526 less ITM, in decibels (a negative difference: P.526 gives less loss):

| Paths | Number | Mean | Median | Standard deviation | RMS |
|---|---:|---:|---:|---:|---:|
| All | 50 | −4.3 | −2.5 | 9.3 | 10.2 |
| **Inside ITM's ranges** | **21** | **+1.4** | **+2.2** | **4.9** | **5.0** |
| — in line of sight (ITM), partly obstructed (P.526) | 10 | +1.8 | +2.2 | 2.5 | |
| — beyond the horizon (ITM), blocked (P.526) | 11 | +1.0 | +0.8 | 6.6 | |
| Outside ITM's ranges | 29 | −8.5 | −7.4 | 9.6 | |
| At 145 MHz | 35 | −2.7 | −0.3 | 9.1 | |
| At 440 MHz | 15 | −8.1 | −6.4 | 9.0 | |

The two rank the paths alike: the correlation of their losses over all 50 is 0.94. Inside ITM's
ranges, 17 of 21 paths agree within 6 dB.

By region, the differences follow the ground: Tampa Bay +1.3 dB (one path of ten outside ITM's
ranges), the Georgia piedmont −1.8 (five), the Colorado Front Range −4.6 (seven), Puget Sound −7.9
(eight) and the southern Appalachians −8.6 (eight).

## 4. Where they differ, and why

ITM says why 29 paths are outside its ranges (some for more than one reason):

* 35 times, a horizon under a tenth of a smooth Earth's: an obstruction close to an antenna. In a
  surface model with an antenna 2 to 10 m up, it is usually a tree line or a building a few hundred
  metres away. ITM's horizon parameters were fitted to measurements that had none so close.
* 11 times, a horizon more than 0.2 rad (11°) above or below an antenna: mountains near it.
* 7 times, a surface refractivity under 250 N-units: ITM scales it down with the path's elevation,
  and the Colorado paths lie at 1 550 to 3 600 m on average.

The largest differences are all on such paths: southern-10 (43 km in the Appalachians, P.526
51.2 dB, ITM 75.9 dB), puget-06 (19.5 km at 440 MHz, 56.4 against 80.8) and colorado-10 (56 km,
58.6 against 81.5). Each has a horizon a few hundred metres from your antenna, 12° to 17° up. On
each, P.526's loss is the Bullington construction's on the real profile: one equivalent edge for
the whole path. ITM's diffraction is built from its two horizons and the terrain's irregularity by
empirical formulas, which by its own checks are out of range on these paths. Bullington's single
edge is known to give too little loss where several separate obstacles stand on a path. These 50
paths cannot say which is nearer the truth.

## 5. Judging 100 gateways

The client's own `Terrain::judge`, as the gateway list calls it, judged 100 gateways on a 2 m FM
channel up to 45 km away, from GLO-90 tiles already held: 25 to 42 ms from cold, the tiles not yet
opened, and 15 to 22 ms warm, on four cores of a 2.3 GHz Xeon. The design asks for under a second.

## 6. What follows

* P.526 stays the model (§8.5 of the design): it agrees with ITM where ITM holds, gives an answer
  on every path, and reduces to one obstacle that can be drawn and explained.
* On paths with an obstruction close to a low antenna, the commonest case outside ITM's ranges,
  P.526 is the more hopeful of the two by about 8 dB. Neither is checked there. Phase 4's recorded
  outcomes decide it (§19.5): the attempts of VHF calls on such paths, against what P.526 said.
* A question for the author, not built: whether the client should say, on such a path, that the
  loss is less certain (a wider spread than the 10 dB the chance is read against, or a word in the
  terrain line).

## 7. Running it again

```
python tools/terrain/paths.py TILES PATHS
cargo run --release -p radiopost-client --example terrain_paths -- TILES PATHS OUT
uv run --with itmlogic==1.2 --with numpy python tools/terrain/itm.py OUT CSV
```

The first fetches the ten tiles (about 50 MB). The second prints the time to judge 100 gateways.
The third prints the tables of § 3. With the same seed and the same tiles, every number here comes
out the same.

The elevation data: produced using Copernicus WorldDEM™-90 © DLR e.V. 2010-2014 and © Airbus
Defence and Space GmbH 2014-2018 provided under COPERNICUS by the European Union and ESA; all
rights reserved. The organisations in charge of the Copernicus programme by law or by delegation
do not incur any liability for any use of the Copernicus WorldDEM™-90.

## Appendix. The 50 paths

Difference is P.526 less ITM. Antennas are yours, then the gateway's, above the ground.

| Path | Region | km | MHz | Antennas, m | Class | P.526, dB | ITM, dB | Difference, dB | ITM's mode | Outside ITM's range |
|---|---|---:|---:|---|---|---:|---:|---:|---|---|
| georgia-01 | Georgia piedmont | 3.5 | 145.05 | 5/15 | partly | 18.7 | 16.5 | +2.2 | line of sight | — |
| georgia-02 | Georgia piedmont | 3.6 | 145.05 | 10/30 | partly | 0.0 | 3.0 | -3.0 | line of sight | — |
| georgia-03 | Georgia piedmont | 8.5 | 440.1 | 2/45 | blocked | 17.7 | 21.4 | -3.7 | diffraction | your end: horizon 0.4 km, under a tenth of a smooth Earth's |
| georgia-04 | Georgia piedmont | 12.9 | 145.05 | 2/30 | blocked | 28.8 | 17.5 | +11.3 | diffraction | — |
| georgia-05 | Georgia piedmont | 20.3 | 145.05 | 2/20 | blocked | 29.5 | 42.5 | -13.1 | diffraction | your end: horizon 0.2 km, under a tenth of a smooth Earth's |
| georgia-06 | Georgia piedmont | 19.0 | 440.1 | 5/30 | blocked | 31.0 | 27.0 | +4.0 | diffraction | — |
| georgia-07 | Georgia piedmont | 21.4 | 145.05 | 5/45 | blocked | 26.8 | 23.5 | +3.3 | diffraction | your end: horizon 0.5 km, under a tenth of a smooth Earth's |
| georgia-08 | Georgia piedmont | 38.9 | 145.05 | 5/45 | blocked | 18.3 | 22.8 | -4.5 | diffraction | your end: horizon 1.3 km, under a tenth of a smooth Earth's |
| georgia-09 | Georgia piedmont | 33.1 | 440.1 | 2/45 | blocked | 26.8 | 39.6 | -12.8 | diffraction | — |
| georgia-10 | Georgia piedmont | 38.4 | 145.05 | 2/20 | blocked | 44.1 | 46.0 | -1.9 | diffraction | your end: horizon 0.3 km, under a tenth of a smooth Earth's |
| southern-01 | Southern Appalachians | 8.3 | 145.05 | 5/15 | blocked | 38.4 | 48.3 | -10.0 | diffraction | the gateway's end: horizon 1.8 km, under a tenth of a smooth Earth's |
| southern-02 | Southern Appalachians | 8.6 | 145.05 | 5/30 | blocked | 53.5 | 60.9 | -7.4 | diffraction | your end: horizon 12° up or down; your end: horizon 1.8 km, under a tenth of a smooth Earth's; the gateway's end: horizon 21° up or down; the gateway's end: horizon 0.2 km, under a tenth of a smooth Earth's |
| southern-03 | Southern Appalachians | 2.3 | 440.1 | 5/15 | blocked | 45.9 | 60.2 | -14.3 | diffraction | your end: horizon 1.0 km, under a tenth of a smooth Earth's; the gateway's end: horizon 12° up or down; the gateway's end: horizon 0.2 km, under a tenth of a smooth Earth's |
| southern-04 | Southern Appalachians | 17.9 | 145.05 | 2/45 | blocked | 35.7 | 30.5 | +5.1 | diffraction | — |
| southern-05 | Southern Appalachians | 20.4 | 145.05 | 10/20 | blocked | 30.7 | 26.2 | +4.5 | diffraction | your end: horizon 0.6 km, under a tenth of a smooth Earth's |
| southern-06 | Southern Appalachians | 12.1 | 440.1 | 2/15 | blocked | 37.3 | 45.6 | -8.3 | diffraction | your end: horizon 1.8 km, under a tenth of a smooth Earth's |
| southern-07 | Southern Appalachians | 12.6 | 145.05 | 2/20 | blocked | 49.5 | 55.0 | -5.5 | diffraction | your end: horizon 0.5 km, under a tenth of a smooth Earth's; the gateway's end: horizon 17° up or down; the gateway's end: horizon 0.2 km, under a tenth of a smooth Earth's |
| southern-08 | Southern Appalachians | 37.7 | 145.05 | 2/20 | blocked | 39.7 | 44.2 | -4.5 | diffraction | — |
| southern-09 | Southern Appalachians | 41.2 | 440.1 | 5/30 | blocked | 50.5 | 71.3 | -20.9 | diffraction | your end: horizon 1.3 km, under a tenth of a smooth Earth's; the gateway's end: horizon 0.4 km, under a tenth of a smooth Earth's |
| southern-10 | Southern Appalachians | 43.2 | 145.05 | 5/45 | blocked | 51.2 | 75.9 | -24.8 | diffraction | your end: horizon 17° up or down; your end: horizon 0.4 km, under a tenth of a smooth Earth's |
| colorado-01 | Colorado Front Range | 7.0 | 145.05 | 10/30 | partly | 15.1 | 12.0 | +3.1 | line of sight | — |
| colorado-02 | Colorado Front Range | 2.3 | 145.05 | 10/45 | partly | 1.6 | 1.9 | -0.3 | line of sight | surface refractivity 221 N-units at the path's elevation |
| colorado-03 | Colorado Front Range | 2.6 | 440.1 | 10/45 | blocked | 35.0 | 52.2 | -17.2 | diffraction | your end: horizon -17° up or down; the gateway's end: horizon 23° up or down; the gateway's end: horizon 0.7 km, under a tenth of a smooth Earth's; surface refractivity 206 N-units at the path's elevation |
| colorado-04 | Colorado Front Range | 10.2 | 145.05 | 2/15 | blocked | 34.1 | 42.2 | -8.1 | diffraction | your end: horizon 0.3 km, under a tenth of a smooth Earth's; surface refractivity 244 N-units at the path's elevation |
| colorado-05 | Colorado Front Range | 11.2 | 145.05 | 10/45 | partly | 0.2 | -0.0 | +0.2 | line of sight | — |
| colorado-06 | Colorado Front Range | 10.6 | 440.1 | 5/30 | blocked | 15.0 | 21.3 | -6.4 | diffraction | your end: horizon 0.5 km, under a tenth of a smooth Earth's; surface refractivity 249 N-units at the path's elevation |
| colorado-07 | Colorado Front Range | 12.4 | 145.05 | 5/45 | blocked | 35.7 | 25.8 | +9.9 | diffraction | your end: horizon 0.7 km, under a tenth of a smooth Earth's; the gateway's end: horizon 2.1 km, under a tenth of a smooth Earth's; surface refractivity 246 N-units at the path's elevation |
| colorado-08 | Colorado Front Range | 76.9 | 145.05 | 2/20 | blocked | 20.7 | 19.9 | +0.8 | diffraction | — |
| colorado-09 | Colorado Front Range | 41.7 | 440.1 | 2/15 | blocked | 37.3 | 42.1 | -4.9 | diffraction | surface refractivity 238 N-units at the path's elevation |
| colorado-10 | Colorado Front Range | 56.1 | 145.05 | 5/15 | blocked | 58.6 | 81.5 | -23.0 | diffraction | your end: horizon 12° up or down; your end: horizon 0.3 km, under a tenth of a smooth Earth's; the gateway's end: horizon 3.1 km, under a tenth of a smooth Earth's; surface refractivity 218 N-units at the path's elevation |
| tampa-01 | Tampa Bay | 6.3 | 145.05 | 2/45 | partly | 21.3 | 19.2 | +2.2 | line of sight | — |
| tampa-02 | Tampa Bay | 8.5 | 145.05 | 2/45 | partly | 26.8 | 20.6 | +6.2 | line of sight | — |
| tampa-03 | Tampa Bay | 3.6 | 440.1 | 2/45 | partly | 7.9 | 4.7 | +3.3 | line of sight | — |
| tampa-04 | Tampa Bay | 29.6 | 145.05 | 5/20 | blocked | 32.9 | 40.0 | -7.1 | diffraction | your end: horizon 0.7 km, under a tenth of a smooth Earth's |
| tampa-05 | Tampa Bay | 19.0 | 145.05 | 2/15 | blocked | 34.0 | 25.5 | +8.4 | diffraction | — |
| tampa-06 | Tampa Bay | 18.4 | 440.1 | 5/15 | blocked | 31.0 | 31.0 | -0.0 | diffraction | — |
| tampa-07 | Tampa Bay | 11.3 | 145.05 | 10/20 | partly | 16.6 | 15.6 | +1.1 | line of sight | — |
| tampa-08 | Tampa Bay | 30.3 | 145.05 | 5/45 | blocked | 31.6 | 29.0 | +2.6 | diffraction | — |
| tampa-09 | Tampa Bay | 44.8 | 440.1 | 10/45 | blocked | 31.0 | 34.1 | -3.1 | diffraction | — |
| tampa-10 | Tampa Bay | 38.1 | 145.05 | 5/20 | blocked | 39.6 | 40.2 | -0.6 | diffraction | — |
| puget-01 | Puget Sound | 3.0 | 145.05 | 5/20 | blocked | 44.5 | 44.3 | +0.2 | diffraction | your end: horizon 24° up or down; your end: horizon 0.1 km, under a tenth of a smooth Earth's; the gateway's end: horizon 2.4 km, under a tenth of a smooth Earth's |
| puget-02 | Puget Sound | 7.5 | 145.05 | 10/45 | partly | 8.5 | 8.6 | -0.0 | line of sight | — |
| puget-03 | Puget Sound | 3.2 | 440.1 | 5/30 | partly | 7.1 | 4.1 | +2.9 | line of sight | — |
| puget-04 | Puget Sound | 11.8 | 145.05 | 5/30 | blocked | 26.1 | 17.5 | +8.7 | diffraction | your end: horizon 0.4 km, under a tenth of a smooth Earth's |
| puget-05 | Puget Sound | 18.5 | 145.05 | 5/30 | blocked | 49.8 | 71.3 | -21.4 | diffraction | the gateway's end: horizon 38° up or down; the gateway's end: horizon 0.5 km, under a tenth of a smooth Earth's |
| puget-06 | Puget Sound | 19.5 | 440.1 | 10/45 | blocked | 56.4 | 80.8 | -24.4 | diffraction | your end: horizon 1.9 km, under a tenth of a smooth Earth's; the gateway's end: horizon 13° up or down; the gateway's end: horizon 0.6 km, under a tenth of a smooth Earth's |
| puget-07 | Puget Sound | 26.1 | 145.05 | 2/15 | blocked | 41.2 | 43.3 | -2.1 | diffraction | the gateway's end: horizon 3.8 km, under a tenth of a smooth Earth's |
| puget-08 | Puget Sound | 71.9 | 145.05 | 2/15 | blocked | 31.0 | 39.0 | -8.1 | diffraction | the gateway's end: horizon 2.3 km, under a tenth of a smooth Earth's |
| puget-09 | Puget Sound | 36.4 | 440.1 | 5/15 | blocked | 49.1 | 64.3 | -15.2 | diffraction | the gateway's end: horizon 2.5 km, under a tenth of a smooth Earth's |
| puget-10 | Puget Sound | 55.5 | 145.05 | 10/15 | blocked | 57.8 | 77.6 | -19.8 | diffraction | your end: horizon 0.6 km, under a tenth of a smooth Earth's |

## Change log

| Version | Date | Change |
|---|---|---|
| 1.0.0 | 2026-11-21 | The 50 paths and the 100 gateways measured, ITM run on the same profiles. |
