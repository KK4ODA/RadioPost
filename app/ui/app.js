// SPDX-License-Identifier: MIT OR Apache-2.0
// The RadioPost client's panel: the folders, a reader, a composer, the station's calls and its
// setup, over the client's JSON API (app/client/src/panel.rs). No build step.

import { WorldMap } from "./map.js";
import { AreaMap, drawProfile } from "./terrain.js";

const $ = (id) => document.getElementById(id);

// The panel stacks its parts in one column at this width and under (style.css's @media rule for
// it): the folders and the station then fill the window and push the rest below it. There Write,
// Reply, Forward and writing to an address open the composer in a window of its own: a pop-up in a
// browser, a window of the desktop app's own in RadioPost (app/src-tauri). That window loads this
// page with `?compose=`, and the page is then the composer alone (`ALONE`).
const STACKED = window.matchMedia("(max-width: 720px)");
const ALONE = new URLSearchParams(location.search).has("compose");

let folder = "inbox";
let logNext = 0;
let meAddress = "";
let lastState = "idle";
let myDevice = "";  // this device's public key, base64url, as the post office writes keys
let myVersion = "";  // this client's version, from its update watch

async function api(path, body) {
  const options = body === undefined
    ? {}
    : {method: "POST", headers: {"Content-Type": "application/json", "X-RadioPost": "1"},
       body: JSON.stringify(body)};
  const response = await fetch(`/api/${path}`, options);
  const value = await response.json();
  if (!response.ok) throw new Error(value.error || response.statusText);
  return value;
}

// a line at the bottom right, and with `action` ({label, run}) a button in it: Undo
function toast(text, bad = false, action = null) {
  const t = $("toast");
  $("toast-text").textContent = text;
  const b = $("toast-act");
  b.hidden = !action;
  b.onclick = null;
  if (action) {
    b.textContent = action.label;
    b.onclick = async () => {
      t.hidden = true;
      try { await action.run(); } catch (e) { toast(e.message, true); }
    };
  }
  t.classList.toggle("bad", bad);
  t.hidden = false;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => { t.hidden = true; }, bad ? 7000 : action ? 8000 : 3000);
}

// a button that asks for a second click before it does what cannot be undone
function armed(button, sure, run) {
  if (button.dataset.armed) {
    delete button.dataset.armed;
    button.textContent = button.dataset.label;
    run();
    return;
  }
  button.dataset.label = button.textContent;
  button.dataset.armed = "1";
  button.textContent = sure;
  setTimeout(() => {
    if (!button.dataset.armed) return;
    delete button.dataset.armed;
    button.textContent = button.dataset.label;
  }, 4000);
}

function icon(name) {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("class", "icon");
  svg.setAttribute("aria-hidden", "true");
  const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
  use.setAttribute("href", `#i-${name}`);
  svg.append(use);
  return svg;
}

const FOLDER_NAMES = {inbox: "Inbox", outbox: "Outbox", sent: "Sent", trash: "Trash"};

// The trash (deleting is this computer's own: the post office and other stations keep theirs)
async function toTrash(id, from) {
  try {
    await api("delete", {folder: from, id});
    toast(`Moved to the trash from the ${from}.`, false,
          {label: "Undo", run: async () => { await api("restore", {id, was: from}); await afterTrash(); }});
    await afterTrash();
  } catch (e) {
    toast(e.message, true);
  }
}

async function fromTrash(path, id, was, said) {
  try {
    await api(path, {id, was});
    toast(said);
    await afterTrash();
  } catch (e) {
    toast(e.message, true);
  }
}

async function afterTrash() {
  await refresh();
  await openFolder(folder);
}

function when(seconds) {
  if (!seconds) return "";
  const d = new Date(seconds * 1000);
  const today = new Date().toDateString() === d.toDateString();
  return today
    ? d.toLocaleTimeString([], {hour: "2-digit", minute: "2-digit"})
    : d.toLocaleDateString([], {month: "short", day: "numeric"});
}

function show(view) {
  for (const id of ["list-view", "waiting-view", "gateways-view", "addresses-view", "alerts-view",
                    "reports-view", "reader", "composer", "setup", "office"]) {
    $(id).hidden = id !== view;
  }
  // the usual gateways are read afresh each time, for the callsign in use now
  if (view === "setup") {
    loadUsual();
    loadPlaces(true);
    loadTerrainSetup();
  }
}

// -- where the station is (choosing a gateway, §6): places, the GPS, the position in use --------

const PLACE_SOURCES = {typed: "Typed", grid: "A grid square's centre", gps: "The GPS", map: "The map"};

// the position in use in a line: where it comes from, its square, and why, when it is not the
// GPS's present fix
function hereWords(here, note) {
  const said = note ? `${note[0].toUpperCase()}${note.slice(1)}` : "";
  if (!here) return said ? `${said}.` : "No position: add a place, or set up a GPS.";
  if (here.from === "gps") return `In use: the GPS, ${here.grid}, ${here.age_s} s ago.`;
  // a note says why a place stands in, and names it: "GPS silent for 6 minutes: using Home"
  return said ? `${said}, ${here.grid}.` : `In use: ${here.name}, ${here.grid}.`;
}

// what the panel says of the GPS, in a line
function gpsWords(v) {
  if (!v.gps) return "No GPS.";
  const st = v.status;
  const what = {serial: "Serial", tcp: "TCP", udp: "UDP", gpsd: "gpsd"}[v.gps.kind] || v.gps.kind;
  const where = v.gps.kind === "serial" ? v.gps.port
    : v.gps.kind === "udp" ? `port ${v.gps.port}` : `${v.gps.host}:${v.gps.port}`;
  return `${what}, ${where}: ${st ? st.words : "starting"}.`;
}

// `fill`: the GPS's settings are put in the form too, with the serial ports listed; otherwise only
// what changes by itself (the position, the GPS's state) is shown, so typing is not undone
async function loadPlaces(fill) {
  let v;
  try {
    v = await api(fill ? "places?ports=1" : "places");
  } catch (e) {
    $("here-line").textContent = e.message;
    return;
  }
  renderPlaces(v, fill);
}

function renderPlaces(v, fill) {
  $("here-line").textContent = v.broken ? v.broken : hereWords(v.here, v.note);
  $("here-line").classList.toggle("warn", Boolean(v.broken));
  $("gps-state").textContent = gpsWords(v);
  const fresh = v.status && ["current", "recent"].includes(v.status.freshness);
  $("place-add-gps").disabled = !fresh;
  if (!fill && !placesChanged(v)) return;
  lastPlaces = JSON.stringify([v.places, v.choice, v.stations, v.station]);
  $("place-rows").replaceChildren(...v.places.map((p) => {
    const tr = document.createElement("tr");
    const name = document.createElement("td");
    name.textContent = p.name;
    const square = document.createElement("td");
    square.textContent = p.square;
    square.title = `${p.lat.toFixed(5)}, ${p.lon.toFixed(5)}`
      + (p.source === "grid" ? `: the centre of ${p.grid}` : "");
    const from = document.createElement("td");
    from.textContent = PLACE_SOURCES[p.source] || p.source;
    const def = document.createElement("td");
    const radio = document.createElement("input");
    radio.type = "radio";
    radio.name = "place-default";
    radio.checked = p.default;
    radio.title = "Make this the default place";
    radio.addEventListener("change", () => placeRequest({op: "default", id: p.id}));
    def.append(radio);
    // the station profile this place uses (§6.5)
    const st = document.createElement("td");
    const pick = el("select", {class: "narrow", title: `The station profile ${p.name} uses`});
    pick.append(new Option("—", ""), ...(v.stations || []).map((x) => new Option(x.name, x.id)));
    pick.value = p.station || "";
    pick.addEventListener("change", () => placeRequest({op: "place-station", id: p.id, station: pick.value}));
    st.append(pick);
    const acts = document.createElement("td");
    acts.className = "acts";
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "quiet";
    remove.textContent = "Remove";
    remove.title = `Forget ${p.name}`;
    remove.addEventListener("click", () => placeRequest({op: "remove", id: p.id}));
    acts.append(remove);
    tr.append(name, square, from, def, st, acts);
    return tr;
  }));
  const choice = $("place-choice");
  const options = [["automatic", "Automatic: the GPS, else a saved place"],
                   ["gps", "The GPS, a saved place only while it has no fix"],
                   ...v.places.map((p) => [`place:${p.id}`, `Always ${p.name}`])];
  choice.replaceChildren(...options.map(([value, label]) => new Option(label, value)));
  choice.value = v.choice.use === "place" ? `place:${v.choice.id}` : v.choice.use;
  renderStations(v);
  if (!fill) return;
  const g = v.gps || {kind: ""};
  $("gps-kind").value = g.kind;
  if (v.ports) fillPorts(v.ports, g.kind === "serial" ? g.port : "");
  $("gps-baud").value = g.baud ? String(g.baud) : "";
  $("gps-host").value = g.host || "";
  $("gps-net-port").value = g.port && g.kind !== "serial" ? g.port : "";
  showGpsKind();
}

let lastPlaces = "";
function placesChanged(v) {
  return JSON.stringify([v.places, v.choice, v.stations, v.station]) !== lastPlaces;
}

// -- station profiles (choosing a gateway, §6.5): every value used, set or assumed ---------------

const ANTENNAS = {dipole: "dipole", vertical: "vertical", whip: "whip", yagi: "Yagi", "j-pole": "J-pole",
                  collinear: "collinear", loop: "loop", nvis: "NVIS", other: "other"};

// one side of a profile in a line: what is set plainly, what is assumed faint
function sideWords(u) {
  const part = (used, text) => el("span", {class: used.said === "assumed" ? "assumed" : "",
    text, title: used.said === "assumed" ? "Assumed: not set in the profile" : "Set in the profile"});
  return el("div", {},
    part(u.power_w, `${u.power_w.value} W`), ", ",
    part(u.antenna, ANTENNAS[u.antenna.value] || u.antenna.value), " ",
    part(u.agl_m, `${u.agl_m.value} m up`), ", ",
    part(u.gain_dbi, `${u.gain_dbi.value} dBi`), ", ",
    part(u.feedline_db, `${u.feedline_db.value} dB feed line`));
}

function renderStations(v) {
  const stations = v.stations || [];
  const inUse = stations.find((x) => x.id === v.station_in_use);
  $("station-line").textContent = inUse
    ? `In use: ${inUse.name}${v.station ? "" : ", by the place in use"}.`
    : "No profile: every value is assumed (100 W at HF with a dipole 10 m up, 25 W at VHF).";
  $("station-rows").replaceChildren(...stations.map((x) => {
    const tr = el("tr", {});
    const name = el("div", {}, el("span", {text: x.name}),
      el("div", {class: "note", text: [x.radio, x.vehicle ? "in a vehicle" : ""].filter(Boolean).join(", ")}));
    const noise = el("span", {class: x.used.noise.said === "assumed" ? "assumed" : "",
      text: x.used.noise.value});
    const editB = el("button", {type: "button", class: "quiet", text: "Edit", title: `Change ${x.name}`});
    editB.addEventListener("click", () => fillStationForm(x));
    const removeB = el("button", {type: "button", class: "quiet", text: "Remove", title: `Forget ${x.name}`});
    removeB.addEventListener("click", () => placeRequest({op: "station-remove", id: x.id}));
    const acts = el("td", {class: "acts"}, editB, removeB);
    for (const c of [name, sideWords(x.used.hf), sideWords(x.used.vhf), noise]) tr.append(el("td", {}, c));
    tr.append(acts);
    return tr;
  }));
  const choice = $("station-choice");
  choice.replaceChildren(new Option("Automatic: the place's, else the first", ""),
    ...stations.map((x) => new Option(x.name, x.id)));
  choice.value = v.station || "";
}

function fillStationForm(x) {
  const p = x || {hf: {antenna: {}}, vhf: {antenna: {}}};
  $("st-id").value = p.id || "";
  $("st-name").value = p.name || "";
  $("st-radio").value = p.radio || "";
  $("st-vehicle").checked = Boolean(p.vehicle);
  for (const k of ["hf", "vhf"]) {
    const side = p[k] || {antenna: {}};
    const a = side.antenna || {};
    $(`st-${k}-power`).value = side.power_w ?? "";
    $(`st-${k}-kind`).value = a.kind || "";
    $(`st-${k}-agl`).value = a.agl_m ?? "";
    $(`st-${k}-gain`).value = a.gain_dbi ?? "";
    $(`st-${k}-feed`).value = side.feedline_db ?? "";
  }
  $("st-noise").value = p.noise || "";
  $("station-form-title").textContent = x ? `Change ${x.name}` : "Add a profile";
  $("station-form").open = true;
}

// the form as a profile: what is empty stays unset, and is assumed
function stationFromForm() {
  const num = (id) => {
    const t = $(id).value.trim().replace(",", ".");
    if (t === "") return undefined;
    const n = Number(t);
    if (!Number.isFinite(n)) {
      $(id).focus();
      throw new Error(`${$(id).closest("label").textContent.trim()}: a number, or empty`);
    }
    return n;
  };
  const side = (k) => {
    const antenna = {kind: $(`st-${k}-kind`).value || undefined, agl_m: num(`st-${k}-agl`),
                     gain_dbi: num(`st-${k}-gain`)};
    return {power_w: num(`st-${k}-power`), antenna, feedline_db: num(`st-${k}-feed`)};
  };
  return {id: $("st-id").value, name: $("st-name").value, radio: $("st-radio").value || undefined,
          vehicle: $("st-vehicle").checked, hf: side("hf"), vhf: side("vhf"),
          noise: $("st-noise").value || undefined};
}

function fillPorts(ports, current) {
  const names = current && !ports.includes(current) ? [current, ...ports] : ports;
  $("gps-port").replaceChildren(...(names.length ? names : [""]).map((n) =>
    new Option(n || "No serial ports found", n)));
  $("gps-port").value = current || names[0] || "";
}

function showGpsKind() {
  const k = $("gps-kind").value;
  $("gps-serial").hidden = k !== "serial";
  $("gps-net").hidden = !["tcp", "udp", "gpsd"].includes(k);
  $("gps-host-label").hidden = k === "udp";
  $("gps-net-port").placeholder = k === "gpsd" ? "2947" : "11123";
  if (k === "gpsd" && !$("gps-host").value) $("gps-host").value = "localhost";
}

async function placeRequest(body, fill = false) {
  try {
    renderPlaces(await api("places", body), fill);
    lastPlaces = "";
    await loadPlaces(fill);
    return true;
  } catch (e) {
    toast(e.message, true);
    return false;
  }
}

function gpsSource() {
  const kind = $("gps-kind").value;
  const port = Number($("gps-net-port").value || ($("gps-net-port").placeholder));
  switch (kind) {
    case "": return null;
    case "serial": {
      const baud = $("gps-baud").value;
      return baud ? {kind, port: $("gps-port").value, baud: Number(baud)}
        : {kind, port: $("gps-port").value};
    }
    case "udp": return {kind, port};
    default: return {kind, host: $("gps-host").value.trim(), port};
  }
}

function wirePlaces() {
  $("place-add").addEventListener("click", async () => {
    const done = await placeRequest(
      {op: "add", name: $("place-name").value, where: $("place-where").value});
    if (done) {
      $("place-name").value = "";
      $("place-where").value = "";
    }
  });
  $("place-add-gps").addEventListener("click", async () => {
    if (await placeRequest({op: "add-gps", name: $("place-name").value})) $("place-name").value = "";
  });
  $("place-choice").addEventListener("change", () => {
    const [use, id] = $("place-choice").value.split(":");
    placeRequest(id ? {op: "choice", use, id} : {op: "choice", use});
  });
  $("gps-kind").addEventListener("change", showGpsKind);
  $("st-save").addEventListener("click", async () => {
    let profile;
    try {
      profile = stationFromForm();
    } catch (e) {
      toast(e.message, true);
      return;
    }
    if (await placeRequest({op: "station-put", profile})) {
      fillStationForm(null);
      $("station-form").open = false;
    }
  });
  $("station-choice").addEventListener("change", () =>
    placeRequest({op: "station-choice", id: $("station-choice").value}));
  $("gps-ports").addEventListener("click", async () => {
    try {
      const v = await api("places?ports=1");
      fillPorts(v.ports || [], $("gps-port").value);
    } catch (e) {
      toast(e.message, true);
    }
  });
  $("gps-save").addEventListener("click", () => placeRequest({op: "gps", source: gpsSource()}, true));
  $("place-add-map").addEventListener("click", async () => {
    if (gwView === "recent") gwView = "recommended";
    await openGateways();
    $("map-card").open = true;
    $("map-point-name").value = $("place-name").value;
    setPlacing(true);
    $("map-card").scrollIntoView({block: "start"});
    toast("Click the spot on the map, then name it and save it.");
  });
  $("places-open").addEventListener("click", () => {
    show("setup");
    $("places-card").scrollIntoView({block: "start"});
  });
  // the position and the GPS's state move by themselves: shown while they are on screen
  setInterval(async () => {
    if (!$("setup").hidden) await loadPlaces(false);
    else if (!$("gateways-view").hidden) {
      try {
        const v = await api("places");
        $("gateways-here").textContent = hereWords(v.here, v.note);
        if (worldMap) worldMap.setHere(v.here);
      } catch { /* shown at the next */ }
    }
  }, 2000);
}

// a choice kept in this browser: whom to call, what to fetch
function kept(name, fallback) {
  try { return localStorage.getItem(`radiopost.${name}`) || fallback; } catch { return fallback; }
}
function keep(name, value) {
  try { localStorage.setItem(`radiopost.${name}`, value); } catch { /* not kept */ }
}

function kind() {
  return document.querySelector("input[name=kind]:checked").value;
}

// the gateway chosen from the list, and its channels: picking one tunes the radio there (W14).
// `channels` are all it lists, `shown` those the Channel list offers (the modem's own, when it
// lists any), each option's value its place in `shown`: one dial may carry two modems
let chosenGateway = null;

// the modem in the Modem box, by its RP-0 §9.4 name, as the panel learned it from the modem's
// hello (GET /api/state, `modem`); null while it is not known
let modemKind = null;

// RP-0 §9.4's modem names as people read them, as RadioPost Gateway's Setup writes them
const MODEM_LABELS = {aether: "Aether HF", "aether-fm": "Aether FM", mercury: "Mercury",
                      "vara-hf": "VARA HF", "vara-fm": "VARA FM", ardop: "ARDOP", ax25: "Packet",
                      tcp: "TCP"};
const modemLabel = (name) => MODEM_LABELS[name] || name;

// the channels of a gateway's that the modem in the Modem box can call on: those naming it, or
// all of them when the modem is not known or the gateway lists none for it. A channel kept from
// an old call names no modem, and is offered whatever the modem
function forModem(channels) {
  if (!modemKind) return channels;
  const mine = channels.filter((c) => !c.modem || c.modem === modemKind);
  return mine.length ? mine : channels;
}

// the channel picked in the Channel list, or null
function chosenChannel() {
  const gw = chosenGateway;
  if (!gw || $("channel-label").hidden) return null;
  return gw.shown[Number($("channel").value)] || null;
}

function showChannels() {
  const gw = chosenGateway;
  const fits = gw && kind() === "gateway"
    && $("callsign").value.trim().toUpperCase() === gw.callsign && gw.shown.length > 0;
  // hidden, not emptied: the gateway's callsign typed again brings its channels back
  $("channel-label").hidden = !fits;
  // a gateway with no channel for this modem stays choosable, and says so
  const named = fits && modemKind ? gw.shown.filter((c) => c.modem) : [];
  const none = named.length > 0 && !named.some((c) => c.modem === modemKind);
  const theirs = [...new Set(named.map((c) => modemLabel(c.modem)))].join(", ");
  $("channel-note").textContent = none
    ? `${gw.callsign} lists no ${modemLabel(modemKind)} channel, only ${theirs}: `
      + "a call from this modem is not answered there."
    : "";
  showTuning();
}

function showKind() {
  const gateway = kind() === "gateway";
  showChannels();
  $("fetch-label").hidden = !gateway;
  $("gateways-open").hidden = !gateway;
  $("station-dial").hidden = gateway;
  $("beacon-split").hidden = gateway;
  if (gateway) closeBeaconMenu();
  $("callsign").placeholder = gateway ? "KK4ODA-1" : "N0CALL";
  showCalledButton();
  showTuning();
  askP2pChance();
  loadHeard();
}

// ── a P2P call's chance ────────────────────────────────────────────────────────────────────────
// Predicted on the dial picked at the site of the gateway listed under the station's base
// callsign (GET /api/p2p-chance), with a band of that gateway's that does better beside it, or,
// with none listed, where the station's own beacon said it is (W59, the P2P design §7). Three
// P2P calls on 40 m at about 95 km, an hour and a half after sunset, were heard by nobody
// (2026-11-23): the band had closed for so short a path, and nothing said so.

let p2pChanceSerial = 0;
let p2pChanceTimer = null;

function askP2pChance() {
  clearTimeout(p2pChanceTimer);
  p2pChanceTimer = setTimeout(showP2pChance, 600);
}

async function showP2pChance() {
  const line = $("p2p-chance");
  const callsign = $("callsign").value.trim().toUpperCase();
  const dial = hzOf($("dial").value);
  const mine = ++p2pChanceSerial;
  if (kind() !== "station" || !callsign || !dial) {
    line.textContent = "";
    return;
  }
  try {
    const r = await api(`p2p-chance?callsign=${encodeURIComponent(callsign)}&dial_hz=${dial}`
      + `&bandwidth=${$("bandwidth").value}`);
    if (mine !== p2pChanceSerial) return;
    const [text, poor] = p2pChanceWords(callsign, r);
    line.textContent = text;
    line.classList.toggle("by-hand", poor);
  } catch {
    if (mine === p2pChanceSerial) line.textContent = "";
  }
}

// the band as people say it: "40 m"
const bandWords = (band) => String(band || "").replace(/^(\d+)m$/, "$1 m");

function p2pChanceWords(callsign, r) {
  const hf = r && r.chosen;
  if (!hf || !hf.words) return ["", false];
  const far = hf.distance_km != null ? `, ${hf.distance_km} km away` : "";
  const band = hf.band ? bandWords(hf.band) : "This dial";
  // at the site of the gateway listed under its callsign, or where its own beacon said it is
  const site = r.station ? "the position its beacon gave" : `${r.gateway}'s site`;
  let text = `${band} to ${callsign}, predicted at ${site}${far}. ${hf.words}.`;
  const b = r.better;
  if (b && b.hf && b.hf.chance_words) {
    text += ` ${bandWords(b.hf.band)}, ${mhzOf(b.dial_hz)} MHz: ${b.hf.chance_words}.`;
  }
  return [text, hf.chance != null && hf.chance < 0.4];
}

// ── the call box remembers each kind ───────────────────────────────────────────────────────────
// Switching between A gateway and P2P brings back what Call held for that kind when the operator
// left it, or else its last call: a gateway with the channel it was called on, a station with the
// dial and bandwidth agreed for it. ▾ lists the gateways, or the stations, called before, newest
// first (GET /api/called, from the call records this computer keeps). Picking one there puts it
// in the box and tunes the radio at once, as a channel picked does; switching kinds tunes nothing.

let calledBefore = {gateway: [], station: []};
let boxKind = null;  // the kind whose call the box holds
const leftAs = {};   // what the box held for each kind when the operator switched away

async function loadCalled() {
  try { calledBefore = await api("called"); } catch { /* the list stays as it was */ }
  showCalledButton();
}

// the calls of a kind the box offers: a gateway the operator blocked is left out
function offered(k) {
  return (calledBefore[k] || []).filter((e) => !(e.gateway && e.gateway.blocked));
}

function showCalledButton() {
  const k = kind();
  const b = $("called-open");
  const heard = k === "station" ? (heardView.stations || []).length : 0;
  b.hidden = !offered(k).length && !heard;
  const recent = k === "station" ? heardView.recent || 0 : 0;
  const count = b.querySelector(".n");
  if (count) count.textContent = recent ? String(recent) : "";
  b.classList.toggle("counted", recent > 0);
  $("callsign").classList.toggle("counted", recent > 0);
  b.title = k === "gateway"
    ? "The gateways you called before, newest first, each on the channel you called it on"
    : `The stations your modem heard${recent ? `, ${recent} in the last half hour` : ""}, `
      + "by where they were heard, and the stations you called before";
  b.setAttribute("aria-label", k === "gateway" ? "Gateways called before" : "Stations heard");
}

// a gateway record's channels the box offers: favourites first, those left out dropped
function channelsOf(gw) {
  return (gw.channels || []).filter((c) => !c.blocked)
    .sort((a, b) => Number(Boolean(b.favourite)) - Number(Boolean(a.favourite)));
}

// a gateway in the box with its channels, those the modem can call on offered, the one at
// `dialHz` picked when it is one of them (of the modem `modem`, when the dial carries two)
function putGateway(callsign, channels, dialHz, modem) {
  const shown = forModem(channels);
  chosenGateway = {callsign, channels, shown};
  $("callsign").value = callsign;
  $("channel").replaceChildren(...shown.map((c, i) =>
    el("option", {value: String(i), text: channelWords(c)})));
  const onDial = shown.map((c, i) => [c, i]).filter(([c]) => c.dial_hz === dialHz);
  const pick = onDial.find(([c]) => c.modem === (modem || modemKind)) || onDial[0];
  if (pick) $("channel").value = String(pick[1]);
  showChannels();
}

// the modem named anew: the gateway in the box offers its channels again, the one picked kept,
// and the callsign typed left as it is
function showModem(m) {
  const kindNow = (m && m.kind) || null;
  if (kindNow === modemKind) return;
  modemKind = kindNow;
  const gw = chosenGateway;
  if (!gw) return;
  const was = chosenChannel();
  const typed = $("callsign").value;
  putGateway(gw.callsign, gw.channels, was && was.dial_hz, was && was.modem);
  $("callsign").value = typed;
  showChannels();
}

// one call made before, in the box: its callsign and its channel. A gateway no longer listed
// keeps the one channel it was called on
function putCalled(k, e) {
  if (k === "gateway") {
    const channels = e.gateway ? channelsOf(e.gateway)
      : e.dial_hz ? [{dial_hz: e.dial_hz, bandwidth_hz: e.bandwidth_hz}] : [];
    putGateway(e.callsign, channels, e.dial_hz);
    return;
  }
  $("callsign").value = e.callsign;
  if (e.dial_hz) $("dial").value = mhzOf(e.dial_hz);
  if ([...$("bandwidth").options].some((o) => Number(o.value) === e.bandwidth_hz)) {
    $("bandwidth").value = String(e.bandwidth_hz);
  }
}

// the box turned to kind `k`: what it held is kept for the kind it leaves, and what `k` held, or
// its last call, comes back. `fill` false leaves the callsign to the caller (a gateway chosen)
function switchKind(k, fill = true) {
  document.querySelector(`input[name=kind][value=${k}]`).checked = true;
  keep("kind", k);
  if (boxKind !== k) {
    if (boxKind) {
      leftAs[boxKind] = {callsign: $("callsign").value, gateway: chosenGateway,
                         channel: chosenChannel()};
    }
    const back = leftAs[k];
    if (!fill) {
      // the caller puts in its own
    } else if (back) {
      if (k === "gateway" && back.gateway) {
        putGateway(back.gateway.callsign, back.gateway.channels,
                   back.channel && back.channel.dial_hz, back.channel && back.channel.modem);
      } else if (k === "gateway") {
        chosenGateway = null;
      }
      $("callsign").value = back.callsign;
    } else if (offered(k).length) {
      putCalled(k, offered(k)[0]);
    } else {
      $("callsign").value = "";
    }
    boxKind = k;
  }
  showKind();
}

// ── the stations heard, in Call (W59, the P2P design §5.2, RP-4 §4.4) ──────────────────────────
// In P2P, ▾ lists the stations the modem heard (GET /api/stations), one line a callsign, grouped
// by where each was heard against the radio's dial, the most recently heard first; then the
// stations called before and not heard lately. ▾ carries how many were heard in the last half
// hour. A line under half an hour old is plain, one up to six hours faded, and older ones wait
// behind Show older. Picking a line puts its callsign, the dial it was heard on and the
// bandwidth it stated in the call box, and tunes the radio there. The list opens over the page:
// the side column scrolls, and would cut it at its edge.

let heardView = {stations: [], recent: 0};
const HEARD_FADED_S = 30 * 60;
const HEARD_HIDDEN_S = 6 * 3600;
const HEARD_REFRESH_MS = 15000;

async function loadHeard() {
  if (kind() !== "station") return;
  const dial = hzOf($("dial").value);
  try {
    heardView = await api(`stations${dial ? `?dial_hz=${dial}` : ""}`);
  } catch { /* the list stays as it was */ }
  showCalledButton();
}

// how long ago, short: "2 min", "3 h", "2 days"
function agoShort(s) {
  if (s < 90) return "now";
  if (s < 3600) return `${Math.round(s / 60)} min`;
  if (s < 2 * 86400) return `${Math.round(s / 3600)} h`;
  return `${Math.round(s / 86400)} days`;
}

const dateShort = (t) => new Date(t * 1000).toLocaleDateString(undefined,
  {month: "short", day: "numeric"});

const HEARD_KINDS = {beacon: () => "beacon", call: (to) => `calling ${to || "a station"}`,
  answer: (to) => `answering ${to || "a call"}`, probe: (to) => `probing ${to || "a station"}`,
  session: () => "in a session with you", datagram: () => "a broadcast", id: () => "identifying"};

// a station heard, as its second line says it
function heardWords(s) {
  const parts = [];
  if (s.where === "band" && s.dial_hz) parts.push(`${mhzOf(s.dial_hz)} MHz`);
  if ((s.where === "other" || s.where === "unknown") && (s.band || s.dial_hz)) {
    parts.push([s.band && bandWords(s.band), s.dial_hz && `${mhzOf(s.dial_hz)} MHz`]
      .filter(Boolean).join(" · "));
  }
  const said = HEARD_KINDS[s.kind];
  if (said) parts.push(said(s.to));
  if (s.kind === "beacon" && s.answers === true) parts.push(["answers", "answers calls"]);
  if (s.kind === "beacon" && s.answers === false) parts.push("not answering calls");
  if (s.bandwidth) parts.push(`${s.bandwidth} Hz`);
  if (s.km != null) parts.push(`${s.km} km, ${String(s.bearing).padStart(3, "0")}°`);
  if (s.worked_at) parts.push(`P2P worked ${dateShort(s.worked_at)}`);
  if (s.other_modem) parts.push(["caution", `heard by ${modemLabel(s.modem)}, not this modem`]);
  return parts;
}

// the stations heard, and those called before not among them, as the list's items: a heading
// is {head}, a line {heard} or {called}
function stationItems(filter, older) {
  const typed = filter.trim().toUpperCase();
  const fits = (c) => !typed || c.startsWith(typed);
  const items = [];
  const heading = {here: `On ${heardView.radio_hz ? mhzOf(heardView.radio_hz) : "?"} MHz, where the radio is`,
    band: `${bandWords(heardView.band || "")}, other dials`, other: "Other bands",
    unknown: "Heard (the modem cannot read the dial)"};
  let group = null;
  let hidden = 0;
  for (const s of heardView.stations || []) {
    if (!fits(s.callsign)) continue;
    if (s.age_s > HEARD_HIDDEN_S && !older && !s.worked_at) {
      hidden += 1;
      continue;
    }
    if (s.where !== group) {
      group = s.where;
      items.push({head: heading[group] || heading.unknown});
    }
    items.push({heard: s});
  }
  const heardCalls = new Set((heardView.stations || []).map((s) => s.callsign));
  const called = offered("station").filter((e) => !heardCalls.has(e.callsign) && fits(e.callsign));
  if (called.length) items.push({head: "Called before, not heard lately"});
  for (const e of called) items.push({called: e});
  if (hidden) items.push({more: hidden});
  const placed = (heardView.stations || []).filter((s) => s.position).length;
  if (placed) items.push({map: placed});
  return items;
}

// ── a station heard: its details, pinning, and the map (W59, the P2P design §5.3, §7, §8) ──────
// A right click on a line of the list, or Shift+F10 on the line picked, opens what the client
// knows of the station: its readings, where else it was heard, where its beacon says it is and how
// far, the P2P sessions with it, and Pin to the top and Call on its dial. Show on the map, the
// list's last line, puts the stations whose beacons gave a position on the world map.

let heardMap = null;

function closeDetails() {
  $("station-details").hidden = true;
}

// where a station's beacon says it is, in words
function placeWords(s) {
  const p = s.position;
  if (!p) return "";
  const where = p.grid || `${p.lat.toFixed(4)}, ${p.lon.toFixed(4)}`.replaceAll("-", "\u2212");
  const far = s.km != null ? `, ${s.km} km at ${String(s.bearing).padStart(3, "0")}° from you` : "";
  const within = p.precision === "grid6" ? " (within about 4 km)"
    : p.precision === "grid4" ? " (within about 100 km)" : "";
  return `At ${where}${within}${far}, from its beacon.`;
}

function showDetails(s, at, take) {
  const box = $("station-details");
  const rows = [...(s.readings || [])].reverse().map(([t, snr]) =>
    el("tr", {}, el("td", {text: clockOf(t)}), el("td", {text: s.dial_hz ? `${mhzOf(s.dial_hz)} MHz` : ""}),
       el("td", {class: "n", text: db(snr)})));
  for (const e of s.elsewhere || []) {
    rows.push(el("tr", {}, el("td", {text: dateShort(e.at)}),
      el("td", {text: [e.band && bandWords(e.band), e.dial_hz && `${mhzOf(e.dial_hz)} MHz`]
        .filter(Boolean).join(" · ")}),
      el("td", {class: "n", text: db(e.snr_db)})));
  }
  const said = HEARD_KINDS[s.kind];
  const lines = [
    `Heard ${s.count || 1} time${s.count === 1 ? "" : "s"}, last ${agoShort(s.age_s)} ago`
      + `${said ? `: ${said(s.to)}` : ""}${s.bandwidth ? `, ${s.bandwidth} Hz` : ""}.`,
    s.answers === true ? "Its last beacon said it answers calls."
      : s.answers === false ? "Its last beacon said it does not answer calls now." : "",
    placeWords(s),
    s.worked_at ? `P2P worked ${dateShort(s.worked_at)}`
      + `${s.worked_heard_db != null ? `; it heard you at ${db(s.worked_heard_db)}` : ""}.` : "",
  ].filter(Boolean);
  const pin = el("button", {type: "button", text: s.pinned ? "Unpin" : "Pin to the top",
    title: s.pinned ? "No longer first in its group" : "First in its group, with a star; Setup's sounds can mark it heard again after three hours unheard",
    onclick: async () => {
      try {
        await api("stations", {pin: s.callsign, on: !s.pinned});
        await loadHeard();
      } catch (e) { toast(e.message, true); }
      closeDetails();
    }});
  const call = el("button", {type: "button", class: "primary",
    text: s.dial_hz ? `Call on ${mhzOf(s.dial_hz)} MHz` : "Put in Call",
    title: "Its callsign, the dial it was heard on and its bandwidth into the call box",
    onclick: () => { closeDetails(); take(); }});
  box.replaceChildren(
    el("h3", {text: `${s.pinned ? "★ " : ""}${s.callsign}`}),
    ...lines.map((t) => el("p", {text: t})),
    rows.length ? el("table", {}, el("tbody", {}, ...rows)) : null,
    el("div", {class: "d-acts"}, call, pin));
  box.hidden = false;
  const left = Math.max(8, Math.min(at.left, window.innerWidth - box.offsetWidth - 8));
  const top = Math.max(8, Math.min(at.top, window.innerHeight - box.offsetHeight - 8));
  Object.assign(box.style, {left: `${left}px`, top: `${top}px`});
  pin.focus();
}

