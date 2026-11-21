// SPDX-License-Identifier: MIT OR Apache-2.0
// The RadioPost client's panel: the folders, a reader, a composer, the station's calls and its
// setup, over the client's JSON API (app/client/src/panel.rs). No build step.

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
                    "reader", "composer", "setup", "office"]) {
    $(id).hidden = id !== view;
  }
  // the usual gateways are read afresh each time, for the callsign in use now
  if (view === "setup") {
    loadUsual();
    loadPlaces(true);
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
  lastPlaces = JSON.stringify([v.places, v.choice]);
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
    const acts = document.createElement("td");
    acts.className = "acts";
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "quiet";
    remove.textContent = "Remove";
    remove.title = `Forget ${p.name}`;
    remove.addEventListener("click", () => placeRequest({op: "remove", id: p.id}));
    acts.append(remove);
    tr.append(name, square, from, def, acts);
    return tr;
  }));
  const choice = $("place-choice");
  const options = [["automatic", "Automatic: the GPS, else a saved place"],
                   ["gps", "The GPS, a saved place only while it has no fix"],
                   ...v.places.map((p) => [`place:${p.id}`, `Always ${p.name}`])];
  choice.replaceChildren(...options.map(([value, label]) => new Option(label, value)));
  choice.value = v.choice.use === "place" ? `place:${v.choice.id}` : v.choice.use;
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
  return JSON.stringify([v.places, v.choice]) !== lastPlaces;
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
  $("gps-ports").addEventListener("click", async () => {
    try {
      const v = await api("places?ports=1");
      fillPorts(v.ports || [], $("gps-port").value);
    } catch (e) {
      toast(e.message, true);
    }
  });
  $("gps-save").addEventListener("click", () => placeRequest({op: "gps", source: gpsSource()}, true));
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

// the gateway chosen from the list, and its channels: picking one tunes the radio there (W14)
let chosenGateway = null;

function showChannels() {
  const gw = chosenGateway;
  const fits = gw && kind() === "gateway"
    && $("callsign").value.trim().toUpperCase() === gw.callsign && gw.channels.length > 0;
  $("channel-label").hidden = !fits;
  if (!fits) $("channel").replaceChildren();
  showTuning();
}

function showKind() {
  const gateway = kind() === "gateway";
  showChannels();
  $("fetch-label").hidden = !gateway;
  $("gateways-open").hidden = !gateway;
  $("station-dial").hidden = gateway;
  $("callsign").placeholder = gateway ? "KK4ODA-1" : "N0CALL";
  showTuning();
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
    return $("channel-label").hidden ? null : Number($("channel").value) || null;
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
  if (["idle", "listening"].includes(a.state) && lastState === "session") {
    play(a.failed ? "failed" : "session-down");
  }
  if (a.state === "idle" && lastState === "calling" && a.failed) play("failed");
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
                 renewing: "Renewing credential", alerting: "Sending an alert"}[a.state];
  const state = $("state");
  state.dataset.state = a.state === "idle" && a.failed ? "failed" : a.state;
  state.textContent = a.state === "idle" && a.failed ? "Last session failed" : label;
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
  // a session that ends shows what it brought, whether the station is idle or listening again; a
  // renewal or an alert changes no folder, and must not close a message being written
  if ((lastState !== "idle" && lastState !== "renewing" && lastState !== "alerting"
       && a.state === "idle")
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
}

// Listen for P2P's tooltip: the callsigns it answers as (every callsign of this computer, the one
// chosen first, W46), that it holds until pressed again, and that a gateway on the station is not
// this client's to answer
function listenTitle(s, a) {
  const mine = s.me ? [s.me.operator] : [];
  for (const c of s.callsigns || []) {
    if (c.operator && !mine.includes(c.operator)) mine.push(c.operator);
  }
  const names = mine.length > 1
    ? `${mine.slice(0, -1).join(", ")} or ${mine[mine.length - 1]}`
    : mine[0] || "your callsign";
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
  if (a.state === "calling") {
    mode = "waiting";
    head = "Calling…";
  } else if (ending) {
    mode = "done";
    const all = total > 0 && out.whole === out.messages && inc.whole === inc.messages && !a.failed;
    box.classList.toggle("all", all || (total === 0 && !a.failed));
    head = a.failed ? `Stopped at ${total ? Math.floor(100 * crossed / total) : 0}%`
      : total === 0 ? "Nothing to carry" : all ? "All carried" : "Carried what it could";
  } else if (total === 0) {
    mode = "waiting";
    head = "Checking the mail…";
  } else {
    head = `${Math.floor(100 * crossed / total)}% carried`;
  }
  if (mode !== "done") box.classList.remove("all");
  box.dataset.mode = mode;
  box.dataset.dir = !q || mode !== "carrying" ? ""
    : q.receiving_now ? "down" : out.crossed < out.bytes ? "up" : inc.crossed < inc.bytes ? "down" : "";
  for (const target of [box, mini]) {
    target.dataset.mode = mode;
    if (mode === "waiting") {
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
    const cells = [box, e.from, e.subject + (e.held ? " (attachments held)" : ""),
                   `${(e.size / 1000).toFixed(1)} kB`];
    cells.forEach((c, i) => {
      const td = document.createElement("td");
      if (typeof c === "string") td.textContent = c; else td.append(c);
      if (i === 3) td.className = "n";
      tr.append(td);
    });
    tr.title = `${e.local ? "kept for you at the gateway" : `#${e.serial}`}${e.third_party ? " · third-party traffic" : ""}`;
    return tr;
  }));
  show("waiting-view");
}

