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