function showHeardMap(pick) {
  const panel = $("heard-map-panel");
  panel.hidden = false;
  if (!heardMap) {
    heardMap = new WorldMap($("heard-map"), {
      onReport: (callsign) => {
        const s = (heardView.stations || []).find((x) => x.callsign === callsign);
        if (s) { panel.hidden = true; pick(s); }
      }});
  }
  const placed = (heardView.stations || []).filter((s) => s.position);
  heardMap.show({here: heardView.here, gateways: []});
  heardMap.showReports(placed.map((s) => ({id: s.callsign, lat: s.position.lat,
    lon: s.position.lon,
    colour: s.age_s <= HEARD_FADED_S ? "var(--ok)" : "var(--muted)",
    title: `${s.callsign}, ${agoShort(s.age_s)} ago${s.snr_db != null ? ` at ${db(s.snr_db)}` : ""}`
      + `${s.km != null ? `, ${s.km} km` : ""}`})));
  heardMap.fit();
  const unplaced = (heardView.stations || []).length - placed.length;
  $("heard-map-note").textContent = unplaced
    ? `${unplaced} station${unplaced === 1 ? "" : "s"} heard gave no position and ${unplaced === 1 ? "is" : "are"} not on it.`
    : "";
  $("heard-map-close").focus();
}

// ▾ beside Call: the calls of the kind shown, or in P2P the stations heard. Arrows move, Enter
// takes one, Escape closes. The list is read again as it opens, so a call just made is in it; in
// P2P its lines change in place while it is open, and nothing moves under the operator
function wireCalled() {
  const input = $("callsign");
  const list = $("called-list");
  const opener = $("called-open");
  let shown = [];
  let active = -1;
  let older = false;
  let refresh = null;
  const pickable = (item) => item && !item.head;
  const close = () => {
    list.hidden = true;
    shown = [];
    active = -1;
    older = false;
    clearInterval(refresh);
    refresh = null;
    opener.setAttribute("aria-expanded", "false");
    input.setAttribute("aria-expanded", "false");
    input.removeAttribute("aria-activedescendant");
    list.classList.remove("heard");
    list.removeAttribute("style");
  };
  const take = (item) => {
    if (item.map) {
      close();
      showHeardMap((s) => take({heard: s}));
      return;
    }
    if (item.more) {
      older = true;
      shown = stationItems(input.value, true);
      active = Math.min(active, shown.length - 1);
      render();
      return;
    }
    close();
    const k = kind();
    if (item.heard) {
      const s = item.heard;
      putCalled("station", {callsign: s.callsign, dial_hz: s.dial_hz,
                            bandwidth_hz: s.bandwidth || null});
    } else {
      putCalled(k, item.called || item);
    }
    if (k === "station") {
      keep("dial", $("dial").value);
      keep("bandwidth", $("bandwidth").value);
    }
    showKind();
    input.focus();
    pickChannel();
  };
  // over the page, as a popover is: the side column scrolls, and would cut the list at its edge
  // the room below Call, or above it when there is more there: the list never runs off the window
  const place = () => {
    if (kind() !== "station") return;
    const r = input.getBoundingClientRect();
    const below = window.innerHeight - r.bottom - 12;
    const above = r.top - 12;
    const down = below >= 220 || below >= above;
    list.classList.add("heard");
    Object.assign(list.style, {position: "fixed", left: `${r.left}px`, right: "auto",
      top: down ? `${r.bottom + 4}px` : "auto",
      bottom: down ? "auto" : `${window.innerHeight - r.top + 4}px`,
      maxHeight: `${Math.max(120, Math.min(416, down ? below : above))}px`,
      width: STACKED.matches ? `${r.width}px` : ""});
  };
  const lineOf = (item, i) => {
    if (item.head) return el("li", {class: "s-head", role: "presentation", text: item.head});
    if (item.more) {
      return el("li", {role: "option", id: `called-${i}`, class: "more",
        text: `Show older (${item.more})`, title: "Stations heard more than six hours ago"});
    }
    if (item.map) {
      return el("li", {role: "option", id: `called-${i}`, class: "more",
        text: `Show on the map (${item.map})`,
        title: "The stations heard whose beacons said where they are, on the world map"});
    }
    if (item.heard) {
      const s = item.heard;
      const parts = heardWords(s);
      const line = el("span", {class: "s-addr"});
      parts.forEach((p, n) => {
        if (n) line.append(" · ");
        if (Array.isArray(p)) line.append(el("span", {class: p[0], text: p[1]}));
        else line.append(p);
      });
      const li = el("li", {role: "option", id: `called-${i}`,
        class: [s.age_s > HEARD_FADED_S ? "stale" : "", s.pinned ? "pinned" : ""]
          .filter(Boolean).join(" "),
        title: `${s.callsign}, heard ${day(s.at)}${s.dial_hz ? ` on ${mhzOf(s.dial_hz)} MHz` : ""}`
          + `${s.snr_db != null ? ` at ${db(s.snr_db)}` : ""}. What this station heard of it: one `
          + "direction only. A beacon's or a call's strength reads low on a strong or fading path. "
          + "A right click, or Shift+F10, shows what is known of it"},
        el("span", {class: "s-top"},
          el("span", {class: "s-name", text: s.callsign}),
          el("span", {class: "s-snr", text: db(s.snr_db)}),
          el("span", {class: "s-age", text: agoShort(s.age_s)})),
        line);
      return li;
    }
    const e = item.called || item;
    const k = kind();
    const bandwidth = e.bandwidth_hz ? `, ${e.bandwidth_hz} Hz` : "";
    const where = e.dial_hz ? `${mhzOf(e.dial_hz)} MHz${bandwidth}`
      : k === "gateway" ? "no channel named" : "no dial named";
    return el("li", {role: "option", id: `called-${i}`, class: item.called ? "stale" : "",
      title: `${e.callsign}, called ${day(e.at)}${e.dial_hz ? ` on ${mhzOf(e.dial_hz)} MHz` : ""}`
        + (k === "gateway" && !e.gateway ? ". It is not in the gateway list now" : "")},
      el("span", {class: "s-name", text: e.callsign}),
      el("span", {class: "s-addr", text: `${where} · ${age(e.at)}`}));
  };
  const render = () => {
    list.replaceChildren(...shown.map((item, i) => {
      const li = lineOf(item, i);
      if (pickable(item)) {
        li.setAttribute("aria-selected", String(i === active));
        li.addEventListener("mousedown", (event) => {
          event.preventDefault();
          if (event.button === 0) take(item);
        });
        if (item.heard) {
          li.addEventListener("contextmenu", (event) => {
            event.preventDefault();
            showDetails(item.heard, {left: event.clientX, top: event.clientY},
                        () => take(item));
          });
        }
        // inside the Call label: a click there is not the label's
        li.addEventListener("click", (event) => event.preventDefault());
      }
      return li;
    }));
    list.hidden = false;
    place();
    opener.setAttribute("aria-expanded", "true");
    input.setAttribute("aria-expanded", "true");
    input.setAttribute("aria-activedescendant", `called-${active}`);
    const item = list.children[active];
    if (item) item.scrollIntoView({block: "nearest"});
  };
  // the next line that can be picked, from `from` in steps of `step`
  const step = (from, by) => {
    for (let n = 1; n <= shown.length; n += 1) {
      const i = (from + by * n + shown.length * n) % shown.length;
      if (pickable(shown[i])) return i;
    }
    return -1;
  };
  // while open, the lines change in place: the order holds, a new station joins the end
  const update = async () => {
    await loadHeard();
    if (list.hidden || kind() !== "station") return;
    const fresh = new Map((heardView.stations || []).map((s) => [s.callsign, s]));
    const there = new Set();
    shown = shown.map((item) => {
      if (!item.heard) return item;
      there.add(item.heard.callsign);
      return {heard: fresh.get(item.heard.callsign) || item.heard};
    });
    const typed = input.value.trim().toUpperCase();
    for (const s of heardView.stations || []) {
      if (!there.has(s.callsign) && (!typed || s.callsign.startsWith(typed))
          && s.age_s <= HEARD_HIDDEN_S) {
        shown.push({heard: s});
      }
    }
    render();
  };
  const open = async (filter = "") => {
    const k = kind();
    if (k === "station") {
      await Promise.all([loadCalled(), loadHeard()]);
      shown = stationItems(filter, older);
    } else {
      await loadCalled();
      shown = offered(k);
    }
    if (!shown.length) { close(); return; }
    const typed = input.value.trim().toUpperCase();
    const at = shown.findIndex((item) => pickable(item)
      && ((item.heard || item.called || item).callsign === typed));
    active = at >= 0 ? at : step(-1, 1);
    render();
    if (k === "station" && !refresh) refresh = setInterval(update, HEARD_REFRESH_MS);
  };
  // the button keeps the focus in Call, so that leaving Call closes the list
  opener.addEventListener("mousedown", (event) => event.preventDefault());
  opener.addEventListener("click", (event) => {
    event.preventDefault();
    input.focus();
    if (list.hidden) open(); else close();
  });
  // typing filters the stations heard; a gateway's list closes
  input.addEventListener("input", () => {
    if (kind() === "station" && input.value.trim()) open(input.value);
    else close();
  });
  // leaving Call closes the list, unless Call has the focus again by then
  input.addEventListener("blur", () => setTimeout(() => {
    if (document.activeElement !== input) close();
  }, 120));
  window.addEventListener("resize", close);
  input.addEventListener("keydown", (event) => {
    if (list.hidden) {
      const any = offered(kind()).length || (kind() === "station" && heardView.stations.length);
      if (event.key === "ArrowDown" && any) { event.preventDefault(); open(); }
      return;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const next = step(active, event.key === "ArrowDown" ? 1 : -1);
      if (next >= 0) active = next;
      render();
    } else if (event.key === "Enter") {
      event.preventDefault();
      if (active >= 0 && pickable(shown[active])) take(shown[active]);
    } else if ((event.key === "F10" && event.shiftKey) || event.key === "ContextMenu") {
      event.preventDefault();
      const item = shown[active];
      const li = list.children[active];
      if (item && item.heard && li) {
        const r = li.getBoundingClientRect();
        showDetails(item.heard, {left: r.right - 40, top: r.top}, () => take(item));
      }
    } else if (event.key === "Escape" || event.key === "Tab") {
      if (event.key === "Escape") event.preventDefault();
      close();
    }
  });
  // the count beside ▾ is kept fresh in P2P
  setInterval(loadHeard, HEARD_REFRESH_MS);
}

// ── the channel picked, tuned at once (W14, RP-4 §4.1) ─────────────────────────────────────────
// Picking a channel (a gateway's, or a station call's dial) tunes the radio there now, so the
// operator can set the radio and the antenna tuner before Call; Call finds the radio there. A
// modem that cannot set the dial says so, with the dial to set by hand. The tuner tone (RP-4 §4.3)
// keys a carrier for ten seconds; a second press stops it.

// the dial in MHz to the kilohertz, or finer when it needs it: 7.101, 3.5915
const mhzOf = (hz) => (hz / 1e6).toFixed(4).replace(/0$/, "");

// a dial typed in MHz, in hertz, read as digits (7.1015 is 7 101 500, no rounding); null if not one
function hzOf(text) {
  const m = /^\s*(\d{1,3})(?:\.(\d{1,6}))?\s*$/.exec(text);
  if (!m) return null;
  const hz = Number(m[1]) * 1e6 + Number((m[2] || "").padEnd(6, "0"));
  return hz > 0 ? hz : null;
}

// the dial of the call box as it stands: the gateway's channel, or the station call's dial
function currentDial() {
  if (kind() === "gateway") {
    const c = chosenChannel();
    return c ? c.dial_hz : null;
  }
  return hzOf($("dial").value);
}

let picked = null;  // the last pick: {kind, dial_hz, tuned, said, tone}
let pickSerial = 0;
let modemTone = true;  // whether the modem keys a tuner tone, as the last pick said

const sentence = (text) => text.charAt(0).toUpperCase() + text.slice(1) + ".";

function showTuning() {
  const dial = currentDial();
  $("tuning").hidden = !dial;
  const p = picked && picked.kind === kind() && picked.dial_hz === dial ? picked : null;
  const at = $("radio-at");
  const how = !p ? "" : {by_hand: "by-hand", error: "bad", there: "ok", moved: "ok"}[p.tuned] || "";
  at.className = `radio-at ${how}`.trim();
  at.textContent = p ? p.said : "";
  $("tone").hidden = !modemTone;
}

async function pickChannel() {
  const dial = currentDial();
  showTuning();
  if (!dial) return;
  const mine = ++pickSerial;
  const here = {kind: kind(), dial_hz: dial};
  picked = {...here, tuned: "asking", said: `Tuning the radio to ${mhzOf(dial)} MHz…`};
  showTuning();
  try {
    const r = await api("tune", {adapter: adapter(), dial_hz: dial});
    if (mine !== pickSerial) return;
    modemTone = r.tone !== false;
    picked = {...here, tuned: r.tuned, said: sentence(r.said), tone: r.tone};
    if (r.tuned === "by_hand") toast(sentence(r.said));
  } catch (e) {
    if (mine !== pickSerial) return;
    picked = {...here, tuned: "error", said: `Not tuned: ${e.message}`};
    toast(e.message, true);
  }
  showTuning();
}

let toneTimer = null;

function toneOff() {
  clearTimeout(toneTimer);
  toneTimer = null;
  delete $("tone").dataset.on;
  $("tone").textContent = "Tuner tone";
}

async function toggleTone() {
  const on = !$("tone").dataset.on;
  try {
    await api("tone", {adapter: adapter(), on});
    if (!on) { toneOff(); return; }
    $("tone").dataset.on = "1";
    $("tone").textContent = "Stop tone";
    toneTimer = setTimeout(toneOff, 10000);
  } catch (e) {
    toneOff();
    toast(e.message, true);
  }
}

// a newer release: said at the top, with the update or the download
function showUpdate(u, name) {
  const box = $("update");
  if (!u || !u.offer) { box.hidden = true; return; }
  box.hidden = false;
  const text = $("update-text");
  text.textContent = u.installing
    ? `Installing ${name} ${u.offer.version}: it restarts by itself.`
    : u.failed
      ? `The update to ${u.offer.version} failed: ${u.failed}`
      : `${name} ${u.offer.version} is out (this is ${u.current}).`;
  text.title = u.offer.notes || "";
  const go = $("update-go");
  go.hidden = u.installing;
  go.textContent = u.can_install ? "Update and restart" : "Download";
  go.title = u.can_install
    ? "Download it, check that the project signed it, install it and restart"
    : `Open ${u.offer.page} in your browser`;
}

// this version, in the header and the Updates card in Setup, and what the channel said when
// last asked
function showVersion(u, name) {
  $("version").textContent = u ? u.current : "";
  const said = $("updates-state");
  if (!u) return;
  const at = u.checked
    ? new Date(u.checked * 1000).toLocaleString([], {dateStyle: "medium", timeStyle: "short"}) : "";
  said.textContent = `This is ${name} ${u.current}. ` + (u.checking ? "Checking…"
    : !u.checked ? "Not checked yet."
    : u.error ? `Checked ${at}, but the update channel could not be reached: ${u.error}`
    : u.offer ? `Checked ${at}: ${u.offer.version} is out, at the top of the window.`
    : `Checked ${at}: this is the newest.`);
  $("updates-check").disabled = Boolean(u.checking);
}

// what the station's state says to the ear: a session up or ended, a call failed, a message
// delivered, mail arrived (checked by its senders' levels)
let lastCounts = null;
function soundsOfState(s, a) {
  const was = lastCounts;
  lastCounts = {inbox: s.counts.inbox, sent: s.counts.sent};
  if (a.state === "session" && lastState !== "session") play("session-up");
  // a session answered ends with the station listening again, Listen still on
  // a session the operator ended is not a failure: it ends as any other does
  if (["idle", "listening"].includes(a.state) && lastState === "session") {
    play(a.failed && !a.stopped ? "failed" : "session-down");
  }
  if (a.state === "idle" && lastState === "calling" && a.failed && !a.stopped) play("failed");
  if (!was) return;
  if (s.counts.sent > was.sent) play("delivered");
  if (s.counts.inbox !== was.inbox) {
    const fromSession = ["session", "calling", "listening"].includes(a.state)
      || ["session", "calling", "listening"].includes(lastState);
    checkMail(fromSession).catch(() => {});
  }
}

async function refresh() {
  const s = await api("state");
  showModem(s.modem);
  showUpdate(s.update, "RadioPost");
  showVersion(s.update, "RadioPost");
  myVersion = (s.update && s.update.current) || "";
  $("who").textContent = s.me ? `${s.me.address}  ·  ${s.me.operator}` : "No credential yet";
  meAddress = s.me ? s.me.address : "";
  showCallsigns(s);
  for (const name of ["inbox", "outbox", "sent", "waiting", "trash"]) {
    $(`n-${name}`).textContent = s.counts[name] || "";
  }
  $("reports").textContent = s.counts.reports
    ? `${s.counts.reports} session${s.counts.reports > 1 ? "s" : ""} to report to the post office`
    : "";
  $("report").hidden = !s.counts.reports;
  $("po-state").textContent = s.post_office
    ? `Reached at ${s.post_office}.`
    : "Found by its domain's DNS records.";
  if (!$("po-url").matches(":focus")) $("po-url").value = s.post_office || "";
  const a = s.activity;
  const label = {idle: "Idle", calling: `Calling ${a.remote || ""}`,
                 listening: a.held ? "Waiting for the modem" : "Listening",
                 session: `In session with ${a.remote || ""}`,
                 renewing: "Renewing credential", alerting: "Sending an alert",
                 beaconing: "Sending a beacon"}[a.state];
  const stopping = a.state === "calling" ? "Stopping the call"
    : a.stopping === "abort" ? `Aborting the session with ${a.remote || ""}`
    : `Disconnecting from ${a.remote || ""}`;
  const state = $("state");
  // a call or session the operator ended reads stopped, not failed
  const ended = a.state === "idle" && a.failed;
  state.dataset.state = ended ? (a.stopped ? "stopped" : "failed") : a.state;
  state.textContent = ended ? (a.stopped ? "Stopped" : "Last session failed")
    : a.stopping && ["calling", "session"].includes(a.state) ? stopping : label;
  state.title = a.held || a.failed || "What the station is doing";
  const busy = a.state !== "idle";
  document.body.dataset.busy = String(busy);
  // messages are written, edited and deleted between sessions, listening for P2P too; a call, a
  // session, a renewal or an alert holds them until it ends
  const held = !["idle", "listening"].includes(a.state);
  document.body.dataset.home = held ? "held" : "free";
  $("call").disabled = busy || !s.me;
  $("call").title = a.listen
    ? "Listening for P2P: press Listening for P2P to stop it, then call"
    : CALL_TITLE;
  showListen(s, a, busy);
  showBeacon(s, a);
  heardAgain(s.heard_again);
  $("write").disabled = held || !s.me;
  $("device").textContent = s.device || "No key yet.";
  myDevice = s.device ? hexToB64u(s.device) : "";
  $("make-key").hidden = Boolean(s.device);
  showEnrollment(s.me);
  showApplication(s.application);
  showTransfer(a, s.progress);
  $("cred-state").textContent = s.me
    ? `${s.me.address} (${s.me.operator}), valid until ${new Date(s.me.expires * 1000).toLocaleDateString()}`
    : "No credential yet.";
  $("trusted").textContent = s.trusted.length ? s.trusted.join(", ") : "None yet.";
  if (s.log !== logNext) await readLog();
  soundsOfState(s, a);
  if (a.state === "idle" && ["calling", "session"].includes(lastState)) loadCalled();
  // a session that ends shows what it brought, whether the station is idle or listening again; a
  // renewal or an alert changes no folder, and must not close a message being written
  if ((lastState !== "idle" && lastState !== "renewing" && lastState !== "alerting"
       && lastState !== "beaconing" && a.state === "idle")
      || (lastState === "session" && a.state === "listening")) await openFolder(folder);
  lastState = a.state;
  if (s.alerts) await syncAlerts(s.alerts);
  return s;
}

const CALL_TITLE = "Call the station on the channel picked and carry a session. "
  + "The radio is tuned there first if it is not there already";

// Listen for P2P is a push-on, push-off switch (the 40 m sessions of 2026-11-18, when a second
// call found the station no longer listening). Pressed, it stays in, its lamp lit, and reads
// Listening for P2P: the station answers P2P calls, goes back to listening after each session and
// when the modem comes back, until it is pressed again. Its lamp is amber while the modem is away.
// Released during a session, the session goes on and listening stops when it ends.
function showListen(s, a, busy) {
  const b = $("listen");
  const on = Boolean(a.listen);
  b.setAttribute("aria-pressed", String(on));
  b.toggleAttribute("data-held", on && Boolean(a.held));
  $("listen-label").textContent = on ? "Listening for P2P" : "Listen for P2P";
  // on, it stays pressable, so that a second press stops listening even during a session
  b.disabled = !s.me || (busy && !on);
  b.title = listenTitle(s, a);
  const [text, warn] = listenAt(s, a);
  $("listen-at").textContent = text;
  $("listen-at").classList.toggle("by-hand", warn);
}

// A favourite heard again after three hours unheard sounds once, if Setup gives it a sound (W51);
// none by default
let heardAgainAt = null;
function heardAgain(h) {
  if (!h) return;
  if (heardAgainAt !== null && h.at !== heardAgainAt) play("favourite");
  heardAgainAt = h.at;
}

// ── Beacon (W59, the P2P design §5.1, §6, §7) ─────────────────────────────────────────────────
// A split button beside Call in P2P. Its left part is a latch, as Listen for P2P is: pressed in,
// the beacon repeats, its lamp green, until it is pressed again or four hours pass. ▾, or a right
// click on either part, opens its menu: Send a beacon now, Repeat, Only while listening, and how
// much of where the station is the beacon says. The lamp is amber while a beacon waits for the
// channel. The line under Listen says what the beacon does, or what the last one did.

let beaconView = null;

// "15:21"
function clockOf(t) {
  return new Date(t * 1000).toLocaleTimeString([], {hour: "2-digit", minute: "2-digit"});
}

// What the last beacon did, in a line: an hour after, it is no longer news
function beaconDid(b) {
  const l = b && b.last;
  if (!l || Date.now() / 1000 - l.at > 3600) return "";
  if (l.sent) {
    return `Beacon sent at ${clockOf(l.at)}${l.dial_hz ? ` on ${mhzOf(l.dial_hz)} MHz` : ""}`;
  }
  return `Beacon not sent at ${clockOf(l.at)}: ${l.text || "no reason given"}`;
}

// What the next beacon says of where the station is
function beaconSays(b) {
  const p = b.position || {};
  if (b.settings.position === "none") return "Nothing of where you are.";
  if (p.goes && p.goes.precision === "exact") {
    const deg = (v) => v.toFixed(5).replace("-", "\u2212");
    return `${deg(p.goes.lat)}, ${deg(p.goes.lon)}, from ${p.from}.`;
  }
  if (p.goes) return `${p.grid}, from ${p.from}.`;
  return "No position in use (Setup, Places): the modem puts in its own, if it has one, as "
    + "Aether's Setup grid square.";
}

// When the next repeating beacon falls due, in words: "about 15:38, in 14 min"
function beaconNext(b, a) {
  const r = b.repeating;
  if (!r) return "";
  const at = a.listening_at;
  if (at && at.beaconing) {
    return `Aether repeats its own beacon every ${Math.round(at.beaconing / 60)} min: `
      + "RadioPost sends none of its own";
  }
  if (b.settings.only_while_listening && !(a.listen && a.state === "listening")) {
    return `Every ${b.settings.every_min} min while you listen for P2P`;
  }
  const left = Math.max(0, Math.round((r.next - Date.now() / 1000) / 60));
  return `Next: about ${clockOf(r.next)}${left ? `, in ${left} min` : ""}`;
}

// The line under Listen: what the beacon does, or what the last one did
function beaconLine(b, a) {
  if (b.waiting || a.state === "beaconing") return "Beacon waiting for a clear channel";
  const did = beaconDid(b);
  if (b.repeating) {
    const parts = [`Beacon every ${b.settings.every_min} min`];
    const next = beaconNext(b, a);
    if (next.startsWith("Next: ")) parts.push(`next ${next.slice(6)}`);
    else if (next) return next;
    if (did && b.last.sent) parts.push(`last ${clockOf(b.last.at)}`);
    else if (did) parts.push(did.replace(/^Beacon /, "last "));
    return parts.join(" · ");
  }
  if (b.ended) return `Beacon stopped repeating ${b.ended}: press Beacon to start again`;
  return did;
}

function showBeacon(s, a) {
  const b = s.beacon;
  if (!b) return;
  beaconView = b;
  const latch = $("beacon");
  const waiting = b.waiting || a.state === "beaconing";
  latch.setAttribute("aria-pressed", String(Boolean(b.repeating)));
  latch.dataset.state = waiting ? "waiting" : "";
  latch.disabled = !s.me;
  latch.title = b.repeating
    ? `On: a beacon every ${b.settings.every_min} min until ${clockOf(b.repeating.until)}, `
      + "four hours from the start. Press again to stop"
    : `Repeat your beacon every ${b.settings.every_min} min for four hours at most, until you `
      + "press it again. A right click, or the arrow beside it, opens its menu";
  $("beacon-more").disabled = !s.me;
  $("beacon-at").textContent = beaconLine(b, a);
  fillBeaconMenu(s, a, b);
}

function fillBeaconMenu(s, a, b) {
  const now = $("beacon-now");
  const free = ["idle", "listening"].includes(a.state);
  now.disabled = !s.me || b.waiting || Boolean(b.again_in_s) || !free;
  now.title = b.waiting ? "A beacon is waiting for a clear channel"
    : b.again_in_s ? `Another may go in ${b.again_in_s} s: a minute after the last`
    : !free ? "The station is busy: a beacon goes when it is idle or listening"
    : "One beacon now, on the channel picked or where the radio is. It waits up to two minutes "
      + "for a clear channel";
  for (const button of $("beacon-position").querySelectorAll("button")) {
    button.setAttribute("aria-pressed", String(button.dataset.precision === b.settings.position));
  }
  const every = b.repeating ? String(b.settings.every_min) : "0";
  for (const button of $("beacon-repeat").querySelectorAll("button")) {
    button.setAttribute("aria-pressed", String(button.dataset.every === every));
  }
  $("beacon-listening").setAttribute("aria-checked", String(b.settings.only_while_listening));
  $("beacon-position-says").textContent = beaconSays(b);
  $("beacon-next").textContent = beaconNext(b, a);
  $("beacon-last").textContent = beaconDid(b) ? `Last: ${beaconDid(b).replace(/^Beacon /, "")}` : "";
  $("beacon-until").textContent = b.repeating
    ? `Repeats until ${clockOf(b.repeating.until)}, four hours from the start` : "";
}

// Beacon's settings, changed: what the page holds, with `change` over it
async function setBeacon(change) {
  if (!beaconView) return;
  try {
    await api("beacon", {settings: {...beaconView.settings, ...change}});
  } catch (e) {
    toast(e.message, true);
  }
}

function openBeaconMenu() {
  const menu = $("beacon-menu");
  const r = $("beacon-split").getBoundingClientRect();
  menu.hidden = false;
  // under the button, kept on the screen: the side column scrolls and would cut it
  const left = Math.max(8, Math.min(r.left, window.innerWidth - menu.offsetWidth - 8));
  const below = r.bottom + 4 + menu.offsetHeight <= window.innerHeight;
  Object.assign(menu.style, {left: `${left}px`,
    top: below ? `${r.bottom + 4}px` : `${Math.max(8, r.top - 4 - menu.offsetHeight)}px`});
  for (const id of ["beacon", "beacon-more"]) $(id).setAttribute("aria-expanded", "true");
  $("beacon-now").focus();
}

function closeBeaconMenu() {
  $("beacon-menu").hidden = true;
  for (const id of ["beacon", "beacon-more"]) $(id).setAttribute("aria-expanded", "false");
}

function wireBeacon() {
  // the latch: pressed in, the beacon repeats; pressed again, it stops
  $("beacon").addEventListener("click", async () => {
    const on = $("beacon").getAttribute("aria-pressed") !== "true";
    await act("beacon", {repeat: on});
  });
  $("beacon-more").addEventListener("click", (e) => {
    e.preventDefault();
    if ($("beacon-menu").hidden) openBeaconMenu();
    else closeBeaconMenu();
  });
  for (const id of ["beacon", "beacon-more"]) {
    $(id).addEventListener("contextmenu", (e) => {
      e.preventDefault();
      openBeaconMenu();
    });
  }
  // an interval chosen starts repeating at it; Off stops it
  for (const button of $("beacon-repeat").querySelectorAll("button")) {
    button.addEventListener("click", async () => {
      const every = Number(button.dataset.every);
      if (every) {
        await setBeacon({every_min: every});
        await act("beacon", {repeat: true});
      } else {
        await act("beacon", {repeat: false});
      }
    });
  }
  $("beacon-listening").addEventListener("click", async () => {
    await setBeacon({only_while_listening: !beaconView.settings.only_while_listening});
    await refresh().catch(() => {});
  });
  $("beacon-now").addEventListener("click", async () => {
    closeBeaconMenu();
    try {
      await api("beacon", {send: true, adapter: adapter()});
      toast("Beacon asked for: it goes when the channel is clear.");
    } catch (e) {
      toast(e.message, true);
    }
    await refresh().catch(() => {});
  });
  for (const button of $("beacon-position").querySelectorAll("button")) {
    button.addEventListener("click", async () => {
      await setBeacon({position: button.dataset.precision});
      await refresh().catch(() => {});
    });
  }
  document.addEventListener("click", (e) => {
    if (!$("beacon-menu").hidden && !$("beacon-split").contains(e.target)) closeBeaconMenu();
    if (!$("station-details").hidden && !$("station-details").contains(e.target)) closeDetails();
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && !$("beacon-menu").hidden) {
      closeBeaconMenu();
      $("beacon-more").focus();
    }
    if (e.key === "Escape" && !$("station-details").hidden) {
      closeDetails();
      $("callsign").focus();
    }
    if (e.key === "Escape" && !$("heard-map-panel").hidden) $("heard-map-panel").hidden = true;
  });
  $("heard-map-close").addEventListener("click", () => { $("heard-map-panel").hidden = true; });
  window.addEventListener("resize", closeBeaconMenu);
}

// Where and how the station answers P2P calls while it listens, as the modem says: the dial, or
// the channels a gateway on the station scans, and the bandwidths a call is answered in. A
// profile had left the home station at 500 Hz while it listened, and a 2300 Hz call to it went
// unanswered with nothing here to say so (2026-11-23). Warned when a 2300 Hz call would not be
// answered.
function listenAt(s, a) {
  const at = a.listen && !a.held ? a.listening_at : null;
  if (!at) return ["", false];
  let where = "";
  if (at.scanning && at.scanning.length) {
    where = ` on the channels the station scans: ${listOf(at.scanning.map(mhzOf))} MHz`;
  } else if (at.dial_hz) {
    where = ` on ${mhzOf(at.dial_hz)} MHz`;
  }
  const answers = at.answers || [];
  const widest = answers.length ? Math.max(...answers) : 0;
  let how = "";
  if (answers.length > 1) {
    const others = answers.filter((hz) => hz !== widest);
    how = `, ${widest} Hz (answers ${listOf(others.map(String))} Hz calls too)`;
  } else if (widest) {
    how = `, ${widest} Hz calls only`;
  }
  const warn = answers.length === 1 && widest < 2300;
  return [`Listening as ${myNames(s)}${where}${how}`, warn];
}

// "a", "a and b", "a, b and c"
function listOf(items) {
  return items.length > 1 ? `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`
    : items[0] || "";
}

// Every callsign of this computer, the one chosen first (W46), as a listener answers to them
function myNames(s) {
  const mine = s.me ? [s.me.operator] : [];
  for (const c of s.callsigns || []) {
    if (c.operator && !mine.includes(c.operator)) mine.push(c.operator);
  }
  return mine.length > 1
    ? `${mine.slice(0, -1).join(", ")} or ${mine[mine.length - 1]}`
    : mine[0] || "your callsign";
}

// Listen for P2P's tooltip: the callsigns it answers as (every callsign of this computer, the one
// chosen first, W46), that it holds until pressed again, and that a gateway on the station is not
// this client's to answer
function listenTitle(s, a) {
  const names = myNames(s);
  if (a.listen && a.held) {
    return `On: waiting for the modem (${a.held}); tried again every 5 s. `
      + "Press again to stop listening";
  }
  if (a.listen && a.state === "session") {
    return `On: in session with ${a.remote || "a station"}, then listening again for P2P calls `
      + `to ${names}. Press again to stop listening when this session ends`;
  }
  if (a.listen) {
    return `On: answering P2P calls to ${names}, and listening again after each session. `
      + "Press again to stop listening";
  }
  if (a.state === "session" || a.state === "listening") {
    return "Off: listening stops when this session ends";
  }
  return `Answer P2P calls to ${names} until you press it again. `
    + "A gateway on this station keeps answering calls to its own callsign";
}

// ── the callsigns this computer works (wishlist W46, W7) ───────────────────────────────────────
// Each is a credential of its own with its own account's mail. The list at the top chooses the one
// the panel works as, between sessions; with one callsign it does not show. Setup adds and removes.

let callsignsShown = "";
let chosenCallsign = 1;
let lastCallsigns = [];

function callsignLabel(c) {
  if (c.operator) return `${c.operator}  ·  ${c.address}`;
  if (c.application) return `${c.application.operator}  ·  applied for`;
  return "New callsign, not yet enrolled";
}

function showCallsigns(s) {
  const list = s.callsigns || [];
  lastCallsigns = list;
  const chosen = list.find((c) => c.chosen) || {id: 1};
  chosenCallsign = chosen.id;
  const pick = $("who-pick");
  const several = list.length > 1;
  pick.hidden = !several;
  $("who").hidden = several;
  const shown = JSON.stringify(list.map((c) => [c.id, callsignLabel(c)]));
  if (several && shown !== callsignsShown && document.activeElement !== pick) {
    callsignsShown = shown;
    pick.replaceChildren(...list.map((c) => el("option", {value: String(c.id)}, callsignLabel(c))));
  }
  if (document.activeElement !== pick) pick.value = String(chosen.id);
  pick.disabled = s.activity.state !== "idle";
  $("callsign-add").hidden = !s.me;
  $("callsign-remove").hidden = chosen.id === 1;
  $("callsign-tools").hidden = $("callsign-add").hidden && $("callsign-remove").hidden;
}

// what the page held of the callsign before: its application's state, the desk, a half-filled form
function forgetCallsign() {
  application.status = "";
  application.checked = 0;
  office.ok = false;
  $("office-open").hidden = true;
  enrollOpen = false;
  $("enroll").reset();
}

async function changeCallsign(change) {
  const writing = !$("composer").hidden || composersOpen();
  try {
    await api("callsigns", change);
  } catch (e) {
    toast(e.message, true);
    await refresh();
    return null;
  }
  forgetCallsign();
  // a message being edited is in the other callsign's outbox: it stays there as it was
  const editing = answering?.how === "edit";
  if (editing) newComposer();
  const editsClosed = closeEdits();
  const s = await refresh();
  if (s.me) probeOffice();
  if (change.add || !s.me) {
    show("setup");
    $("enroll").address.focus();
  } else if (editing || editsClosed) {
    toast("The edit was closed: the message stays as it was in the other callsign's outbox.");
    await openFolder(folder);
  } else if (writing) {
    toast(`The message being written goes from ${s.me.address} now.`);
  } else if ($("setup").hidden && $("addresses-view").hidden) {
    await openFolder(folder);
  }
  return s;
}

// ── what a session is carrying ──────────────────────────────────────────────────────────────
// Two rings around an envelope: the outer one is what goes out, the inner one what comes in.
// The envelope bobs up while sending and down while receiving, the little waves above it pulse
// while the session runs, and a tick pops in when everything has crossed. Sent bytes count once
// the modem has passed them on, not when they are handed to it (app/client/src/session.rs).
// Colour is state only, as in Aether's panel: while it calls or checks the mail a segment spins
// in the colour of the state at the top (amber calling, blue in session); while mail moves the
// outer ring is the colour of sending and the inner of receiving, the way not moving now faded,
// so the TX colour is bright only while sending; once it ends both rings say how (green all
// carried, amber some, red failed).

const RINGS = {out: 27, in: 19};

