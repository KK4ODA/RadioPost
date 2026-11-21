// SPDX-License-Identifier: MIT OR Apache-2.0
// A VHF path's terrain profile, drawn (the gateway selection design, §16.3): the ground with the
// Earth's bulge added, the direct ray between the two antennas, the lower edge of 60 % of the
// first Fresnel zone, the antennas, and where the path is obstructed. Every number comes from the
// panel's /api/terrain/path; nothing is worked out here but where to draw it.

const NS = "http://www.w3.org/2000/svg";
const W = 720;
const H = 240;
const PAD = {left: 52, right: 12, top: 14, bottom: 26};

function node(tag, attrs = {}, text = "") {
  const n = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) n.setAttribute(k, String(v));
  if (text) n.textContent = text;
  return n;
}

// round steps for an axis: 1, 2 or 5 times a power of ten, about `count` of them over the span
export function niceStep(span, count) {
  if (!(span > 0)) return 1;
  const raw = span / Math.max(1, count);
  const power = 10 ** Math.floor(Math.log10(raw));
  for (const m of [1, 2, 5, 10]) {
    if (raw <= m * power) return m * power;
  }
  return 10 * power;
}

// the drawing's scales: distance across, height up, with a little room above the highest point
export function scales(p) {
  const d = p.d_km;
  const length = d[d.length - 1] || 1;
  const heights = [...p.ground_m, ...p.ray_m, ...p.ends_m];
  let low = Math.min(...p.ground_m);
  let high = Math.max(...heights);
  const room = Math.max(10, (high - low) * 0.08);
  low -= room;
  high += room;
  const x = (km) => PAD.left + (km / length) * (W - PAD.left - PAD.right);
  const y = (m) => H - PAD.bottom - ((m - low) / (high - low)) * (H - PAD.top - PAD.bottom);
  return {x, y, length, low, high};
}

// draws profile `p` into `svg`; `at_km` marks where the path is obstructed, `kind` how
// ("partly" or "blocked")
export function drawProfile(svg, p, mark = null) {
  svg.replaceChildren();
  svg.setAttribute("viewBox", `0 0 ${W} ${H}`);
  if (!p || !p.d_km || p.d_km.length < 2) return;
  const {x, y, length, low, high} = scales(p);
  const n = p.d_km.length;
  const pts = (values) => values.map((v, i) => `${x(p.d_km[i]).toFixed(1)},${y(v).toFixed(1)}`).join(" ");

  // height lines and their labels
  const step = niceStep(high - low, 4);
  for (let m = Math.ceil(low / step) * step; m <= high; m += step) {
    svg.append(node("line", {class: "t-grid", x1: PAD.left, x2: W - PAD.right, y1: y(m), y2: y(m)}));
    svg.append(node("text", {class: "t-label", x: PAD.left - 6, y: y(m) + 4, "text-anchor": "end"},
      `${Math.round(m)} m`));
  }
  const kmStep = niceStep(length, 6);
  for (let km = 0; km <= length + 1e-9; km += kmStep) {
    svg.append(node("text", {class: "t-label", x: x(km), y: H - 8, "text-anchor": "middle"},
      `${Number(km.toFixed(3))} km`));
  }

  // the ground, filled down to the bottom of the drawing
  const bottom = H - PAD.bottom;
  svg.append(node("polygon", {class: "t-ground",
    points: `${x(p.d_km[0])},${bottom} ${pts(p.ground_m)} ${x(p.d_km[n - 1])},${bottom}`}));
  // 60 % of the first Fresnel zone below the ray, and the ray
  svg.append(node("polyline", {class: "t-fresnel", points: pts(p.f60_m)}));
  svg.append(node("polyline", {class: "t-ray", points: pts(p.ray_m)}));
  // the antennas: from the ground at each end up to the antenna
  const ends = [[0, p.ends_m[0], p.agl_m[0]], [n - 1, p.ends_m[1], p.agl_m[1]]];
  for (const [i, top, agl] of ends) {
    const at = x(p.d_km[i]);
    svg.append(node("line", {class: "t-mast", x1: at, x2: at, y1: y(top - agl), y2: y(top)}));
    svg.append(node("circle", {class: "t-antenna", cx: at, cy: y(top), r: 3.5}));
  }
  // where it is obstructed
  if (mark && mark.at_km != null) {
    let i = 0;
    for (let k = 1; k < n; k++) {
      if (Math.abs(p.d_km[k] - mark.at_km) < Math.abs(p.d_km[i] - mark.at_km)) i = k;
    }
    svg.append(node("circle", {class: `t-mark ${mark.kind}`, cx: x(p.d_km[i]), cy: y(p.ground_m[i]), r: 5}));
  }
}

// -- the terrain map (§16.2): you, the VHF gateways whose records give their site, each path
// coloured by its chance and its obstacle marked, over a hillshade of the terrain held --

const AREA_W = 720;
const MAX_SPAN_DEG = 4;

