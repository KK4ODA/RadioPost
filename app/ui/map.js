// SPDX-License-Identifier: MIT OR Apache-2.0
// The world map of the Gateways view (the gateway selection design, §16.2): the land, the
// Maidenhead fields, the night side as it is now, the position in use, the gateways, and the great
// circle to the one picked. No library and no tile server: the land comes with the panel
// (world.js, Natural Earth). The frame is the land's: x = longitude + 180, y = 90 - latitude, in
// degrees, 360 by 180; zooming moves the SVG's viewBox, and marks are redrawn to keep their size.

import { LAND } from "./world.js";

const SVG = "http://www.w3.org/2000/svg";
const EARTH_KM = 6371;
const WIDEST = 360;
const NARROWEST = 4; // degrees across at the most zoom: a grid square's width twice
const STEP = 1.25; // each notch of the wheel, each press of + or -
const DRAG = 4; // pixels a press moves before it is a drag and not a click
const LABELS_UNDER = 40; // degrees across under which every gateway's callsign is shown
const FIELDS_UNDER = 200; // degrees across under which the fields are named
const SQUARES_UNDER = 30; // degrees across under which the squares are drawn
const SQUARE_NAMES_UNDER = 12; // and under which they are named
const SUBSQUARES_UNDER = 1.6; // degrees across under which the subsquares are drawn (a map that
const SUBSQUARE_NAMES_UNDER = 0.5; // zooms that close: the Reports view's), and named

const rad = (d) => (d * Math.PI) / 180;
const deg = (r) => (r * 180) / Math.PI;
const xy = (lat, lon) => [lon + 180, 90 - lat];

function node(tag, attrs = {}) {
  const n = document.createElementNS(SVG, tag);
  for (const [k, v] of Object.entries(attrs)) n.setAttribute(k, String(v));
  return n;
}

/** Where the sun is overhead now, as [latitude, longitude]: the U.S. Naval Observatory's low
 *  precision formulas, good to a few hundredths of a degree for this century. */
export function subsolar(when = new Date()) {
  const d = when.getTime() / 86400000 + 2440587.5 - 2451545.0; // days from J2000.0
  const g = rad(357.529 + 0.98560028 * d); // the sun's mean anomaly
  const q = 280.459 + 0.98564736 * d; // its mean longitude
  const l = rad(q + 1.915 * Math.sin(g) + 0.02 * Math.sin(2 * g)); // its ecliptic longitude
  const e = rad(23.439 - 0.00000036 * d); // the obliquity of the ecliptic
  const ra = deg(Math.atan2(Math.cos(e) * Math.sin(l), Math.cos(l)));
  const dec = deg(Math.asin(Math.sin(e) * Math.sin(l)));
  const gmst = 280.46061837 + 360.98564736629 * d; // Greenwich mean sidereal time, degrees
  const lon = ((((ra - gmst) % 360) + 540) % 360) - 180;
  return [dec, lon];
}

/** The night side as one closed path in the map's frame. */
export function nightPath(when = new Date()) {
  const [dec0, slon] = subsolar(when);
  // at an equinox the terminator is a meridian pair: a tiny declination keeps the formula finite
  const dec = Math.abs(dec0) < 0.05 ? (dec0 < 0 ? -0.05 : 0.05) : dec0;
  const pts = [];
  for (let lon = -180; lon <= 180; lon += 2) {
    const lat = deg(Math.atan(-Math.cos(rad(lon - slon)) / Math.tan(rad(dec))));
    pts.push(xy(lat, lon));
  }
  // the sun north of the equator leaves the south pole dark, and the reverse
  const edge = dec > 0 ? 180 : 0;
  const line = pts.map(([x, y]) => `${x.toFixed(1)} ${y.toFixed(2)}`).join(" L");
  return `M0 ${edge} L${line} L360 ${edge}Z`;
}

/** The great circle from `a` to `b` ({lat, lon}), as pieces of polyline in the map's frame: a new
 *  piece where it crosses the edge of the map. Its length in km and its bearing come with it. */