// -- the Gateways page (choosing a gateway, §16.1): its views, and the marks laid over the lists --

let gatewaysData = null;
let gwView = kept("gview", "recommended");
let showBlocked = false;

async function openGateways(refreshing = false) {
  markFolder("");
  try {
    gatewaysData = refreshing ? await api("gateways", {}) : await api("gateways");
    show("gateways-view");
    await renderGatewaysPage();
  } catch (e) {
    toast(e.message, true);
  }
}

// a channel as the page writes it: the dial in MHz to the kilohertz or finer, the bandwidth, and
// whether the gateway scans it with others (RP-5 §4.4)
function channelWords(c) {
  return `${mhzOf(c.dial_hz)} MHz, ${c.bandwidth_hz} Hz${c.scan_s != null ? ", scanned" : ""}`;
}

// days or hours since a moment, for the ages the page shows
function age(seconds) {
  const s = Math.max(0, Date.now() / 1000 - seconds);
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 2 * 86400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86400)} days ago`;
}

function viewNote(view, count, g) {
  switch (view) {
    case "recommended":
      return g.here ? "Nearest first: nothing is learned yet to rank them better."
        : "As the post offices list them: save a place or set up a GPS to put the nearest first.";
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
  for (const b of document.querySelectorAll("[data-gview]")) {
    b.setAttribute("aria-selected", String(b.dataset.gview === gwView));
  }
  const recent = gwView === "recent";
  $("gateway-table").hidden = recent;
  $("recent-table").hidden = !recent;
  $("own-form").hidden = recent;
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
  $("gateways-view-note").textContent = viewNote(gwView, rows.length, g)
    + (g.marks_error ? ` Your marks could not be read: ${g.marks_error}.` : "");
  if (blocked.length) {
    $("gateways-left-out").hidden = false;
    $("gateways-left-out-text").textContent = `Left out: ${blocked.length} blocked.`;
    $("gateways-show-blocked").textContent = showBlocked ? "Hide them" : "Show them";
  }
  const twice = new Set(g.ambiguous || []);
  $("gateway-rows").replaceChildren(...rows.map((gw) => gatewayRow(gw, twice)));
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
    gatewaysData = await api("gateway-marks", body);
    await renderGatewaysPage();
  } catch (e) {
    toast(e.message, true);
  }
}

function gatewayRow(gw, twice) {
  const tr = document.createElement("tr");
  if (gw.blocked) tr.classList.add("blocked");
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
    where.append(el("div", {class: "note", text: `record changed ${age(gw.updated)}`,
      title: "When its operator last changed its record"}));
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
    return line;
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
  acts.append(choose, block);
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

// a gateway chosen: its channels in the call box, favourites first and those left out dropped,
// and the radio tuned to the first at once (W14)
function chooseGateway(gw) {
  const channels = (gw.channels || []).filter((c) => !c.blocked)
    .sort((a, b) => Number(Boolean(b.favourite)) - Number(Boolean(a.favourite)));
  chosenGateway = {callsign: gw.callsign, channels};
  $("callsign").value = gw.callsign;
  document.querySelector("input[name=kind][value=gateway]").checked = true;
  keep("kind", "gateway");
  showKind();
  $("channel").replaceChildren(...channels.map((c) =>
    el("option", {value: String(c.dial_hz), text: channelWords(c)})));
  showChannels();
  const first = channels[0];
  const scanned = first && first.scan_s != null
    ? ` It scans its channels: the call keeps trying for ${first.scan_s} s or more.`
    : "";
  if (!first) {
    toast(`${gw.callsign} chosen. ${gw.channels && gw.channels.length ? "Every channel is left out." : "Its record names no channel."}`);
  } else {
    toast(`${gw.callsign} chosen, on ${mhzOf(first.dial_hz)} MHz.${scanned}`);
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
  $("recent-rows").replaceChildren(...r.recent.map((a) => {
    const tr = el("tr", {});
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
      again.addEventListener("click", () => chooseGateway(gw));
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
}

function wireGateways() {
  for (const b of document.querySelectorAll("[data-gview]")) {
    b.addEventListener("click", async () => {
      gwView = b.dataset.gview;
      keep("gview", gwView);
      await renderGatewaysPage();
    });
  }
  $("gateways-show-blocked").addEventListener("click", async () => {
    showBlocked = !showBlocked;
    await renderGatewaysPage();
  });
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
  const select = $("c-form");
  select.replaceChildren(el("option", {value: "", text: "None"}),
    ...templates.map((t) => el("option", {value: t.template, text: t.name})));
}

const DAY_MS = 86400000;

// the composer's fields for the form chosen, or none
function showFormFields() {
  const box = $("c-form-fields");
  const t = templates.find((x) => x.template === $("c-form").value);
  box.hidden = !t;
  $("c-body-name").textContent = t ? "Note" : "Text";
  $("composer").body.rows = t ? 3 : 12;
  if (!t) {
    box.replaceChildren();
    return;
  }
  const legend = el("legend", {text: t.name, title: t.note || ""});
  box.replaceChildren(legend, ...t.fields.filter((f) => !f.bound).map((f) => {
    let input;
    if (f.type === "text") {
      input = el("textarea", {rows: 5});
    } else if (f.type === "choice") {
      input = el("select", {}, el("option", {value: "", text: ""}),
        ...f.choices.map((c, i) => el("option", {value: String(i), text: c})));
    } else {
      input = el("input", {type: {date: "date", time: "time", number: "number",
        check: "checkbox"}[f.type] || "text"});
    }
    if (f.max) input.maxLength = f.max;
    input.dataset.field = String(f.n);
    input.dataset.kind = f.type;
    input.required = !!f.required;
    const label = el("label", {class: f.type === "check" ? "check" : "",
      title: f.required ? "Required" : "Optional: left out when empty"});
    label.append(`${f.label}${f.required ? " *" : ""} `, input);
    return label;
  }));
}

// what the form's fields hold: {template, values} by field number, empty fields left out
function formValues() {
  const template = $("c-form").value;
  if (!template) return null;
  const values = {};
  for (const input of $("c-form-fields").querySelectorAll("[data-field]")) {
    const kind = input.dataset.kind;
    let v;
    if (kind === "check") v = input.checked ? true : null;
    else if (input.value.trim() === "") v = null;
    else if (kind === "date") v = Math.floor(Date.parse(`${input.value}T00:00:00Z`) / DAY_MS);
    else if (kind === "time") {
      const [h, m] = input.value.split(":").map(Number);
      v = h * 60 + m;
    } else if (kind === "number" || kind === "choice") v = Number(input.value);
    else v = kind === "line" ? input.value.trim() : input.value.replace(/\s+$/, "");
    if (v !== null) values[input.dataset.field] = v;
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
  const two = (n) => String(n).padStart(2, "0");
  for (const input of $("c-form-fields").querySelectorAll("[data-field]")) {
    const v = values[input.dataset.field];
    if (v === undefined || v === null) continue;
    const kind = input.dataset.kind;
    if (kind === "check") input.checked = v === true;
    else if (kind === "date") input.value = new Date(v * DAY_MS).toISOString().slice(0, 10);
    else if (kind === "time") input.value = `${two(Math.floor(v / 60))}:${two(v % 60)}`;
    else input.value = String(v);
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
    showFormFields();
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
  const known = addressBook.some((e) => e.address === m.from);
  $("r-from-add").hidden = known || m.from === meAddress;
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
  $("r-form-rows").replaceChildren(...(f?.rows ?? []).flatMap((r) =>
    [el("dt", {text: r.label}), el("dd", {class: r.lines ? "lines" : "", text: r.value})]));
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

// what the message read prints as, by its name: the name a PDF of it is offered
let printTitle = "RadioPost";

// A form as the paper form it comes from, for printing: the program's own layout
// (forms/*.print.html, compiled in, never anything from the air), each field's value put in its
// place as text, then what RadioPost knows of the message and the sender's note.
function paper(m, layout) {
  const t = document.createElement("template");
  t.innerHTML = layout;
  const values = new Map(m.form.rows.map((r) => [String(r.n), r.value]));
  for (const place of t.content.querySelectorAll("[data-field]")) {
    place.textContent = values.get(place.dataset.field) ?? "";
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
  $("c-form").addEventListener("change", showFormFields);
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
        address: form.address.value, operator: form.operator.value, name: form.name.value,
        contact: form.contact.value, url: form.url.value,
      })
      : await api("enroll", {
        address: form.address.value, operator: form.operator.value, code: form.code.value,
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

function addressOf(from) {
  const m = /<([^>]+)>/.exec(from || "");
  return (m ? m[1] : from || "").trim().toLowerCase();
}

function mailLevels() {
  notifySettings.mail = notifySettings.mail || {};
  notifySettings.mail.levels = notifySettings.mail.levels || {};
  return notifySettings.mail.levels;
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

const OUT_STATES = {waiting: "waiting for the modem", sent: "on the air", heard: "heard",
                    seen: "seen", replied: "answered", "not heard": "not heard",
                    failed: "not sent"};

async function syncAlerts(summary, force = false) {
  if (!force && summary && summary.seq === alertsSeq && alertsKnown) return;
  const data = await api("alerts");
  alertsSeq = data.seq;
  alertCircles = data.circles;
  alertsLog = data.log;
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
    + `sent ${new Date(e.time * 1000).toLocaleTimeString([], {hour: "2-digit", minute: "2-digit"})}`;
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

async function openAlerts() {
  folder = "alerts";
  markFolder("alerts");
  await syncAlerts(null, true);
  renderAlerts();
  show("alerts-view");
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
  for (const id of ["alert-circle", "alert-to", "alert-text", "alert-send"]) $(id).disabled = none;
  $("alert-left").textContent = none ? "Add a circle below first." : alertLeft();
  const modem = adapter();
  $("alerts-modem").textContent = modem.startsWith("ws://")
    ? ""
    : `Alerts go only through a modem that carries broadcasts, as Aether does. ${modem || "The modem"} is used for calls; VARA, ARDOP, Mercury and packet carry no alerts.`;
  $("alerts-modem").hidden = modem.startsWith("ws://");
  $("alert-rows").replaceChildren(...alertsLog.map((e) => {
    const tr = el("tr", {class: e.alarm ? "alarming" : e.quiet ? "quiet" : ""});
    tr.append(el("td", {class: "date", text: when(e.at), title: new Date(e.at * 1000).toLocaleString()}));
    tr.append(el("td", {class: "way", text: `${e.out ? "You" : e.from} → ${e.to || "everyone"}`}));
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
  keep("alertCircle", circle);
  try {
    await api("alerts", {send: {circle, to, text}});
    $("alert-text").value = "";
    toast(`Alert to ${to || `everyone in ${circle}`} waiting for the modem`);
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
  const add = $("circle-add");
  add.disabled = true;
  try {
    await api("circles", {add: {name, keyword, level: $("circle-level").value}});
    $("circle-name").value = "";
    $("circle-keyword").value = "";
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
  $("alert-text").addEventListener("input", () => { $("alert-left").textContent = alertLeft(); });
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
    await alertDo({ack: alarmShown.tag});
    document.querySelector("input[name=kind][value=station]").checked = true;
    showKind();
    $("callsign").value = who;
    toast(`Press Call to call ${who}`);
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
    radio.addEventListener("change", () => { keep("kind", kind()); showKind(); });
  }
  $("fetch").value = kept("fetch", "all");
  $("fetch").addEventListener("change", () => keep("fetch", $("fetch").value));
  showKind();
  $("gateways-open").addEventListener("click", () => openGateways());
  $("gateways-refresh").addEventListener("click", async () => {
    $("gateways-refresh").disabled = true;
    await openGateways(true);
    $("gateways-refresh").disabled = false;
  });
  wirePlaces();
  wireGateways();
  // the channels are the chosen gateway's: another callsign typed has none here
  $("callsign").addEventListener("input", showChannels);
  // a channel picked, or a station call's dial typed, is tuned at once (W14)
  $("channel").addEventListener("change", pickChannel);
  $("dial").value = kept("dial", "");
  $("dial").addEventListener("input", showTuning);
  $("dial").addEventListener("change", () => {
    const text = $("dial").value.trim();
    if (text && !hzOf(text)) { toast("The dial is a frequency in MHz, such as 7.101.", true); return; }
    keep("dial", text);
    pickChannel();
  });
  $("bandwidth").value = kept("bandwidth", "2300");
  $("bandwidth").addEventListener("change", () => keep("bandwidth", $("bandwidth").value));
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
    const channel = !station && dial && chosenGateway
      ? chosenGateway.channels.find((c) => c.dial_hz === dial) : null;
    act("call", {
      callsign, station, adapter: adapter(),
      fetch: /^[0-9]+$/.test(fetchRule) ? Number(fetchRule) : fetchRule,
      dial_hz: dial || null,
      call_s: channel && channel.scan_s != null ? channel.scan_s : null,
      // a station call's bandwidth, as the two operators agreed; a gateway call names none
      bandwidth: station ? Number($("bandwidth").value) : null,
    });
  });
  // one switch: pressed in, it listens; pressed again, it stops
  $("listen").addEventListener("click", () => {
    if ($("listen").getAttribute("aria-pressed") === "true") act("stop", {});
    else act("listen", {adapter: adapter()});
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