function ringsSvg(withMail) {
  const ring = (which) => {
    const r = RINGS[which];
    return `<circle class="track" cx="32" cy="32" r="${r}"/>`
      + `<circle class="arc ${which}" cx="32" cy="32" r="${r}" transform="rotate(-90 32 32)"`
      + ` stroke-dasharray="${(2 * Math.PI * r).toFixed(2)}" stroke-dashoffset="${(2 * Math.PI * r).toFixed(2)}"/>`;
  };
  const mail = withMail
    ? `<g class="mail">
         <g class="waves"><path d="M28.6 23.6q3.4-2.6 6.8 0"/><path d="M26.4 20.6q5.6-4.4 11.2 0"/></g>
         <g class="envelope"><rect x="23.5" y="26.5" width="17" height="12" rx="2.4"/><path d="M24.4 27.8 32 33.4l7.6-5.6"/></g>
         <path class="check" d="M25.2 32.6l4.4 4.4 9.2-9.6"/>
       </g>`
    : "";
  return `<svg class="rings" viewBox="0 0 64 64" aria-hidden="true"><g class="spin">${ring("out")}${ring("in")}</g>${mail}</svg>`;
}

function setRing(box, which, frac) {
  const arc = box.querySelector(`.arc.${which}`);
  const len = 2 * Math.PI * RINGS[which];
  arc.setAttribute("stroke-dashoffset", (len * (1 - Math.max(0, Math.min(1, frac)))).toFixed(2));
}

function kb(n) {
  return n < 1000 ? `${n} B` : `${(n / 1000).toFixed(n < 10000 ? 1 : 0)} kB`;
}

function crossedLine(way, verb) {
  if (!way || !way.messages) return "";
  const of = way.bytes < 1000 ? `${way.crossed}/${way.bytes} B`
    : `${(way.crossed / 1000).toFixed(way.bytes < 10000 ? 1 : 0)}/${kb(way.bytes)}`;
  return `${verb} ${way.whole} of ${way.messages} · ${of}`;
}

function timeLeft(seconds) {
  if (!Number.isFinite(seconds) || seconds <= 0) return "";
  if (seconds < 45) return "Under a minute left";
  if (seconds < 90) return "About a minute left";
  if (seconds < 3600) return `About ${Math.round(seconds / 60)} minutes left`;
  return `About ${(seconds / 3600).toFixed(1)} hours left`;
}

const transfer = {samples: [], shownUntil: 0, last: null, wasSession: false};

// the session card's button: Stop calling while the call goes out, Disconnect in a session, then
// Abort once Disconnect has been asked and the modem is still sending what it holds
function showHangUp(a) {
  const b = $("hang-up");
  const calling = a.state === "calling";
  b.hidden = !calling && a.state !== "session";
  if (b.hidden) return;
  b.classList.toggle("danger", !calling && Boolean(a.stopping));
  b.disabled = a.stopping === "abort" || (calling && Boolean(a.stopping));
  if (calling) {
    b.textContent = "Stop calling";
    b.dataset.how = "abort";
    b.title = "Stop the call now. Nothing has been sent; your Outbox is as it was";
  } else if (!a.stopping) {
    b.textContent = "Disconnect";
    b.dataset.how = "disconnect";
    b.title = "End the session in order: what the modem holds goes out first, then the link "
      + "ends. What the other station did not get whole stays in your Outbox for your next "
      + "call, and a message you received in part goes on from where it stopped";
  } else {
    b.textContent = a.stopping === "abort" ? "Aborting…" : "Abort";
    b.dataset.how = "abort";
    b.title = "End the link now, without waiting for the modem to send what it holds. What did "
      + "not cross is kept as with Disconnect";
  }
}

function showTransfer(a, p) {
  const box = $("transfer");
  const mini = $("mini");
  if (!box.firstElementChild.firstElementChild) {
    $("transfer-rings").innerHTML = ringsSvg(true);
    $("mini-rings").innerHTML = ringsSvg(false);
  }
  const now = Date.now();
  const inSession = a.state === "session";
  if (inSession && !transfer.wasSession) transfer.samples = [];
  if (!inSession && transfer.wasSession) transfer.shownUntil = now + 8000;
  transfer.wasSession = inSession;
  if (inSession) transfer.last = p;
  const ending = !inSession && now < transfer.shownUntil && transfer.last;
  const visible = a.state === "calling" || inSession || ending;
  box.hidden = mini.hidden = !visible;
  if (!visible) return;
  const q = inSession || ending ? (inSession ? p : transfer.last) : null;
  const out = q ? q.sending : null;
  const inc = q ? q.receiving : null;
  const total = q ? out.bytes + inc.bytes : 0;
  const crossed = q ? out.crossed + inc.crossed : 0;
  let mode = "carrying";
  let head = "";
  // how it ended: all of it carried (or nothing to carry), some of it, or failed
  let end = "";
  if (a.state === "calling") {
    mode = "calling";
    head = a.stopping ? "Stopping the call…" : "Calling…";
  } else if (ending) {
    mode = "done";
    const all = total > 0 && out.whole === out.messages && inc.whole === inc.messages && !a.failed;
    // stopped by the operator: amber, what crossed kept, not red
    end = a.failed ? (a.stopped ? "part" : "failed") : all || total === 0 ? "all" : "part";
    head = a.failed ? `Stopped at ${total ? Math.floor(100 * crossed / total) : 0}%`
      : total === 0 ? "Nothing to carry" : all ? "All carried" : "Carried what it could";
  } else if (total === 0) {
    mode = "waiting";
    head = "Checking the mail…";
  } else {
    head = `${Math.floor(100 * crossed / total)}% carried`;
  }
  if (inSession && a.stopping) head = a.stopping === "abort" ? "Aborting…" : "Disconnecting…";
  showHangUp(a);
  const dir = !q || mode !== "carrying" ? ""
    : q.receiving_now ? "down" : out.crossed < out.bytes ? "up" : inc.crossed < inc.bytes ? "down" : "";
  for (const target of [box, mini]) {
    target.dataset.mode = mode;
    target.dataset.dir = dir;
    target.dataset.end = end;
    if (mode === "calling" || mode === "waiting") {
      setRing(target, "out", 0.22);
      setRing(target, "in", 0);
    } else {
      setRing(target, "out", out.bytes ? out.crossed / out.bytes : out.messages ? 0 : 1);
      setRing(target, "in", inc.bytes ? inc.crossed / inc.bytes : inc.messages ? 0 : 1);
    }
  }
  $("t-head").textContent = head;
  $("t-out").textContent = crossedLine(out, "Sending");
  $("t-in").textContent = crossedLine(inc, "Receiving");
  $("t-out").hidden = !$("t-out").textContent;
  $("t-in").hidden = !$("t-in").textContent;
  // the time left: from what crossed in the last minute and a half, else from the modem's rate
  let left = "";
  if (mode === "carrying") {
    transfer.samples.push({t: now, done: crossed});
    transfer.samples = transfer.samples.filter((s) => now - s.t <= 90000);
    const first = transfer.samples[0];
    const span = (now - first.t) / 1000;
    const rate = span >= 10 && crossed > first.done ? (crossed - first.done) / span
      : q.rate_bps ? q.rate_bps / 8 / 1.3 : 0;
    left = rate ? timeLeft((total - crossed) / rate) : "";
  }
  $("t-left").textContent = left;
  $("t-left").hidden = !left;
  $("mini-pct").textContent = mode === "carrying" || (mode === "done" && total)
    ? `${Math.floor(100 * crossed / Math.max(total, 1))}%` : "";
  mini.title = [head, $("t-out").textContent, $("t-in").textContent, left].filter(Boolean).join("\n");
}

async function readLog() {
  const {lines, next} = await api(`log?since=${logNext}`);
  logNext = next;
  if (!lines.length) return;
  const log = $("log");
  log.textContent += lines.map((l) => `${new Date().toLocaleTimeString()}  ${l}\n`).join("");
  log.scrollTop = log.scrollHeight;
}

// A list's paperclip: a message that carries files, faded when its files are all held at the
// post office (RP-6 §2.5); its tooltip says how many and how large. `a` is the panel's
// `attachments` ({carried, bytes, held, held_bytes}); the Waiting list knows only `held`.
function clipCell(a) {
  const td = el("td", {class: "clip"});
  if (!a || !(a.carried || a.held)) return td;
  const files = (n) => `${n} file${n === 1 ? "" : "s"}`;
  const carried = a.carried ? `It carries ${files(a.carried)}, ${kb(a.bytes)}` : "";
  const ask = `open it to ask for ${a.held === 1 ? "it" : "them"}`;
  const words = a.held === true
    ? "Its files are held at the post office: they do not come with it. Once it has come, "
      + `${ask}.`
    : !a.held ? `${carried}.`
    : `${carried ? `${carried}, and has ${a.held} more` : `It has ${files(a.held)}`} held at the `
      + `post office, ${kb(a.held_bytes)}: ${ask}.`;
  const mark = el("span", {class: a.carried ? "clip-mark" : "clip-mark held", role: "img",
    title: words, "aria-label": words});
  mark.append(icon("clip"));
  td.append(mark);
  return td;
}

function markFolder(name) {
  for (const b of document.querySelectorAll("[data-folder]")) {
    b.setAttribute("aria-current", String(b.dataset.folder === name));
  }
}

async function openWaiting() {
  folder = "waiting";
  markFolder("waiting");
  const w = await api("waiting");
  $("waiting-note").textContent = w.via
    ? `What waited at your post office at the call to ${w.via}, ${new Date(w.at * 1000).toLocaleString()}. `
      + "Tick what to fetch on the next call to a gateway."
    : "No call to a gateway yet: what waits for you shows here after one.";
  $("waiting-rows").replaceChildren(...w.entries.map((e) => {
    const tr = document.createElement("tr");
    tr.className = ["", "priority", "emergency"][e.priority] || "";
    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = e.fetch;
    box.title = "Fetch it on the next call to a gateway";
    // a local message, handed over by a gateway cut off from the Internet, has no serial: it comes
    // with what you choose to fetch (RP-7 §4.4)
    if (e.local) {
      box.disabled = true;
      box.title = "Handed over by the gateway while it was cut off: it comes with what you fetch";
    }
    box.addEventListener("change", async () => {
      try {
        await api("fetch", {serial: e.serial, on: box.checked});
      } catch (err) {
        box.checked = !box.checked;
        toast(err.message, true);
      }
    });
    const cells = [box, e.from, e.subject, `${(e.size / 1000).toFixed(1)} kB`];
    cells.forEach((c, i) => {
      const td = document.createElement("td");
      if (typeof c === "string") td.textContent = c; else td.append(c);
      if (i === 3) td.className = "n";
      tr.append(td);
    });
    // the listing says only whether its files are held (RP-3 §5.6), not whether it carries any
    tr.children[3].before(clipCell(e.held ? {carried: 0, held: true} : null));
    tr.title = `${e.local ? "kept for you at the gateway" : `#${e.serial}`}${e.third_party ? " · third-party traffic" : ""}`;
    return tr;
  }));
  show("waiting-view");
}

// -- the Gateways page (choosing a gateway, §16.1): its views, and the marks laid over the lists --

let gatewaysData = null;
let gwView = kept("gview", "recommended");
let gwAt = null;  // the time the gateways are judged for (§14.7), seconds since 1970; null, now
let showBlocked = false;

async function openGateways(refreshing = false) {
  markFolder("");
  try {
    whenOptions();
    gatewaysData = refreshing ? await api(gatewaysPath(), {}) : await api(gatewaysPath());
    show("gateways-view");
    await renderGatewaysPage();
  } catch (e) {
    toast(e.message, true);
  }
}

// a channel as the page writes it: the dial in MHz to the kilohertz or finer, its modem, the
// bandwidth (an FM channel's width in kHz), and whether the gateway scans it with others (RP-5
// §4.4)
function channelWords(c) {
  const modem = c.modem ? `, ${modemLabel(c.modem)}` : "";
  const width = c.bandwidth_hz == null ? ""
    : c.mode === "FM" ? `, FM, ${c.bandwidth_hz / 1000} kHz` : `, ${c.bandwidth_hz} Hz`;
  return `${mhzOf(c.dial_hz)} MHz${modem}${width}${c.scan_s != null ? ", scanned" : ""}`;
}

// days or hours since a moment, for the ages the page shows
function age(seconds) {
  const s = Math.max(0, Date.now() / 1000 - seconds);
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 2 * 86400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86400)} days ago`;
}

// what the view says of HF predictions, once: what they are, or why there are none
function hfNote(hf) {
  if (!hf) return "";
  if (!hf.engine) return ` HF channels are not predicted: ${hf.why}.`;
  return " Each HF channel shows VOACAP's prediction for the hour, both ways"
    + (hf.heard ? ", and what hf-predict heard here in the last hour." : ".");
}

// -- another time (§§14.7, 16.7): the gateways judged for a chosen hour --

function atQuery() {
  return gwAt == null ? "" : `?at=${gwAt}`;
}

function gatewaysPath() {
  return `gateways${atQuery()}`;
}

// the next time the local clock reads `hour`:00, seconds since 1970
function nextLocal(hour) {
  const d = new Date();
  d.setMinutes(0, 0, 0);
  if (d.getHours() >= hour) d.setDate(d.getDate() + 1);
  d.setHours(hour);
  return Math.floor(d.getTime() / 1000);
}

// the choices: now, the next evening and morning, or a date and hour; the one in use kept
function whenOptions() {
  const today = new Date().toDateString();
  const say = (t) => `${new Date(t * 1000).toDateString() === today ? "Today" : "Tomorrow"}, `
    + new Date(t * 1000).toLocaleTimeString([], {hour: "2-digit", minute: "2-digit"});
  const times = [nextLocal(20), nextLocal(8)].sort((a, b) => a - b);
  $("gw-when").replaceChildren(el("option", {value: "now", text: "Now"}),
    ...times.map((t) => el("option", {value: String(t), text: say(t)})),
    el("option", {value: "pick", text: "A date and hour…"}));
  const listed = gwAt == null ? "now" : times.includes(gwAt) ? String(gwAt) : "pick";
  $("gw-when").value = listed;
  $("gw-at").hidden = listed !== "pick";
}

async function judgeAt(t) {
  gwAt = t;
  try {
    gatewaysData = await api(gatewaysPath());
    await renderGatewaysPage();
  } catch (e) {
    toast(e.message, true);
  }
}

// a local date and time as a datetime-local input writes it
function localInput(t) {
  const d = new Date(t * 1000);
  const two = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())}T${two(d.getHours())}:00`;
}

function wireWhen() {
  $("gw-when").addEventListener("change", async () => {
    const v = $("gw-when").value;
    $("gw-at").hidden = v !== "pick";
    if (v === "pick") {
      $("gw-at").value = localInput(gwAt ?? nextLocal((new Date().getHours() + 1) % 24));
      $("gw-at").focus();
      return;
    }
    await judgeAt(v === "now" ? null : Number(v));
  });
  $("gw-at").addEventListener("change", async () => {
    const t = Date.parse($("gw-at").value);
    if (Number.isNaN(t)) return;
    const d = new Date(t);
    d.setMinutes(0, 0, 0);
    await judgeAt(Math.floor(d.getTime() / 1000));
  });
}

// what is assumed at another time, heading the view (§16.7)
function atNote(t) {
  const d = new Date(t * 1000);
  const day = d.toDateString() === new Date().toDateString() ? "Today"
    : d.toLocaleDateString([], {weekday: "long", day: "numeric", month: "long"});
  const local = d.toLocaleTimeString([], {hour: "2-digit", minute: "2-digit"});
  const utc = `${String(d.getUTCHours()).padStart(2, "0")}Z`;
  return `${day}, ${local} local (${utc}): the month's prediction for that hour; today's conditions `
    + "not included; no storm forecast; your position as in use now.";
}

function viewNote(view, count, g) {
  switch (view) {
    case "recommended":
      return (g.here ? "Nearest first: what your calls teach shows on each channel, and ranks them in a later version."
        : "As the post offices list them: save a place or set up a GPS to put the nearest first.")
        + hfNote(g.hf);
    case "favourites":
      return count ? "" : "No favourites yet: click ☆ beside a gateway, or beside one of its channels.";
    case "worked":
      if (!g.here) return "Where you are is not known: save a place or set up a GPS.";
      return count ? "Gateways you finished a session with from within 50 km of here (2 km on channels of 30 MHz and up)."
        : "No session finished from within 50 km of here yet (2 km on channels of 30 MHz and up).";
    default:
      return "";
  }
}

async function renderGatewaysPage() {
  const g = gatewaysData;
  if (!g) return;
  const others = (g.others || []).length;
  const missed = Object.entries(g.errors || {}).map(([d, why]) => `${d} (${why})`);
  $("gateways-note").textContent = g.domain
    ? `From ${g.domain}'s post office${others ? ` and ${others} it knows` : ""}, fetched `
      + `${new Date(g.fetched * 1000).toLocaleString()}. Kept here for when there is no Internet.`
      + (missed.length ? ` Not taken this time: ${missed.join("; ")}.` : "")
    : "No list yet: refresh it while you have the Internet. It is kept for when you do not.";
  $("gateways-here").textContent = hereWords(g.here, g.note);
  const warning = g.learning && g.learning.health && g.learning.health.warning;
  $("gateways-health").hidden = !warning;
  $("gateways-health").textContent = warning || "";
  for (const b of document.querySelectorAll("[data-gview]")) {
    b.setAttribute("aria-selected", String(b.dataset.gview === gwView));
  }
  const recent = gwView === "recent";
  $("gateway-table").hidden = recent;
  $("recent-table").hidden = !recent;
  $("own-form").hidden = recent;
  // past calls are not judged for a time
  $("gw-when").parentElement.hidden = recent;
  $("gw-at").hidden = recent || $("gw-when").value !== "pick";
  $("map-card").hidden = recent;
  $("learning-card").hidden = !recent;
  $("gateways-left-out").hidden = true;
  if (recent) {
    await renderRecent();
    return;
  }
  const blocked = g.gateways.filter((gw) => gw.blocked);
  let rows = g.gateways.filter((gw) => showBlocked || !gw.blocked);
  if (gwView === "favourites") {
    rows = rows.filter((gw) => gw.favourite || (gw.channels || []).some((c) => c.favourite));
  }
  if (gwView === "worked") rows = rows.filter((gw) => gw.worked);
  $("gateways-view-note").textContent = (g.at ? `${atNote(g.at)} ` : "")
    + viewNote(gwView, rows.length, g)
    + (g.marks_error ? ` Your marks could not be read: ${g.marks_error}.` : "");
  if (blocked.length) {
    $("gateways-left-out").hidden = false;
    $("gateways-left-out-text").textContent = `Left out: ${blocked.length} blocked.`;
    $("gateways-show-blocked").textContent = showBlocked ? "Hide them" : "Show them";
  }
  const twice = new Set(g.ambiguous || []);
  $("gateway-rows").replaceChildren(...rows.map((gw) => gatewayRow(gw, twice)));
  showMap(rows);
  showArea(rows);
}

// the terrain map: the VHF gateways of the view whose records give their site, from where you are
let areaMap = null;
function showArea(rows) {
  const here = gatewaysData.here;
  const vhf = rows.filter((gw) => gw.site && gw.terrain && gw.terrain.state !== "far");
  $("area-card").hidden = !here || !vhf.length;
  if ($("area-card").hidden) return;
  if (!areaMap) areaMap = new AreaMap($("area-map"), {onPick: (c) => pickGateway(c, "area")});
  areaMap.picked = pickedGateway;
  areaMap.show({here, gateways: vhf});
  // the notice the licence of the data held asks for, under the picture drawn from it
  api("terrain").then((t) => {
    $("area-notice").replaceChildren(...(t.notices || []).map((n) =>
      el("p", {class: "note terrain-notice", text: `${n.data}: ${n.notice}`})));
  }).catch(() => {});
}

// -- the map (§16.2): the rows of the view, the one picked in the list or on the map, and places
// picked on it --

let worldMap = null;
let mapFitted = false;
let pickedGateway = null;
let mapPoint = null;

// a gateway as the map draws it
function mapGateway(gw) {
  return {callsign: gw.callsign, at: gw.at, own: gw.own, worked: gw.worked, blocked: gw.blocked,
          favourite: gw.favourite || (gw.channels || []).some((c) => c.favourite),
          old: Boolean(gw.seen_old || gw.list_old), km: gw.km, bearing: gw.bearing,
          where: [gw.place, gw.grid].filter(Boolean).join(", ")};
}

function showMap(rows) {
  if (!worldMap) {
    worldMap = new WorldMap($("world-map"), {
      onPick: (callsign) => pickGateway(callsign, "map"),
      onPoint: pointPicked,
    });
    // the night side moves a degree every four minutes
    setInterval(() => worldMap.drawNight(), 4 * 60 * 1000);
  }
  worldMap.show({here: gatewaysData.here, gateways: rows.map(mapGateway)});
  if (!mapFitted && (rows.length || gatewaysData.here)) {
    worldMap.fit();
    mapFitted = true;
  }
  const off = rows.filter((gw) => !gw.at).length;
  $("map-card").querySelector("summary").textContent = off
    ? `Map: ${off} of the gateways here ${off === 1 ? "gives" : "give"} no grid square and ${off === 1 ? "is" : "are"} not on it`
    : "Map";
  pickGateway(rows.some((gw) => gw.callsign === pickedGateway) ? pickedGateway : null, "page");
}

// one gateway picked, in the list or on the map: marked in both, with what it is under the map
function pickGateway(callsign, from) {
  pickedGateway = callsign;
  if (from !== "map" && worldMap) worldMap.pick(callsign);
  if (areaMap) areaMap.pick(callsign);
  for (const tr of $("gateway-rows").children) {
    const on = tr.dataset.callsign === callsign;
    tr.classList.toggle("picked", on);
    tr.setAttribute("aria-selected", String(on));
    if (on && (from === "map" || from === "area")) tr.scrollIntoView({block: "nearest"});
  }
  const gw = callsign && (gatewaysData.gateways || []).find((g) => g.callsign === callsign);
  $("map-picked").hidden = !gw;
  if (!gw) return;
  const far = gw.km == null ? "" : `${gw.km} km${gw.bearing == null ? "" : ` at ${gw.bearing}°`}`;
  $("map-picked-text").textContent =
    [gw.callsign, [gw.place, gw.grid].filter(Boolean).join(", "), far].filter(Boolean).join(" · ");
  $("map-choose").disabled = gw.blocked;
}

function setPlacing(on) {
  if (worldMap) worldMap.placing(on);
  $("map-place").setAttribute("aria-pressed", String(on));
  $("map-place").classList.toggle("on", on);
  $("map-place").textContent = on ? "Click a spot on the map" : "Save a place from the map";
  if (!on) {
    mapPoint = null;
    $("map-point").hidden = true;
  }
}

function pointPicked(lat, lon) {
  mapPoint = {lat, lon};
  $("map-point").hidden = false;
  $("map-point-text").textContent = `${Math.abs(lat).toFixed(4)}° ${lat >= 0 ? "N" : "S"}, `
    + `${Math.abs(lon).toFixed(4)}° ${lon >= 0 ? "E" : "W"}`;
  $("map-point-name").focus();
}

async function savePoint() {
  const name = $("map-point-name").value.trim();
  if (!mapPoint) return;
  if (!name) {
    toast("A name for the place: Home, Mobile, Field Day.", true);
    $("map-point-name").focus();
    return;
  }
  try {
    await api("places", {op: "add", name, from: "map",
                         where: `${mapPoint.lat.toFixed(5)}, ${mapPoint.lon.toFixed(5)}`});
    $("map-point-name").value = "";
    $("place-name").value = "";
    setPlacing(false);
    toast(`${name} saved as a place. Setup's Places and GPS card has it.`);
    await openGateways();
  } catch (e) {
    toast(e.message, true);
  }
}

function wireMap() {
  const card = $("map-card");
  card.open = kept("gmap", "open") === "open";
  card.addEventListener("toggle", () => keep("gmap", card.open ? "open" : "closed"));
  const area = $("area-card");
  area.open = kept("garea", "open") === "open";
  area.addEventListener("toggle", () => keep("garea", area.open ? "open" : "closed"));
  $("map-fit").addEventListener("click", () => worldMap && worldMap.fit());
  $("report-form").addEventListener("change", () => {
    fillColourBy();
    renderReports(true);
  });
  $("report-age").addEventListener("change", () => renderReports(true));
  $("report-colour").addEventListener("change", () => renderReports());
  $("report-fit").addEventListener("click", () => reportsMap && reportsMap.fit());
  $("report-in").addEventListener("click", () => reportsMap && reportsMap.zoom(1.5));
  $("report-out").addEventListener("click", () => reportsMap && reportsMap.zoom(1 / 1.5));
  $("report-csv").addEventListener("click", () => exportReports("csv"));
  $("report-geojson").addEventListener("click", () => exportReports("geojson"));
  $("map-world").addEventListener("click", () => worldMap && worldMap.world());
  $("map-in").addEventListener("click", () => worldMap && worldMap.zoom(1.5));
  $("map-out").addEventListener("click", () => worldMap && worldMap.zoom(1 / 1.5));
  $("map-place").addEventListener("click", () =>
    setPlacing($("map-place").getAttribute("aria-pressed") !== "true"));
  $("map-point-save").addEventListener("click", savePoint);
  $("map-point-name").addEventListener("keydown", (event) => {
    if (event.key === "Enter") savePoint();
  });
  $("map-point-cancel").addEventListener("click", () => setPlacing(false));
  $("map-choose").addEventListener("click", () => {
    const gw = (gatewaysData.gateways || []).find((g) => g.callsign === pickedGateway);
    if (gw) chooseGateway(gw);
  });
}

// a small toggle: the star, or the block of one channel
function toggle(on, glyphs, title, run) {
  const b = el("button", {type: "button", class: `toggle${on ? " on" : ""}`, title,
                          "aria-label": title, "aria-pressed": String(on)});
  b.textContent = on ? glyphs[0] : glyphs[1];
  b.addEventListener("click", (event) => { event.stopPropagation(); run(); });
  return b;
}

async function markGateway(body) {
  try {
    gatewaysData = await api(`gateway-marks${atQuery()}`, body);
    await renderGatewaysPage();
  } catch (e) {
    toast(e.message, true);
  }
}

function gatewayRow(gw, twice) {
  const tr = document.createElement("tr");
  if (gw.blocked) tr.classList.add("blocked");
  tr.dataset.callsign = gw.callsign;
  // a click on the row, not on one of its buttons, picks it on the map
  tr.addEventListener("click", (event) => {
    if (!event.target.closest("button, input, select, a")) pickGateway(gw.callsign, "list");
  });
  const star = toggle(gw.favourite, ["★", "☆"],
    gw.favourite ? `${gw.callsign} is a favourite: click to unmark it` : `Mark ${gw.callsign} a favourite`,
    () => markGateway({op: gw.favourite ? "unfavourite" : "favourite", callsign: gw.callsign}));
  const by = el("div", {class: "note", text: gw.own ? "" : gw.post_office || ""});
  if (gw.own) by.append(el("span", {class: "tag", text: "yours", title: "A gateway of your own: no post office lists it"}));
  if (twice.has(gw.callsign)) {
    by.append(" ", el("span", {class: "tag", text: "listed twice",
      title: "Two post offices other than yours list this callsign, each its own way"}));
  }
  const name = el("div", {class: "gw-name"}, star, el("span", {text: gw.callsign}));
  const where = el("div", {text: [gw.place, gw.grid].filter(Boolean).join(", ")});
  if (gw.hours) where.append(el("div", {class: "note", text: `answers ${gw.hours}`, title: "When it answers"}));
  if (gw.updated) {
    where.append(el("div", {class: "note", text: `changed ${age(gw.updated)}`,
      title: "When its operator last changed its record"}));
  }
  // when it listens, from its windows (RP-5 §4.4); and when its post office last saw it
  const clockAt = (t) => {
    const d = new Date(t * 1000);
    return new Date().toDateString() === d.toDateString()
      ? d.toLocaleTimeString([], {hour: "2-digit", minute: "2-digit"})
      : d.toLocaleString([], {weekday: "short", hour: "2-digit", minute: "2-digit"});
  };
  if (gw.listening) {
    const l = gw.listening;
    const then = gatewaysData && gatewaysData.at ? "then" : "now";
    where.append(el("div", {class: l.open ? "note" : "note warn",
      text: l.open ? (l.changes ? `listening ${then}, until ${clockAt(l.changes)}` : `listening ${then}`)
        : `not listening ${then}: from ${clockAt(l.changes)}`,
      title: "From the listening times its record gives"}));
  }
  // when this station's modem last heard it on the air (W59)
  if (gw.heard) {
    const h = gw.heard;
    where.append(el("div", {class: "note",
      text: `heard ${age(h.at)}${h.snr_db != null ? ` at ${db(h.snr_db)}` : ""}`
        + `${h.dial_hz ? ` on ${mhzOf(h.dial_hz)} MHz` : ""}`,
      title: "When your modem last heard it on the air: a beacon, an identifier or a frame of a "
        + "session. One direction only"}));
  }
  if (gw.seen) {
    where.append(el("div", {class: "note", text: `seen ${age(gw.seen)}`,
      title: "When its post office last saw it: its computer was on, and online"}));
  }
  if (gw.seen_old) {
    tr.classList.add("old");
    where.lastChild.title = "Its post office has not seen it for days: its computer may be off, or it may be cut off and still serving by radio";
  }
  if (gw.list_old) {
    tr.classList.add("old");
    where.append(el("div", {class: "note", text: `its list was fetched ${age(gw.list_fetched)}`,
      title: "The post office's list could not be fetched since: refresh it over the Internet"}));
  }
  const channels = el("div", {}, ...(gw.channels || []).map((c) => {
    const line = el("div", {class: c.blocked ? "chan struck" : "chan"});
    line.append(
      toggle(c.favourite, ["★", "☆"], c.favourite ? "A favourite channel: click to unmark it"
        : "Mark this channel a favourite: it is offered first",
        () => markGateway({op: c.favourite ? "unfavourite" : "favourite", callsign: gw.callsign,
                           dial_hz: c.dial_hz})),
      el("span", {text: channelWords(c)}),
      toggle(c.blocked, ["⊘", "⊘"], c.blocked ? "Left out: click to put this channel back"
        : "Leave this channel out when the gateway is chosen",
        () => markGateway({op: c.blocked ? "unblock" : "block", callsign: gw.callsign,
                           dial_hz: c.dial_hz})));
    const parts = [line];
    if (c.hf && c.hf.state !== "no-engine") parts.push(hfLine(c.hf));
    const t = gw.terrain;
    if (t && t.dial_hz === c.dial_hz) parts.push(terrainLine(gw, t));
    return parts.length === 1 ? line : el("div", {}, ...parts);
  }));
  const last = el("div", {});
  if (gw.last) last.append(el("div", {text: `${gw.last.words}, ${when(gw.last.at)}`}));
  if (gw.worked) {
    last.append(el("span", {class: "tag", text: "worked from here",
      title: "A session finished with it from within 50 km of here (2 km at VHF)"}));
  }
  const acts = el("div", {class: "pair-buttons"});
  const choose = el("button", {type: "button", text: "Choose",
    title: `Call ${gw.callsign}: its channel goes in the call box, and the radio is tuned there at once when the modem can`});
  choose.addEventListener("click", () => chooseGateway(gw));
  choose.disabled = gw.blocked;
  const block = el("button", {type: "button", class: "quiet", text: gw.blocked ? "Unblock" : "Block",
    title: gw.blocked ? `Put ${gw.callsign} back in the list` : `Leave ${gw.callsign} out of the list`});
  block.addEventListener("click", () =>
    markGateway({op: gw.blocked ? "unblock" : "block", callsign: gw.callsign}));
  const details = el("button", {type: "button", class: "quiet", text: "Details",
    title: `What the model says of ${gw.callsign}, what your calls have taught and what they measured`});
  details.addEventListener("click", () => openDetails(gw.callsign));
  acts.append(choose, details, block);
  if (gw.own) {
    const remove = el("button", {type: "button", class: "quiet", text: "Remove",
      title: `Forget ${gw.callsign}, a gateway of your own`});
    remove.addEventListener("click", () => markGateway({op: "remove-own", callsign: gw.callsign}));
    acts.append(remove);
  }
  const cells = [[el("div", {}, name, by), "mono nowrap"], [where, ""],
                 [gw.km == null ? "" : `${gw.km} km`, "n"], [channels, "lines"],
                 [last, ""], [acts, "acts"]];
  for (const [c, cls] of cells) {
    const td = document.createElement("td");
    if (typeof c === "string") td.textContent = c; else td.append(c);
    if (cls) td.className = cls;
    tr.append(td);
  }
  return tr;
}

// -- HF (choosing a gateway, §10): each HF channel predicted both ways by VOACAP at this hour

const MONTHS = ["January", "February", "March", "April", "May", "June", "July", "August",
  "September", "October", "November", "December"];

// an HF channel's prediction in a line under it; its title says how it was worked out
function hfLine(h) {
  const shown = h.learned || h;
  const cls = h.state === "predicted" ? chanceClass(shown.chance_words) : "";
  const line = el("div", {class: `note verdict ${cls}`, text: shown.words, title: hfTitle(h)});
  const heard = h.heard && h.heard.words;
  if (!heard) return line;
  return el("div", {}, line, el("div", {class: "note", text: heard,
    title: "From hf-predict, running on this computer: the FT8 your receiver decoded in the last hour "
      + "from stations near this gateway or along the way to it"
      + (h.heard.examples && h.heard.examples.length ? `, such as ${h.heard.examples.join(", ")}` : "")
      + (h.heard.hearing_stations ? `; ${h.heard.hearing_stations} there heard reporting you or your neighbours` : "")}));
}

function hfTitle(h) {
  if (h.state !== "predicted" && h.state !== "no-modem") return h.why ? `${h.words}: ${h.why}` : h.words;
  const month = MONTHS[Number(String(h.month).slice(5)) - 1] || h.month;
  const way = (w, name) => `${name}: SNR ${w.snr_db} dB in 3 kHz on a median day, one day in ten `
    + `${w.lower_db} dB lower and one in ten ${w.upper_db} dB higher`
    + (w.rung ? `; ${w.rung} on a median day` : "");
  return [
    `VOACAP for ${month} at ${String(h.hour).padStart(2, "0")}Z, sunspot number ${h.ssn}, `
      + `${h.distance_km} km: ${h.class_words}, read as ITU ${h.class}`,
    h.open === h.open_model ? `The band is open on ${Math.round(h.open * 100)} % of days`
      : `The band is open on ${Math.round(h.open_model * 100)} % of days, taken as `
        + `${Math.round(h.open * 100)} % with what was heard here (2:1 in odds, until learned)`,
    way(h.up, "Sending"), way(h.down, "Receiving"),
    ...h.notes,
    learnedTitle(h),
    `Assumed: ${h.assumed.join("; ")}`,
  ].join(".\n") + ".";
}

// what was learned of an HF channel, in its line's title
function learnedTitle(h) {
  const l = h.learned;
  if (!l) return "Nothing is learned from your calls on this band yet";
  return `Learned from ${Math.round(l.calls * 10) / 10} of your calls: the path ${offsetWords(l.down_offset)} `
    + `receiving and ${offsetWords(l.up_offset)} sending against the model, the gateway answering `
    + `${pct(l.there)} of calls at this hour, the modem carrying ${pct(l.eta)} of its model's rate. `
    + `The model alone said: ${h.words}`;
}

function pct(x) {
  return x == null ? "—" : `${Math.round(x * 100)} %`;
}

function db(x) {
  return x == null ? "—" : `${x > 0 ? "+" : x < 0 ? "−" : ""}${Math.abs(Math.round(x))} dB`;
}

// an offset learned, with its spread: "−6 dB (±2)"
function offsetWords(o) {
  return o ? `${db(o.mean)} (±${Math.round(o.sd)})` : "—";
}

function bits(bps) {
  if (bps == null) return "—";
  return bps < 1000 ? `${Math.round(bps)} bit/s` : `${(bps / 1000).toFixed(1)} kbit/s`;
}

// -- terrain (choosing a gateway, §§8, 16.3): a VHF path over the ground, from elevation tiles
// fetched only when asked --