export function greatCircle(a, b) {
  const v = ({lat, lon}) => [Math.cos(rad(lat)) * Math.cos(rad(lon)),
                             Math.cos(rad(lat)) * Math.sin(rad(lon)), Math.sin(rad(lat))];
  const [p, q] = [v(a), v(b)];
  const dot = Math.min(1, Math.max(-1, p[0] * q[0] + p[1] * q[1] + p[2] * q[2]));
  const w = Math.acos(dot);
  const n = Math.max(2, Math.ceil(deg(w)));
  const pieces = [[]];
  let last = null;
  for (let i = 0; i <= n; i += 1) {
    const t = i / n;
    const [s1, s2] = w < 1e-9 ? [1 - t, t] : [Math.sin((1 - t) * w) / Math.sin(w), Math.sin(t * w) / Math.sin(w)];
    const [x, y, z] = [0, 1, 2].map((k) => s1 * p[k] + s2 * q[k]);
    const lat = deg(Math.atan2(z, Math.hypot(x, y)));
    const lon = deg(Math.atan2(y, x));
    if (last !== null && Math.abs(lon - last) > 180) pieces.push([]);
    pieces.at(-1).push(xy(lat, lon));
    last = lon;
  }
  return {pieces: pieces.filter((piece) => piece.length > 1), km: EARTH_KM * w};
}

/** The map, drawn into `svg`. `onPick(callsign)` when a gateway is clicked, `onPoint(lat, lon)`
 *  when the map is clicked while placing. */
export class WorldMap {
  /** `narrowest`: degrees across at the most zoom (a grid square's width twice by default; the
   *  Reports view goes to a town's); `fitLeast`: the least a fit shows, in degrees across. */
  constructor(svg, {onPick = () => {}, onPoint = () => {}, onReport = () => {},
                    narrowest = NARROWEST, fitLeast = 20} = {}) {
    this.svg = svg;
    this.onPick = onPick;
    this.onPoint = onPoint;
    this.onReport = onReport;
    this.narrowest = narrowest;
    this.fitLeast = fitLeast;
    this.here = null;
    this.gateways = [];
    this.reports = [];
    this.pickedReport = null;
    this.picked = null;
    this.point = null;
    this.placingOn = false;
    this.view = {x: 0, y: 0, w: WIDEST};
    svg.setAttribute("preserveAspectRatio", "xMidYMid meet");
    const sea = node("rect", {class: "m-sea", x: 0, y: 0, width: 360, height: 180});
    const land = node("path", {class: "m-land", d: LAND});
    const fields = [];
    for (let x = 20; x < 360; x += 20) fields.push(`M${x} 0V180`);
    for (let y = 10; y < 180; y += 10) fields.push(`M0 ${y}H360`);
    const grid = node("path", {class: "m-fields", d: fields.join("")});
    grid.append(Object.assign(node("title"), {textContent: "Maidenhead fields: 20 degrees of longitude by 10 of latitude"}));
    this.night = node("path", {class: "m-night"});
    this.layer = node("g");
    svg.replaceChildren(sea, land, this.night, grid, this.layer);
    this.wire();
    this.drawNight();
    this.apply();
  }

  /** What to show: the position in use ({lat, lon, grid}, or null) and the gateways, each with
   *  `callsign` and `at` ({lat, lon}), and `favourite`, `worked`, `own`, `blocked`, `old`. */
  show({here, gateways}) {
    this.here = here && here.lat != null ? here : null;
    this.gateways = (gateways || []).filter((g) => g.at && g.at.lat != null);
    if (this.picked && !this.gateways.some((g) => g.callsign === this.picked)) this.picked = null;
    this.draw();
  }

  /** The reports to show (the Reports view): each with `id`, `lat`, `lon`, `colour` (a CSS
   *  colour) and `title`. */
  showReports(reports) {
    this.reports = (reports || []).filter((r) => r.lat != null);
    if (this.pickedReport && !this.reports.some((r) => r.id === this.pickedReport)) {
      this.pickedReport = null;
    }
    this.draw();
  }

  /** Marks one report picked, or none. */
  pickReport(id) {
    this.pickedReport = id || null;
    this.draw();
  }

  /** Moves the position in use alone, as a GPS moves it. */
  setHere(here) {
    const next = here && here.lat != null ? here : null;
    const near = (a, b) => Math.abs(a.lat - b.lat) < 1e-4 && Math.abs(a.lon - b.lon) < 1e-4;
    if ((!next && !this.here) || (next && this.here && near(next, this.here))) return;
    this.here = next;
    this.draw();
  }

  /** Marks one gateway picked, with its great circle drawn, or none. */
  pick(callsign) {
    this.picked = callsign || null;
    this.draw();
  }

