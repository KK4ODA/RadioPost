// SPDX-License-Identifier: MIT OR Apache-2.0
// Checks the client's world map (app/ui/map.js, app/ui/world.js) against cases worked out by hand:
// where the sun is overhead at the solstices, an equinox and in November (the equation of time),
// which pole the night side holds, two great circles (one across the antimeridian), and the land.
//
//     node tools/map/check.mjs
//
// The panel's files are modules named .js, which an older node reads as scripts: they are copied
// as .mjs into a folder of their own first.

import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const ui = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "app", "ui");
const dir = mkdtempSync(join(tmpdir(), "radiopost-map-"));
let failed = 0;
try {
  writeFileSync(join(dir, "world.mjs"), readFileSync(join(ui, "world.js"), "utf8"));
  const map = readFileSync(join(ui, "map.js"), "utf8").replace('from "./world.js"', 'from "./world.mjs"');
  writeFileSync(join(dir, "map.mjs"), map);
  const { subsolar, nightPath, greatCircle } = await import(pathToFileURL(join(dir, "map.mjs")).href);
  const { LAND } = await import(pathToFileURL(join(dir, "world.mjs")).href);

  const cases = [];
  const check = (name, run) => cases.push([name, run]);
  const near = (got, want, within, what) =>
    assert.ok(Math.abs(got - want) <= within, `${what}: ${got}, wanted ${want} ± ${within}`);

  // the June solstice of 2026 is at 08:24 UTC on the 21st: the sun over the Tropic of Cancer,
  // and with the equation of time near -1.7 minutes over 15 x (12 - 8.4) + 0.4 = 54.4 degrees east
  check("the June solstice", () => {
    const [lat, lon] = subsolar(new Date("2026-06-21T08:24:00Z"));
    near(lat, 23.44, 0.05, "declination");
    near(lon, 54.4, 0.5, "longitude");
  });
  // the December solstice of 2026 is at 20:50 UTC on the 21st
  check("the December solstice", () => {
    const [lat] = subsolar(new Date("2026-12-21T20:50:00Z"));
    near(lat, -23.44, 0.05, "declination");
  });
  // the March equinox of 2026 is at 14:46 UTC on the 20th
  check("the March equinox", () => {
    const [lat] = subsolar(new Date("2026-03-20T14:46:00Z"));
    near(lat, 0, 0.05, "declination");
  });
  // early November the sundial runs about 16.4 minutes ahead of the clock: at noon UTC the sun is
  // overhead 4.1 degrees west of Greenwich, not over it
  check("the equation of time in November", () => {
    const [, lon] = subsolar(new Date("2026-11-03T12:00:00Z"));
    near(lon, -4.1, 0.3, "longitude");
  });
  // in June the south pole is dark all day, in December the north
  check("the night side holds the pole away from the sun", () => {
    assert.ok(nightPath(new Date("2026-06-21T08:24:00Z")).startsWith("M0 180 "), "June: the south");
    assert.ok(nightPath(new Date("2026-12-21T20:50:00Z")).startsWith("M0 0 "), "December: the north");
  });
  // Atlanta to London: about 6 760 km on the sphere, the great circle bulging north of London's
  // latitude on the way, in one piece
  check("a great circle across the Atlantic", () => {
    const gc = greatCircle({lat: 33.75, lon: -84.39}, {lat: 51.51, lon: -0.13});
    near(gc.km, 6760, 40, "km");
    assert.equal(gc.pieces.length, 1);
    const north = Math.min(...gc.pieces[0].map(([, y]) => y)); // y = 90 - latitude
    assert.ok(90 - north > 52, `it reaches ${90 - north} degrees north`);
  });
  // Fiji to Samoa crosses the antimeridian: two pieces, about 1 150 km, not round the world
  check("a great circle across the antimeridian", () => {
    const gc = greatCircle({lat: -18.1, lon: 178.4}, {lat: -13.8, lon: -171.8});
    near(gc.km, 1150, 60, "km");
    assert.equal(gc.pieces.length, 2);
    for (const piece of gc.pieces) {
      const xs = piece.map(([x]) => x);
      assert.ok(Math.max(...xs) - Math.min(...xs) < 20, "neither piece crosses the map");
    }
  });
  check("a great circle to where one stands", () => {
    const gc = greatCircle({lat: 33.75, lon: -84.39}, {lat: 33.75, lon: -84.39});
    assert.equal(gc.km, 0);
  });
  // Natural Earth's 1:110m land: 126 rings, three of which cross the antimeridian (Afro-Eurasia
  // at Chukotka, Wrangel Island, Fiji) and are drawn twice, 360 degrees apart, every one closed.
  // No step of a ring may be wider than half the map: a ring joined across the antimeridian as
  // given draws a line from one edge of the map to the other
  check("the land", () => {
    const rings = LAND.split("z").filter(Boolean);
    assert.equal(rings.length, 129);
    assert.ok(LAND.startsWith("M") && LAND.endsWith("z"));
    for (const ring of rings) {
      const n = ring.match(/-?(?:\d+\.?\d*|\.\d+)/g).map(Number);
      let [x, y] = n;
      assert.ok(x > -200 && x < 560 && y >= 0 && y <= 180, `a ring starts at ${x} ${y}`);
      const pole = (v) => Math.abs(v) < 1e-6 || Math.abs(v - 180) < 1e-6;
      for (let i = 2; i < n.length; i += 2) {
        // a ring round a pole (Antarctica) is closed along the map's edge there: the one wide step
        const along = n[i + 1] === 0 && pole(y);
        assert.ok(Math.abs(n[i]) <= 180 || along, `a step of ${n[i]} degrees from ${x} ${y}`);
        x += n[i];
        y += n[i + 1];
        // the sums of the steps drift by a few parts in 10^14
        assert.ok(y >= -1e-6 && y <= 180 + 1e-6, `a point off the map at ${x} ${y}`);
      }
    }
  });

  for (const [name, run] of cases) {
    try {
      run();
      console.log(`ok   ${name}`);
    } catch (e) {
      failed += 1;
      console.log(`FAIL ${name}: ${e.message}`);
    }
  }
} finally {
  rmSync(dir, {recursive: true, force: true});
}
if (failed) {
  console.log(`${failed} failed`);
  process.exit(1);
}