let terrainShown = null;  // the path in the card: {callsign, dial}
let terrainAgl = null;    // a height tried for your antenna, m, or null for the profile's
let terrainWatch = null;  // the timer that follows a fetch
let terrainTiles = null;  // what the card's buttons fetch: {glo90: [...], glo30: [...]}
let terrainBase = null;   // the path at your profile's height: {agl, margin}, to compare a height tried
let terrainTry = null;    // the timer that waits for the slider to rest

// what the chance says, as a class for its colour
function chanceClass(words) {
  return {"likely to work": "likely", "may work": "may", "unlikely to work": "unlikely"}[words] || "";
}

// a VHF gateway's terrain in a line under the channel judged, with the button that shows it
function terrainLine(gw, t) {
  const line = el("div", {class: `note verdict ${t.state === "judged" ? chanceClass(t.chance) : ""}`,
    text: t.words, title: t.state === "judged"
      ? "Worked out from the terrain held on this computer, your station profile and the gateway's record. Terrain shows how"
      : "Why the path is not worked out"});
  if (["judged", "fetch"].includes(t.state)) {
    const b = el("button", {type: "button", class: "quiet",
      text: t.state === "fetch" ? "Terrain…" : "Terrain",
      title: t.state === "fetch" ? "Fetch the terrain this path crosses, or see what it would take"
        : `The ground between you and ${gw.callsign}, and the link budget each way`});
    b.addEventListener("click", (event) => {
      event.stopPropagation();
      terrainAgl = null;
      terrainBase = null;
      openTerrain(gw.callsign, t.dial_hz);
    });
    line.append(b);
  }
  const l = t.learned;
  if (!l) return line;
  return el("div", {}, line, el("div", {class: `note verdict ${chanceClass(l.words)}`,
    text: `Learned from your calls: margin about ${Math.round(l.margin_db)} dB · ${l.words}`,
    title: `From ${Math.round(l.calls * 10) / 10} of your calls: the path ${offsetWords(l.down_offset)} `
      + `receiving and ${offsetWords(l.up_offset)} sending against the terrain's budget, the gateway `
      + `answering ${pct(l.there)} of calls at this hour: ${pct(l.chance)} in all`}));
}

async function openTerrain(callsign, dial) {
  terrainShown = {callsign, dial};
  const agl = terrainAgl == null ? "" : `&agl=${terrainAgl}`;
  let t;
  try {
    t = await api(`terrain/path?callsign=${encodeURIComponent(callsign)}&dial=${dial}${agl}`);
  } catch (e) {
    toast(e.message, true);
    return;
  }
  renderTerrain(t);
  const card = $("terrain-card");
  const opening = card.hidden;
  card.hidden = false;
  if (opening) card.scrollIntoView({block: "nearest"});
}

function renderTerrain(t) {
  $("terrain-title").textContent = `Terrain to ${t.callsign} on ${mhzOf(t.dial_hz)} MHz`;
  $("terrain-words").textContent = t.state === "judged"
    ? `${t.length_km} km at ${t.bearing_deg}° · ${t.words}` : t.words;
  $("terrain-words").className = t.state === "judged" ? `verdict ${chanceClass(t.chance)}` : "";
  const judged = t.state === "judged";
  $("terrain-judged").hidden = !judged;
  $("terrain-notice").textContent = judged ? `${t.data}: ${t.notice}` : "";
  terrainTiles = {glo90: t.tiles || [], glo30: (t.finer || {}).tiles || t.all_tiles || []};
  const fetchRow = $("terrain-fetch");
  fetchRow.hidden = !(t.state === "fetch" || (judged && t.finer && t.finer.tiles.length));
  $("terrain-fetch-90").hidden = t.state !== "fetch";
  if (t.state === "fetch") {
    $("terrain-fetch-90").textContent = `Fetch the terrain, about ${t.mb.glo90} MB`;
    $("terrain-fetch-30").textContent = `Finer, 30 m, about ${t.mb.glo30} MB`;
  } else if (judged && t.finer) {
    $("terrain-fetch-30").textContent = `Fetch finer data, 30 m, about ${t.finer.mb} MB`;
  }
  if (!judged) return;
  const mark = ["partly", "blocked"].includes(t.class.kind) ? t.class : null;
  drawProfile($("terrain-profile"), t.profile, mark);
  $("terrain-profile").setAttribute("aria-label", `${t.class_words}, over ${t.length_km} km`);
  const you = t.ends.you;
  if (you.said !== "tried") terrainBase = {agl: you.agl_m, margin: t.margin_db};
  $("terrain-agl").value = String(Math.round(you.agl_m));
  let said = you.said === "assumed" ? ", assumed" : ", from your station profile";
  if (you.said === "tried" && terrainBase) {
    const more = Math.round((t.margin_db - terrainBase.margin) * 10) / 10;
    said = `, tried: ${Math.abs(more)} dB ${more >= 0 ? "more" : "less"} margin than at ${terrainBase.agl} m`;
  }
  $("terrain-agl-value").textContent = `${you.agl_m} m up${said}`;
  const way = (name, b) => {
    const tr = el("tr", {});
    for (const [text, cls] of [[name, ""], [`${b.received_dbm} dBm`, "n"], [`${b.threshold_dbm} dBm`, "n"],
                               [`${b.margin_db} dB`, "n"], [b.words, `verdict ${chanceClass(b.words)}`]]) {
      tr.append(el("td", {class: cls, text}));
    }
    return tr;
  };
  $("terrain-rows").replaceChildren(way(`You to ${t.callsign}`, t.up), way(`${t.callsign} to you`, t.down));
  $("terrain-assumed").textContent = `Free-space loss ${t.fspl_db} dB, diffraction ${t.diffraction_db} dB `
    + `(ITU-R P.526-16 § 4.5). `
    + (t.close ? `Less certain: ${t.close}, where no terrain model is checked, so the chance is read `
      + `against a ${t.spread_db} dB spread, not 10 dB. ` : "")
    + `Assumed: ${t.assumed.join("; ")}.`;
}

// fetching: what is being fetched, in a line, until it is done; then what it changes is shown
async function followTerrain() {
  if (terrainWatch) return;
  const step = async () => {
    let s;
    try {
      s = await api("terrain");
    } catch {
      return;
    }
    const busy = Boolean(s.fetching) || s.queued > 0;
    const words = terrainProgress(s);
    for (const id of ["terrain-progress", "ts-progress"]) {
      $(id).hidden = !words;
      $(id).textContent = words;
    }
    $("ts-cancel").hidden = !busy;
    if (busy) return;
    clearInterval(terrainWatch);
    terrainWatch = null;
    if (!$("setup").hidden) renderTerrainSetup(s);
    if (!$("gateways-view").hidden) {
      if (terrainShown && !$("terrain-card").hidden) await openTerrain(terrainShown.callsign, terrainShown.dial);
      try {
        gatewaysData = await api(gatewaysPath());
        await renderGatewaysPage();
      } catch { /* shown at the next */ }
    }
  };
  terrainWatch = setInterval(step, 1000);
  await step();
}

function terrainProgress(s) {
  const parts = [];
  if (s.fetching) {
    const mb = (n) => (n / 1e6).toFixed(1);
    parts.push(`Fetching ${s.fetching.tile}: ${mb(s.fetching.read)}${s.fetching.of ? ` of ${mb(s.fetching.of)}` : ""} MB`);
  }
  if (s.queued) parts.push(`${s.queued} more to fetch`);
  if (s.done && !s.fetching && !s.queued) parts.push(`${s.done} fetched`);
  if (s.failed && s.failed.length) parts.push(`Not fetched: ${s.failed.join("; ")}`);
  return parts.join(" · ");
}

async function terrainRequest(body) {
  try {
    const s = await api("terrain", body);
    if (!$("setup").hidden) renderTerrainSetup(s);
    await followTerrain();
  } catch (e) {
    toast(e.message, true);
  }
}

// Setup's terrain card: what is held, and preparing the area around the position in use
let terrainStatus = null;
function renderTerrainSetup(s) {
  terrainStatus = s;
  const n = (k) => s.held[k];
  const held = [n("glo90") && `${n("glo90")} at 90 m`, n("glo30") && `${n("glo30")} at 30 m`].filter(Boolean);
  $("ts-held").textContent = held.length
    ? `Held: ${held.join(" and ")} (tiles of a degree square), ${s.held_mb} MB, in ${s.dir}.`
    : `None held yet. Tiles are kept in ${s.dir}.`;
  const radius = $("ts-radius");
  const was = radius.value || kept("tradius", "50");
  radius.replaceChildren(...(s.around || []).map((a) => new Option(`${a.radius_km} km`, String(a.radius_km))));
  radius.value = (s.around || []).some((a) => String(a.radius_km) === was) ? was : "50";
  radius.disabled = !s.around;
  $("ts-prepare").disabled = !s.around;
  terrainSize();
  $("ts-notice").replaceChildren(...(s.notices || []).map((n) =>
    el("p", {class: "note terrain-notice", text: `${n.data}: ${n.notice}`})));
  const words = terrainProgress(s);
  $("ts-progress").hidden = !words;
  $("ts-progress").textContent = words;
  $("ts-cancel").hidden = !(s.fetching || s.queued);
  // a fetch begun elsewhere is followed here too
  if (s.fetching || s.queued) followTerrain();
}

function terrainSize() {
  const s = terrainStatus;
  if (!s || !s.around) {
    $("ts-size").textContent = "Where you are is not known: save a place or set up a GPS first.";
    return;
  }
  const a = s.around.find((x) => String(x.radius_km) === $("ts-radius").value) || s.around[0];
  const source = $("ts-source").value;
  $("ts-size").textContent = `${a.tiles} ${a.tiles === 1 ? "tile" : "tiles"}, about ${a.mb[source]} MB at most: `
    + "the sea has no tiles, and what is held is not fetched again.";
}

async function loadTerrainSetup() {
  try {
    renderTerrainSetup(await api("terrain"));
  } catch (e) {
    toast(e.message, true);
  }
}

function wireTerrain() {
  $("terrain-close").addEventListener("click", () => {
    $("terrain-card").hidden = true;
    terrainShown = null;
  });
  // worked out again as the slider moves, once it rests a moment
  const agl = $("terrain-agl");
  agl.addEventListener("input", () => {
    $("terrain-agl-value").textContent = `${agl.value} m up, tried`;
    clearTimeout(terrainTry);
    terrainTry = setTimeout(() => {
      terrainAgl = Number(agl.value);
      if (terrainShown) openTerrain(terrainShown.callsign, terrainShown.dial);
    }, 150);
  });
  $("terrain-fetch-90").addEventListener("click", () =>
    terrainRequest({op: "fetch", tiles: terrainTiles.glo90, source: "glo90"}));
  $("terrain-fetch-30").addEventListener("click", () =>
    terrainRequest({op: "fetch", tiles: terrainTiles.glo30, source: "glo30"}));
  $("ts-radius").addEventListener("change", () => { keep("tradius", $("ts-radius").value); terrainSize(); });
  $("ts-source").addEventListener("change", terrainSize);
  $("ts-prepare").addEventListener("click", () =>
    terrainRequest({op: "prepare", radius_km: Number($("ts-radius").value), source: $("ts-source").value}));
  $("ts-cancel").addEventListener("click", () => terrainRequest({op: "cancel"}));
}

// a gateway chosen: its channels in the call box, favourites first and those left out dropped,
// and the radio tuned at once (W14) to the first, or to `dialHz` when it is one of them (a call
// from the Recent list chosen again)
function chooseGateway(gw, dialHz = null) {
  const channels = channelsOf(gw);
  switchKind("gateway", false);
  putGateway(gw.callsign, channels, dialHz);
  showKind();
  // the channel put in the box: the modem's own, when the gateway lists any
  const first = chosenChannel();
  const scanned = first && first.scan_s != null
    ? ` It scans its channels: the call keeps trying for ${first.scan_s} s or more.`
    : "";
  if (!first) {
    toast(`${gw.callsign} chosen. ${gw.channels && gw.channels.length ? "Every channel is left out." : "Its record names no channel."}`);
  } else {
    const modem = first.modem ? `, ${modemLabel(first.modem)}` : "";
    toast(`${gw.callsign} chosen, on ${mhzOf(first.dial_hz)} MHz${modem}.${scanned}`);
    pickChannel();
  }
}

async function renderRecent() {
  let r;
  try {
    r = await api("attempts?limit=100");
  } catch (e) {
    toast(e.message, true);
    return;
  }
  $("gateways-view-note").textContent = r.recent.length
    ? "Your last calls, newest first. They are kept on this computer only."
    : "No calls yet. Each call you place is kept here, with how it ended.";
  const byCall = new Map((gatewaysData.gateways || []).map((gw) => [gw.callsign, gw]));
  const shown = r.recent.filter((a) => showForgotten || !a.forgotten);
  $("recent-rows").replaceChildren(...shown.map((a) => {
    const tr = el("tr", {class: a.forgotten ? "forgotten" : ""});
    if (a.forgotten) tr.title = "Forgotten: learning leaves this call out";
    const gw = byCall.get(a.remote);
    const called = el("div", {}, el("span", {class: "mono", text: a.remote}),
      el("div", {class: "note", text: a.kind === "station" ? "a station" : "a gateway"}));
    const channel = a.dial_hz ? `${mhzOf(a.dial_hz)} MHz${a.bandwidth_hz ? `, ${a.bandwidth_hz} Hz` : ""}` : "";
    const ended = el("span", {text: a.words, class: a.completed ? "" : "warn"});
    if (a.why) ended.title = a.why;
    const took = a.took_s == null ? "" : a.took_s < 120 ? `${a.took_s} s` : `${Math.round(a.took_s / 60)} min`;
    const acts = el("div", {});
    if (gw && !gw.blocked) {
      const again = el("button", {type: "button", class: "quiet", text: "Choose",
        title: `Choose ${a.remote} again`});
      again.addEventListener("click", () => chooseGateway(gw, a.dial_hz));
      acts.append(again);
    }
    for (const [c, cls] of [[when(a.at), ""], [called, ""], [channel, ""], [ended, ""],
                            [took, "n"], [acts, "acts"]]) {
      const td = el("td", {class: cls});
      if (typeof c === "string") td.textContent = c; else td.append(c);
      tr.append(td);
    }
    return tr;
  }));
  await renderLearning();
}

// -- what the call records teach (choosing a gateway, §§12.9, 16.4): one gateway's details, and
// forgetting --

let detailsShown = null;
let showForgotten = false;

async function openDetails(callsign) {
  let d;
  try {
    d = await api(`learned?callsign=${encodeURIComponent(callsign)}`);
  } catch (e) {
    toast(e.message, true);
    return;
  }
  detailsShown = callsign;
  renderDetails(d);
  const card = $("details-card");
  const opening = card.hidden;
  card.hidden = false;
  if (opening) card.scrollIntoView({block: "nearest"});
}

function renderDetails(d) {
  $("details-title").textContent = d.callsign;
  const g = d.gateway;
  const about = [];
  if (g && g.post_office) about.push(`Listed by ${g.post_office}`);
  if (g && g.seen) about.push(`its post office saw it ${age(g.seen)}`);
  if (d.service) about.push(`sessions it served: ${pct(d.service.mean)}`);
  if (d.forgotten) about.push("its history is forgotten");
  $("details-about").textContent = about.join(" · ");
  $("details-channels").replaceChildren(...d.channels.map(detailsTable));
  const a = d.availability;
  const hh = (h) => String(h).padStart(2, "0");
  $("details-day").textContent = a
    ? "Answers through the day: "
      + a[1].map((p, i) => `${hh(i * 3)}–${hh(i * 3 + 3)}Z ${pct(p)}`).join(" · ")
    : "";
  $("details-calls").replaceChildren(...(d.recent.length ? d.recent.map((r) =>
    el("li", {class: r.forgotten ? "forgotten" : "",
      text: `${when(r.at)}: ${r.words}${r.dial_hz ? `, ${mhzOf(r.dial_hz)} MHz` : ""}`
        + (r.forgotten ? " (forgotten)" : "")}))
    : [el("li", {text: "No calls to it yet."})]));
  const forget = $("details-forget");
  forget.textContent = d.forgotten ? "Bring its history back" : "Forget its history";
  forget.title = d.forgotten ? `Count your calls to ${d.callsign} in learning again`
    : `Leave your calls to ${d.callsign} so far out of learning: after it changed its station, say. They stay in the Recent list`;
  forget.onclick = async () => {
    try {
      if (d.forgotten) {
        const rules = await api("forget");
        const i = rules.rules.findIndex((r) => r.what === "gateway"
          && r.callsign.toUpperCase() === d.callsign.toUpperCase());
        if (i >= 0) await api("forget", {undo: i});
      } else {
        await api("forget", {what: "gateway", callsign: d.callsign});
      }
      await afterForgetting();
    } catch (e) {
      toast(e.message, true);
    }
  };
}

// one channel's three columns (§16.4): the model, what was learned and what was measured
function detailsTable(c) {
  const hf = c.model && c.model.hf;
  const t = c.model && c.model.terrain;
  const l = c.learned;
  const m = c.measured;
  const learnedHf = hf && hf.learned;
  const name = /^\d+$/.test(c.channel) ? `${mhzOf(Number(c.channel))} MHz` : c.channel;
  const signed = (x) => `${x > 0 ? "+" : x < 0 ? "−" : ""}${Math.abs(Math.round(x))}`;
  const range = (r) => r ? `${signed(r.min)} … ${signed(r.max)} dB, ${r.n} session${r.n === 1 ? "" : "s"}` : "—";
  const way = (key) => {
    const learnedWay = l && l[key];
    if (hf && hf[key]) {
      const said = hf[key].snr_db;
      return [db(said), learnedHf ? `${db(learnedHf[key].snr_db)} (±${Math.round(learnedWay.sd)})`
        : learnedWay ? offsetWords(learnedWay) : "—"];
    }
    if (t && t[key]) {
      const said = t[key].margin_db;
      return [`${db(said)} margin`, learnedWay ? `${db(said + learnedWay.mean)} margin (±${Math.round(learnedWay.sd)})` : "—"];
    }
    return ["—", learnedWay ? `${offsetWords(learnedWay)} against the model` : "—"];
  };
  const [downModel, downLearned] = way("down");
  const [upModel, upLearned] = way("up");
  const rows = [
    ["Signal there → here", downModel, downLearned, range(m.down),
     "The gateway's signal as your modem hears it, dB in 3 kHz (VHF: the margin over what the modem needs)"],
    ["Signal here → there", upModel, upLearned, m.up ? `${range(m.up)}, from its reports` : "—",
     "Your signal as the gateway hears it, from what it reports during a session"],
    ["Answers when called", "—", l ? `${pct(l.there)}, from about ${Math.max(1, Math.round(l.calls))} call${Math.round(l.calls) > 1 ? "s" : ""}` : "—",
     m.calls ? `${m.answered} of ${m.calls}` : "—",
     "How often it answered: on the grid of what was learned, and counted"],
    ["Links made", hf && hf.chance != null ? pct(hf.chance) : t && t.chance ? t.chance : "—",
     learnedHf ? pct(learnedHf.chance) : t && t.learned ? pct(t.learned.chance) : "—",
     m.calls ? `${m.sessions} of ${m.calls} calls` : "—",
     "The chance a call links now, at this hour: the model's, then with what was learned"],
    ["Rate", hf && hf.down ? `${bits(hf.down.bps)} receiving` : "—",
     learnedHf && learnedHf.down ? `${bits(learnedHf.down.bps)} receiving` : l ? `${pct(l.efficiency.mean)} of the model's` : "—",
     m.bps ? `${bits(m.bps.min).replace(/ .*/, "")} … ${bits(m.bps.max)}` : "—",
     "Data carried once linked: the modem's model, then with its efficiency learned here; measured over sessions long enough to judge"],
    ["Links lost", "—", l ? `about ${l.losses_per_hour.toFixed(1)} an hour` : "—",
     m.sessions ? `${m.lost} in ${m.sessions} session${m.sessions === 1 ? "" : "s"}` : "—",
     "Sessions that ended early, the signal strong before: not fades"],
  ];
  const head = el("tr", {}, el("th", {text: name, title: c.listed ? channelWords(c.listed) : "A channel it no longer lists"}),
    el("th", {text: "Model", title: "What the prediction says now"}),
    el("th", {text: "Learned", title: "The model corrected by what your calls taught, with its spread"}),
    el("th", {text: "Measured, last 30 days", title: "What your calls measured"}));
  const body = rows.map(([what, model, learned, measured, title]) => el("tr", {},
    el("th", {text: what, title}), el("td", {text: model}), el("td", {text: learned}),
    el("td", {text: measured})));
  return el("table", {class: "list details-table", "aria-label": `${name}: model, learned and measured`},
    el("thead", {}, head), el("tbody", {}, ...body));
}

// the learning card in the Recent view: what is forgotten, and the ways to forget
async function renderLearning() {
  let f;
  try {
    f = await api("forget");
  } catch (e) {
    toast(e.message, true);
    return;
  }
  $("learning-counts").textContent = f.records
    ? `Learning uses ${f.records - f.forgotten} of your ${f.records} calls`
      + (f.forgotten ? `; ${f.forgotten} forgotten.` : ".")
    : "No calls yet: learning starts with your first.";
  const day = (s) => new Date(s * 1000).toLocaleDateString();
  const words = (r) => ({
    gateway: () => `${r.callsign}'s calls up to ${day(r.at)}`,
    place: () => `calls within ${r.km} km of ${r.lat.toFixed(3)}, ${r.lon.toFixed(3)}, up to ${day(r.at)}`,
    before: () => `calls before ${day(r.before)}`,
    everything: () => `every call up to ${day(r.at)}`,
  }[r.what] || (() => r.what))();
  $("forget-rules").replaceChildren(...f.rules.map((r, i) => {
    const undo = el("button", {type: "button", class: "quiet", text: "Undo",
      title: "Count these calls in learning again"});
    undo.addEventListener("click", async () => {
      try {
        await api("forget", {undo: i});
        await afterForgetting();
      } catch (e) {
        toast(e.message, true);
      }
    });
    return el("li", {}, el("span", {text: `Forgotten: ${words(r)}`}), undo);
  }));
}

async function forgetting(body) {
  try {
    await api("forget", body);
    await afterForgetting();
  } catch (e) {
    toast(e.message, true);
  }
}

// what forgetting changes: the lines learned, the open details and the Recent view
async function afterForgetting() {
  gatewaysData = await api(gatewaysPath());
  await renderGatewaysPage();
  if (detailsShown && !$("details-card").hidden) await openDetails(detailsShown);
}

function wireGateways() {
  for (const b of document.querySelectorAll("[data-gview]")) {
    b.addEventListener("click", async () => {
      gwView = b.dataset.gview;
      keep("gview", gwView);
      await renderGatewaysPage();
    });
  }
  wireMap();
  wireWhen();
  $("gateways-show-blocked").addEventListener("click", async () => {
    showBlocked = !showBlocked;
    await renderGatewaysPage();
  });
  $("details-close").addEventListener("click", () => {
    $("details-card").hidden = true;
    detailsShown = null;
  });
  $("learning-show-forgotten").addEventListener("change", async (e) => {
    showForgotten = e.currentTarget.checked;
    await renderRecent();
  });
  $("forget-here").addEventListener("click", (e) => armed(e.currentTarget,
    "Click again to forget them", () => forgetting({what: "place", km: 1})));
  $("forget-before").addEventListener("click", () => {
    const day = $("forget-date").value;
    if (!day) {
      toast("Choose the day first.", true);
      return;
    }
    forgetting({what: "before", before: Math.floor(new Date(`${day}T00:00`).getTime() / 1000)});
  });
  $("forget-everything").addEventListener("click", (e) => armed(e.currentTarget,
    "Click again to forget every call", () => forgetting({what: "everything"})));
  $("own-add").addEventListener("click", async () => {
    const mhz = Number(String($("own-mhz").value).replace(",", "."));
    await markGateway({op: "add-own", callsign: $("own-callsign").value, grid: $("own-grid").value,
                       place: $("own-place").value, hours: $("own-hours").value,
                       dial_hz: Number.isFinite(mhz) && mhz > 0 ? Math.round(mhz * 1e6) : 0,
                       bandwidth_hz: Number($("own-bw").value)});
    $("own-mhz").value = "";
  });
}

// a row's buttons: the trash can, or in the trash put back and delete for good
function rowActions(m, name) {
  const td = document.createElement("td");
  td.className = "acts";
  const button = (glyph, title, run, extra = "") => {
    const b = document.createElement("button");
    b.type = "button";
    b.className = `act changes-home ${extra}`;
    b.title = title;
    b.setAttribute("aria-label", title);
    b.append(icon(glyph));
    b.addEventListener("click", (event) => { event.stopPropagation(); run(b); });
    td.append(b);
  };
  if (name === "trash") {
    button("restore", `Put it back in the ${m.was}`,
           () => fromTrash("restore", m.id, m.was, `Back in the ${m.was}.`));
    button("gone", "Delete it for good (click twice)", (b) => {
      if (b.dataset.armed) {
        delete b.dataset.armed;
        b.classList.remove("armed");
        fromTrash("purge", m.id, m.was, "Deleted for good.");
        return;
      }
      b.dataset.armed = "1";
      b.classList.add("armed");
      b.title = "Click again to delete it for good";
      setTimeout(() => { delete b.dataset.armed; b.classList.remove("armed"); }, 4000);
    }, "danger");
  } else {
    if (name === "outbox") {
      // a message a gateway queued is that gateway's to pass on: the panel says so if asked
      button("write", m.queued ? `Queued at ${m.queued.gateway}: it can no longer be changed`
                               : "Edit it: the changed message takes its place", () => edit(m.id));
    }
    button("trash", "Move it to the trash", () => toTrash(m.id, name));
  }
  return td;
}

async function openFolder(name) {
  if (name === "waiting") return openWaiting();
  if (name === "addresses") return openAddresses();
  if (name === "alerts") return openAlerts();
  if (name === "reports") return openReports();
  folder = name;
  markFolder(name);
  const trash = name === "trash";
  $("folder-title").textContent = FOLDER_NAMES[name] || name;
  $("who-col").textContent = trash ? "From or to" : name === "inbox" ? "From" : "To";
  $("who-col").title = trash ? "Who wrote it, or whom it was for" : name === "inbox" ? "Who wrote it" : "Whom it is for";
  const rows = await api(`folder?name=${name}`);
  const body = $("rows");
  body.replaceChildren(...rows.map((m) => {
    const tr = document.createElement("tr");
    tr.className = ["", "priority", "emergency"][m.priority] || "";
    const received = trash ? m.was === "inbox" : name === "inbox";
    const who = received ? m.from : m.to.concat(m.cc).join(", ");
    for (const text of [who, m.subject || "(no subject)", when(m.date)]) {
      const td = document.createElement("td");
      td.textContent = text;
      tr.append(td);
    }
    tr.children[2].className = "date";
    tr.children[2].before(clipCell(m.attachments));
    if (trash) {
      const tag = document.createElement("span");
      tag.className = "tag";
      tag.textContent = FOLDER_NAMES[m.was];
      tag.title = `Deleted from the ${m.was} ${new Date(m.deleted * 1000).toLocaleString()}`;
      tr.children[1].prepend(tag);
    }
    // where a message sent by radio is (RP-7 §3.6): queued at a cut-off gateway, or in doubt
    if (m.queued) {
      const doubtful = m.state === "may have been dropped";
      const since = new Date(m.queued.at * 1000).toLocaleString();
      const tag = el("span", {class: doubtful ? "tag bad" : "tag",
        text: doubtful ? "may have been dropped" : `queued at ${m.queued.gateway}`,
        title: doubtful
          ? `Queued at ${m.queued.gateway} since ${since}, with no word that it reached your post office: it may have been dropped unsent. It is offered again on each call.`
          : `Queued at ${m.queued.gateway} since ${since}: that gateway could not reach your post office. It is offered again until a gateway that can confirms it.`});
      tr.children[1].prepend(tag);
    }
    tr.append(rowActions(m, name));
    tr.title = `${m.short}${m.station ? ` · via ${m.station}` : ""}`;
    tr.addEventListener("click", () => openMessage(m.id, {folder: name, was: m.was}));
    return tr;
  }));
  $("empty").textContent = trash ? "The trash is empty." : "Nothing here.";
  $("empty").hidden = rows.length > 0;
  $("empty-trash").hidden = !trash || rows.length === 0;
  show("list-view");
}

// ── the address list ─────────────────────────────────────────────────────────────────────────
// Kept by the client (addresses.json), learned from every message written, offered as To and Cc
// are typed, and managed in the Addresses view. The browser's own memory of the fields is off:
// the user could not see or delete what it kept.

let addressBook = [];
// W2: groups of addresses, each with its members written as To takes them (groups.json)
let groupBook = [];

async function loadAddresses() {
  try {
    const r = await api("addresses");
    addressBook = r.entries;
    groupBook = r.groups || [];
  } catch { /* kept as it was */ }
  $("n-addresses").textContent = addressBook.length || "";
  return addressBook;
}