  /** While on, a click on the map that is not on a gateway marks a point and is handed on. */
  placing(on) {
    this.placingOn = on;
    this.svg.classList.toggle("placing", on);
    if (!on) this.point = null;
    this.draw();
  }

  /** The whole world. */
  world() {
    this.view = {x: 0, y: 0, w: WIDEST};
    this.apply();
  }

  /** The position in use, the gateways and the reports shown, with a margin; the world when there
   *  is none. */
  fit() {
    const pts = this.gateways.map((g) => xy(g.at.lat, g.at.lon))
      .concat(this.reports.map((r) => xy(r.lat, r.lon)));
    if (this.here) pts.push(xy(this.here.lat, this.here.lon));
    if (!pts.length) {
      this.world();
      return;
    }
    const xs = pts.map((p) => p[0]);
    const ys = pts.map((p) => p[1]);
    const [x0, x1, y0, y1] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
    const w = Math.max(this.fitLeast, (x1 - x0) * 1.3, (y1 - y0) * 2 * 1.3);
    this.view = {x: (x0 + x1) / 2 - w / 2, y: (y0 + y1) / 2 - w / 4, w};
    this.apply();
  }

  /** Zooms by `factor` (above 1 in) about a point of the frame, the view's centre by default. */
  zoom(factor, at) {
    const v = this.view;
    const [cx, cy] = at || [v.x + v.w / 2, v.y + v.w / 4];
    const w = Math.min(WIDEST, Math.max(this.narrowest, v.w / factor));
    const k = w / v.w;
    this.view = {x: cx - (cx - v.x) * k, y: cy - (cy - v.y) * k, w};
    this.apply();
  }

  // keeps the view on the map, sets the viewBox, and redraws the marks at the new scale
  apply() {
    const v = this.view;
    v.w = Math.min(WIDEST, Math.max(this.narrowest, v.w));
    const h = v.w / 2;
    v.x = Math.min(360 - v.w, Math.max(0, v.x));
    v.y = Math.min(180 - h, Math.max(0, v.y));
    this.svg.setAttribute("viewBox", `${v.x} ${v.y} ${v.w} ${h}`);
    this.draw();
  }

  /** The night side redrawn for now: the page calls it every few minutes. */
  drawNight() {
    this.night.setAttribute("d", nightPath());
  }

  // map units per screen pixel
  unit() {
    return this.view.w / Math.max(1, this.svg.clientWidth || 720);
  }

  // the frame's point under a pointer event
  frameAt(event) {
    const box = this.svg.getBoundingClientRect();
    const v = this.view;
    const scale = Math.min(box.width / v.w, box.height / (v.w / 2)) || 1;
    const left = box.left + (box.width - v.w * scale) / 2;
    const top = box.top + (box.height - (v.w / 2) * scale) / 2;
    return [v.x + (event.clientX - left) / scale, v.y + (event.clientY - top) / scale];
  }