// the area to show: you and the sites, with room round them, never under 0.08° a side
export function areaOf(here, sites) {
  const lats = [here.lat, ...sites.map((s) => s.lat)];
  const lons = [here.lon, ...sites.map((s) => s.lon)];
  let [south, north] = [Math.min(...lats), Math.max(...lats)];
  let [west, east] = [Math.min(...lons), Math.max(...lons)];
  const cos = Math.cos(((south + north) / 2) * Math.PI / 180);
  // square on the ground, then 12 % of room each side
  const tall = Math.max(north - south, (east - west) * cos, 0.08);
  const wide = tall / cos;
  const [midLat, midLon] = [(south + north) / 2, (west + east) / 2];
  south = midLat - tall * 0.62;
  north = midLat + tall * 0.62;
  west = midLon - wide * 0.62;
  east = midLon + wide * 0.62;
  const r4 = (x) => Math.round(x * 1e4) / 1e4;
  return {south: r4(Math.max(-90, south)), north: r4(Math.min(90, north)),
          west: r4(west), east: r4(east)};
}

export class AreaMap {
  constructor(svg, {onPick}) {
    this.svg = svg;
    this.onPick = onPick;
    this.picked = null;
  }

  // `here` {lat, lon}; `gateways` the list's VHF gateways, each with its `site` and `terrain`
  show({here, gateways}) {
    const svg = this.svg;
    svg.replaceChildren();
    const area = areaOf(here, gateways.map((g) => g.site));
    const cos = Math.cos(((area.south + area.north) / 2) * Math.PI / 180);
    const h = Math.round(AREA_W * (area.north - area.south) / ((area.east - area.west) * cos));
    svg.setAttribute("viewBox", `0 0 ${AREA_W} ${h}`);
    const x = (lon) => (lon - area.west) / (area.east - area.west) * AREA_W;
    const y = (lat) => (area.north - lat) / (area.north - area.south) * h;
    if (area.north - area.south <= MAX_SPAN_DEG && area.east - area.west <= MAX_SPAN_DEG) {
      const q = new URLSearchParams({...area, width: AREA_W});
      svg.append(node("image", {href: `/api/terrain/hillshade?${q}`, x: 0, y: 0, width: AREA_W,
                                height: h, preserveAspectRatio: "none"}));
    }
    this.lines = new Map();
    for (const g of gateways) {
      const t = g.terrain;
      const kind = t.state === "judged" ? {"likely to work": "likely", "may work": "may",
                                           "unlikely to work": "unlikely"}[t.chance] : "none";
      const line = node("line", {class: `a-path ${kind}`, x1: x(here.lon), y1: y(here.lat),
                                 x2: x(g.site.lon), y2: y(g.site.lat)});
      line.append(node("title", {}, `${g.callsign}: ${t.words}`));
      line.addEventListener("click", () => this.onPick(g.callsign));
      svg.append(line);
      this.lines.set(g.callsign, line);
      // where the path is obstructed, along the line
      const c = t.class;
      if (c && c.at_km != null && t.length_km) {
        const f = Math.min(1, c.at_km / t.length_km);
        const ox = x(here.lon + (g.site.lon - here.lon) * f);
        const oy = y(here.lat + (g.site.lat - here.lat) * f);
        const mark = node("path", {class: `a-obstacle ${c.kind}`,
                                   d: `M${ox} ${oy - 6} L${ox + 5.5} ${oy + 4} L${ox - 5.5} ${oy + 4} Z`});
        mark.append(node("title", {}, `${g.callsign}: ${t.class_words || t.words}`));
        svg.append(mark);
      }
    }
    for (const g of gateways) {
      const at = node("g", {class: "a-gateway", tabindex: 0, role: "button",
                            "aria-label": `${g.callsign}: ${g.terrain.words}`});
      // the label to the right, or to the left near the right edge
      const right = x(g.site.lon) < AREA_W - 100;
      at.append(node("circle", {cx: x(g.site.lon), cy: y(g.site.lat), r: 5}),
                node("text", {x: x(g.site.lon) + (right ? 8 : -8), y: y(g.site.lat) + 4,
                              "text-anchor": right ? "start" : "end"}, g.callsign));
      at.addEventListener("click", () => this.onPick(g.callsign));
      at.addEventListener("keydown", (e) => { if (e.key === "Enter") this.onPick(g.callsign); });
      svg.append(at);
    }
    svg.append(node("circle", {class: "a-you", cx: x(here.lon), cy: y(here.lat), r: 7}),
               node("circle", {class: "a-you-dot", cx: x(here.lon), cy: y(here.lat), r: 2.5}));
    // a scale: a round number of kilometres, a fifth to two fifths of the width
    const kmWide = (area.east - area.west) * 111.32 * cos;
    const step = niceStep(kmWide / 5, 1);
    const px = step / kmWide * AREA_W;
    svg.append(node("line", {class: "a-scale", x1: 12, x2: 12 + px, y1: h - 14, y2: h - 14}),
               node("text", {class: "a-scale-text", x: 12, y: h - 20}, `${step} km`));
    this.pick(this.picked);
  }

  pick(callsign) {
    this.picked = callsign;
    for (const [c, line] of this.lines || []) line.classList.toggle("picked", c === callsign);
  }
}