function writtenAs(e) {
  if (!e.name) return e.address;
  return /[,;"<>]/.test(e.name) ? `"${e.name.replace(/"/g, "")}" <${e.address}>` : `${e.name} <${e.address}>`;
}

async function changeAddresses(change) {
  const r = await api("addresses", change);
  addressBook = r.entries;
  if (r.groups) groupBook = r.groups;
  $("n-addresses").textContent = addressBook.length || "";
  return r;
}

// a group's members, written as To takes them, less those already in `head`
function groupWritten(g, head = "") {
  const taken = head.toLowerCase();
  return g.members.filter((m) => !taken.includes(m.address)).map(writtenAs);
}

function writeToGroup(g) {
  if (popOut({to: `${groupWritten(g).join(", ")}, `})) return;
  const form = newComposer();
  form.to.value = `${groupWritten(g).join(", ")}, `;
  show("composer");
  form.subject.focus();
}

// the group form: a new group, or the one being changed (`editingGroup`, its name)
let editingGroup = null;

function editGroup(g) {
  const f = $("group-form");
  editingGroup = g ? g.name : null;
  f.name.value = g ? g.name : "";
  f.members.value = g ? `${groupWritten(g).join(", ")}, ` : "";
  $("group-cancel").hidden = !g;
  $("group-save").textContent = g ? "Save changes" : "Save";
  if (g) f.members.focus();
}

async function saveGroup(event) {
  event.preventDefault();
  const f = $("group-form");
  const name = f.name.value.trim();
  try {
    await changeAddresses({group_save: {was: editingGroup, name, members: f.members.value}});
    toast(editingGroup ? `${name} is changed.` : `${name} is a group now. Type its name in To.`);
    editGroup(null);
    renderAddresses();
  } catch (e) {
    toast(e.message, true);
  }
}

async function forgetGroup(g) {
  try {
    const r = await changeAddresses({group_remove: g.name});
    renderAddresses();
    toast(`The group ${g.name} is gone; its addresses stay on the list.`, false,
      {label: "Undo", run: async () => {
        await changeAddresses({group_put_back: {name: r.removed_group.name,
          members: r.removed_group.members}});
        renderAddresses();
      }});
  } catch (e) {
    toast(e.message, true);
  }
}

function renderGroups() {
  const find = $("address-filter").value.trim().toLowerCase();
  const shown = groupBook.filter((g) => !find || g.name.toLowerCase().includes(find)
    || g.members.some((m) => m.address.includes(find)
      || (m.name || "").toLowerCase().includes(find)));
  $("group-rows").replaceChildren(...shown.map((g) => {
    const tr = document.createElement("tr");
    const who = g.members.map((m) => m.name || m.address).join(", ");
    [g.name, `${g.members.length} · ${who}`].forEach((text, i) => {
      const td = document.createElement("td");
      td.textContent = text;
      if (i === 1) td.className = "date";
      tr.append(td);
    });
    tr.append(levelSelect(`group:${g.name}`, `How mail from anyone in ${g.name} arrives`));
    const acts = document.createElement("td");
    acts.className = "acts";
    for (const [glyph, title, run] of [
      ["write", `Write to everyone in ${g.name}`, () => writeToGroup(g)],
      ["people", `Change ${g.name}: its name and addresses`, () => editGroup(g)],
      ["trash", `Take the group ${g.name} away; its addresses stay`, () => forgetGroup(g)],
    ]) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "act";
      b.title = title;
      b.setAttribute("aria-label", title);
      b.append(icon(glyph));
      b.addEventListener("click", (event) => { event.stopPropagation(); run(); });
      acts.append(b);
    }
    tr.append(acts);
    tr.title = `Write to everyone in ${g.name}`;
    tr.addEventListener("click", () => writeToGroup(g));
    return tr;
  }));
  const empty = $("groups-empty");
  empty.hidden = shown.length > 0;
  empty.textContent = groupBook.length
    ? "No group has that in its name or addresses."
    : "No groups yet. Give one a name and its addresses above.";
}

// W2: a contact file read into the list and the groups
async function importContacts(file) {
  if (!file) return;
  try {
    const text = await file.text();
    const r = await changeAddresses({import: text});
    const d = r.imported;
    const parts = [`${d.added} address${d.added === 1 ? "" : "es"} added`];
    if (d.named) parts.push(`${d.named} named`);
    if (d.groups) parts.push(`${d.groups} group${d.groups === 1 ? "" : "s"}`);
    if (d.skipped.length) parts.push(`${d.skipped.length} left out, not email addresses`);
    toast(`${file.name}: ${parts.join(", ")}.`);
    renderAddresses();
  } catch (e) {
    toast(e.message, true);
  }
}

async function forgetAddress(address, after) {
  try {
    const r = await changeAddresses({remove: address});
    toast(`${address} is off the address list.`, false, {label: "Undo", run: async () => {
      await changeAddresses({put_back: r.removed});
      await after();
    }});
    await after();
  } catch (e) {
    toast(e.message, true);
  }
}

function writeTo(e) {
  if (popOut({to: `${writtenAs(e)}, `})) return;
  const form = newComposer();
  form.to.value = `${writtenAs(e)}, `;
  show("composer");
  form.subject.focus();
}

async function openAddresses() {
  folder = "addresses";
  markFolder("addresses");
  await loadAddresses();
  renderAddresses();
  show("addresses-view");
}

function renderAddresses() {
  const find = $("address-filter").value.trim().toLowerCase();
  const shown = addressBook.filter((e) => !find
    || e.address.includes(find) || (e.name || "").toLowerCase().includes(find));
  $("address-rows").replaceChildren(...shown.map((e) => {
    const tr = document.createElement("tr");
    const used = e.uses
      ? `${e.uses} message${e.uses === 1 ? "" : "s"} · ${when(e.used)}`
      : "added by hand";
    [e.name || "", e.address, used].forEach((text, i) => {
      const td = document.createElement("td");
      td.textContent = text;
      if (i === 1) td.className = "mono";
      if (i === 2) td.className = "date";
      tr.append(td);
    });
    tr.append(levelSelect(`address:${e.address}`, `How mail from ${e.address} arrives`));
    const acts = document.createElement("td");
    acts.className = "acts";
    for (const [glyph, title, run] of [
      ["write", `Write to ${e.address}`, () => writeTo(e)],
      ["trash", "Take it off the address list", () => forgetAddress(e.address, openAddresses)],
    ]) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "act";
      b.title = title;
      b.setAttribute("aria-label", title);
      b.append(icon(glyph));
      b.addEventListener("click", (event) => { event.stopPropagation(); run(); });
      acts.append(b);
    }
    tr.append(acts);
    tr.title = `Write to ${writtenAs(e)}`;
    tr.addEventListener("click", () => writeTo(e));
    return tr;
  }));
  const empty = $("addresses-empty");
  empty.hidden = shown.length > 0;
  empty.textContent = addressBook.length
    ? "None has that in its name or address."
    : "No addresses yet. Everyone you write to is added here.";
  renderGroups();
}

// Suggestions under To and Cc: the address list's entries that match what is being typed after
// the last comma, those written to most first. Arrows move, Enter or Tab takes one, Escape
// closes; the × takes an address off the list there and then.
function wireSuggestions(input) {
  const list = input.parentElement.querySelector(".suggestions");
  let shown = [];
  let active = -1;
  const split = () => {
    const v = input.value;
    const cut = Math.max(v.lastIndexOf(","), v.lastIndexOf(";"));
    return [v.slice(0, cut + 1), v.slice(cut + 1).trim().toLowerCase()];
  };
  const close = () => { list.hidden = true; shown = []; active = -1; };
  const pick = (e) => {
    const [head] = split();
    const written = e.members ? groupWritten(e, head).join(", ") : writtenAs(e);
    input.value = `${head}${head ? " " : ""}${written}${written ? ", " : ""}`;
    close();
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
  };
  const render = () => {
    const [head, typed] = split();
    const taken = head.toLowerCase();
    const groups = !typed || input.name === "members" ? [] : groupBook
      .filter((g) => g.name.toLowerCase().startsWith(typed)
        || g.name.toLowerCase().split(/\s+/).some((w) => w.startsWith(typed)))
      .slice(0, 3);
    shown = !typed ? [] : [...groups, ...addressBook
      .filter((e) => !taken.includes(e.address))
      .filter((e) => e.address.startsWith(typed) || e.address.includes(typed)
        || (e.name || "").toLowerCase().split(/\s+/).some((w) => w.startsWith(typed)))
      .sort((a, b) => (b.uses - a.uses) || (b.used - a.used))
      .slice(0, 6 - groups.length)];
    if (!shown.length) { close(); return; }
    active = Math.min(Math.max(active, 0), shown.length - 1);
    list.replaceChildren(...shown.map((e, i) => {
      const li = document.createElement("li");
      li.setAttribute("role", "option");
      li.setAttribute("aria-selected", String(i === active));
      if (e.members) {
        // a group: its name, and how many addresses it puts in
        const name = document.createElement("span");
        name.className = "s-name s-group";
        name.textContent = e.name;
        const count = document.createElement("span");
        count.className = "s-addr";
        count.textContent = `group · ${e.members.length} address${e.members.length === 1 ? "" : "es"}`;
        li.append(name, count);
        li.title = e.members.map((m) => m.address).join(", ");
        li.addEventListener("mousedown", (event) => { event.preventDefault(); pick(e); });
        return li;
      }
      const name = document.createElement("span");
      name.className = "s-name";
      name.textContent = e.name || "";
      const addr = document.createElement("span");
      addr.className = "s-addr";
      addr.textContent = e.address;
      const del = document.createElement("button");
      del.type = "button";
      del.className = "s-del";
      del.title = `Take ${e.address} off the address list`;
      del.setAttribute("aria-label", del.title);
      del.append(icon("gone"));
      del.addEventListener("mousedown", (event) => {
        event.preventDefault();
        event.stopPropagation();
        forgetAddress(e.address, async () => { render(); });
      });
      li.append(name, addr, del);
      li.addEventListener("mousedown", (event) => { event.preventDefault(); pick(e); });
      return li;
    }));
    list.hidden = false;
  };
  input.addEventListener("input", () => { active = 0; render(); });
  input.addEventListener("blur", () => setTimeout(close, 120));
  input.addEventListener("keydown", (event) => {
    if (list.hidden || !shown.length) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      active = (active + (event.key === "ArrowDown" ? 1 : shown.length - 1)) % shown.length;
      render();
    } else if (event.key === "Enter" || event.key === "Tab") {
      if (active < 0) return;
      event.preventDefault();
      pick(shown[active]);
    } else if (event.key === "Escape") {
      event.preventDefault();
      close();
    }
  });
}

// what the reader's buttons act on: the folder the message was opened from
let reading = null;

// W1: the message being answered or forwarded, while the composer is open on it
let answering = null;

const ANSWER_TITLES = {reply: "Reply", "reply-all": "Reply to all", forward: "Forward"};

// the composer emptied, for a new message
// Forms (RP-2 §8): the templates this computer holds, and the fields of the one chosen. Only the
// values go on the air; each station makes the form's text from its own copy of the template.
let templates = [];

async function loadForms() {
  try {
    templates = await api("forms");
  } catch {
    templates = [];
  }
  showKeptForms();
  const select = $("c-form");
  const byName = [...templates].sort((a, b) =>
    a.name.localeCompare(b.name, undefined, {numeric: true}));
  select.replaceChildren(el("option", {value: "", text: "None"}),
    ...byName.map((t) => el("option", {value: t.template, text: t.name})));
}

// Setup's forms kept here (RP-2 §8.3): each with Export and Remove; the published ones are the
// program's own and are not listed
function showKeptForms() {
  const kept = templates.filter((t) => t.kept)
    .sort((a, b) => a.name.localeCompare(b.name, undefined, {numeric: true}));
  $("forms-kept").replaceChildren(...(kept.length ? kept.map((t) => {
    const name = el("span", {text: t.name, title: `Fingerprint ${t.template}`});
    const out = el("button", {class: "quiet", type: "button", text: "Export",
      title: "Write it to your Downloads folder as its JSON, to give to someone"});
    out.addEventListener("click", async () => {
      try { toast(`Saved as ${(await api("forms/export", {template: t.template})).name} in Downloads.`); }
      catch (e) { toast(e.message, true); }
    });
    const gone = el("button", {class: "quiet danger", type: "button", text: "Remove",
      title: "Stop holding it here: its forms then show by number, until you get it again"});
    gone.addEventListener("click", (e) => armed(e.currentTarget, "Click again to remove", async () => {
      try {
        await api("forms/remove", {template: t.template});
        await loadForms();
      } catch (err) { toast(err.message, true); }
    }));
    return el("li", {}, name, el("span", {class: "entry-tools"}, out, gone));
  }) : [el("li", {class: "note", text: "None yet."})]));
}

// a form's file, its JSON or CBOR, kept and handed in to the post office
async function importForm(file) {
  if (!file) return;
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = "";
  for (const b of bytes) binary += String.fromCharCode(b);
  const data = btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  try {
    const t = await api("forms/import", {data});
    await loadForms();
    toast(`Kept ${t.name}.`);
  } catch (e) {
    toast(e.message, true);
  }
}

const DAY_MS = 86400000;

// the inputs of the form chosen, each {n, get, set}: get() is the value to send, null when empty;
// set(v) puts back a value as the panel gives it (Edit)
let formInputs = [];

// the composer's fields for the form chosen, or none. A position field starts with the position in
// use (where it came from said under it), unless `fresh` is false (Edit puts the values back)
function showFormFields(fresh = true) {
  const box = $("c-form-fields");
  const t = templates.find((x) => x.template === $("c-form").value);
  box.hidden = !t;
  $("c-body-name").textContent = t ? "Note" : "Text";
  $("composer").body.rows = t ? 3 : 12;
  formInputs = [];
  if (!t) {
    box.replaceChildren();
    return;
  }
  box.replaceChildren(...formFieldNodes(t, fresh, formInputs));
}

// a template's fields as inputs, each pushed on `inputs` ({n, get, set, here}) as it is made
function formFieldNodes(t, fresh, inputs) {
  const legend = el("legend", {text: t.name, title: t.note || ""});
  return [legend, ...t.fields.filter((f) => !f.bound).map((f) => {
    if (f.type === "heading") return el("h3", {class: "form-heading", text: f.label});
    const input = fieldInput(f, fresh);
    inputs.push({n: f.n, get: input.get, set: input.set, here: input.here});
    // the input itself, by the field's number and type, as a test or a script finds it
    const target = input.input || input.node;
    target.dataset.field = String(f.n);
    target.dataset.kind = f.type;
    const name = `${f.label}${f.unit ? ` (${f.unit})` : ""}${f.type === "datetime" ? " (UTC)" : ""}`
      + (f.required ? " *" : "");
    const title = f.required ? "Required" : "Optional: left out when empty";
    if (f.type === "check") return el("label", {class: "check", title}, input.node, name);
    // several inputs (check boxes, a table) are a group named by the field; one is its label's
    if (f.type === "multi" || f.type === "rows") {
      return el("div", {class: "form-field", role: "group", "aria-label": f.label, title},
        el("span", {text: name}), input.node);
    }
    return el("label", {title}, `${name} `, input.node);
  })];
}

// decimal degrees from ten-millionths, as RP-2 §8.4 writes them: seven places less trailing zeros
function degreesText(e7) {
  const fixed = (e7 / 1e7).toFixed(7).replace(/0+$/, "");
  return fixed.endsWith(".") ? `${fixed}0` : fixed;
}

// an integer with `scale` decimal places as a decimal, exactly ("1.25" for 125 and 2)
function scaledText(n, scale) {
  if (!scale) return String(n);
  const digits = String(Math.abs(n)).padStart(scale + 1, "0");
  return `${n < 0 ? "-" : ""}${digits.slice(0, -scale)}.${digits.slice(-scale)}`;
}

const two = (n) => String(n).padStart(2, "0");

// An input for one field or column of a form, as {node, get, set} (RP-2 §8.1). Text is sent as
// typed; a position, a number and a choice by name are read by the panel as their field's type,
// so what the operator types is checked once, there.
function fieldInput(f, fresh = true, compact = false) {
  const plain = (node, get, set) => ({node, get, set});
  if (f.type === "text") {
    const node = el("textarea", {rows: 5});
    if (f.max) node.maxLength = f.max;
    return plain(node, () => (node.value.replace(/\s+$/, "") || null),
      (v) => { node.value = v; });
  }
  if (f.type === "choice") {
    const node = el("select", {class: "narrow"}, el("option", {value: "", text: ""}),
      ...f.choices.map((c, i) => el("option", {value: String(i), text: c})));
    return plain(node, () => (node.value === "" ? null : Number(node.value)),
      (v) => { node.value = String(v); });
  }
  if (f.type === "check") {
    const node = el("input", {type: "checkbox"});
    return plain(node, () => (node.checked ? true : null), (v) => { node.checked = v === true; });
  }
  if (f.type === "date" || f.type === "time" || f.type === "datetime") {
    const node = el("input", {type: f.type === "datetime" ? "datetime-local" : f.type,
      title: "UTC"});
    const get = () => {
      if (!node.value) return null;
      if (f.type === "date") return Math.floor(Date.parse(`${node.value}T00:00:00Z`) / DAY_MS);
      if (f.type === "datetime") return Math.floor(Date.parse(`${node.value}:00Z`) / 60000);
      const [h, m] = node.value.split(":").map(Number);
      return h * 60 + m;
    };
    const set = (v) => {
      if (f.type === "date") node.value = new Date(v * DAY_MS).toISOString().slice(0, 10);
      else if (f.type === "datetime") node.value = new Date(v * 60000).toISOString().slice(0, 16);
      else node.value = `${two(Math.floor(v / 60))}:${two(v % 60)}`;
    };
    return plain(node, get, set);
  }
  if (f.type === "number") {
    const node = el("input", {type: "text", inputmode: f.scale ? "decimal" : "numeric",
      class: "narrow", title: f.scale ? `Up to ${f.scale} decimal places` : "A whole number"});
    return plain(node, () => (node.value.trim() || null),
      (v) => { node.value = scaledText(v, f.scale || 0); });
  }
  if (f.type === "multi") {
    const node = el("div", {class: "multi"});
    const boxes = f.choices.map((c, i) => {
      const b = el("input", {type: "checkbox"});
      b.dataset.bit = String(i);
      node.append(el("label", {class: "check"}, b, c));
      return b;
    });
    const get = () => {
      let bits = 0;
      for (const b of boxes) if (b.checked) bits += 2 ** Number(b.dataset.bit);
      return bits || null;
    };
    const set = (v) => {
      for (const b of boxes) b.checked = Math.floor(v / 2 ** Number(b.dataset.bit)) % 2 === 1;
    };
    return plain(node, get, set);
  }
  if (f.type === "position") return positionInput(fresh, compact);
  if (f.type === "rows") return rowsInput(f);
  const node = el("input", {type: "text"});
  if (f.max) node.maxLength = f.max;
  return plain(node, () => (node.value.trim() || null), (v) => { node.value = v; });
}

// A position: decimal degrees or a grid square, as typed; under it, where the one filled in came
// from and exactly what goes (RP-2 §8.4's text, from the panel). It goes as shown, to the
// ten-millionth of a degree: type fewer decimals or a grid square to send less.
function positionInput(fresh, compact) {
  const input = el("input", {type: "text", placeholder: "33.78123, -84.38765 or EM73",
    title: "Decimal degrees, north and east positive, or a grid square. It goes exactly as shown below: type fewer decimals or a grid square to send less"});
  const said = el("span", {class: "form-note"});
  const node = compact ? input : el("span", {class: "position"}, input, said);
  let from = "";
  const preview = async () => {
    if (compact) return;
    const text = input.value.trim();
    if (!text) {
      said.textContent = from;
      said.classList.remove("warn");
      return;
    }
    try {
      const v = await api("form-position", {text});
      said.textContent = `${from ? `${from} ` : ""}Goes as: ${v.shown}`;
      said.classList.remove("warn");
    } catch (e) {
      said.textContent = e.message;
      said.classList.add("warn");
    }
  };
  input.addEventListener("change", () => { from = ""; preview(); });
  if (fresh && !compact) {
    api("form-position").then((v) => {
      if (input.value) return;
      input.value = v.text;
      from = v.says;
      preview();
    }).catch(() => {});
  }
  const set = (v) => {
    const off = v[2] === undefined ? ""
      : v[2] < 1000 ? ` ±${v[2]} m` : ` ±${v[2] / 1000} km`;
    input.value = `${degreesText(v[0])}, ${degreesText(v[1])}${off}`;
    preview();
  };
  return {node, input, get: () => (input.value.trim() || null), set, here: () => from !== ""};
}

// Rows: a table of the columns' inputs, a row added or removed by its buttons, up to the field's
// max. Empty cells and rows are left out.
function rowsInput(f) {
  const body = el("tbody");
  const rows = [];
  const add = el("button", {type: "button", class: "quiet", text: "Add a row",
    title: f.max ? `Up to ${f.max} rows` : "Another row"});
  const addRow = () => {
    const cells = f.columns.map((c) => ({n: c.n, ...fieldInput(c, false, true)}));
    const tr = el("tr", {}, ...cells.map((c) => el("td", {}, c.node)));
    const row = {cells, tr};
    const remove = el("button", {type: "button", class: "quiet", text: "×", title: "Remove this row",
      onclick: () => {
        rows.splice(rows.indexOf(row), 1);
        tr.remove();
        add.disabled = false;
      }});
    tr.append(el("td", {}, remove));
    rows.push(row);
    body.append(tr);
    add.disabled = Boolean(f.max) && rows.length >= f.max;
    return row;
  };
  add.addEventListener("click", () => addRow());
  addRow();
  const head = el("thead", {}, el("tr", {}, ...f.columns.map((c) =>
    el("th", {text: `${c.label}${c.unit ? ` (${c.unit})` : ""}${c.required ? " *" : ""}`,
      title: c.required ? "Required in each row" : "Optional"})), el("th")));
  const node = el("div", {class: "rows"}, el("table", {class: "rows-table"}, head, body), add);
  const get = () => {
    const out = [];
    for (const r of rows) {
      const cells = {};
      for (const c of r.cells) {
        const v = c.get();
        if (v !== null) cells[c.n] = v;
      }
      if (Object.keys(cells).length) out.push(cells);
    }
    return out.length ? out : null;
  };
  const set = (v) => {
    rows.splice(0).forEach((r) => r.tr.remove());
    for (const cells of v) {
      const row = addRow();
      for (const c of row.cells) if (cells[c.n] !== undefined) c.set(cells[c.n]);
    }
    if (!rows.length) addRow();
  };
  return {node, get, set};
}

// what the form's fields hold: {template, values} by field number, empty fields left out
function formValues() {
  const template = $("c-form").value;
  if (!template) return null;
  const values = {};
  for (const input of formInputs) {
    const v = input.get();
    if (v !== null) values[input.n] = v;
  }
  return {template, values};
}

function newComposer() {
  const form = $("composer");
  form.reset();
  showFormFields();
  answering = null;
  $("c-title").textContent = "New message";
  $("c-submit").textContent = "Put in outbox";
  $("c-submit").title = "Put it in the outbox";
  $("cancel").textContent = "Discard";
  $("cancel").title = "Discard it";
  $("c-files").title = "The forwarded message's files: untick one to leave it out. Every byte goes on the air";
  $("c-files").replaceChildren();
  $("c-held").replaceChildren();
  $("c-original").hidden = true;
  return form;
}

// a file a forward or an edit carries, ticked: untick it to leave it out
function carriedFile(f, i) {
  const li = document.createElement("li");
  const label = document.createElement("label");
  label.className = "check";
  const box = document.createElement("input");
  box.type = "checkbox";
  box.checked = true;
  box.dataset.carry = String(i);
  label.append(box, `${f.name} (${f.type}, ${f.size} bytes)`);
  li.append(label);
  return li;
}

// a form's values, as the panel gives them, back in its fields (the inverse of formValues)
function fillForm(values) {
  for (const input of formInputs) {
    const v = values[String(input.n)];
    if (v !== undefined && v !== null) input.set(v);
  }
}

// Edit: a message in the outbox, not yet handed over, in the composer as it was written; saved,
// the changed message takes its place there. The panel says why when it cannot be changed: a
// session may be sending it, or a gateway holds it already. False when it cannot be edited.
async function edit(id) {
  if (popOut({how: "edit", id})) return true;
  let m;
  try {
    m = await api(`edit?id=${id}`);
  } catch (e) {
    toast(e.message, true);
    return false;
  }
  if (m.form && !templates.some((t) => t.template === m.form.template)) {
    toast("Its form is not one this computer holds, so it cannot be edited here.", true);
    return false;
  }
  const form = newComposer();
  answering = {how: "edit", id: m.id};
  $("c-title").textContent = "Edit message";
  $("c-submit").textContent = "Save in outbox";
  $("c-submit").title = "Put the changed message in the outbox, in place of the one it was";
  $("cancel").textContent = "Cancel";
  $("cancel").title = "Leave the message in the outbox as it was";
  form.to.value = m.to.length ? `${m.to.join(", ")}, ` : "";
  form.cc.value = m.cc.length ? `${m.cc.join(", ")}, ` : "";
  form.subject.value = m.subject;
  form.priority.value = String(m.priority);
  if (m.form) {
    $("c-form").value = m.form.template;
    showFormFields(false);
    fillForm(m.form.values);
  }
  form.body.value = m.body;
  $("c-files").title = "The message's files: untick one to leave it out. Every byte goes on the air";
  $("c-files").replaceChildren(...m.files.map(carriedFile));
  show("composer");
  form.subject.focus();
  return true;
}

// Reply, Reply all or Forward: the composer filled in from the message read (`id`). A reply
// quotes nothing (every byte goes on the air); the original shows under the composer for
// reference. False when the message cannot be answered.
async function answer(how, id = reading.id) {
  if (popOut({how, id})) return true;
  let a;
  try {
    a = await api(`answer?id=${encodeURIComponent(id)}&how=${how}`);
  } catch (e) {
    toast(e.message, true);
    return false;
  }
  const form = newComposer();
  answering = {how, id: a.original.id, replyTo: a.reply_to};
  $("c-title").textContent = ANSWER_TITLES[how];
  form.to.value = a.to.length ? `${a.to.join(", ")}, ` : "";
  form.cc.value = a.cc.length ? `${a.cc.join(", ")}, ` : "";
  form.subject.value = a.subject;
  form.body.value = how === "forward" ? `\n\n${a.body}` : "";
  $("c-files").replaceChildren(...a.files.map(carriedFile));
  $("c-held").replaceChildren(...a.held.map((f) => {
    const li = document.createElement("li");
    li.textContent = `Not included: ${f.name} (${f.type}, ${f.size} bytes), held at the post office`;
    if (a.can_ask) {
      const ask = document.createElement("button");
      ask.type = "button";
      ask.textContent = f.asked ? "Asked for" : "Ask on the next call";
      ask.disabled = f.asked;
      ask.title = "Ask for it on your next call to a gateway: it comes on the call after, in a new message you can forward";
      ask.addEventListener("click", async () => {
        try {
          await api("held", {id: a.original.id, names: [f.name]});
          ask.textContent = "Asked for";
          ask.disabled = true;
          toast(`${f.name} will be asked for on your next call to a gateway`);
        } catch (e) {
          toast(e.message, true);
        }
      });
      li.append(" ", ask);
    }
    return li;
  }));
  const o = a.original;
  $("c-original").hidden = how === "forward";
  $("c-original-head").textContent =
    `${o.from} wrote on ${new Date(o.date * 1000).toLocaleString()}: not sent, for reference`;
  // what the message shows as: a form's text (RP-2 §8.4), the sender's note below it
  $("c-original-body").textContent = o.text;
  show("composer");
  const first = how === "forward" ? form.to : form.body;
  first.focus();
  if (first === form.body) form.body.setSelectionRange(0, 0);
  return true;
}

async function openMessage(id, from = {folder}) {
  const m = await api(`message?id=${id}`);
  reading = {id, folder: from.folder, was: from.was};
  const trash = from.folder === "trash";
  // a reply answers a message received; anything may be forwarded
  const received = from.folder === "inbox" || (trash && from.was === "inbox");
  $("r-reply").hidden = $("r-reply-all").hidden = !received;
  $("r-delete").hidden = trash || !FOLDER_NAMES[from.folder];
  $("r-edit").hidden = from.folder !== "outbox";
  $("r-restore").hidden = !trash;
  $("r-purge").hidden = !trash;
  $("r-restore").title = trash ? `Put it back in the ${from.was}` : "";
  $("r-subject").textContent = m.subject || "(no subject)";
  $("r-from").textContent = m.from;
  // one address whatever the case of its letters (RP-0 §6.2): mail from an older program may
  // carry capitals, and the list keeps addresses in lowercase
  const known = addressBook.some((e) => e.address === addressOf(m.from));
  $("r-from-add").hidden = known || addressOf(m.from) === addressOf(meAddress);
  $("r-from-add").onclick = async () => {
    try {
      await changeAddresses({add: m.from});
      $("r-from-add").hidden = true;
      toast(`${m.from} is on your address list.`);
    } catch (e) {
      toast(e.message, true);
    }
  };
  $("r-to").textContent = m.to.join(", ");
  $("r-cc").textContent = m.cc.join(", ");
  $("r-cc").hidden = $("r-cc-t").hidden = !m.cc.length;
  $("r-date").textContent = new Date(m.date * 1000).toLocaleString();
  $("r-id").textContent = m.id;
  // each file saved in the Downloads folder, to open with the program of the operator's choice
  $("r-files").replaceChildren(...m.files.map((f, n) => {
    const li = el("li", {text: `${f.name} (${f.type}, ${f.size} bytes) `});
    const save = el("button", {type: "button", class: "link", text: "Save",
      title: "Save it as a file in your Downloads folder, to open with the program of your choice. A file already there is never written over"});
    save.addEventListener("click", async () => {
      try {
        const s = await api("save", {id, n});
        save.textContent = "Saved";
        li.title = s.path;
        toast(`Saved as ${s.name} in ${s.path.slice(0, s.path.length - s.name.length - 1)}`);
      } catch (e) {
        toast(e.message, true);
      }
    });
    li.append(save);
    return li;
  }));
  $("r-held").replaceChildren(...m.held.map((f) => {
    const li = document.createElement("li");
    li.textContent = `${f.name} (${f.type}, ${f.size} bytes), held at the post office `;
    const ask = document.createElement("button");
    ask.type = "button";
    ask.textContent = f.asked ? "Asked for" : "Ask on the next call";
    ask.disabled = f.asked;
    ask.title = "Ask for it on your next call to a gateway: it comes on the call after, in a new message";
    ask.addEventListener("click", async () => {
      try {
        await api("held", {id, names: [f.name]});
        ask.textContent = "Asked for";
        ask.disabled = true;
        toast(`${f.name} will be asked for on your next call to a gateway`);
      } catch (e) {
        toast(e.message, true);
      }
    });
    li.append(ask);
    return li;
  }));
  // a form shows as the form: its name, then each field filled in, its label beside its value, in
  // the template's order (RP-2 §8.4), made by the panel from the template this computer holds;
  // what the sender added below it is the message's text
  const f = m.form;
  $("r-form").hidden = !f;
  $("r-form-title").textContent = f ? f.title : "";
  $("r-form-title").title = f && !f.filled
    ? "This computer does not hold this form: its fields show by number" : "";
  $("r-form-get").hidden = !f || !!f.name;
  $("r-form-fetch").onclick = async () => {
    try {
      const t = await api("forms/fetch", {template: f.template, from: m.from});
      await loadForms();
      toast(`Got ${t.name}.`);
      await openMessage(id, from);
    } catch (e) {
      toast(e.message, true);
    }
  };
  $("r-form-rows").replaceChildren(...(f?.rows ?? []).flatMap((r) => {
    if (r.heading) return [el("dt", {class: "heading", text: r.label})];
    if (r.table) return [el("dt", {text: r.label}), el("dd", {}, formTable(r.table))];
    return [el("dt", {text: r.label}), el("dd", {class: r.lines ? "lines" : "", text: r.value})];
  }));
  $("r-body").textContent = m.body;
  $("r-body").hidden = !!f && !m.body;
  // printed, a form with a layout is the paper form it comes from; anything else, the message as
  // it shows here. The PDF's name is the form's and the subject
  const layout = f && f.filled && templates.find((t) => t.template === f.template)?.print;
  $("reader").classList.toggle("has-paper", !!layout);
  $("r-paper").replaceChildren(...(layout ? paper(m, layout) : []));
  printTitle = [f && f.filled ? f.title : "", m.subject || "Message"].filter(Boolean).join(" - ");
  show("reader");
}

// a form's rows field as a table: its columns' labels, then each row's cells (RP-2 §8.4)
function formTable(t) {
  return el("table", {class: "form-table"},
    el("thead", {}, el("tr", {}, ...t.columns.map((c) => el("th", {text: c})))),
    el("tbody", {}, ...t.rows.map((row) => el("tr", {}, ...row.map((c) => el("td", {text: c}))))));
}

// what the message read prints as, by its name: the name a PDF of it is offered
let printTitle = "RadioPost";

// A form as the paper form it comes from, for printing: the program's own layout
// (forms/*.print.html, compiled in, never anything from the air), each field's value put in its
// place as text, then what RadioPost knows of the message and the sender's note.
function paper(m, layout) {
  const t = document.createElement("template");
  t.innerHTML = layout;
  const rows = new Map(m.form.rows.map((r) => [String(r.n), r]));
  for (const place of t.content.querySelectorAll("[data-field]")) {
    const r = rows.get(place.dataset.field);
    if (place.tagName !== "TBODY") {
      place.textContent = r?.value ?? "";
      continue;
    }
    // a table's rows, then empty lines to the paper form's own number of them (data-rows)
    const columns = place.closest("table").querySelectorAll("thead th").length;
    const lines = r?.table?.rows ?? [];
    const blank = Math.max(0, Number(place.dataset.rows || 0) - lines.length);
    place.replaceChildren(
      ...lines.map((cells) => el("tr", {}, ...cells.map((c) => el("td", {text: c})))),
      ...Array.from({length: blank}, () =>
        el("tr", {}, ...Array.from({length: columns}, () => el("td")))));
  }
  const when = `${new Date(m.date * 1000).toISOString().slice(0, 16).replace("T", " ")} UTC`;
  const out = [t.content, el("p", {class: "paper-line",
    text: `Sent by RadioPost from ${m.from} to ${[...m.to, ...m.cc].join(", ")}, ${when}. Message ${m.id}.`})];
  if (m.files.length) {
    out.push(el("p", {class: "paper-line", text: `Attachments: ${m.files.map((x) => x.name).join(", ")}.`}));
  }
  if (m.body) out.push(el("p", {class: "paper-line", text: "The sender's note:"}), el("pre", {class: "paper-note", text: m.body}));
  return out;
}

// The system's print dialog, where Save as PDF makes a PDF. In the app (`radiopostPrints`, set by
// the app) the window prints itself: macOS's webview does nothing at window.print(), so the page
// goes to /print instead, which the app stops and takes as the word to print (app/src-tauri).
function printMessage() {
  document.title = printTitle;
  addEventListener("afterprint", () => { document.title = "RadioPost"; }, {once: true});
  if (window.radiopostPrints) location.assign("/print");
  else window.print();
}

function fileData(file) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result).split(",", 2)[1] || "");
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
}

async function send(event) {
  event.preventDefault();
  const form = $("composer");
  const files = await Promise.all([...form.files.files].map(async (f) => ({
    name: f.name, data: await fileData(f),
  })));
  const message = {
    to: form.to.value, cc: form.cc.value, subject: form.subject.value,
    priority: Number(form.priority.value), body: form.body.value, files,
  };
  const filled = formValues();
  if (filled) message.form = filled;
  if (answering?.replyTo) message.reply_to = answering.replyTo;
  const carried = () => [...$("c-files").querySelectorAll("input[data-carry]")]
    .filter((b) => b.checked).map((b) => Number(b.dataset.carry));
  if (answering?.how === "forward") {
    message.forward = answering.id;
    message.carry = carried();
    // the note written above the message forwarded, without the blank lines left before it
    message.body = message.body.replace(/^\s*\n/, "");
  }
  const edited = answering?.how === "edit";
  if (edited) {
    message.edit = answering.id;
    message.carry = carried();
  }
  try {
    await api("compose", message);
    newComposer();
    if (ALONE) {
      // the panel's window does the rest, as below
      windowsChannel?.postMessage({composed: true, edited});
      closeWindow();
      return;
    }
    await written(false, edited);
  } catch (e) {
    toast(e.message, true);
  }
}

// a message just put in the outbox, or changed there (`edited`), by this window's composer or by
// one in a window of its own (`elsewhere`): the address list read again, the folders counted, and
// the Outbox shown, unless this window was busy with something else meanwhile
async function written(elsewhere = false, edited = false) {
  loadAddresses();
  toast(edited ? "Changed: it waits in the outbox in the old one's place."
               : "In the outbox: the next session carries it.");
  await refresh();
  if (!elsewhere || !$("list-view").hidden || !$("reader").hidden) await openFolder("outbox");
}

function discard() {
  newComposer();
  if (ALONE) closeWindow();
  else openFolder(folder);
}

// ── the composer in a window of its own (stacked layouts) ───────────────────────────────────
// Each window is asked for by what it holds: Write's new message, an answer to a message, or a
// message to an address. Asked for again while it is open, it comes forward with what was typed,
// as the panel keeps a message half written. A window refused (a pop-up blocker) leaves the
// composer here. The window's page is this one: the same composer, the same API. What it puts in
// the outbox it tells the panel's window over a BroadcastChannel, and closes.

const windowsChannel = "BroadcastChannel" in window ? new BroadcastChannel("radiopost") : null;
const composerWindows = new Map();

function composersOpen() {
  return [...composerWindows.values()].some((w) => !w.closed);
}

// the composer in a window of its own, opened on `start` ({how, id} or {to}), when the panel is
// stacked; false when it opens here
function popOut(start) {
  if (ALONE || !STACKED.matches) return false;
  const query = new URLSearchParams({compose: start.how || "new"});
  if (start.id) query.set("id", start.id);
  if (start.to) query.set("to", start.to);
  const key = query.toString();
  const open = composerWindows.get(key);
  if (open && !open.closed) {
    bringForward(open);
    return true;
  }
  const w = window.open(`/?${key}`, "_blank", "popup,width=780,height=760");
  if (!w) {
    toast("No window of its own could open, so the message is written here: allow pop-ups for the panel.", true);
    return false;
  }
  composerWindows.set(key, w);
  return true;
}

// A browser's pop-up closes itself and comes forward when asked. A window of the desktop app's
// (`radiopostApp`, set by the app) does neither on every system, so its page is sent to /closed or
// /front instead, which the app stops and takes as the word to close the window or bring it
// forward (app/src-tauri): the page stays as it was.
function closeWindow(w = window) {
  if (w.radiopostApp) w.location.replace("/closed");
  else w.close();
}

function bringForward(w) {
  if (w.radiopostApp) w.location.assign("/front");
  else w.focus();
}

// the composer windows editing a message, closed, the message staying as it was; true if any were
function closeEdits() {
  let any = false;
  for (const [key, w] of composerWindows) {
    if (key.startsWith("compose=edit&") && !w.closed) {
      closeWindow(w);
      any = true;
    }
  }
  return any;
}

function wireComposer() {
  wireSuggestions($("composer").to);
  wireSuggestions($("composer").cc);
  $("cancel").addEventListener("click", discard);
  $("composer").addEventListener("submit", send);
  $("c-form").addEventListener("change", () => showFormFields());
}

// the page as a composer window: the composer alone, on what the panel's window asked for
async function startAlone() {
  wireComposer();
  await loadAddresses();
  await loadForms();
  const q = new URLSearchParams(location.search);
  const how = q.get("compose");
  const on = how === "edit" ? edit : ANSWER_TITLES[how] ? (id) => answer(how, id) : null;
  if (on && !await on(q.get("id"))) {
    // what could not be opened is said where it was asked for, as there
    windowsChannel?.postMessage({said: $("toast-text").textContent});
    closeWindow();
    return;
  }
  if (!on) {
    const form = newComposer();
    form.to.value = q.get("to") || "";
    show("composer");
    (form.to.value ? form.subject : form.to).focus();
  }
  document.title = `${$("c-title").textContent} · RadioPost`;
}

function adapter() {
  const value = $("adapter").value.trim();
  try { localStorage.setItem("radiopost.adapter", value); } catch { /* not kept */ }
  return value;
}

// the Modem box, told to the panel when the page opens and when it changes: between calls the app
// holds a connection to Aether that names it there (app/client/src/presence.rs). Quiet: a panel
// that cannot take it says nothing here.
function tellModem() {
  api("modem", {adapter: adapter()}).catch(() => { /* the next change or call tells it */ });
}

// A call. A P2P call to a listed gateway's callsign is not placed: the panel says whom to call
// instead, and the toast's button calls that one (on 2026-11-23 Aether advised the portable to call
// the gateway KK4ODA-1 for P2P; such a call reaches the gateway, which takes no P2P calls).
async function placeCall(body) {
  try {
    const result = await api("call", body);
    const refused = result && result.not_a_station;
    if (refused) {
      const instead = refused.call;
      toast(`${refused.why}.`, true, instead ? {
        label: `Call ${instead}`,
        run: async () => {
          $("callsign").value = instead;
          await placeCall({...body, callsign: instead});
        },
      } : null);
      return;
    }
    await refresh();
  } catch (e) {
    toast(e.message, true);
  }
}

async function act(path, body) {
  try {
    await api(path, body);
    await refresh();
  } catch (e) {
    toast(e.message, true);
  }
}

function showAccept(entries) {
  $("accept").replaceChildren(...entries.map((e) => {
    const li = document.createElement("li");
    const span = document.createElement("span");
    span.textContent = e;
    const b = document.createElement("button");
    b.className = "quiet";
    b.textContent = "Remove";
    b.title = `Take ${e} off the list`;
    b.addEventListener("click", () => changeAccept({remove: [e]}));
    li.append(span, b);
    return li;
  }));
}

async function changeAccept(change) {
  try {
    showAccept((await api("accept", change)).entries);
  } catch (e) {
    toast(e.message, true);
  }
}

// Usual gateways (RP-5 §6.3, §5.8), picked from the gateway list kept here, and whether mail read
// in a mail program is skipped by radio: the settings at the post office, merged there.
let usualChosen = new Set();
// Save sends both settings whole, so it waits until they have been read from the post office:
// saved before, it would put an empty list in place of the usual gateways kept there.
let usualReading = 0;

async function loadUsual() {
  const reading = ++usualReading;
  const ready = (yes) => {
    $("usual-save").disabled = !yes;
    $("skip-read").disabled = !yes;
  };
  ready(false);
  if (!meAddress) {
    usualChosen = new Set();
    $("usual").replaceChildren();
    $("usual-state").textContent = "Read from your post office once this computer is enrolled.";
    return;
  }
  $("usual-state").textContent = "Reading your settings from your post office…";
  try {
    const [settings, list] = await Promise.all([api("settings"), api("gateways")]);
    if (reading !== usualReading) return;
    usualChosen = new Set(settings.usual_gateways || []);
    $("skip-read").checked = settings.radio_skips_read === true;
    // only listed gateways: the post office places copies at none of the operator's own
    const listed = list.gateways.filter((g) => !g.own);
    const known = new Map(listed.map((g) => [g.callsign, g]));
    const names = [...new Set([...listed.map((g) => g.callsign), ...usualChosen])];
    $("usual").replaceChildren(...(names.length ? names : [null]).map((call) => {
      if (call === null) {
        return el("li", {class: "note",
          text: "No gateway list yet: refresh it in Gateways while you have the Internet."});
      }
      const g = known.get(call);
      const box = el("input", {type: "checkbox"});
      box.checked = usualChosen.has(call);
      box.addEventListener("change", () => {
        if (box.checked) usualChosen.add(call); else usualChosen.delete(call);
      });
      const holds = g && g.copies && Object.keys(g.copies).length;
      const note = !g ? "not in the list kept here"
        : holds ? `holds copies, ${g.copies.days} days` : "holds no copies";
      const where = g ? [g.place, g.post_office].filter(Boolean).join(", ") : "";
      return el("li", {}, el("label", {class: "check", title: note},
        box, el("span", {class: "mono", text: call}),
        el("span", {class: "note", text: [where, note].filter(Boolean).join(" · ")})));
    }));
    $("usual-state").textContent = "";
    ready(true);
  } catch (e) {
    if (reading !== usualReading) return;
    usualChosen = new Set();
    $("usual").replaceChildren();
    $("usual-state").textContent =
      `Your settings could not be read (${e.message}): Read again with the Internet to change them.`;
  }
}

async function saveUsual() {
  try {
    await api("settings", {usual_gateways: [...usualChosen],
                           radio_skips_read: $("skip-read").checked});
    toast("Saved at your post office.");
  } catch (e) {
    toast(e.message, true);
  }
}

// The Setup's enrollment card: once this computer is enrolled it says so in place of the form,
// which comes back on "Enroll again" or when the credential has run out.
let enrollOpen = false;

function showEnrollment(me) {
  const day = (t) => new Date(t * 1000).toLocaleDateString(undefined,
    {year: "numeric", month: "short", day: "numeric"});
  const expired = Boolean(me) && me.expires * 1000 <= Date.now();
  const box = $("enrolled");
  box.hidden = !me;
  box.classList.toggle("expired", expired);
  if (me) {
    $("enrolled-who").textContent = expired
      ? `The credential for ${me.address} (${me.operator}) ran out on ${day(me.expires)}.`
      : `Enrolled as ${me.address} (${me.operator}).`;
    $("enrolled-until").textContent = expired
      ? "Enroll again with a new code from your post office's operator."
      : `Credential good until ${day(me.expires)}. RadioPost renews it by itself in its last month, over the Internet or on a gateway call.`;
  }
  const pending = application.status === "pending";
  const open = (!me || expired || enrollOpen) && !pending;
  $("enroll-title").textContent = me && !expired ? "Enrolled with your post office" : "Enroll with your post office";
  $("enroll-fields").hidden = !open;
  $("enroll-again").hidden = open;
  showJoin();
  const form = $("enroll");
  if (me && open && !form.contains(document.activeElement)) {
    if (!form.address.value) form.address.value = me.address;
    if (!form.operator.value) form.operator.value = me.operator;
  }
}

async function enroll(event) {
  event.preventDefault();
  const form = $("enroll");
  const applying = joinBy() === "apply";
  try {
    const r = applying
      ? await api("apply", {
        address: addressOf(form.address.value), operator: form.operator.value,
        name: form.name.value,
        contact: form.contact.value, url: form.url.value,
      })
      : await api("enroll", {
        address: addressOf(form.address.value), operator: form.operator.value,
        code: form.code.value,
        url: form.url.value,
      });
    form.reset();
    enrollOpen = false;
    toast(r.said);
    await refresh();
  } catch (e) {
    toast(e.message, true);
  }
}

// ── joining: a code from the operator, or an application they decide (RP-5 §6.1, §6.7) ──────

function hexToB64u(hex) {
  const bytes = hex.match(/../g).map((h) => parseInt(h, 16));
  return btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function joinBy() {
  const checked = document.querySelector("input[name=join]:checked");
  return checked ? checked.value : "code";
}

function showJoin() {
  const applying = joinBy() === "apply";
  const form = $("enroll");
  for (const l of form.querySelectorAll(".by-code")) l.hidden = applying;
  for (const l of form.querySelectorAll(".by-apply")) l.hidden = !applying;
  form.code.required = !applying;
  form.name.required = applying;
  form.contact.required = applying;
  $("join-note").textContent = applying
    ? "Ask your post office for an account. Its operators check your licence and decide; this client takes the credential when they approve. Over the Internet."
    : "Your post office's operator gives you a one-time code once they have seen your licence. Over the Internet.";
  $("enroll-go").textContent = applying ? "Apply" : ($("enrolled").hidden ? "Enroll" : "Enroll again");
  $("enroll-go").title = applying
    ? "Make this device's key if needed and send the application, signed with it"
    : "Make this device's key if needed, enroll it, and keep the credential";
}

const application = {status: "", checked: 0};

function showApplication(app) {
  const box = $("applied");
  box.hidden = !app;
  if (!app) { application.status = ""; return; }
  box.dataset.status = app.status;
  const who = `${app.address} (${app.operator})`;
  const what = {
    pending: `Applied for ${who}.`,
    declined: `The application for ${who} was declined.`,
    postcard: `A code for ${who} is coming by post.`,
    forgotten: `The post office no longer knows the application for ${who}.`,
  }[app.status] || `Applied for ${who}.`;
  $("applied-what").textContent = what;
  $("applied-note").textContent = {
    pending: "Waiting for the post office's operators. This client asks every minute while it is open, and takes the credential once they approve.",
    declined: app.text ? `They said: ${app.text}` : "They gave no reason.",
    postcard: "It goes to the address on your licence. When it comes, choose “I have a code” and enroll with it.",
    forgotten: "Applications are kept 30 days. Apply again.",
  }[app.status] || "";
  $("applied-check").hidden = app.status !== "pending" && app.status !== "postcard";
  const form = $("enroll");
  $("enroll-fields").hidden = app.status === "pending";
  if (app.status !== "pending" && !$("enroll-fields").contains(document.activeElement)) {
    // a postcard's code goes in the code form; a declined or forgotten one may apply again
    const by = app.status === "postcard" ? "code" : "apply";
    if (joinBy() !== by && application.status !== app.status) {
      document.querySelector(`input[name=join][value=${by}]`).checked = true;
      showJoin();
    }
    form.address.value = form.address.value || app.address;
    form.operator.value = form.operator.value || app.operator;
    form.url.value = form.url.value || app.base || "";
    if (by === "apply") {
      form.name.value = form.name.value || app.name || "";
      form.contact.value = form.contact.value || app.contact || "";
    }
  }
  if (application.status && application.status !== app.status && app.status !== "pending") {
    toast(what);
  }
  application.status = app.status;
  if (app.status === "pending" && Date.now() - application.checked > 60000) {
    application.checked = Date.now();
    checkApplication(false);
  }
}

async function checkApplication(loud = true) {
  try {
    const r = await api("application", {});
    if (loud || r.status === "approved") toast(r.said);
    await refresh();
    if (r.status === "approved") await probeOffice();
  } catch (e) {
    if (loud) toast(e.message, true);
  }
}

// ── the post office's desk, for its operators (RP-5 §7) ───────────────────────────────────────

const office = {ok: false, tab: "applications", status: null, applications: [], accounts: []};

// whether version `a` comes before `b` (SemVer 2.0.0 §11: 0.1.0-beta.12 before 0.1.0-beta.13,
// before 0.1.0); a version that does not parse comes before nothing
function versionBefore(a, b) {
  const parts = (v) => {
    const s = String(v).replace(/^v/, "").split("+")[0];
    const dash = s.indexOf("-");
    return {core: (dash < 0 ? s : s.slice(0, dash)).split(".").map(Number),
            pre: dash < 0 ? [] : s.slice(dash + 1).split(".")};
  };
  const x = parts(a), y = parts(b);
  for (let i = 0; i < 3; i++) {
    if (x.core[i] !== y.core[i]) return x.core[i] < y.core[i];
  }
  if (!x.pre.length || !y.pre.length) return x.pre.length > 0 && !y.pre.length;
  for (let i = 0; i < Math.max(x.pre.length, y.pre.length); i++) {
    const p = x.pre[i], q = y.pre[i];
    if (p === undefined) return true;
    if (q === undefined) return false;
    if (p === q) continue;
    const pn = /^\d+$/.test(p), qn = /^\d+$/.test(q);
    if (pn && qn) return Number(p) < Number(q);
    return pn !== qn ? pn : p < q;
  }
  return false;
}

// a desk request an older post office does not know is answered `unknown-message` (it answers
// any route it lacks that way), which reads as if a message were missing: say what it is.
// Not for a request naming an application, where the answer means no such application.
function olderOffice(path, body, answer) {
  const s = office.status || {};
  if (answer.name !== "unknown-message" || !s.version || !myVersion) return "";
  if (path.startsWith("/office/applications/") || (body && body.application)) return "";
  if (!versionBefore(s.version, myVersion)) return "";
  const domain = (s.domains || [])[0] || "The post office";
  return `${domain} does not know this request: it runs radiopost-po ${s.version}, `
    + `older than this client (${myVersion}). Update it with the Server tab's update job.`;
}

// one request to the post office as this device: its body, or the refusal it gave
async function desk(method, path, body) {
  const r = await api("office", {method, path, body});
  if (r.status >= 400) {
    const b = r.body || {};
    const err = new Error(b.operates
      ? `${b.operates} operates this post office (RP-5 §7.1).`
      : olderOffice(path, body, b)
        || (b.title ? `${b.title} (${b.name})` : `The post office answered ${r.status}`));
    err.status = r.status;
    err.body = b;
    throw err;
  }
  return r.body;
}

// whether this client's credential runs its post office: the tab shows only then
async function probeOffice() {
  try {
    office.status = await desk("GET", "/office/status");
    office.ok = true;
  } catch {
    office.ok = false;
  }
  $("office-open").hidden = !office.ok;
  if (office.ok) {
    const n = office.status.applications || 0;
    $("n-office").textContent = n ? String(n) : "";
    $("o-n-applications").textContent = n ? String(n) : "";
    $("o-n-mail").textContent = office.status.outgoing ? String(office.status.outgoing) : "";
    const fails = office.status.health ? office.status.health.fails : 0;
    $("o-n-health").textContent = fails ? String(fails) : "";
    $("o-n-others").textContent = office.status.seen ? String(office.status.seen) : "";
  }
  return office.ok;
}

function officeSummary() {
  const s = office.status || {};
  $("o-domain").textContent = (s.domains || []).join(", ");
  if (s.domains && s.domains[0]) $("o-code").address.placeholder = `ana@${s.domains[0]}`;
  if (!$("o-ssh").value && s.domains && s.domains[0]) $("o-ssh").placeholder = `root@po.${s.domains[0]}`;
  const parts = [`${s.accounts ?? 0} accounts`, `${s.applications ?? 0} waiting`,
                 `${s.codes ?? 0} codes out`, `${s.sessions ?? 0} sessions open`];
  if (s.outgoing) parts.push(`${s.outgoing} going out by email`);
  if (s.version) parts.push(`radiopost-po ${s.version}`);
  if (s.update && s.update.offer) parts.push(`${s.update.offer.version} is out`);
  $("o-summary").textContent = parts.join("  ·  ");
}

async function openOffice(tab = office.tab) {
  office.tab = tab;
  show("office");
  for (const b of document.querySelectorAll("[data-otab]")) {
    b.setAttribute("aria-selected", String(b.dataset.otab === tab));
  }
  for (const pane of document.querySelectorAll(".o-pane")) pane.hidden = pane.id !== `o-${tab}`;
  try {
    await probeOffice();
    officeSummary();
    if (tab === "applications") await renderApplications();
    if (tab === "accounts") await renderAccounts();
    if (tab === "gateways") await renderGateways();
    if (tab === "mail") await renderMail();
    if (tab === "others") await renderOthers();
    if (tab === "settings") await renderOfficeSettings();
    if (tab === "health") await renderHealth();
    if (tab === "server") await renderServer();
  } catch (e) {
    toast(e.message, true);
  }
}

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") node.className = v;
    else if (k === "text") node.textContent = v;
    else if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
    else node.setAttribute(k, v);
  }
  node.append(...children.filter((c) => c !== null && c !== undefined));
  return node;
}

function day(t) {
  return new Date(t * 1000).toLocaleString(undefined,
    {year: "numeric", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit"});
}

// what the post office found of the callsign, when it says (radiopost-po's own members)
function evidenceBlock(a) {
  const e = a.evidence;
  if (!e) return null;
  const box = el("div", {class: `evidence ${e.verdict || ""}`, title: "What the post office found of this callsign. A licence database shows a callsign exists and in whose name, which anyone can read: it is evidence, not proof."});
  for (const line of e.lines || []) box.append(el("p", {text: line}));
  for (const flag of e.flags || []) box.append(el("p", {class: "flag", text: flag}));
  return box;
}

const DECLINE_REASONS = [
  "No licence in the licence database matches this callsign.",
  "The name does not match the licence for this callsign.",
  "The licence for this callsign has expired.",
  "Please ask for an address with your own name or callsign.",
];

// whether a credential naming `call` would run this post office (RP-5 §7.1)
function operatesHere(call) {
  return ((office.status && office.status.operators) || []).includes(call);
}

function applicantCard(a) {
  const theirs = operatesHere(a.operator);
  const role = el("input", {type: "checkbox"});
  const approve = el("button", {class: "primary", text: "Approve",
    title: "Make the account and issue its credential; the applicant's client takes it at once",
    onclick: () => decide(a, {decision: "approve", flags: role.checked ? ["send", "receive", "role"] : undefined})});
  const why = el("input", {placeholder: "Why, in a sentence the applicant reads", list: "decline-reasons"});
  const send = el("button", {text: "Send", title: "Decline with this reason",
    onclick: () => {
      if (!why.value.trim()) { toast("Say why, for the applicant.", true); why.focus(); return; }
      decide(a, {decision: "decline", text: why.value.trim()});
    }});
  const whyRow = el("div", {class: "decline-why", hidden: ""}, why, send);
  const decline = el("button", {class: "quiet danger", text: "Decline…", title: "Refuse it, saying why",
    onclick: () => { whyRow.hidden = !whyRow.hidden; if (!whyRow.hidden) why.focus(); }});
  const postcard = el("button", {class: "quiet", text: "Send a postcard",
    title: "Make a code good for 30 days, to send by post to the address on the licence: proof that the applicant lives where the licence says",
    onclick: () => decide(a, {decision: "postcard"})});
  const warning = theirs
    ? el("div", {class: "operates"},
        el("p", {}, el("strong", {text: `${a.operator} operates this post office.`})),
        el("p", {text: "An application naming an operator's callsign can only be declined: approved, it would give its holder this desk. An operator adds a device of their own with New code."}))
    : null;
  return el("article", {class: "applicant"},
    el("div", {class: "applicant-head"},
      el("span", {class: "applicant-name", text: a.name}),
      el("span", {class: "applicant-call", text: a.operator, title: "The callsign, as licensed"}),
      el("span", {class: "applicant-when", text: `asked ${day(a.received)}`})),
    el("dl", {class: "fields"},
      el("dt", {text: "Address"}), el("dd", {class: "mono", text: a.address}),
      el("dt", {text: "Email now"}), el("dd", {class: "mono", text: a.contact})),
    warning,
    evidenceBlock(a) || el("p", {class: "note", text: "Looking the callsign up\u2026"}),
    el("div", {class: "row"}, theirs ? null : approve,
      theirs ? null : el("label", {class: "check", title: "A shared mailbox, used by several licensed operators"}, role, " Role mailbox"),
      decline, theirs ? null : postcard,
      el("button", {class: "quiet", text: "Look up again", title: "Ask the FCC copy and QRZ again",
        onclick: async () => {
          try { await desk("POST", "/office/lookup", {application: a.application}); await renderApplications(); }
          catch (e) { toast(e.message, true); }
        }}),
      whyRow));
}

async function renderApplications() {
  const list = (await desk("GET", "/office/applications")).applications || [];
  office.applications = list;
  const pending = list.filter((a) => a.status === "pending");
  const decided = list.filter((a) => a.status !== "pending");
  $("o-none").hidden = pending.length > 0;
  $("o-pending").replaceChildren(...pending.map(applicantCard));
  $("o-decided-box").hidden = decided.length === 0;
  $("o-decided").replaceChildren(...decided.reverse().map((a) => {
    const decision = a.status === "postcard"
      ? el("span", {}, el("span", {class: "pill postcard", text: "postcard"}), " ",
          el("span", {class: "mono", text: a.code || ""}),
          a.code ? el("button", {class: "link", type: "button", text: "Print the card",
            title: "A card to post to the address on the licence", onclick: () => printCard(a)}) : null)
      : el("span", {class: `pill ${a.status}`, text: a.status, title: a.text || ""});
    const after = [];
    if (a.mailbox) {
      after.push(a.mailbox.error
        ? el("span", {class: "pill declined", text: "no mailbox", title: a.mailbox.error})
        : el("span", {class: "pill approved", text: "mailbox", title: a.mailbox.made}));
    }
    if (a.told) {
      after.push(a.told.error
        ? el("span", {class: "pill declined", text: "not emailed", title: a.told.error})
        : el("span", {class: "pill approved", text: "emailed", title: `Told at ${a.told.to}`}));
    }
    return el("tr", {},
      el("td", {text: a.name}), el("td", {class: "mono", text: a.operator}),
      el("td", {class: "mono", text: a.address}), el("td", {}, decision),
      el("td", {}, ...after), el("td", {text: day(a.decided)}));
  }));
}

async function decide(a, body) {
  try {
    const entry = await desk("POST", `/office/applications/${a.application}`, body);
    const said = {
      approved: `${a.address} approved: ${a.name}'s client takes the credential at its next check.`,
      declined: `${a.address} declined.`,
      postcard: `Postcard code ${entry.code} for ${a.address}: print the card and post it.`,
    }[entry.status];
    toast(said || entry.status);
    if (entry.status === "postcard") printCard(entry);
    await openOffice("applications");
  } catch (e) {
    toast(e.message, true);
  }
}

// a card to print and post to the address on the licence: the code, and what to do with it
function printCard(a) {
  const w = window.open("", "_blank", "width=640,height=480");
  if (!w) { toast("The browser would not open the card: allow pop-ups for the panel.", true); return; }
  const until = a.code_expires ? new Date(a.code_expires * 1000).toLocaleDateString() : "";
  const mailTo = (a.evidence && a.evidence.mail_to) || [];
  const esc = (s) => String(s || "").replace(/[&<>"]/g, (c) => ({"&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;"}[c]));
  w.document.write(`<!doctype html><meta charset="utf-8"><title>RadioPost postcard for ${esc(a.operator)}</title>
<style>body{font:15px system-ui,sans-serif;margin:24px}.card{border:1px dashed #888;border-radius:10px;padding:24px;max-width:520px}
.code{font:600 34px ui-monospace,monospace;letter-spacing:.1em;margin:12px 0}h1{font-size:20px;margin:0 0 8px}p{margin:6px 0}
.to{margin:18px 0 0 260px;font:16px ui-monospace,monospace;text-transform:uppercase}.small{font:12px system-ui,sans-serif;color:#666;text-transform:none}
@media print{button{display:none}}</style>
<div class="card"><h1>RadioPost: your enrollment code</h1>
<p>${esc(a.name)}, ${esc(a.operator)}</p>
<p>For the account <b>${esc(a.address)}</b>.</p>
<p class="code">${esc(a.code)}</p>
<p>In the RadioPost client: Setup, “I have a code”, then your address, your callsign and this code.</p>
<p>Good until ${esc(until)}. It works once.</p></div>
${mailTo.length ? `<div class="to"><p class="small">Mail to, as on the licence:</p><p>${mailTo.map(esc).join("<br>")}</p></div>` : `<p class="small">No address on file here: send it to the address on the licence.</p>`}
<p><button onclick="print()">Print</button></p>`);
  w.document.close();
}

async function renderAccounts() {
  office.accounts = (await desk("GET", "/office/accounts")).accounts || [];
  drawAccounts();
}

function drawAccounts() {
  const find = $("o-filter").value.trim().toLowerCase();
  const shown = office.accounts.filter((a) => !find || a.address.includes(find)
    || (a.name || "").toLowerCase().includes(find)
    || a.credentials.some((c) => c.operator.toLowerCase().includes(find)));
  $("o-account-rows").replaceChildren(...shown.map((a) => {
    const name = el("td", {}, el("span", {text: a.name || ""}),
      el("button", {class: "link", type: "button", text: a.name ? "Rename" : "Name it",
        title: "The name on the email this account sends", onclick: () => rename(a, name)}));
    const devices = el("td", {}, ...a.credentials.map((c) => {
      const mine = c.device === myDevice;
      return el("div", {class: `dev${c.revoked ? " revoked" : ""}`},
        el("span", {class: "mono", text: c.operator}),
        el("span", {text: `until ${new Date(c.expires * 1000).toLocaleDateString()}`,
          title: `Credential ${c.serial}, device key ${c.device}`}),
        mine ? el("span", {class: "pill", text: "this computer", title: "The device you are using now"}) : null,
        c.revoked ? el("span", {text: "revoked"}) : el("button", {class: "quiet danger", type: "button", text: "Revoke",
          title: mine
            ? "This is the computer you are using: revoked, it can no longer run the post office from here"
            : "Revoke every credential of this device, as when it is lost: it can no longer log in or call",
          onclick: (e) => armed(e.currentTarget, mine ? "Click again: your own computer" : "Click again to revoke",
            () => revoke(c))}));
    }));
    if (!a.credentials.length) devices.append(el("span", {class: "note", text: "No device yet"}));
    const mailbox = el("td", {});
    if (a.mailbox === true) {
      mailbox.append(el("span", {text: "Yes", title: "The mail server has its mailbox"}), " ",
        el("button", {class: "link", type: "button", text: "New password",
          title: "For a user who lost their password: set a new one in the mail server, shown once; the old one stops working",
          onclick: (e) => armed(e.currentTarget, "Click again: the old one stops working",
            () => newPassword(a, mailbox))}));
    }
    if (a.mailbox === false) {
      mailbox.append(el("span", {class: "pill declined", text: "none",
          title: "The mail server has no mailbox for it, so email from outside is refused"}),
        el("button", {class: "link", type: "button", text: "Make mailbox",
          title: "Make its mailbox in the mail server now; its password is shown once",
          onclick: () => makeMailbox(a, mailbox)}));
    }
    // an address may break after its @ and nowhere else
    const at = a.address.indexOf("@") + 1;
    const address = el("td", {class: "mono"}, a.address.slice(0, at), el("wbr"), a.address.slice(at));
    return el("tr", {}, address, name, devices,
      el("td", {class: "n", text: String(a.waiting || 0)}), mailbox);
  }));
}

// a password for mail programs, shown once (the post office keeps none) and never to go by radio
function shownOnce(lead, password) {
  const pw = el("span", {class: "mono", text: password});
  return [el("p", {class: "note"}, lead, pw, " ",
    el("button", {class: "quiet", type: "button", text: "Copy", title: "Copy the password",
      onclick: async () => {
        try { await navigator.clipboard.writeText(password); toast("Copied."); }
        catch { toast("Select the password and copy it.", true); }
      }})),
    el("p", {class: "note", text: "It is not kept, and it is shown only now. Give it to them off the air, never in a message by radio."})];
}

// what became of a mailbox made for an account: its password, or why none was made
function mailboxSaid(m) {
  if (!m) return [];
  if (!m.password) {
    return [el("p", {class: "note bad", text: `No mailbox made: ${m.error || "the mail server did not say why"}. Email from outside is refused for ${m.address} until it has one.`})];
  }
  return shownOnce(`Mailbox made for ${m.address}. Its password for mail programs: `, m.password);
}

// what became of a new password for an account's mailbox, or why none was set
function passwordSaid(m) {
  if (!m.password) {
    return [el("p", {class: "note bad", text: `No new password set: ${m.error || "the mail server did not say why"}.`})];
  }
  return shownOnce(`New password for ${m.address}; the old one no longer works: `, m.password);
}

async function makeMailbox(a, cell) {
  try {
    const m = await desk("POST", "/office/mailbox", {address: a.address});
    if (m.password) a.mailbox = true;
    cell.replaceChildren(...mailboxSaid(m));
  } catch (e) {
    toast(e.message, true);
  }
}

async function newPassword(a, cell) {
  try {
    cell.replaceChildren(...passwordSaid(await desk("POST", "/office/password", {address: a.address})));
  } catch (e) {
    toast(e.message, true);
  }
}

function rename(a, cell) {
  const input = el("input", {value: a.name || "", placeholder: "Ana Lopez"});
  const save = el("button", {type: "button", text: "Save", onclick: async () => {
    try {
      await desk("POST", "/office/name", {address: a.address, name: input.value.trim()});
      await renderAccounts();
    } catch (e) { toast(e.message, true); }
  }});
  cell.replaceChildren(el("div", {class: "row"}, input, save));
  input.focus();
}

async function revoke(c) {
  try {
    const r = await desk("POST", "/office/revoke", {device: c.device});
    toast(`Revoked credential${r.revoked.length === 1 ? "" : "s"} ${r.revoked.join(", ")}.`);
    await renderAccounts();
  } catch (e) {
    toast(e.message, true);
  }
}

async function makeCode(event) {
  event.preventDefault();
  const form = $("o-code");
  const body = {address: form.address.value.trim().toLowerCase(),
                operator: form.operator.value.trim().toUpperCase(),
                flags: form.role.checked ? ["send", "receive", "role"] : ["send", "receive"]};
  if (form.name.value.trim()) body.name = form.name.value.trim();
  await sendCode(body);
}

// RP-5 §7.5: a code that would make its holder an operator is made only when asked for in so
// many words, so the post office refuses it first and the desk asks
async function sendCode(body) {
  const form = $("o-code");
  $("o-operates").hidden = true;
  try {
    const r = await desk("POST", "/office/codes", body);
    $("o-made").hidden = false;
    $("o-made-code").textContent = r.code;
    $("o-made-for").textContent = `For ${r.address} (${r.operator}), good until ${new Date(r.expires * 1000).toLocaleString()}.`
      + (r.operates ? ` ${r.operator} operates this post office: whoever enrolls with this code can use this desk.` : "");
    $("o-made-mailbox").replaceChildren(...mailboxSaid(r.mailbox));
    form.reset();
  } catch (e) {
    if (e.body && e.body.operates && !body.operates) {
      askOperates(body, e.body.operates);
      return;
    }
    toast(e.message, true);
  }
}

function askOperates(body, call) {
  const box = $("o-operates");
  box.replaceChildren(
    el("p", {}, el("strong", {text: "This person will be able to run the post office."})),
    el("p", {text: `A code for ${body.address} naming ${call} gives whoever enrolls with it this desk: applications, accounts, settings, backups and the server's jobs. ${call} is one of the post office's operators.`}),
    el("div", {class: "row"},
      el("button", {type: "button", class: "danger", text: "Make it anyway",
        title: "Make the code: its holder becomes an operator",
        onclick: () => sendCode({...body, operates: true})}),
      el("button", {type: "button", class: "quiet", text: "Cancel", title: "Make nothing",
        onclick: () => { box.hidden = true; }})));
  box.hidden = false;
  $("o-made").hidden = true;
}

// ── the gateways registered here (RP-5 §6.4), and their withdrawal (§7.7) ─────────────────────

async function renderGateways() {
  const list = (await desk("GET", "/office/gateways")).gateways || [];
  $("o-gateways-none").hidden = list.length > 0;
  $("o-gateway-rows").replaceChildren(...list.map((g) => {
    const live = (g.credentials || []).filter((c) => !c.revoked);
    const button = el("button", {class: "quiet danger", type: "button", text: "Unregister",
      title: `Take ${g.callsign} off the gateway list, revoke its credential and drop the copies placed there`,
      onclick: (e) => armed(e.currentTarget, "Click again to unregister", () => unregisterGateway(g))});
    return el("tr", {},
      el("td", {}, el("span", {class: "mono", text: g.callsign}),
        el("span", {class: "note", text: ` · ${g.operator || "?"}`})),
      el("td", {class: "mono", text: g.address || "—",
        title: live.length ? `Credential ${live.map((c) => c.serial).join(", ")}` : "No credential in force"}),
      el("td", {text: (g.record && g.record.place) || ""}),
      el("td", {text: `${g.placed || 0}${g.last_poll ? `, asked ${when(g.last_poll)}` : ""}`}),
      el("td", {class: "acts"}, button));
  }));
}

async function unregisterGateway(g) {
  const path = `/office/gateways/${encodeURIComponent(g.callsign)}`;
  try {
    const r = await desk("DELETE", path);
    const n = (r.revoked || []).length;
    toast(`${r.callsign} is no longer registered here${n ? `; credential${n === 1 ? "" : "s"} ${r.revoked.join(", ")} revoked` : ""}.`);
  } catch (e) {
    const gone = e.status === 404 && !olderOffice(path, null, e.body || {});
    toast(gone ? `${g.callsign} is not registered here any more.` : e.message, true);
  }
  await renderGateways();
}

async function renderMail() {
  const list = (await desk("GET", "/office/outgoing")).outgoing || [];
  $("o-mail-none").hidden = list.length > 0;
  $("o-mail-rows").replaceChildren(...list.map((m) => el("tr", {},
    el("td", {class: "mono", text: m.id}), el("td", {class: "mono", text: (m.to || []).join(", ")}),
    el("td", {text: m.queued ? day(m.queued) : ""}))));
}

// ── other post offices: known, seen, blocked (RP-1 §7.5) ──────────────────────────────────────

const metHow = (h) => ({door: "at the door", outgoing: "written to", credential: "a credential"}[h]
  || (h.startsWith("list:") ? `in ${h.slice(5)}'s list` : h));

async function decidePost(body) {
  try {
    await renderOthers(await desk("POST", "/office/post-offices", body));
  } catch (e) {
    toast(e.message, true);
  }
}

function postButton(text, title, body, sure) {
  const b = el("button", {type: "button", text, title});
  b.addEventListener("click", (e) => {
    if (sure) armed(e.currentTarget, sure, () => decidePost(body));
    else decidePost(body);
  });
  return b;
}

async function renderOthers(given) {
  const v = given || await desk("GET", "/office/post-offices");
  const who = (p) => [p.operator, p.country].filter(Boolean).join(", ") || "\u2014";
  const signed = (p) => (p.api ? (p.dnssec ? "signed" : "not signed") : "");
  const name = (p, trouble) => el("td", {},
    el("span", {class: "mono", text: p.domain}),
    p.contact ? el("div", {class: "note", text: p.contact}) : null,
    p.error ? el("div", {class: "trouble", text: `${trouble}: ${p.error}`}) : null);
  const known = v.known || [];
  $("o-known").replaceChildren(...known.map((p) => el("tr", {},
    name(p, "The last fetch failed, and what was held is kept"),
    el("td", {"data-label": "Run by", text: who(p)}),
    el("td", {"data-label": "Fetched", text: p.found ? day(p.found) : p.typed ? "its key typed, not fetched yet" : "not yet"}),
    el("td", {"data-label": "DNSSEC", text: signed(p)}),
    el("td", {class: "acts"},
      postButton("Forget", "Know it no more; it may be met and seen again", {forget: p.domain}, "Click again to forget it"),
      postButton("Block", "Know it no more, and do not show it as seen", {block: p.domain}, "Click again to block it")))));
  if (!known.length) {
    $("o-known").append(el("tr", {}, el("td", {class: "note", colspan: "5",
      text: "None yet: your users' mail reaches other post offices as ordinary email."})));
  }
  const seen = v.seen || [];
  $("o-seen").replaceChildren(...seen.map((p) => el("tr", {},
    name(p, "Not all could be found or checked"),
    el("td", {"data-label": "Run by", text: who(p)}),
    el("td", {"data-label": "Met", text: `${(p.how || []).map(metHow).join(", ")}; ${day(p.last)}`}),
    el("td", {"data-label": "DNSSEC", text: signed(p)}),
    el("td", {class: "acts"},
      postButton("Know", "Take its users as licensed: their mail past the accept lists, unflagged on the air", {accept: p.domain}, "Click again to know it"),
      postButton("Block", "Do not show it again", {block: p.domain})))));
  if (!seen.length) {
    $("o-seen").append(el("tr", {}, el("td", {class: "note", colspan: "5", text: "None met yet."})));
  }
  const looking = v.looking_up || [];
  $("o-seen-note").textContent = "Post offices met at the door, in mail your users sent, on a credential, or in a known post office's list."
    + (looking.length ? ` Being looked up: ${looking.join(", ")}.` : "");
  const blocked = v.blocked || [];
  $("o-blocked").replaceChildren(...blocked.map((d) => el("tr", {},
    el("td", {class: "mono", text: d}),
    el("td", {class: "acts"}, postButton("Unblock", "It may be met and seen again", {unblock: d})))));
  if (!blocked.length) $("o-blocked").append(el("tr", {}, el("td", {class: "note", text: "None."})));
  $("o-n-others").textContent = seen.length ? String(seen.length) : "";
}

async function knowPost() {
  const domain = $("o-know").value.trim().toLowerCase();
  if (!domain) return;
  try {
    await renderOthers(await desk("POST", "/office/post-offices", {accept: domain}));
    $("o-know").value = "";
    toast(`${domain} known: its records are being fetched`);
  } catch (e) {
    toast(e.message, true);
  }
}

// ── the server's care: its checks, its jobs, its backups ─────────────────────────────────────

async function renderHealth(fresh = false) {
  if (fresh) $("o-health-when").textContent = "Checking\u2026";
  const h = await desk(fresh ? "POST" : "GET", "/office/health", fresh ? {} : undefined);
  const lines = h.lines || [];
  const fails = lines.filter((l) => l.verdict === "fail").length;
  $("o-health-when").textContent = `${fails ? `${fails} to fix` : "All pass"}. Checked ${h.at ? day(h.at) : "never"}; again every hour, and you are emailed when one turns red.`;
  $("o-checks").replaceChildren(...lines.map((l) => el("li", {class: l.verdict},
    el("span", {class: "mark", text: {ok: "\u2713", fail: "\u2717", warn: "!", note: "\u2022"}[l.verdict] || "\u2022"}),
    el("span", {text: l.text}))));
  $("o-n-health").textContent = fails ? String(fails) : "";
  const s = await desk("GET", "/office/settings");
  if (!$("o-dkim").matches(":focus") && s.dkim) $("o-dkim").value = s.dkim.join(", ");
}

let jobsTimer = null;

function runButton(job) {
  const b = el("button", {type: "button", text: "Run", title: `Ask the server for this: ${job.what}`});
  b.disabled = Boolean(job.asked || job.running);
  b.addEventListener("click", (e) => {
    const go = async () => {
      try { await desk("POST", `/office/jobs/${job.name}`, {}); await renderServer(); }
      catch (err) { toast(err.message, true); }
    };
    if (["update", "restart", "restart-mail"].includes(job.name)) armed(e.currentTarget, "Click again to run it", go);
    else go();
  });
  return b;
}

async function renderServer() {
  const j = await desk("GET", "/office/jobs");
  $("o-runner").textContent = j.runner
    ? "The server runs these as its administrator, from a fixed list: nothing else can be asked for."
    : "The jobs runner is not installed on the server, so these wait. Once, on the server: sudo radiopost-po --dir /var/lib/radiopost jobs --install";
  $("o-jobs").replaceChildren(...(j.jobs || []).map((job) => {
    const last = job.last;
    const state = job.running ? "running now\u2026" : job.asked ? "asked for, waiting for the runner\u2026"
      : last ? `${last.ok ? "done" : "failed"} ${day(last.finished)}${last.by ? `, asked by ${last.by}` : ""}` : "";
    const out = last && last.output ? el("details", {}, el("summary", {text: "What it said"}), el("pre", {text: last.output})) : null;
    return el("div", {class: "job"},
      el("div", {class: "job-head"},
        el("span", {class: "job-what"}, el("b", {text: job.name}), job.what),
        runButton(job)),
      state ? el("p", {class: "job-last", text: state}) : null, out);
  }));
  clearTimeout(jobsTimer);
  if ((j.jobs || []).some((x) => x.asked || x.running) && office.tab === "server" && !$("office").hidden) {
    jobsTimer = setTimeout(() => renderServer().catch(() => {}), 2000);
  }
  const b = await desk("GET", "/office/backups");
  const row = (x, what) => el("tr", {title: what},
    el("td", {class: "mono", text: x.name}), el("td", {class: "n", text: kb(x.bytes)}), el("td", {text: day(x.at)}));
  $("o-backups").replaceChildren(
    ...(b.backups || []).map((x) => row(x, "The post office: its key, settings, state and held attachments")),
    ...(b.mail || []).map((x) => row(x, "The mail server's mailboxes, accounts, passwords and DKIM keys, copied with Stalwart stopped")));
  if (!(b.backups || []).length && !(b.mail || []).length) $("o-backups").append(el("tr", {}, el("td", {class: "note", colspan: "3", text: "None yet: the first is made tonight."})));
  const d = b.downloaded;
  $("o-downloaded").textContent = d
    ? `The last copy off the server: ${day(d.at)}, ${d.to ? `fetched by ${d.to}` : `downloaded by ${d.by}`}, ${d.mail ? "with the mailboxes" : "without the mailboxes"}.`
    : "None downloaded yet: a backup that stays on the server is lost with it.";
  await renderAutoBackups(b.keys || []);
  await renderDkim();
}

// the DKIM keys rotated by hand, one checked step at a time (the post office's dkim.rs)
async function renderDkim() {
  const buttons = ["o-dkim-start", "o-dkim-switch", "o-dkim-removed", "o-dkim-cancel"];
  let v;
  try {
    v = await desk("GET", "/office/dkim");
  } catch (e) {
    $("o-dkim-state").textContent = e.message;
    $("o-dkim-table").hidden = true;
    $("o-dkim-records").replaceChildren();
    for (const id of buttons) $(id).hidden = true;
    return;
  }
  const keys = v.keys || [];
  $("o-dkim-table").hidden = !keys.length;
  $("o-dkim-keys").replaceChildren(...keys.map((k) => el("tr", {title: `Stalwart's ID ${k.id}`},
    el("td", {class: "mono", text: k.selector}), el("td", {text: k.algorithm === "rsa" ? "RSA" : "Ed25519"}),
    el("td", {text: k.stage}), el("td", {text: k.created ? day(Date.parse(k.created) / 1000) : ""}))));
  const r = v.rotation;
  const record = (rec, verb) => {
    const copy = el("button", {type: "button", class: "quiet", text: "Copy",
      title: "Copy the record's value, to paste into your DNS provider's TXT record"});
    copy.addEventListener("click", async () => {
      try { await navigator.clipboard.writeText(rec.value); toast("Copied."); } catch { toast("Select the text and copy it.", true); }
    });
    return el("div", {class: "dkim-record"},
      el("p", {}, el("b", {text: `${verb} `}), el("span", {class: "mono", text: rec.name}), " TXT"),
      el("div", {class: "row"}, el("input", {class: "grow mono", value: rec.value, readonly: "", "aria-label": `${rec.name} TXT`}), copy));
  };
  const state = {
    start: "No rotation under way. The hourly check turns amber when the keys are older than "
      + `${v.rotate_after_days} days.`,
    publish: "Publish these records in your DNS, then Switch: the switch waits until each is visible. "
      + "Most DNS panels want the name without the domain (the part ending in ._domainkey).",
    wait: r && r.remove_after ? `The new keys sign. Keep the old records published until ${day(r.remove_after)}; the hourly check says when.` : "",
    remove: "Remove these records from your DNS, then End the rotation: it checks they are gone.",
  }[v.next] || "";
  $("o-dkim-state").textContent = state;
  $("o-dkim-records").replaceChildren(...(v.next === "publish" ? (r.publish || []).map((x) => record(x, "Publish"))
    : v.next === "remove" ? (r.unpublish || []).map((x) => record(x, "Remove")) : []));
  $("o-dkim-start").hidden = v.next !== "start";
  $("o-dkim-switch").hidden = v.next !== "publish";
  $("o-dkim-cancel").hidden = v.next !== "publish";
  $("o-dkim-removed").hidden = v.next !== "remove";
}

async function dkimStep(step, button) {
  button.disabled = true;
  try {
    await desk("POST", "/office/dkim", {step});
    toast({start: "New keys made: publish their records.", switch: "Switched: the new keys sign.",
           removed: "The rotation is over.", cancel: "Stopped: the new keys were taken back."}[step]);
    await renderDkim();
  } catch (e) {
    toast(e.message, true);
  } finally {
    button.disabled = false;
  }
}

// the backups this computer fetches by itself (wishlist W38), and the computers the post office
// sends them to
async function renderAutoBackups(keys) {
  const a = await api("backups");
  const f = a.fetched;
  $("o-auto-state").textContent = a.on
    ? `On: while this app is open, ${a.computer || "this computer"} fetches each new backup into ${a.folder}. `
      + (f ? `Last fetched ${day(f.at)}: ${f.name}, ${kb(f.bytes)}${f.mail ? ", with the mailboxes" : ""}.` : "Nothing fetched yet.")
    : "Off. Turned on, this computer fetches each new backup while the app is open, sealed to a key only it holds: nothing secret is kept on the server.";
  if (!$("o-auto-computer").matches(":focus") && !$("o-auto-computer").value) $("o-auto-computer").value = a.computer || "";
  if (!$("o-auto-folder").matches(":focus") && !$("o-auto-folder").value) $("o-auto-folder").value = a.folder || "";
  $("o-auto-computer").disabled = $("o-auto-folder").disabled = Boolean(a.on);
  $("o-auto").textContent = a.on ? "Turn off" : "Turn on";
  $("o-auto").title = a.on
    ? "Stop fetching: the post office forgets this computer. The backups already here stay, and still open here"
    : "While this app is open, fetch each new backup the server makes, sealed to a key only this computer holds";
  $("o-auto-now").hidden = !a.on;
  $("o-auto-key").hidden = !a.on && !f;
  $("o-auto-key").textContent = `What opens them is this computer's key, ${a.key_file}: keep a copy of it somewhere else, or turn this on on a second computer of yours too. To open one: radiopost backups open FILE.`;
  $("o-keys-table").hidden = !keys.length;
  $("o-keys").replaceChildren(...keys.map((k) => {
    const forget = el("button", {type: "button", class: "quiet", text: "Forget",
      title: "The post office stops sealing backups to this computer: do this for a computer lost or given away"});
    forget.addEventListener("click", (e) => armed(e.currentTarget, "Click again to forget it", async () => {
      try { await desk("DELETE", `/office/backup-keys/${k.id}`); await renderServer(); }
      catch (err) { toast(err.message, true); }
    }));
    const mine = k.id === a.key && a.on;
    return el("tr", {title: `Key ${k.id}`},
      el("td", {text: `${k.label}${mine ? " (this one)" : ""}`}),
      el("td", {text: `${day(k.added_at)} by ${k.added_by}`}),
      el("td", {text: k.fetched_at ? `${day(k.fetched_at)}, ${kb(k.fetched_bytes)}` : "not yet"}),
      el("td", {}, forget));
  }));
}

async function toggleAutoBackups() {
  const on = $("o-auto").textContent === "Turn on";
  $("o-auto").disabled = true;
  try {
    await api("backups", on
      ? {on: true, computer: $("o-auto-computer").value.trim(), folder: $("o-auto-folder").value.trim()}
      : {on: false});
    toast(on ? "Turned on: the newest backup is on its way." : "Turned off: the backups already here stay.");
    // the first fetch takes a moment
    await renderServer();
    if (on) setTimeout(() => renderServer().catch(() => {}), 3000);
  } catch (e) {
    toast(e.message, true);
  } finally {
    $("o-auto").disabled = false;
  }
}

// what a Claude session needs to help with the server: no mail, no names, no keys
async function forClaude() {
  const [status, health, jobs, settings] = await Promise.all([
    desk("GET", "/office/status"), desk("GET", "/office/health"),
    desk("GET", "/office/jobs"), desk("GET", "/office/settings")]);
  const out = [];
  out.push(`# RadioPost post office ${(status.domains || []).join(", ")}: the server, for a Claude session`);
  out.push("");
  out.push(`radiopost-po ${status.version || "?"}${status.update && status.update.offer ? ` (${status.update.offer.version} is out)` : ""}. ` +
    "The guide it was set up by: docs/user/post-office.md in KK4ODA/RadioPost (Debian, Stalwart behind Caddy).");
  out.push(`Accounts ${status.accounts}, applications waiting ${status.applications}, codes out ${status.codes}, ` +
    `sessions open ${status.sessions}, email waiting to go out ${status.outgoing ?? 0}.`);
  out.push(`Applications ${settings.applications}; FCC copy ${settings.fcc && settings.fcc.on ? `on (${settings.fcc.count} callsigns)` : "off"}; ` +
    `QRZ ${settings.qrz && settings.qrz.username ? "on" : "off"}; mailboxes ${(settings.mailboxes && settings.mailboxes.kind) || "by hand"}; ` +
    `submission port ${settings.smtp ? "set" : "not set"}; DKIM selectors ${(settings.dkim || []).join(", ") || "none given"}.`);
  out.push("");
  out.push(`## Checks (${health.at ? new Date(health.at * 1000).toISOString() : "never"})`);
  for (const l of health.lines || []) out.push(`${l.verdict.toUpperCase().padEnd(4)}  ${l.text}`);
  out.push("");
  out.push(`## Jobs (runner ${jobs.runner ? "installed" : "NOT installed"})`);
  for (const j of jobs.jobs || []) {
    if (!j.last) continue;
    out.push(`### ${j.name}: ${j.last.ok ? "done" : "FAILED"} ${new Date(j.last.finished * 1000).toISOString()}`);
    const lines = (j.last.output || "").trimEnd().split("\n");
    out.push("```", ...lines.slice(-60), "```");
  }
  out.push("");
  out.push("What I need help with: ");
  return out.join("\n");
}

async function downloadBackup() {
  const p1 = $("o-pass1").value;
  if (p1.length < 12) { toast("A passphrase of 12 characters or more.", true); return; }
  if (p1 !== $("o-pass2").value) { toast("The two passphrases differ.", true); return; }
  $("o-backup").disabled = true;
  try {
    const r = await api("office-backup", {passphrase: p1});
    $("o-pass1").value = "";
    $("o-pass2").value = "";
    toast(`Kept on this computer${r.mail ? ", with the mailboxes" : ""}: ${r.path}. Keep the passphrase somewhere safe.`);
    renderServer().catch(() => {});
  } catch (e) {
    toast(e.message, true);
  } finally {
    $("o-backup").disabled = false;
  }
}

function showMailboxKind() {
  const kind = $("o-mb-kind").value;
  $("o-mb-stalwart").hidden = kind !== "stalwart";
  $("o-mb-command").hidden = kind !== "command";
}

async function renderOfficeSettings() {
  const s = await desk("GET", "/office/settings");
  const f = s.fcc || {};
  $("o-fcc").checked = Boolean(f.on);
  $("o-fcc-note").textContent = f.fetching
    ? "Fetching the FCC's file and building the copy: a minute or two."
    : f.count
      ? `${Number(f.count).toLocaleString()} callsigns, copied ${new Date(f.made * 1000).toLocaleDateString()}. A new copy comes each week.${f.last ? ` Last fetch: ${f.last}.` : ""}`
      : f.on ? "No copy yet." : "";
  $("o-fcc-fetch").disabled = Boolean(f.fetching) || !f.on;
  const m = s.mailboxes || {};
  if (!$("o-mb-kind").matches(":focus")) $("o-mb-kind").value = m.kind || "";
  if (!$("o-mb-url").matches(":focus")) $("o-mb-url").value = m.url || "";
  if (!$("o-mb-cmd").matches(":focus")) $("o-mb-cmd").value = (m.command || []).join(" ");
  $("o-mb-key").placeholder = m.key ? "Key kept: type to change it" : "";
  showMailboxKind();
  $("o-mb-note").textContent = s.smtp
    ? "The welcome goes out through the mail server's submission port."
    : "No submission port (smtp) in config.json: no email can go out, the welcome included.";
  if (!$("o-qrz-user").matches(":focus")) $("o-qrz-user").value = (s.qrz && s.qrz.username) || "";
  $("o-qrz-pass").placeholder = s.qrz && s.qrz.password ? "Password kept: type to change it" : "Password";
  $("o-take").checked = s.applications === "open";
  $("o-take-note").textContent = s.applications === "open"
    ? "Licensees can apply from the RadioPost client. Each application waits here for you."
    : "Closed: only the codes you make let anyone in.";
  const facts = [["Domains", (s.domains || []).join(", ")], ["Operators", (s.operators || []).join(", ")],
                 ["Contact", s.contact], ["API", s.base_url]];
  $("o-facts").replaceChildren(...facts.flatMap(([k, v]) => [el("dt", {text: k}), el("dd", {class: "mono", text: v || ""})]));
}

// ── alerts between stations (RP-8, W52) and how things sound (W51) ──────────────────────────────
// Sounds are made here, by Web Audio, on the output chosen in Setup. Their settings are the
// panel's (notify.json), so they hold whatever port the panel runs on. A heard alert sounds every
// 20 seconds until it is acknowledged; mail sounds by its sender's level.

const LEVELS = ["silent", "normal", "important", "time-sensitive"];
const LEVEL_NAMES = {silent: "Silent", normal: "Normal", important: "Important",
                     "time-sensitive": "Time-sensitive"};
const rank = (level) => Math.max(0, LEVELS.indexOf(level));
const higher = (a, b) => (rank(a) >= rank(b) ? a : b);

// each sound: its notes, [hertz, start s, length s, wave]
const TONES = {
  soft: [[880, 0, 0.16, "sine"]],
  chime: [[660, 0, 0.24, "sine"], [880, 0.16, 0.3, "sine"]],
  falling: [[660, 0, 0.24, "sine"], [440, 0.16, 0.3, "sine"]],
  bell: [[1320, 0, 0.9, "sine"], [2640, 0, 0.45, "sine"], [1980, 0.02, 0.4, "triangle"]],
  triple: [[784, 0, 0.12, "sine"], [988, 0.12, 0.12, "sine"], [1175, 0.24, 0.22, "sine"]],
  alarm: [[960, 0, 0.17, "square"], [720, 0.2, 0.17, "square"], [960, 0.4, 0.17, "square"],
          [720, 0.6, 0.17, "square"], [960, 0.8, 0.17, "square"], [720, 1.0, 0.17, "square"]],
  low: [[330, 0, 0.35, "triangle"], [262, 0.3, 0.45, "triangle"]],
  none: [],
};
const TONE_NAMES = {soft: "Soft note", chime: "Chime", falling: "Falling", bell: "Bell",
                    triple: "Three notes", alarm: "Alarm", low: "Low", none: "None"};
// what sounds, in Setup's order, with its sound unless the operator chose another
const SOUND_EVENTS = [
  ["alert", "An alert for you, time-sensitive", "alarm"],
  ["alert-quiet", "An alert at another level", "bell"],
  ["answered", "Your alert heard, seen or answered", "triple"],
  ["mail", "Mail", "soft"],
  ["mail-important", "Important mail", "bell"],
  ["mail-urgent", "Time-sensitive mail", "alarm"],
  ["session-up", "A session starts", "chime"],
  ["session-down", "A session ends", "falling"],
  ["failed", "A call or an alert fails", "low"],
  ["delivered", "A message delivered", "soft"],
  ["favourite", "A favourite station heard again after three hours", "none"],
];
const ALARM_EVERY_MS = 20000;

let notifySettings = {};
let audio = null;

function sounds() {
  notifySettings.sounds = notifySettings.sounds || {};
  return notifySettings.sounds;
}

function soundOf(event) {
  const chosen = (sounds().events || {})[event];
  return chosen in TONES ? chosen : (SOUND_EVENTS.find(([e]) => e === event) || [])[2] || "none";
}

// the browser plays nothing before the operator's first click or key, so the context is made, or
// woken, then; a sound before it is lost, which the window coming forward makes up for
function wakeAudio() {
  try {
    if (!audio) {
      audio = new (window.AudioContext || window.webkitAudioContext)();
      routeAudio();
    }
    if (audio.state === "suspended") audio.resume().catch(() => {});
  } catch { audio = null; }
}

function canRoute() {
  return typeof AudioContext !== "undefined" && "setSinkId" in AudioContext.prototype;
}

function routeAudio() {
  if (!audio || !canRoute()) return;
  const id = (sounds().device || {}).id || "";
  audio.setSinkId(id).catch(() => audio.setSinkId("").catch(() => {}));
}

function quietNow() {
  const q = notifySettings.quiet || {};
  if (!q.on || !q.from || !q.to) return false;
  const minutes = (t) => { const [h, m] = t.split(":").map(Number); return h * 60 + m; };
  const d = new Date();
  const now = d.getHours() * 60 + d.getMinutes();
  const from = minutes(q.from), to = minutes(q.to);
  return from <= to ? now >= from && now < to : now >= from || now < to;
}

// plays an event's sound; in quiet hours only what is time-sensitive, and a test always
function play(event, {level = "normal", test = false} = {}) {
  const s = sounds();
  if (!test && (s.on === false || (quietNow() && level !== "time-sensitive"))) return;
  const notes = TONES[soundOf(event)] || [];
  if (!notes.length) return;
  wakeAudio();
  if (!audio) return;
  const volume = (s.volume ?? 60) / 100;
  const t0 = audio.currentTime + 0.03;
  for (const [hz, at, length, wave] of notes) {
    const osc = audio.createOscillator();
    const gain = audio.createGain();
    osc.type = wave;
    osc.frequency.value = hz;
    const peak = Math.max(0.3 * volume * (wave === "square" ? 0.35 : 1), 0.0002);
    gain.gain.setValueAtTime(0.0001, t0 + at);
    gain.gain.exponentialRampToValueAtTime(peak, t0 + at + 0.012);
    gain.gain.exponentialRampToValueAtTime(0.0001, t0 + at + length);
    osc.connect(gain).connect(audio.destination);
    osc.start(t0 + at);
    osc.stop(t0 + at + length + 0.05);
  }
}

// the system's own notification, when the window is behind others and the operator asked
function notifySystem(title, body) {
  if (!notifySettings.notify || !document.hidden || !("Notification" in window)) return;
  if (Notification.permission !== "granted") return;
  try { new Notification(title, {body, tag: "radiopost"}); } catch { /* not shown */ }
}

let bannerAct = null;
function showBanner(text, open, alarm = false) {
  $("banner-text").textContent = text;
  $("banner").classList.toggle("alert", alarm);
  bannerAct = open;
  $("banner-open").hidden = !open;
  $("banner").hidden = false;
}

async function saveNotify() {
  try {
    notifySettings = await api("notify", notifySettings);
  } catch (e) {
    toast(e.message, true);
  }
}

// ── how mail arrives: its sender's level ───────────────────────────────────────────────────────

// an address as RadioPost writes it (RP-0 §6.2): the one in angle brackets if any, its ASCII
// capitals made small and nothing else
function addressOf(from) {
  const m = /<([^>]+)>/.exec(from || "");
  return (m ? m[1] : from || "").trim().replace(/[A-Z]/g, (c) => c.toLowerCase());
}

function mailLevels() {
  notifySettings.mail = notifySettings.mail || {};
  const levels = notifySettings.mail.levels || {};
  // an address's level kept before addresses were written in lowercase is found all the same
  for (const key of Object.keys(levels)) {
    const lower = key.startsWith("address:") ? `address:${addressOf(key.slice(8))}` : key;
    if (lower !== key) {
      if (!(lower in levels)) levels[lower] = levels[key];
      delete levels[key];
    }
  }
  notifySettings.mail.levels = levels;
  return levels;
}

// an address's own level, else the highest of its groups', else the default; Emergency brings it
// up to Important; only a level set by the operator reaches Time-sensitive
function mailLevel(from, priority) {
  const address = addressOf(from);
  const levels = mailLevels();
  let set = levels[`address:${address}`] || null;
  for (const g of groupBook) {
    const l = levels[`group:${g.name}`];
    if (l && g.members.some((m) => m.address === address)) set = set ? higher(set, l) : l;
  }
  let level = set || notifySettings.mail.default || "normal";
  if (!set && level === "time-sensitive") level = "important";
  if (priority >= 2) level = higher(level, "important");
  return level;
}

// a select of the four levels, with Default first, for an address or a group in Addresses
function levelSelect(key, title) {
  const levels = mailLevels();
  const select = el("select", {class: "level", title, "aria-label": title});
  select.append(new Option("Default", ""), ...LEVELS.map((l) => new Option(LEVEL_NAMES[l], l)));
  select.value = levels[key] || "";
  select.addEventListener("click", (event) => event.stopPropagation());
  select.addEventListener("change", async () => {
    if (select.value) levels[key] = select.value;
    else delete levels[key];
    await saveNotify();
  });
  const td = el("td", {class: "level-cell"}, select);
  td.addEventListener("click", (event) => event.stopPropagation());
  return td;
}

let inboxSeen = null;  // every inbox ID this page has seen
let urgentMail = null;  // a time-sensitive message not yet opened: it sounds as an alarm does

// new mail after a session: its sound, toast, banner or alarm, by the highest level among it. The
// inbox also grows when a message comes back from the trash: then the IDs are only noted.
async function checkMail(fromSession) {
  const rows = await api("folder?name=inbox");
  const first = inboxSeen === null;
  inboxSeen = inboxSeen || new Set();
  const fresh = rows.filter((m) => !inboxSeen.has(m.id));
  for (const m of rows) inboxSeen.add(m.id);
  if (first || !fromSession || !fresh.length) return;
  let top = null;
  for (const m of fresh) {
    const level = mailLevel(m.from, m.priority);
    if (!top || rank(level) > rank(top.level)) top = {level, m};
  }
  const {level, m} = top;
  const text = fresh.length === 1
    ? `Mail from ${m.from}: ${m.subject || "(no subject)"}`
    : `${fresh.length} new messages, one from ${m.from}: ${m.subject || "(no subject)"}`;
  const open = () => { $("banner").hidden = true; urgentMail = null; openMessage(m.id, {folder: "inbox"}); };
  if (level === "silent") return;
  notifySystem(fresh.length === 1 ? `Mail from ${m.from}` : `${fresh.length} new messages`,
               m.subject || "");
  if (level === "normal") {
    play("mail");
    toast(text);
  } else if (level === "important") {
    play("mail-important", {level});
    showBanner(text, open);
  } else {
    urgentMail = m;
    play("mail-urgent", {level});
    showBanner(text, open, true);
    keepAlarming();
  }
}

// ── the alerts kept by the panel ───────────────────────────────────────────────────────────────

let alertsSeq = -1;
let alertsKnown = null;  // each alert's state and answers when last read, to tell what is new
let alertsLog = [];
let alertCircles = [];
let alertDials = {radio: null, scanned: []};  // the radio's dial, and the dials this station's gateway scans
let alertGateways = [];  // the gateway list, for an alert's Dial (RP-8 §7)

const OUT_STATES = {waiting: "waiting for the modem", sent: "on the air", heard: "heard",
                    seen: "seen", replied: "answered", "not heard": "not heard",
                    failed: "not sent"};

async function syncAlerts(summary, force = false) {
  if (!force && summary && summary.seq === alertsSeq && alertsKnown) return;
  const data = await api("alerts");
  alertsSeq = data.seq;
  alertCircles = data.circles;
  alertsLog = data.log;
  alertDials = {radio: data.radio_hz ?? null, scanned: data.scanned_hz || []};
  circleRepeat = data.repeat || null;
  const before = alertsKnown;
  alertsKnown = new Map(data.log.map((e) => [e.tag, {state: e.state, answers: e.answers.length}]));
  if (before) {
    for (const e of data.log) told(e, before.get(e.tag));
  }
  $("n-alerts").textContent = data.log.filter((e) => !e.out && e.state === "new").length || "";
  showAlarm();
  if (!$("alerts-view").hidden) renderAlerts();
}

// what changed in one alert since it was last read, said once
function told(e, was) {
  if (!e.out && !was) {
    if (e.alarm) {
      play("alert", {level: "time-sensitive"});
      notifySystem(`Alert from ${e.from}`, e.text);
    } else if (!e.quiet && e.level !== "silent") {
      play("alert-quiet", {level: e.level});
      showBanner(`Alert from ${e.from} in ${e.circle}: ${e.text}`, () => openAlerts(), true);
      notifySystem(`Alert from ${e.from}`, e.text);
    }
    return;
  }
  if (!e.out || !was) return;
  if (e.answers.length > was.answers) {
    const last = e.answers[e.answers.length - 1];
    play("answered");
    const said = last.kind === "reply"
      ? `${last.from} answered your alert: ${last.text}`
      : `${last.from} has ${last.kind} your alert`;
    toast(said);
    if (last.kind === "reply") notifySystem(`${last.from} answered`, last.text || "");
  }
  if (was.state !== e.state && (e.state === "not heard" || e.state === "failed")) {
    play("failed");
    toast(e.state === "failed"
      ? `Your alert to ${e.to || "the circle"} was not sent: ${e.why || ""}`
      : `Your alert to ${e.to || "the circle"} was not heard`, true);
  }
}

function alarmsWaiting() {
  // the oldest first: it was heard first
  return alertsLog.filter((e) => e.alarm).reverse();
}

let alarmTimer = null;
function keepAlarming() {
  if (alarmTimer) return;
  alarmTimer = setInterval(() => {
    if (alarmsWaiting().length) play("alert", {level: "time-sensitive"});
    else if (urgentMail && !$("banner").hidden) play("mail-urgent", {level: "time-sensitive"});
    else { clearInterval(alarmTimer); alarmTimer = null; }
  }, ALARM_EVERY_MS);
}

let alarmShown = null;
function showAlarm() {
  const waiting = alarmsWaiting();
  const box = $("alarm");
  if (!waiting.length) {
    box.hidden = true;
    alarmShown = null;
    return;
  }
  const e = waiting[0];
  const fresh = !alarmShown || alarmShown.tag !== e.tag;
  alarmShown = e;
  $("alarm-head").textContent = `Alert from ${e.from} · ${e.circle}`;
  $("alarm-text").textContent = e.text;
  $("alarm-when").textContent = `${e.to ? `To ${e.to}` : "To everyone in the circle"} · `
    + `sent ${new Date(e.time * 1000).toLocaleTimeString([], {hour: "2-digit", minute: "2-digit"})}`
    + (e.dial_hz ? ` · heard on ${mhzOf(e.dial_hz)} MHz` : "");
  $("alarm-more").textContent = waiting.length > 1 ? `${waiting.length - 1} more waiting after this one` : "";
  box.hidden = false;
  if (fresh) {
    $("alarm-line").value = "";
    $("alarm-ack").focus();
  }
  keepAlarming();
}

async function alertDo(body, said) {
  try {
    await api("alerts", body);
    if (said) toast(said);
  } catch (e) {
    toast(e.message, true);
  }
  await syncAlerts(null, true);
}

// -- reports (the forms design, §4.3): the forms received or sent that carry a position --------
let reportList = [];
let reportsMap = null;
let pickedReport = null; // `${id}:${n}`: a report's nth position

async function openReports() {
  folder = "reports";
  markFolder("reports");
  show("reports-view");
  try {
    reportList = await api("reports");
  } catch (e) {
    toast(e.message, true);
    reportList = [];
  }
  if (!templates.length) await loadForms();
  if (!reportsMap) {
    reportsMap = new WorldMap($("reports-map"), {narrowest: 0.02, fitLeast: 0.05,
      onReport: (key) => pickReportAt(key, "map")});
  }
  fillReportForms();
  renderReports(true);
}

// the forms the reports are of, by name, "All forms" first; the one chosen kept
function fillReportForms() {
  const select = $("report-form");
  const was = select.value;
  const forms = [...new Map(reportList.map((r) => [r.template, r.form])).entries()]
    .sort((a, b) => a[1].localeCompare(b[1], undefined, {numeric: true}));
  select.replaceChildren(el("option", {value: "", text: "All forms"}),
    ...forms.map(([fp, name]) => el("option", {value: fp, text: name})));
  select.value = forms.some(([fp]) => fp === was) ? was : "";
  fillColourBy();
}

// the choice fields of the form chosen, to colour the marks by: its first required one, else its
// first, by default; none for all forms together
function fillColourBy() {
  const select = $("report-colour");
  const t = templates.find((x) => x.template === $("report-form").value);
  const choices = t ? t.fields.filter((f) => f.type === "choice") : [];
  select.replaceChildren(el("option", {value: "", text: "None"}),
    ...choices.map((f) => el("option", {value: String(f.n), text: f.label})));
  const first = choices.find((f) => f.required) || choices[0];
  select.value = first ? String(first.n) : "";
  select.disabled = !choices.length;
}

// a choice's colour: in the form's order, the first green and the last red; Unknown and Other grey
function choiceColours(choices) {
  const grey = (c) => /^(unknown|other)$/i.test(c);
  const ranked = choices.filter((c) => !grey(c));
  return choices.map((c) => {
    if (grey(c)) return "hsl(0, 0%, 60%)";
    const j = ranked.indexOf(c);
    const hue = ranked.length < 2 ? 120 : 120 * (1 - j / (ranked.length - 1));
    return `hsl(${Math.round(hue)}, 70%, 45%)`;
  });
}

function shownReports() {
  const fp = $("report-form").value;
  const age = Number($("report-age").value);
  const since = age ? Date.now() / 1000 - age : 0;
  return reportList.filter((r) => (!fp || r.template === fp) && r.date >= since);
}

function renderReports(fit = false) {
  const shown = shownReports();
  const t = templates.find((x) => x.template === $("report-form").value);
  const by = t && t.fields.find((f) => String(f.n) === $("report-colour").value);
  const colours = by ? choiceColours(by.choices) : [];
  const colourOf = (r) => {
    const i = by ? r.choices[String(by.n)] : undefined;
    return i === undefined ? "var(--accent)" : colours[i];
  };
  $("reports-legend").replaceChildren(...(by ? by.choices.map((c, i) =>
    el("span", {}, el("span", {class: "swatch", style: `background: ${colours[i]}`}), c)) : []));
  reportsMap.showReports(shown.flatMap((r) => r.places.map((p, n) => ({
    id: `${r.id}:${n}`, lat: p.lat, lon: p.lon, colour: colourOf(r),
    title: `${r.form}, ${r.folder === "sent" ? `to ${r.to.join(", ")}` : `from ${r.from}`}: ${p.label} ${p.text}`,
  }))));
  $("report-rows").replaceChildren(...shown.map((r) => {
    const who = r.folder === "sent" ? `To ${r.to.join(", ")}` : r.from;
    const what = r.folder === "circle" ? `${r.form}, in ${r.subject}`
      : r.subject ? `${r.form}: ${r.subject}` : r.form;
    const where = r.places.length > 1
      ? `${r.places[0].text} and ${r.places.length - 1} more` : r.places[0].text;
    const tr = el("tr", {"data-report": r.id},
      el("td", {}, el("span", {class: "swatch", style: `background: ${colourOf(r)}`})),
      el("td", {class: "date", text: when(r.date)}),
      el("td", {text: who}),
      el("td", {text: what, title: r.folder === "circle" ? "By broadcast to a circle" : ""}),
      el("td", {class: "where", text: where}));
    tr.title = new Date(r.date * 1000).toUTCString();
    tr.addEventListener("click", () => pickReportAt(`${r.id}:0`, "list"));
    return tr;
  }));
  $("reports-empty").textContent = reportList.length
    ? "No report of this form was written in that time."
    : "No form you received or sent carries a position yet. A storm, damage or situation report, a check-in or a position report does.";
  $("reports-empty").hidden = shown.length > 0;
  if (pickedReport && !shown.some((r) => pickedReport.startsWith(`${r.id}:`))) pickReportAt(null);
  if (fit) reportsMap.fit();
}

// a report's position picked, on the map or in the list: marked in both, and said under the map
function pickReportAt(key, from) {
  pickedReport = key;
  if (from !== "map") reportsMap.pickReport(key);
  const [id, n] = key ? key.split(":") : [null, 0];
  for (const tr of $("report-rows").children) {
    tr.classList.toggle("picked", tr.dataset.report === id);
  }
  const r = reportList.find((x) => x.id === id);
  $("report-picked").hidden = !r;
  if (!r) return;
  const p = r.places[Number(n)];
  $("report-picked-text").textContent = `${r.form}, ${r.folder === "sent" ? `to ${r.to.join(", ")}` : `from ${r.from}`}, `
    + `${new Date(r.date * 1000).toISOString().slice(0, 16).replace("T", " ")} UTC. ${p.label}: ${p.text}`;
  $("report-open").hidden = r.folder === "circle";
  $("report-open").onclick = () => openMessage(r.id, {folder: r.folder});
  const osm = $("report-osm");
  osm.href = p.osm;
  // the desktop app's window opens no other site: the panel asks the system's browser
  osm.onclick = window.radiopostPrints ? async (event) => {
    event.preventDefault();
    try {
      await api("reports/map", {lat: p.lat, lon: p.lon, within: p.within});
    } catch (e) {
      toast(e.message, true);
    }
  } : null;
  if (from === "list") {
    const row = [...$("report-rows").children].find((tr) => tr.dataset.report === id);
    if (row) row.scrollIntoView({block: "nearest"});
  }
}

// the reports shown written to a file in the Downloads folder
async function exportReports(format) {
  const ids = shownReports().map((r) => r.id);
  if (!ids.length) {
    toast("No report is shown to save.", true);
    return;
  }
  try {
    const s = await api("reports/export", {format, ids});
    toast(`Saved ${s.count} report${s.count > 1 ? "s" : ""} as ${s.name} in ${s.path.slice(0, s.path.length - s.name.length - 1)}`);
  } catch (e) {
    toast(e.message, true);
  }
}

async function openAlerts() {
  folder = "alerts";
  markFolder("alerts");
  await syncAlerts(null, true);
  renderAlerts();
  show("alerts-view");
  // the gateways' channels, with their predictions, for the alert's Dial
  api("gateways").then((list) => {
    alertGateways = list.gateways || [];
    fillAlertDials();
    fillAlertDials("crep");
  }).catch(() => {});
}

// a callsign's base (RP-0 §6.1), as the post office compares them: the SSID off; of three parts
// the middle one, of two the longer
function callBase(call) {
  const parts = String(call).trim().toUpperCase().split("-")[0].split("/");
  if (parts.length === 3) return parts[1];
  return parts.reduce((best, p) => (p.length >= best.length ? p : best), "");
}

// the dials an alert may go on (RP-8 §7): the channels of the station's listed gateways, the
// best predicted first; the circle's watch dial; wherever the radio is
function alertDialChoices(prefix = "alert") {
  const to = $(`${prefix}-to`).value.trim();
  const circle = alertCircles.find((c) => c.name === $(`${prefix}-circle`).value);
  const choices = [];
  if (to) {
    const base = callBase(to);
    const channels = [];
    for (const g of alertGateways) {
      if (callBase(g.callsign) !== base) continue;
      for (const c of g.channels || []) {
        if (!c.dial_hz) continue;
        const h = c.hf && c.hf.state === "predicted" ? c.hf : null;
        const words = [`${mhzOf(c.dial_hz)} MHz`, g.callsign];
        if (h) words.push(h.chance_words);
        if (c.scan_s) words.push("scans");
        channels.push({hz: c.dial_hz, chance: h ? h.chance : -1, label: words.join(" · ")});
      }
    }
    channels.sort((a, b) => b.chance - a.chance);
    choices.push(...channels);
  }
  if (circle && circle.watch_hz && !choices.some((c) => c.hz === circle.watch_hz)) {
    choices.push({hz: circle.watch_hz,
                  label: `${mhzOf(circle.watch_hz)} MHz · ${circle.name}'s watch dial`});
  }
  choices.push({hz: null, label: alertDials.radio
    ? `Where the radio is, ${mhzOf(alertDials.radio)} MHz` : "Where the radio is"});
  return choices;
}

function fillAlertDials(prefix = "alert") {
  const select = $(`${prefix}-dial`);
  const was = select.value;
  const choices = alertDialChoices(prefix);
  select.replaceChildren(...choices.map((c) => new Option(c.label, c.hz ? String(c.hz) : "")));
  // a dial the operator picked stays while it is offered; else the first, the best
  const offered = choices.some((c) => (c.hz ? String(c.hz) : "") === was);
  if (select.dataset.picked && offered) select.value = was;
  else delete select.dataset.picked;
}

// what the circles' watch dials miss here (RP-8 §7): at a station whose gateway scans, a watch
// dial among the dials it scans; at another, the radio on it
function alertDialWarnings() {
  const out = [];
  const scanned = alertDials.scanned || [];
  for (const c of alertCircles.filter((x) => x.watch_hz)) {
    const watch = `${mhzOf(c.watch_hz)} MHz`;
    if (scanned.length) {
      if (!scanned.includes(c.watch_hz)) {
        out.push(`Your gateway scans ${scanned.map(mhzOf).join(", ")} MHz: ${c.name}'s watch dial, ${watch}, is not one of them, so alerts to the whole circle are not heard here.`);
      }
    } else if (alertDials.radio && alertDials.radio !== c.watch_hz) {
      out.push(`The radio is on ${mhzOf(alertDials.radio)} MHz: ${c.name}'s alerts to the whole circle go on ${watch}.`);
    }
  }
  return out;
}

function stateTag(e) {
  if (e.out) {
    const bad = e.state === "failed" || e.state === "not heard";
    const good = ["heard", "seen", "replied"].includes(e.state);
    return el("span", {class: bad ? "tag bad" : good ? "tag ok" : "tag busy",
                       text: OUT_STATES[e.state] || e.state, title: e.why || ""});
  }
  if (e.state === "new") {
    return el("span", {class: e.alarm ? "tag bad" : "tag busy",
                       text: e.alarm ? "sounding" : "not answered"});
  }
  return el("span", {class: "tag ok", text: e.state === "replied" ? "answered" : "seen"});
}

function answersOf(e) {
  const at = (t) => new Date(t * 1000).toLocaleTimeString([], {hour: "2-digit", minute: "2-digit"});
  if (!e.out) return e.reply ? `You: ${e.reply}` : "";
  return e.answers.map((a) => (a.kind === "reply"
    ? `${a.from} ${at(a.time)}: ${a.text}`
    : `${a.from} ${a.kind} ${at(a.time)}`)).join(" · ");
}

function renderAlerts() {
  const circleSelect = $("alert-circle");
  const chosen = circleSelect.value || kept("alertCircle", "");
  circleSelect.replaceChildren(...alertCircles.map((c) => new Option(c.name, c.name)));
  if (alertCircles.some((c) => c.name === chosen)) circleSelect.value = chosen;
  const none = !alertCircles.length;
  for (const id of ["alert-circle", "alert-to", "alert-dial", "alert-text", "alert-send"]) $(id).disabled = none;
  fillAlertDials();
  renderReportForm(none);
  const warnings = alertDialWarnings();
  $("alerts-dials").textContent = warnings.join(" ");
  $("alerts-dials").hidden = !warnings.length;
  $("alert-left").textContent = none ? "Add a circle below first." : alertLeft();
  const modem = adapter();
  $("alerts-modem").textContent = modem.startsWith("ws://")
    ? ""
    : `Alerts go only through a modem that carries broadcasts, as Aether does. ${modem || "The modem"} is used for calls; VARA, ARDOP, Mercury and packet carry no alerts.`;
  $("alerts-modem").hidden = modem.startsWith("ws://");
  $("alert-rows").replaceChildren(...alertsLog.map((e) => {
    const tr = el("tr", {class: e.alarm ? "alarming" : e.quiet ? "quiet" : ""});
    tr.append(el("td", {class: "date", text: when(e.at), title: new Date(e.at * 1000).toLocaleString()}));
    const way = el("td", {class: "way", text: `${e.out ? "You" : e.from} → ${e.to || "everyone"}`});
    if (e.dial_hz) {
      way.append(el("span", {class: "note", text: ` · ${mhzOf(e.dial_hz)} MHz`,
        title: e.out ? "The dial it went on" : "The dial it was heard on, where the answers go"}));
    }
    tr.append(way);
    tr.append(el("td", {class: "text", text: e.text}));
    tr.append(el("td", {text: e.circle}));
    const cell = el("td", {class: "answers"}, stateTag(e), answersOf(e));
    if (!e.out && e.state === "new") {
      cell.append(" ", el("button", {type: "button", class: "quiet", text: "Acknowledge",
        title: "The other station hears that you have seen it",
        onclick: () => alertDo({ack: e.tag})}));
    }
    tr.append(cell);
    return tr;
  }));
  $("alerts-empty").textContent = "No alerts yet.";
  $("alerts-empty").hidden = alertsLog.length > 0;
  $("circle-rows").replaceChildren(...alertCircles.map((c) => {
    const level = el("select", {title: `How ${c.name}'s alerts arrive here`});
    level.append(...LEVELS.map((l) => new Option(LEVEL_NAMES[l], l)));
    level.value = c.level;
    level.addEventListener("change", () => api("circles", {set: {name: c.name, level: level.value}})
      .then(() => syncAlerts(null, true)).catch((e) => toast(e.message, true)));
    const watch = el("input", {class: "dial", inputmode: "decimal", placeholder: "none",
      title: `${c.name}'s watch dial, in MHz; empty for none`});
    watch.value = c.watch_hz ? mhzOf(c.watch_hz) : "";
    watch.addEventListener("change", () => {
      const text = watch.value.trim();
      const hz = text ? hzOf(text) : null;
      if (text && !hz) { toast("The watch dial is a frequency in MHz, such as 7.101.", true); return; }
      api("circles", {set: {name: c.name, watch_hz: hz}})
        .then(() => syncAlerts(null, true)).catch((e) => toast(e.message, true));
    });
    const answer = el("input", {type: "checkbox", title: "Say heard by itself when an alert for this station arrives"});
    answer.checked = c.answer;
    answer.addEventListener("change", () => api("circles", {set: {name: c.name, answer: answer.checked}})
      .then(() => syncAlerts(null, true)).catch((e) => toast(e.message, true)));
    const forget = el("button", {type: "button", class: "act danger",
      title: `Forget ${c.name}: its alerts are no longer heard here`,
      "aria-label": `Forget ${c.name}`});
    forget.append(icon("trash"));
    forget.addEventListener("click", () => armed(forget, "Forget?", async () => {
      try { await api("circles", {remove: c.name}); } catch (e) { toast(e.message, true); }
      await syncAlerts(null, true);
    }));
    return el("tr", {},
      el("td", {text: c.name}),
      el("td", {class: "check-text", text: c.check}),
      el("td", {}, watch),
      el("td", {}, level),
      el("td", {}, answer),
      el("td", {class: "acts"}, forget));
  }));
  $("circles-empty").textContent = "No circle yet. Add one with a keyword the others type too.";
  $("circles-empty").hidden = alertCircles.length > 0;
}

function alertLeft() {
  const used = new TextEncoder().encode($("alert-text").value.trim()).length;
  return `${100 - used} bytes left`;
}

async function sendAlert(event) {
  event.preventDefault();
  const circle = $("alert-circle").value;
  const to = $("alert-to").value.trim().toUpperCase();
  const text = $("alert-text").value.trim();
  if (!text) { toast("A line to send", true); return; }
  const dial = $("alert-dial").value;
  const dial_hz = dial ? Number(dial) : null;
  keep("alertCircle", circle);
  try {
    await api("alerts", {send: {circle, to, text, dial_hz}});
    $("alert-text").value = "";
    delete $("alert-dial").dataset.picked;
    toast(`Alert to ${to || `everyone in ${circle}`} waiting for the modem${dial_hz ? `, on ${mhzOf(dial_hz)} MHz` : ""}`);
  } catch (e) {
    toast(e.message, true);
  }
  await syncAlerts(null, true);
}

// Reports by broadcast (RP-8 §3, §7): a form with a position, to a circle, once or again
let circleRepeat = null;  // the report being sent again, as the panel says
let crepInputs = [];

// the forms a report may carry: those with a position, in a field or a table's column
function hasPosition(t) {
  return t.fields.some((f) => f.type === "position"
    || (f.columns || []).some((c) => c.type === "position"));
}

function renderReportForm(none) {
  const circleSelect = $("crep-circle");
  const chosen = circleSelect.value || kept("alertCircle", "");
  circleSelect.replaceChildren(...alertCircles.map((c) => new Option(c.name, c.name)));
  if (alertCircles.some((c) => c.name === chosen)) circleSelect.value = chosen;
  const select = $("crep-template");
  const was = select.value || kept("reportTemplate", "");
  const usable = templates.filter(hasPosition)
    .sort((a, b) => a.name.localeCompare(b.name, undefined, {numeric: true}));
  if (select.options.length !== usable.length) {
    select.replaceChildren(...usable.map((t) => new Option(t.name, t.template)));
    const position = usable.find((t) => t.name === "Position Report");
    select.value = usable.some((t) => t.template === was) ? was : position?.template || "";
    showReportFields();
  }
  for (const id of ["crep-circle", "crep-to", "crep-dial", "crep-every", "crep-template", "crep-send"]) {
    $(id).disabled = none;
  }
  fillAlertDials("crep");
  const r = circleRepeat;
  $("crep-repeat").hidden = !r;
  // a report going again keeps the card open, its Stop in sight
  if (r) $("crep-card").open = true;
  if (r) {
    const name = templates.find((t) => t.template === r.template)?.name || "The report";
    const at = (t) => new Date(t * 1000).toLocaleTimeString([], {hour: "2-digit", minute: "2-digit"});
    $("crep-repeat-text").textContent = `${name} to ${r.to || r.circle} goes again every `
      + `${r.every_s / 60} min, next at ${at(r.next_at)}, until ${at(r.until)}.`;
  }
}

function showReportFields() {
  const t = templates.find((x) => x.template === $("crep-template").value);
  const box = $("crep-fields");
  box.hidden = !t;
  crepInputs = [];
  box.replaceChildren(...(t ? formFieldNodes(t, true, crepInputs) : []));
}

async function sendReport(event) {
  event.preventDefault();
  const template = $("crep-template").value;
  if (!template) { toast("Choose a form", true); return; }
  const values = {};
  const here = [];
  for (const input of crepInputs) {
    const v = input.get();
    if (v === null) continue;
    values[input.n] = v;
    if (input.here && input.here()) here.push(input.n);
  }
  const circle = $("crep-circle").value;
  const to = $("crep-to").value.trim().toUpperCase();
  const dial = $("crep-dial").value;
  const every = $("crep-every").value;
  keep("alertCircle", circle);
  keep("reportTemplate", template);
  try {
    await api("alerts", {report: {circle, to, dial_hz: dial ? Number(dial) : null,
      form: {template, values}, every_s: every ? Number(every) : null, here}});
    toast(`Report to ${to || `everyone in ${circle}`} waiting for the modem`
      + (every ? `; again every ${Number(every) / 60} min` : ""));
  } catch (e) {
    toast(e.message, true);
  }
  await syncAlerts(null, true);
}

// a keyword nobody would guess, easy to say: four words of two syllables, about 50 bits
function makeKeyword() {
  const consonants = "bdfghjklmnprstvz", vowels = "aeiou";
  const draws = new Uint32Array(8);
  crypto.getRandomValues(draws);
  const syllable = (n) => consonants[n % 16] + vowels[Math.floor(n / 16) % 5];
  const words = [];
  for (let i = 0; i < 4; i++) words.push(syllable(draws[2 * i]) + syllable(draws[2 * i + 1]));
  return words.join(" ");
}

// the keyword as the circle's key will take it (RP-8 §2), for the length the panel checks
function normalisedKeyword(text) {
  return text.normalize("NFC").replace(/[A-Z]/g, (c) => c.toLowerCase())
    .replace(/[ \t]+/g, " ").replace(/^ | $/g, "");
}

function keywordStrength() {
  const k = normalisedKeyword($("circle-keyword").value);
  const note = $("circle-strength");
  if (!k) note.textContent = "";
  else if ([...k].length < 8) note.textContent = "Too short: at least 8 characters.";
  else if (k.split(" ").length < 4 && [...k].length < 20) {
    note.textContent = "Short keywords can be guessed from one alert heard on the air. Use Make one, or four words or more.";
  } else note.textContent = "";
}

async function addCircle(event) {
  event.preventDefault();
  const name = $("circle-name").value.trim();
  const keyword = $("circle-keyword").value;
  const watchText = $("circle-watch").value.trim();
  const watch_hz = watchText ? hzOf(watchText) : null;
  if (watchText && !watch_hz) { toast("The watch dial is a frequency in MHz, such as 7.101.", true); return; }
  const add = $("circle-add");
  add.disabled = true;
  try {
    await api("circles", {add: {name, keyword, level: $("circle-level").value, watch_hz}});
    $("circle-name").value = "";
    $("circle-keyword").value = "";
    $("circle-watch").value = "";
    keywordStrength();
    toast(`${name} kept: read its check to the others`);
  } catch (e) {
    toast(e.message, true);
  }
  add.disabled = false;
  await syncAlerts(null, true);
}

// ── Setup's Sounds and notifications ───────────────────────────────────────────────────────────

async function fillDevices() {
  const select = $("snd-device");
  let outputs = [];
  try {
    outputs = (await navigator.mediaDevices.enumerateDevices())
      .filter((d) => d.kind === "audiooutput" && d.deviceId && d.deviceId !== "default");
  } catch { outputs = []; }
  const chosen = sounds().device || {};
  select.replaceChildren(new Option("The system's default output", ""),
    ...outputs.map((d, i) => new Option(d.label || `Output ${i + 1}`, d.deviceId)));
  // a device's ID differs from one panel's port to another's: its name is kept too
  const match = outputs.find((d) => d.deviceId === chosen.id)
    || outputs.find((d) => d.label && d.label === chosen.label);
  select.value = match ? match.deviceId : "";
  if (match && match.deviceId !== chosen.id) {
    chosen.id = match.deviceId;
    routeAudio();
  }
  select.disabled = !canRoute();
  select.title = canRoute()
    ? "Where the sounds play"
    : "This window cannot choose: the sounds play on the system's default output";
  $("snd-names").hidden = !canRoute() || outputs.some((d) => d.label);
}

function renderSounds() {
  const s = sounds();
  $("snd-on").checked = s.on !== false;
  $("snd-volume").value = s.volume ?? 60;
  $("snd-rows").replaceChildren(...SOUND_EVENTS.map(([event, words]) => {
    const select = el("select", {title: `The sound for: ${words}`, "aria-label": words});
    select.append(...Object.keys(TONES).map((t) => new Option(TONE_NAMES[t], t)));
    select.value = soundOf(event);
    select.addEventListener("change", () => {
      s.events = s.events || {};
      s.events[event] = select.value;
      play(event, {test: true});
      saveNotify();
    });
    const test = el("button", {type: "button", class: "quiet", text: "Play",
      title: "Play it now, at the volume set", onclick: () => play(event, {test: true})});
    return el("tr", {}, el("td", {text: words}), el("td", {}, select), el("td", {class: "acts"}, test));
  }));
  const q = notifySettings.quiet || {};
  $("quiet-on").checked = Boolean(q.on);
  $("quiet-from").value = q.from || "22:00";
  $("quiet-to").value = q.to || "07:00";
  $("mail-default").value = (notifySettings.mail || {}).default || "normal";
  $("snd-notify").checked = Boolean(notifySettings.notify);
  fillDevices();
}

function wireAlerts() {
  for (const kind of ["pointerdown", "keydown"]) document.addEventListener(kind, wakeAudio);
  $("alert-form").addEventListener("submit", sendAlert);
  $("crep-form").addEventListener("submit", sendReport);
  $("crep-template").addEventListener("change", showReportFields);
  $("crep-circle").addEventListener("change", () => fillAlertDials("crep"));
  $("crep-to").addEventListener("change", () => fillAlertDials("crep"));
  $("crep-dial").addEventListener("change", () => { $("crep-dial").dataset.picked = "1"; });
  $("crep-stop").addEventListener("click", async () => {
    try { await api("alerts", {stop_repeat: true}); } catch (e) { toast(e.message, true); }
    await syncAlerts(null, true);
  });
  $("alert-text").addEventListener("input", () => { $("alert-left").textContent = alertLeft(); });
  $("alert-to").addEventListener("input", () => fillAlertDials());
  $("alert-dial").addEventListener("change", () => { $("alert-dial").dataset.picked = "1"; });
  $("alert-circle").addEventListener("change", () => fillAlertDials());
  $("circle-form").addEventListener("submit", addCircle);
  $("circle-keyword").addEventListener("input", keywordStrength);
  $("circle-make").addEventListener("click", () => {
    $("circle-keyword").value = makeKeyword();
    $("circle-keyword").type = "text";
    $("circle-show").checked = true;
    keywordStrength();
  });
  $("circle-show").addEventListener("change", () => {
    $("circle-keyword").type = $("circle-show").checked ? "text" : "password";
  });
  $("alarm-ack").addEventListener("click", () => {
    if (alarmShown) alertDo({ack: alarmShown.tag});
  });
  $("alarm-call").addEventListener("click", async () => {
    if (!alarmShown) return;
    const who = alarmShown.from;
    const dial = alarmShown.dial_hz;
    await alertDo({ack: alarmShown.tag});
    switchKind("station", false);
    $("callsign").value = who;
    // the dial the alert was heard on, where its sender listens for the answers
    if (dial) {
      $("dial").value = mhzOf(dial);
      keep("dial", $("dial").value);
      showTuning();
    }
    toast(`Press Call to call ${who}${dial ? ` on ${mhzOf(dial)} MHz` : ""}`);
  });
  for (const b of document.querySelectorAll("#alarm-quick [data-reply]")) {
    b.addEventListener("click", () => {
      if (alarmShown) alertDo({reply: {tag: alarmShown.tag, text: b.dataset.reply}}, "Answer waiting for the modem");
    });
  }
  $("alarm-reply").addEventListener("submit", (event) => {
    event.preventDefault();
    const text = $("alarm-line").value.trim();
    if (alarmShown && text) alertDo({reply: {tag: alarmShown.tag, text}}, "Answer waiting for the modem");
  });
  $("banner-open").addEventListener("click", () => { if (bannerAct) bannerAct(); });
  $("banner-close").addEventListener("click", () => { $("banner").hidden = true; urgentMail = null; });
  $("snd-on").addEventListener("change", () => { sounds().on = $("snd-on").checked; saveNotify(); });
  $("snd-volume").addEventListener("change", () => {
    sounds().volume = Number($("snd-volume").value);
    play("mail", {test: true});
    saveNotify();
  });
  $("snd-device").addEventListener("change", () => {
    const option = $("snd-device").selectedOptions[0];
    sounds().device = $("snd-device").value ? {id: $("snd-device").value, label: option.text} : {};
    routeAudio();
    play("mail", {test: true});
    saveNotify();
  });
  $("snd-names").addEventListener("click", async () => {
    try {
      const stream = await navigator.mediaDevices.getUserMedia({audio: true});
      for (const t of stream.getTracks()) t.stop();
    } catch { toast("This window was not allowed to list the devices by name", true); }
    fillDevices();
  });
  const quiet = () => {
    notifySettings.quiet = {on: $("quiet-on").checked, from: $("quiet-from").value,
                            to: $("quiet-to").value};
    saveNotify();
  };
  for (const id of ["quiet-on", "quiet-from", "quiet-to"]) $(id).addEventListener("change", quiet);
  $("mail-default").addEventListener("change", () => {
    notifySettings.mail = notifySettings.mail || {};
    notifySettings.mail.default = $("mail-default").value;
    saveNotify();
  });
  $("snd-notify").addEventListener("change", async () => {
    notifySettings.notify = $("snd-notify").checked;
    if (notifySettings.notify && "Notification" in window && Notification.permission === "default") {
      try { await Notification.requestPermission(); } catch { /* asked */ }
    }
    if (notifySettings.notify && (!("Notification" in window) || Notification.permission === "denied")) {
      toast("This window may not show notifications: the alarm and the window coming forward still tell you", true);
    }
    saveNotify();
  });
}

function wire() {
  try {
    const kept = localStorage.getItem("radiopost.adapter");
    if (kept) $("adapter").value = kept;
  } catch { /* not kept */ }
  tellModem();
  $("adapter").addEventListener("change", tellModem);
  for (const b of document.querySelectorAll("[data-folder]")) {
    b.addEventListener("click", () => openFolder(b.dataset.folder));
  }
  $("write").addEventListener("click", () => {
    if (popOut({})) return;
    // a reply left half written stays; a new message starts empty
    if (answering) newComposer();
    show("composer");
  });
  wireComposer();
  $("address-filter").addEventListener("input", renderAddresses);
  const addAddress = async () => {
    const typed = $("address-new").value.trim();
    if (!typed) return;
    try {
      await changeAddresses({add: typed});
      $("address-new").value = "";
      renderAddresses();
    } catch (e) {
      toast(e.message, true);
    }
  };
  $("address-add").addEventListener("click", addAddress);
  $("forms-import").addEventListener("click", () => $("forms-file").click());
  $("forms-file").addEventListener("change", async (e) => {
    await importForm(e.target.files[0]);
    e.target.value = "";
  });
  $("address-import").addEventListener("click", () => $("address-file").click());
  $("address-file").addEventListener("change", async (e) => {
    await importContacts(e.target.files[0]);
    e.target.value = "";
  });
  wireSuggestions($("group-form").members);
  $("group-form").addEventListener("submit", saveGroup);
  $("group-cancel").addEventListener("click", () => editGroup(null));
  $("address-new").addEventListener("keydown", (e) => { if (e.key === "Enter") addAddress(); });
  $("back").addEventListener("click", () => openFolder(folder));
  $("r-reply").addEventListener("click", () => answer("reply"));
  $("r-reply-all").addEventListener("click", () => answer("reply-all"));
  $("r-forward").addEventListener("click", () => answer("forward"));
  $("r-edit").addEventListener("click", () => edit(reading.id));
  $("r-print").addEventListener("click", printMessage);
  $("r-delete").addEventListener("click", () => toTrash(reading.id, reading.folder));
  $("r-restore").addEventListener("click",
    () => fromTrash("restore", reading.id, reading.was, `Back in the ${reading.was}.`));
  $("r-purge").addEventListener("click", (e) => armed(e.currentTarget, "Click again to delete for good",
    () => fromTrash("purge", reading.id, reading.was, "Deleted for good.")));
  $("empty-trash").addEventListener("click", (e) => armed(e.currentTarget, "Click again to empty it",
    async () => {
      try {
        const r = await api("empty-trash", {});
        toast(`${r.deleted} message${r.deleted === 1 ? "" : "s"} deleted for good.`);
        await afterTrash();
      } catch (err) {
        toast(err.message, true);
      }
    }));
  document.addEventListener("keydown", (e) => {
    const typing = e.target.closest("input, textarea, select");
    if (e.key === "Delete" && !typing && !$("reader").hidden && !$("r-delete").hidden) {
      $("r-delete").click();
    }
  });
  $("setup-open").addEventListener("click", () => show("setup"));
  for (const radio of document.querySelectorAll("input[name=kind]")) {
    radio.checked = radio.value === kept("kind", "gateway");
    radio.addEventListener("change", () => switchKind(radio.value));
  }
  $("fetch").value = kept("fetch", "all");
  $("fetch").addEventListener("change", () => keep("fetch", $("fetch").value));
  boxKind = kind();
  showKind();
  wireCalled();
  // the box opens on the last call of its kind, unless something was put in it meanwhile
  loadCalled().then(() => {
    const k = kind();
    if (!$("callsign").value.trim() && !leftAs[k] && offered(k).length) {
      putCalled(k, offered(k)[0]);
      showKind();
    }
  });
  $("gateways-open").addEventListener("click", () => openGateways());
  $("gateways-refresh").addEventListener("click", async () => {
    $("gateways-refresh").disabled = true;
    await openGateways(true);
    $("gateways-refresh").disabled = false;
  });
  wirePlaces();
  wireGateways();
  wireTerrain();
  // the channels are the chosen gateway's: another callsign typed has none here
  $("callsign").addEventListener("input", showChannels);
  // a channel picked, or a station call's dial typed, is tuned at once (W14)
  $("channel").addEventListener("change", pickChannel);
  $("dial").value = kept("dial", "");
  $("dial").addEventListener("input", showTuning);
  $("dial").addEventListener("input", askP2pChance);
  $("callsign").addEventListener("input", askP2pChance);
  $("dial").addEventListener("change", () => {
    const text = $("dial").value.trim();
    if (text && !hzOf(text)) { toast("The dial is a frequency in MHz, such as 7.101.", true); return; }
    keep("dial", text);
    pickChannel();
  });
  $("bandwidth").value = kept("bandwidth", "2300");
  $("bandwidth").addEventListener("change", () => {
    keep("bandwidth", $("bandwidth").value);
    askP2pChance();
  });
  $("tone").addEventListener("click", toggleTone);
  showTuning();
  $("call").addEventListener("click", () => {
    const callsign = $("callsign").value.trim();
    if (!callsign) { toast("Whom to call?", true); return; }
    const station = kind() === "station";
    if (station && $("dial").value.trim() && !hzOf($("dial").value)) {
      toast("The dial is a frequency in MHz, such as 7.101.", true);
      return;
    }
    const fetchRule = $("fetch").value;
    const dial = currentDial();
    // a gateway that scans is found by a call that keeps trying as long as its record says
    const channel = !station ? chosenChannel() : null;
    placeCall({
      callsign, station, adapter: adapter(),
      fetch: /^[0-9]+$/.test(fetchRule) ? Number(fetchRule) : fetchRule,
      dial_hz: dial || null,
      call_s: channel && channel.scan_s != null ? channel.scan_s : null,
      // the call's bandwidth: a station call's, as the two operators agreed; a gateway call's,
      // the channel picked's, so the call goes at the bandwidth the channel says (an FM channel's
      // width is no modem's bandwidth)
      bandwidth: station ? Number($("bandwidth").value)
        : channel && channel.mode !== "FM" && channel.bandwidth_hz ? channel.bandwidth_hz : null,
    });
  });
  $("hang-up").addEventListener("click", () => act("hang-up", {how: $("hang-up").dataset.how}));
  wireBeacon();
  // one switch: pressed in, it listens; pressed again, it stops
  $("listen").addEventListener("click", () => {
    if ($("listen").getAttribute("aria-pressed") === "true") act("stop", {});
    // the P2P dial typed is the channel listened on: the radio goes there first (W15)
    else act("listen", {adapter: adapter(), dial_hz: hzOf(kept("dial", "")) || null});
  });
  $("update-go").addEventListener("click", () => act("update", {}));
  $("updates-check").addEventListener("click", () => act("update/check", {}));
  $("make-key").addEventListener("click", () => act("key", {}));
  $("cred-save").addEventListener("click", async () => {
    await act("credential", {hex: $("cred-hex").value});
    $("cred-hex").value = "";
  });
  $("enroll").addEventListener("submit", enroll);
  for (const radio of document.querySelectorAll("input[name=join]")) {
    radio.addEventListener("change", showJoin);
  }
  showJoin();
  $("applied-check").addEventListener("click", () => checkApplication(true));
  $("applied-forget").addEventListener("click", (e) => armed(e.currentTarget, "Click again to forget it",
    () => act("application-forget", {})));
  $("office-open").addEventListener("click", () => openOffice());
  $("o-refresh").addEventListener("click", () => openOffice());
  for (const b of document.querySelectorAll("[data-otab]")) {
    b.addEventListener("click", () => openOffice(b.dataset.otab));
  }
  $("o-filter").addEventListener("input", drawAccounts);
  $("o-know-add").addEventListener("click", knowPost);
  $("o-know").addEventListener("keydown", (e) => { if (e.key === "Enter") knowPost(); });
  $("o-know-fetch").addEventListener("click", () => decidePost({refresh: true}));
  $("o-code").addEventListener("submit", makeCode);
  $("o-code").addEventListener("input", () => { $("o-operates").hidden = true; });
  $("o-made-copy").addEventListener("click", async () => {
    try { await navigator.clipboard.writeText($("o-made-code").textContent); toast("Copied."); }
    catch { toast("Select the code and copy it.", true); }
  });
  const settingsChange = async (body, said) => {
    try {
      await desk("PUT", "/office/settings", body);
      if (said) toast(said);
      await renderOfficeSettings();
    } catch (e) {
      toast(e.message, true);
    }
  };
  $("o-fcc").addEventListener("change", () => settingsChange({fcc: $("o-fcc").checked},
    $("o-fcc").checked ? "The FCC copy is on. A new one is fetched each week." : "The FCC copy is off."));
  $("o-fcc-fetch").addEventListener("click", async () => {
    try { await desk("POST", "/office/licences", {}); toast("Fetching the FCC's file."); await renderOfficeSettings(); }
    catch (e) { toast(e.message, true); }
  });
  $("o-qrz-save").addEventListener("click", async () => {
    const body = {qrz_username: $("o-qrz-user").value.trim()};
    if ($("o-qrz-pass").value) body.qrz_password = $("o-qrz-pass").value;
    $("o-qrz-pass").value = "";
    await settingsChange(body, "QRZ lookups saved.");
  });
  $("o-qrz-clear").addEventListener("click", () => settingsChange({qrz_username: "", qrz_password: ""}, "QRZ lookups off."));
  $("o-mb-kind").addEventListener("change", showMailboxKind);
  $("o-health-now").addEventListener("click", async () => {
    $("o-health-now").disabled = true;
    try { await renderHealth(true); } catch (e) { toast(e.message, true); }
    $("o-health-now").disabled = false;
  });
  $("o-dkim-save").addEventListener("click", async () => {
    $("o-dkim-save").disabled = true;
    try {
      await desk("PUT", "/office/settings", {dkim: $("o-dkim").value.split(/[\s,]+/).filter(Boolean)});
      toast("Saved. The hourly check asks for these from now on; checking now.");
      await renderHealth(true);
    } catch (e) {
      toast(e.message, true);
    }
    $("o-dkim-save").disabled = false;
  });
  $("o-backup").addEventListener("click", downloadBackup);
  $("o-auto").addEventListener("click", toggleAutoBackups);
  for (const [id, step] of [["o-dkim-start", "start"], ["o-dkim-switch", "switch"],
                            ["o-dkim-removed", "removed"], ["o-dkim-cancel", "cancel"]]) {
    $(id).addEventListener("click", (e) => dkimStep(step, e.currentTarget));
  }
  $("o-auto-now").addEventListener("click", async () => {
    try { await api("backups/fetch", {}); setTimeout(() => renderServer().catch(() => {}), 2000); }
    catch (e) { toast(e.message, true); }
  });
  $("o-ssh").value = kept("ssh", "");
  $("o-term").addEventListener("click", async () => {
    const target = $("o-ssh").value.trim();
    keep("ssh", target);
    try { toast(`Opened ${(await api("terminal", {target})).opened}`); } catch (e) { toast(e.message, true); }
  });
  $("o-claude").addEventListener("click", async () => {
    try {
      const text = await forClaude();
      const area = $("o-claude-text");
      area.value = text;
      area.hidden = false;
      try { await navigator.clipboard.writeText(text); toast("Copied: paste it into a Claude session, and say what you need."); }
      catch { area.select(); toast("Select the text below and copy it.", true); }
    } catch (e) {
      toast(e.message, true);
    }
  });
  $("o-mb-save").addEventListener("click", () => {
    const m = {kind: $("o-mb-kind").value, url: $("o-mb-url").value.trim(), command: $("o-mb-cmd").value.trim()};
    if ($("o-mb-key").value) m.api_key = $("o-mb-key").value;
    $("o-mb-key").value = "";
    settingsChange({mailboxes: m}, m.kind ? "Mailboxes are made at each approval." : "Mailboxes are made by hand.");
  });
  $("o-mb-check").addEventListener("click", async () => {
    try {
      const r = await desk("POST", "/office/mailboxes/check", {});
      toast(r.said, !r.ok);
    } catch (e) {
      toast(e.message, true);
    }
  });
  $("o-code-check").addEventListener("click", async () => {
    const form = $("o-code");
    const call = form.operator.value.trim().toUpperCase();
    if (!call) { toast("Which callsign?", true); return; }
    try {
      const e = await desk("GET", `/office/lookup/${encodeURIComponent(call)}?name=${encodeURIComponent(form.name.value.trim())}`);
      $("o-code-evidence").replaceChildren(evidenceBlock({evidence: e}));
    } catch (err) {
      toast(err.message, true);
    }
  });
  $("o-take").addEventListener("change", async () => {
    try {
      await desk("PUT", "/office/settings", {applications: $("o-take").checked ? "open" : "closed"});
      await renderOfficeSettings();
    } catch (e) {
      toast(e.message, true);
    }
  });
  const reasons = document.createElement("datalist");
  reasons.id = "decline-reasons";
  reasons.append(...DECLINE_REASONS.map((r) => el("option", {value: r})));
  document.body.append(reasons);
  $("who-pick").addEventListener("change", async () => {
    const id = Number($("who-pick").value);
    $("who-pick").blur();
    const s = await changeCallsign({choose: id});
    if (s && s.me) toast(`Working as ${s.me.operator}, ${s.me.address}.`);
  });
  $("callsign-add").addEventListener("click", async () => {
    const s = await changeCallsign({add: true});
    if (s) toast("A new callsign: enroll it with its own code, or apply for it. Your others stay in the list at the top.");
  });
  $("callsign-remove").addEventListener("click", (event) => {
    const c = lastCallsigns.find((x) => x.chosen) || {};
    const name = c.operator ? `${c.operator} (${c.address})`
      : c.application ? c.application.operator : "the new callsign";
    armed(event.currentTarget, "Remove it from this computer", async () => {
      const s = await changeCallsign({remove: chosenCallsign});
      if (s) toast(`Removed ${name} from this computer.`);
    });
  });
  $("enroll-again").addEventListener("click", () => {
    enrollOpen = true;
    refresh().then(() => $("enroll").code.focus()).catch(() => {});
  });
  $("po-save").addEventListener("click", () => act("post-office", {url: $("po-url").value}));
  $("renew").addEventListener("click", async () => {
    try { toast((await api("renew", {})).said); await refresh(); } catch (e) { toast(e.message, true); }
  });
  $("report").addEventListener("click", async () => {
    try {
      const r = await api("report", {});
      toast(r.lines.join("; ") || "Nothing to report.");
      await refresh();
    } catch (e) {
      toast(e.message, true);
    }
  });
  $("accept-add").addEventListener("click", async () => {
    const entry = $("accept-new").value.trim();
    if (!entry) return;
    await changeAccept({add: [entry]});
    $("accept-new").value = "";
  });
  $("accept-load").addEventListener("click", () => changeAccept({}));
  $("usual-load").addEventListener("click", loadUsual);
  $("usual-save").addEventListener("click", saveUsual);
  $("t-save").addEventListener("click", async () => {
    await act("trust", {domain: $("t-domain").value, public: $("t-key").value});
    $("t-key").value = "";
  });
}

async function start() {
  wire();
  wireAlerts();
  // what a composer window of its own put in the outbox, or could not open
  windowsChannel?.addEventListener("message", (e) => {
    if (e.data && e.data.composed) written(true, e.data.edited).catch(() => {});
    if (e.data && e.data.said) toast(e.data.said, true);
  });
  try { notifySettings = await api("notify"); } catch { notifySettings = {}; }
  renderSounds();
  await loadAddresses();
  await loadForms();
  checkMail(false).catch(() => {});
  const s = await refresh();
  if (!s.device || !s.me) show("setup");
  else await openFolder("inbox");
  setInterval(() => refresh().catch(() => {}), 1000);
  if (s.me) probeOffice();
  // a new application shows on the button within a minute
  setInterval(() => { if (meAddress) probeOffice(); }, 60000);
}

(ALONE ? startAlone() : start()).catch((e) => toast(e.message, true));