  draw() {
    const u = this.unit();
    const marks = this.grid(u);
    const pickedGw = this.gateways.find((g) => g.callsign === this.picked);
    if (pickedGw && this.here) {
      const gc = greatCircle(this.here, pickedGw.at);
      for (const piece of gc.pieces) {
        marks.push(node("path", {class: "m-path",
          d: `M${piece.map(([x, y]) => `${x.toFixed(2)} ${y.toFixed(2)}`).join("L")}`}));
      }
    }
    const labels = this.view.w < LABELS_UNDER;
    // the picked one last, so it is drawn on top
    const order = [...this.gateways].sort((a, b) =>
      Number(a.callsign === this.picked) - Number(b.callsign === this.picked));
    for (const g of order) {
      const [x, y] = xy(g.at.lat, g.at.lon);
      const picked = g.callsign === this.picked;
      const cls = ["m-gw", g.favourite ? "fav" : g.worked ? "worked" : g.own ? "own" : "listed"];
      if (g.blocked) cls.push("blocked");
      if (g.old) cls.push("old");
      if (picked) cls.push("picked");
      const mark = node("g", {class: cls.join(" "), "data-callsign": g.callsign});
      const r = (picked ? 7 : 5) * u;
      mark.append(node("circle", {class: "m-hit", cx: x, cy: y, r: 11 * u}), shape(cls[1], x, y, r));
      if (labels || picked) {
        mark.append(Object.assign(node("text", {x: x + r + 3 * u, y: y + 4 * u,
          "font-size": 12 * u}), {textContent: g.callsign}));
      }
      const words = [g.callsign, g.where, g.km != null ? `${g.km} km` : "",
                     g.bearing != null ? `at ${g.bearing}°` : ""].filter(Boolean).join(", ");
      mark.append(Object.assign(node("title"), {textContent: words}));
      marks.push(mark);
    }
    // the reports, the picked one last and larger, on top
    const reports = [...this.reports].sort((a, b) =>
      Number(a.id === this.pickedReport) - Number(b.id === this.pickedReport));
    for (const r of reports) {
      const [x, y] = xy(r.lat, r.lon);
      const picked = r.id === this.pickedReport;
      const mark = node("g", {class: picked ? "m-report picked" : "m-report", "data-report": r.id});
      mark.append(node("circle", {class: "m-hit", cx: x, cy: y, r: 11 * u}),
        node("circle", {class: "m-dot", cx: x, cy: y, r: (picked ? 7 : 5) * u, fill: r.colour}),
        Object.assign(node("title"), {textContent: r.title}));
      marks.push(mark);
    }
    if (this.here) {
      const [x, y] = xy(this.here.lat, this.here.lon);
      const you = node("g", {class: "m-here"});
      you.append(node("circle", {cx: x, cy: y, r: 7 * u, class: "ring"}),
                 node("circle", {cx: x, cy: y, r: 2.5 * u, class: "dot"}),
                 Object.assign(node("title"), {textContent: `You${this.here.grid ? `, ${this.here.grid}` : ""}: the position in use`}));
      marks.push(you);
    }
    if (this.point) {
      const [x, y] = xy(this.point.lat, this.point.lon);
      const s = 7 * u;
      marks.push(node("path", {class: "m-point", d: `M${x - s} ${y}H${x + s}M${x} ${y - s}V${y + s}`}));
    }
    this.layer.replaceChildren(...marks);
  }

  // the Maidenhead grid as far as the view shows it: the fields named (EM), and closer in the
  // squares drawn and then named (EM73), as amateurs find their way
  grid(u) {
    const v = this.view;
    const h = v.w / 2;
    const out = [];
    const letter = (i) => String.fromCharCode(65 + i);
    const label = (x, y, text, cls) => Object.assign(
      node("text", {class: cls, x: x + 4 * u, y: y + 13 * u, "font-size": 11 * u}), {textContent: text});
    if (v.w <= SUBSQUARES_UNDER) {
      // the subsquares: 5 minutes of longitude by 2.5 of latitude, 24 to a square each way
      const [dx, dy] = [2 / 24, 1 / 24];
      const lines = [];
      for (let x = Math.ceil(v.x / dx) * dx; x <= v.x + v.w; x += dx) lines.push(`M${x} ${v.y}V${v.y + h}`);
      for (let y = Math.ceil(v.y / dy) * dy; y <= v.y + h; y += dy) lines.push(`M${v.x} ${y}H${v.x + v.w}`);
      out.push(node("path", {class: "m-squares", d: lines.join("")}));
      if (v.w <= SUBSQUARE_NAMES_UNDER) {
        const sub = (i) => String.fromCharCode(97 + i);
        for (let x = Math.floor(v.x / dx) * dx; x < v.x + v.w; x += dx) {
          for (let y = Math.floor(v.y / dy) * dy; y < v.y + h; y += dy) {
            // the subsquare whose top-left corner is (x, y): its name, as the locator reads
            const lon = x - 180 + dx / 2;
            const lat = 90 - y - dy / 2;
            const fx = Math.floor((lon + 180) / 20);
            const fy = Math.floor((lat + 90) / 10);
            const sx = Math.floor(((lon + 180) % 20) / 2);
            const sy = Math.floor((lat + 90) % 10);
            const tx = Math.floor((((lon + 180) % 2) * 12));
            const ty = Math.floor((((lat + 90) % 1) * 24));
            out.push(label(x, y, `${letter(fx)}${letter(fy)}${sx}${sy}${sub(tx)}${sub(ty)}`,
              "m-grid-name"));
          }
        }
      }
    }
    if (v.w <= SQUARES_UNDER) {
      const lines = [];
      for (let x = Math.ceil(v.x / 2) * 2; x <= v.x + v.w; x += 2) lines.push(`M${x} ${v.y}V${v.y + h}`);
      for (let y = Math.ceil(v.y); y <= v.y + h; y += 1) lines.push(`M${v.x} ${y}H${v.x + v.w}`);
      out.push(node("path", {class: "m-squares", d: lines.join("")}));
    }
    if (v.w <= FIELDS_UNDER) {
      for (let i = Math.max(0, Math.floor(v.x / 20)); i * 20 < v.x + v.w && i < 18; i += 1) {
        for (let j = 0; j < 18; j += 1) {
          const top = 180 - 10 * (j + 1);
          if (top + 10 <= v.y || top >= v.y + h) continue;
          if (v.w <= SQUARE_NAMES_UNDER) {
            for (let a = 0; a < 10; a += 1) {
              for (let b = 0; b < 10; b += 1) {
                const [x, y] = [20 * i + 2 * a, top + 9 - b];
                if (x + 2 <= v.x || x >= v.x + v.w || y + 1 <= v.y || y >= v.y + h) continue;
                out.push(label(x, y, `${letter(i)}${letter(j)}${a}${b}`, "m-grid-name"));
              }
            }
          } else {
            out.push(label(20 * i, Math.max(top, v.y), `${letter(i)}${letter(j)}`, "m-grid-name"));
          }
        }
      }
    }
    return out;
  }

