# The client's world map

The Gateways page of the client's panel (`app/ui`) has a map of the world: the land, the
Maidenhead fields, the night side as it is now, the position in use, the gateways of the view, and
the great circle to the one picked (the gateway selection design,
[`docs/design/04-gateway-selection.md`](../../docs/design/04-gateway-selection.md) §16.2). It is
drawn with no library and no tile server, so it works with no Internet.

| File | What it is |
|---|---|
| `app/ui/map.js` | The map: drawing, zoom and pan, the sun's position, great circles |
| `app/ui/world.js` | The land, one SVG path, written by `world.py`; never edited by hand |
| `tools/map/world.py` | Writes `world.js` from Natural Earth's land (standard library only) |
| `tools/map/check.mjs` | Checks `map.js`'s sums against cases worked out by hand (CI runs it) |

## The land

The land is Natural Earth's 1:110m land, version 4.1.0, which is in the public domain
([naturalearthdata.com](https://www.naturalearthdata.com/)). It is taken from world-atlas 2.0.2,
which redistributes it as TopoJSON (`land-110m.json`, SHA-256
`ead5f681…b704658a`; `world.py` refuses any other file). To write `world.js` again:

```bash
npm pack world-atlas@2.0.2
```

```bash
tar -xzf world-atlas-2.0.2.tgz
```

```bash
python tools/map/world.py package/land-110m.json
```

`--check` compares instead of writing. Each point is kept to a tenth of a degree, about 11 km,
which is finer than the 1:110m data itself: `world.js` is about 35 kB.

world-atlas is under the ISC licence, which asks that its notice go with every copy. `world.py`
writes the notice into `world.js`, and it is here too:

```text
Copyright 2013-2019 Michael Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
```

## The sums

* **The frame**: x = longitude + 180, y = 90 − latitude, in degrees, 360 by 180 (equirectangular).
  Zooming moves the SVG's viewBox; the marks are redrawn so they keep their size on screen.
* **The sun**: where it is overhead, from the U.S. Naval Observatory's low-precision formulas
  (mean anomaly, mean longitude, ecliptic longitude, obliquity, right ascension, Greenwich mean
  sidereal time), good to a few hundredths of a degree this century. The night side is the
  terminator, latitude = atan(−cos(longitude − the sun's longitude) / tan(declination)), filled
  towards the pole away from the sun. It is drawn again every four minutes.
* **Great circles**: interpolated on the sphere a degree at a time, cut where they cross the edge
  of the map. The distance and bearing the page writes come from the client (`app/client/src/grid.rs`),
  from the grid square's centre the gateway gives.