  wire() {
    const svg = this.svg;
    let press = null;
    svg.addEventListener("wheel", (event) => {
      event.preventDefault();
      this.zoom(event.deltaY < 0 ? STEP : 1 / STEP, this.frameAt(event));
    }, {passive: false});
    svg.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      press = {sx: event.clientX, sy: event.clientY, view: {...this.view}, dragged: false,
               id: event.pointerId};
    });
    svg.addEventListener("pointermove", (event) => {
      if (!press || event.pointerId !== press.id) return;
      const dx = event.clientX - press.sx;
      const dy = event.clientY - press.sy;
      if (!press.dragged && Math.hypot(dx, dy) < DRAG) return;
      if (!press.dragged) svg.setPointerCapture(event.pointerId);
      press.dragged = true;
      svg.classList.add("dragging");
      const box = svg.getBoundingClientRect();
      const scale = Math.min(box.width / press.view.w, box.height / (press.view.w / 2)) || 1;
      this.view = {x: press.view.x - dx / scale, y: press.view.y - dy / scale, w: press.view.w};
      this.apply();
    });
    const end = (event) => {
      if (!press || event.pointerId !== press.id) return;
      const was = press;
      press = null;
      svg.classList.remove("dragging");
      if (was.dragged || event.type === "pointercancel") return;
      const report = event.target.closest && event.target.closest(".m-report");
      if (report) {
        this.pickReport(report.dataset.report);
        this.onReport(report.dataset.report);
        return;
      }
      const mark = event.target.closest && event.target.closest(".m-gw");
      if (mark) {
        this.pick(mark.dataset.callsign);
        this.onPick(mark.dataset.callsign);
      } else if (this.placingOn) {
        const [x, y] = this.frameAt(event);
        const lat = Math.max(-90, Math.min(90, 90 - y));
        const lon = ((((x - 180) % 360) + 540) % 360) - 180;
        this.point = {lat, lon};
        this.draw();
        this.onPoint(lat, lon);
      }
    };
    svg.addEventListener("pointerup", end);
    svg.addEventListener("pointercancel", end);
    new ResizeObserver(() => this.draw()).observe(svg);
  }
}

// a gateway's mark: ○ listed, ■ yours, ★ a favourite, ◆ worked from here
function shape(kind, x, y, r) {
  if (kind === "fav") {
    const pts = [];
    for (let i = 0; i < 10; i += 1) {
      const a = -Math.PI / 2 + (i * Math.PI) / 5;
      const rr = i % 2 ? r * 0.45 : r * 1.15;
      pts.push(`${(x + rr * Math.cos(a)).toFixed(3)},${(y + rr * Math.sin(a)).toFixed(3)}`);
    }
    return node("polygon", {class: "m-shape", points: pts.join(" ")});
  }
  if (kind === "worked") {
    const s = r * 1.15;
    return node("polygon", {class: "m-shape", points: `${x},${y - s} ${x + s},${y} ${x},${y + s} ${x - s},${y}`});
  }
  if (kind === "own") {
    return node("rect", {class: "m-shape", x: x - r * 0.85, y: y - r * 0.85, width: r * 1.7, height: r * 1.7});
  }
  return node("circle", {class: "m-shape", cx: x, cy: y, r});
}
