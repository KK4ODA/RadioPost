// SPDX-License-Identifier: MIT OR Apache-2.0
// The gateway's panel: it asks the gateway's own server (core/gateway/src/panel.rs) for what the
// gateway is doing, once a second, and sends the operator's changes with X-RadioPost: 1.

const $ = (id) => document.getElementById(id);

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

let toastTimer = 0;
function toast(text, bad = false) {
  const t = $("toast");
  t.textContent = text;
  t.classList.toggle("bad", bad);
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { t.hidden = true; }, bad ? 9000 : 5000);
}

async function act(what, body, done) {
  try {
    const answer = await api(what, body);
    if (done) done(answer);
    return answer;
  } catch (e) {
    toast(e.message, true);
    return null;
  }
}

const when = (secs) => new Date(secs * 1000)
  .toLocaleString(undefined, {dateStyle: "short", timeStyle: "short"});
const day = (secs) => new Date(secs * 1000).toLocaleDateString();

function length(from, to) {
  const s = Math.max(0, to - from);
  if (s < 60) return `${s} s`;
  return s % 60 ? `${Math.floor(s / 60)} min ${s % 60} s` : `${s / 60} min`;
}

// how a post office failed a session, in words
function postOffice(code) {
  if (code === "post-office-unreachable") return "the post office did not answer";
  return `the post office said ${code.replaceAll("-", " ")}`;
}

const STATES = {
  stopped: "Stopped",
  opening: "Waiting for the modem",
  listening: "Listening",
  outside: "Not answering here",
  session: "Session",
};

let state = null;
let shownSessions = -1;
let formsFilled = false;

function show(view) {
  $("station-view").hidden = view !== "station";
  $("setup").hidden = view !== "setup";
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
  const at = u.checked ? when(u.checked) : "";
  said.textContent = `This is ${name} ${u.current}. ` + (u.checking ? "Checking…"
    : !u.checked ? "Not checked yet."
    : u.error ? `Checked ${at}, but the update channel could not be reached: ${u.error}`
    : u.offer ? `Checked ${at}: ${u.offer.version} is out, at the top of the window.`
    : `Checked ${at}: this is the newest.`);
  $("updates-check").disabled = Boolean(u.checking);
}

function render(s) {
  state = s;
  showUpdate(s.update, "RadioPost Gateway");
  showVersion(s.update, "RadioPost Gateway");
  const settings = s.settings;
  const st = s.status;
  const answering = s.running;
  const label = STATES[st.state] || st.state;
  $("state").textContent = label;
  $("state").dataset.state = st.state === "stopped" && st.problem ? "problem" : st.state;
  $("state").title = st.problem || `${label} since ${when(st.since)}`;
  $("start").hidden = answering;
  $("stop").hidden = !answering;
  $("start").disabled = !settings || !s.credential;
  $("who").textContent = settings
    ? `${settings.callsign}${s.credential ? ` · ${s.credential.address}` : ""}`
    : "";

  // what keeps the gateway from answering
  let missing = "";
  if (!settings) missing = "This gateway is not set up yet: its callsign and its modem.";
  else if (!s.credential) missing = "This gateway is not registered with a post office yet.";
  $("not-ready").hidden = !missing;
  $("not-ready-text").textContent = missing;

  $("modem").textContent = settings
    ? [settings.modem, ...(settings.more_modems || [])].join(" · ")
    : "—";
  $("modem-state").textContent = st.problem
    || (st.state === "listening" || st.state === "session"
      ? `Listening as ${settings.callsign} since ${when(st.since)}`
      : st.state === "stopped" ? "Not answering." : "");
  $("scan-line").hidden = !st.scan;
  $("scan-line").textContent = scanLine(st.scan);

  $("checks").replaceChildren(...(st.checks.length ? st.checks : []).map((c) => {
    const li = document.createElement("li");
    li.className = "check";
    const name = document.createElement("span");
    name.textContent = c.domain;
    name.title = c.url || "";
    const said = document.createElement("span");
    said.className = c.ok ? "ok" : "bad";
    said.textContent = c.ok ? "the gateway logs in" : c.said;
    said.title = `Checked ${when(c.at)}`;
    li.append(name, said);
    return li;
  }));
  if (!st.checks.length) {
    const li = document.createElement("li");
    li.className = "note";
    li.textContent = answering ? "Checking…" : "Checked while the gateway answers.";
    $("checks").append(li);
  }
  $("check").disabled = !s.credential;

  const left = s.credential ? s.credential.expires - Date.now() / 1000 : 0;
  $("registration").textContent = !s.credential ? "Not registered."
    : left <= 0 ? `The credential ran out on ${day(s.credential.expires)}: register again (Setup).`
      : left < 14 * 86400
        ? `The credential runs out on ${day(s.credential.expires)} and has not renewed itself (the log says why): register again soon (Setup).`
        : `Registered with ${domainOf(s.credential.address)}'s post office for ${s.credential.address}: credential ${s.credential.serial}, until ${day(s.credential.expires)}.`;
  $("registration").className = s.credential && left < 14 * 86400 ? "bad" : "";

  const now = st.session;
  $("session").hidden = !now;
  if (now) {
    $("s-remote").textContent = now.remote;
    $("s-account").textContent = now.account || "—";
    $("s-since").textContent = when(now.started);
    $("s-in").textContent = now.handed_in;
    $("s-out").textContent = now.picked_up;
  }

  const h = s.held || {};
  $("held-copies").textContent = !settings || !settings.copies
    ? "Copies placed ahead: not held (Setup)."
    : `${h.copies || 0} cop${h.copies === 1 ? "y" : "ies"} held for ${h.users || 0} user${h.users === 1 ? "" : "s"}.`;
  $("held-queue").textContent = h.records
    ? `${h.records} session${h.records === 1 ? "" : "s"} carried while cut off, with ${h.queued} message${h.queued === 1 ? "" : "s"}, waiting to be reported.`
    : "Nothing waiting to be reported.";
  $("held-queue").className = h.records ? "bad" : "";

  $("setup-open").hidden = !settings;
  if (!formsFilled) fillForms(s);
  renderPostOffices(s);
  $("key-line").hidden = !s.key;
  $("key").textContent = s.key || "";
  renderRegistered(s);
  planLine();
  $("where").textContent = `The gateway's files: ${s.dir}. RadioPost Gateway ${s.version}.`;
  $("held").hidden = !answering;
  for (const form of ["gateway-form", "register-form", "po-form"]) {
    for (const el of $(form).elements) el.disabled = answering;
  }
}

// the first modem's scan (W50): its channels, where the radio is, and the call's length; or why it
// is not scanning though set to
function scanLine(scan) {
  if (!scan) return "";
  if (!scan.dials.length) return scan.not || "Not scanning.";
  const mhz = (hz) => (hz / 1e6).toFixed(3);
  const at = scan.at_hz ? `, on ${mhz(scan.at_hz)} MHz` : "";
  const last = scan.scan_s != null ? `: a call must keep trying ${scan.scan_s} s` : "";
  return `Scanning ${scan.dials.length} channels${at}${last}.`;
}

function fillForms(s) {
  formsFilled = true;
  const g = $("gateway-form").elements;
  const settings = s.settings;
  g.callsign.value = settings ? settings.callsign : "";
  g.modem.value = settings ? settings.modem : s.aether;
  g.more_modems.value = settings ? (settings.more_modems || []).join("\n") : "";
  g.country.value = settings ? settings.country : "US";
  g.operator_checks_rules.checked = settings ? settings.operator_checks_rules : false;
  g.answer_at_start.checked = settings ? settings.answer_at_start : true;
  const copies = settings && settings.copies;
  g.copies_on.checked = !!copies;
  if (copies) {
    g.copies_users.value = copies.users;
    g.copies_days.value = copies.days;
    g.copies_kb.value = Math.round(copies.max_air_bytes / 1024);
  }
  const r = $("register-form").elements;
  r.home.value = s.client.home;
  fillAccounts(s.client.accounts, s.registration);
  const scan = (settings && settings.scan) || [];
  r.scan.checked = scan.length > 0;
  // the record last registered, or withdrawn, fills the rest: registering again changes only
  // what the operator changes
  const last = s.last_record;
  if (last) {
    for (const k of ["grid", "place", "hours"]) if (last[k]) r[k].value = last[k];
    r.listens.value = listensText(last.listens);
    const site = last.site || {};
    for (const [field, k] of [["site_lat", "lat"], ["site_lon", "lon"], ["site_agl", "agl_m"]]) {
      if (site[k] != null) r[field].value = String(site[k]);
    }
  }
  if (!$("channel-rows").children.length) {
    const channels = (last && last.channels) || [];
    if (channels.length) for (const c of channels) addChannel(c.dial_hz / 1e6, c.bandwidth_hz, c.modem, c.station);
    else if (scan.length) for (const hz of scan) addChannel(hz / 1e6, 2300);
    else addChannel(7.101, 2300);
  }
}

const domainOf = (address) => (address || "").split("@").pop();

// the accounts of the RadioPost app (W46): the one the gateway is registered with chosen, else the
// one the app has chosen
let accountsShown = [];
function fillAccounts(accounts, registration) {
  accountsShown = accounts || [];
  const select = $("register-form").elements.account;
  const was = select.value;
  select.replaceChildren(...accountsShown.map((a) => {
    const o = document.createElement("option");
    o.value = a.address;
    o.textContent = `${a.operator} · ${a.address} (${a.domain})`;
    return o;
  }));
  const pick = accountsShown.find((a) => a.address === was)
    || accountsShown.find((a) => registration && a.address === registration.address)
    || accountsShown.find((a) => a.chosen)
    || accountsShown[0];
  if (pick) select.value = pick.address;
  planLine();
}

// what the gateway is registered as now, with Unregister beside it
function renderRegistered(s) {
  const r = s.registration;
  $("registered").hidden = !r;
  if (!r) return;
  const at = r.post_office ? ` (${r.post_office})` : "";
  $("registered-text").textContent = `${r.callsign} is registered with ${r.domain}'s post office${at}, from ${r.operator ? `${r.operator} · ` : ""}${r.address}.`;
}

// what Register will do, in words, before it is clicked
function planLine() {
  const f = $("register-form").elements;
  const a = accountsShown.find((x) => x.address === f.account.value);
  const callsign = (state && state.settings && state.settings.callsign) || "The gateway";
  const now = state && state.registration;
  $("register-go").textContent = a ? `Register with ${a.domain}` : "Register";
  $("register-go").disabled = !a || (state && state.running);
  $("enrolled").className = "note";
  if (!a) {
    $("enrolled").textContent = "The RadioPost app at this home holds no enrolled account: enroll it first (its Setup), or give the home of one that is.";
  } else if (now && now.domain !== a.domain) {
    $("enrolled").className = "note bad";
    $("enrolled").textContent = `${callsign} is registered with ${now.domain}'s post office now. Unregister it there first, then register it with ${a.domain}'s.`;
  } else {
    $("enrolled").textContent = `${callsign} registers with ${a.domain}'s post office, logged in as ${a.operator}, ${a.address}: it is listed there, and its credential names that address.`;
  }
}

function renderPostOffices(s) {
  const named = s.settings ? Object.entries(s.settings.post_offices) : [];
  $("po-list").replaceChildren(...named.map(([domain, url]) => {
    const li = document.createElement("li");
    const text = document.createElement("span");
    text.className = "mono";
    text.textContent = `${domain}: ${url}`;
    const forget = document.createElement("button");
    forget.type = "button";
    forget.className = "quiet";
    forget.textContent = "Forget";
    forget.disabled = s.running;
    forget.addEventListener("click", () => act("post-office", {domain, url: ""}, render));
    li.append(text, forget);
    return li;
  }));
}

// RP-0 §9.4's modem names a channel may give: each one's radio mode and bandwidths (RP-5 §4.4: on
// an FM channel, the channel's width)
const MODEMS = [
  {name: "aether", label: "Aether HF", mode: "USB", bandwidths: [2300, 500]},
  {name: "mercury", label: "Mercury", mode: "USB", bandwidths: [2300, 500, 2750]},
  {name: "vara-hf", label: "VARA HF", mode: "USB", bandwidths: [2300, 500, 2750]},
  {name: "ardop", label: "ARDOP", mode: "USB", bandwidths: [2000, 1000, 500, 200]},
  {name: "vara-fm", label: "VARA FM", mode: "FM", bandwidths: [12500, 25000]},
  {name: "ax25", label: "Packet", mode: "FM", bandwidths: [12500, 25000]},
];
const modemNamed = (name) => MODEMS.find((m) => m.name === name) || MODEMS[0];
// the kinds of antenna a channel's station may name (RP-0 §9.6)
const ANTENNAS = ["vertical", "dipole", "yagi", "j-pole", "collinear", "loop", "nvis", "whip", "other"];
const STATION = [
  ["power_w", "Power, W", "The transmitter's power, watts"],
  ["antenna", "Antenna", "The kind of antenna"],
  ["agl_m", "Height, m", "The antenna's height above the ground, metres: this channel's, in place of the site's"],
  ["gain_dbi", "Gain, dBi", "The antenna's gain over isotropic"],
  ["azimuth_deg", "Points to, °", "Where a beam points, degrees east of true north. Empty: the same all round"],
];

// a channel's station (RP-5 §4.4): its fields in a row of their own under the channel's, opened
// from a button in the channel's row that says what was given
function stationFields(station, onChange) {
  const fields = document.createElement("div");
  fields.className = "station-fields";
  for (const [k, label, title] of STATION) {
    const l = document.createElement("label");
    l.textContent = label;
    l.title = title;
    let input;
    if (k === "antenna") {
      input = document.createElement("select");
      input.className = "narrow";
      for (const a of ["", ...ANTENNAS]) {
        const o = document.createElement("option");
        o.value = a;
        o.textContent = a || "—";
        input.append(o);
      }
    } else {
      input = document.createElement("input");
      input.inputMode = "decimal";
      input.spellcheck = false;
    }
    input.dataset.k = k;
    input.value = station && station[k] != null ? String(station[k]) : "";
    input.addEventListener("input", onChange);
    input.addEventListener("change", onChange);
    l.append(input);
    fields.append(l);
  }
  return fields;
}

// what a station holds, in a few words: 50 W, j-pole, 18 m, 3 dBi, to 270°
function stationWords(fields) {
  const v = Object.fromEntries([...fields.querySelectorAll("[data-k]")].map((i) => [i.dataset.k, i.value.trim()]));
  const words = [v.power_w && `${v.power_w} W`, v.antenna, v.agl_m && `${v.agl_m} m`,
                 v.gain_dbi && `${v.gain_dbi} dBi`, v.azimuth_deg && `to ${v.azimuth_deg}°`].filter(Boolean);
  return words.length ? words.join(", ") : "Not given";
}

function addChannel(mhz = "", bandwidth = 2300, modemName = "aether", station = null) {
  const tr = document.createElement("tr");
  tr.className = "channel";
  const sub = document.createElement("tr");
  sub.className = "station-row";
  sub.hidden = true;
  const open = document.createElement("button");
  open.type = "button";
  open.className = "quiet station-open";
  open.title = "How the gateway transmits and receives on this channel: click to give or change it";
  const fields = stationFields(station, () => { open.textContent = stationWords(fields); });
  open.textContent = stationWords(fields);
  open.setAttribute("aria-expanded", "false");
  open.addEventListener("click", () => {
    sub.hidden = !sub.hidden;
    open.setAttribute("aria-expanded", String(!sub.hidden));
  });
  const cell = document.createElement("td");
  cell.colSpan = 5;
  cell.append(fields);
  sub.append(cell);
  tr.stationRow = sub;
  const dial = document.createElement("input");
  dial.className = "ch-dial";
  dial.type = "number";
  dial.step = "0.0001";
  dial.min = "1.8";
  dial.placeholder = "7.101";
  dial.value = mhz;
  const modem = document.createElement("select");
  modem.className = "narrow ch-modem";
  for (const m of MODEMS) {
    const o = document.createElement("option");
    o.value = m.name;
    o.textContent = m.label;
    modem.append(o);
  }
  modem.value = modemNamed(modemName).name;
  modem.title = "The modem callers use on this channel";
  const bw = document.createElement("select");
  bw.className = "narrow ch-bw";
  const fill = (want) => {
    const m = modemNamed(modem.value);
    bw.replaceChildren(...m.bandwidths.map((hz) => {
      const o = document.createElement("option");
      o.value = hz;
      o.textContent = hz >= 10000 ? `${hz / 1000} kHz` : `${hz} Hz`;
      return o;
    }));
    bw.value = m.bandwidths.includes(Number(want)) ? want : m.bandwidths[0];
    bw.title = m.mode === "FM"
      ? "The FM channel's width"
      : `${m.label}'s bandwidth on this channel`;
    dial.title = m.mode === "FM"
      ? "The channel's carrier frequency, in MHz, on an FM radio"
      : "The dial frequency, in MHz, upper sideband";
  };
  fill(bandwidth);
  modem.addEventListener("change", () => fill(bw.value));
  const remove = document.createElement("button");
  remove.type = "button";
  remove.className = "quiet";
  remove.textContent = "Remove";
  remove.addEventListener("click", () => { tr.remove(); sub.remove(); });
  for (const el of [dial, modem, bw, open, remove]) {
    const td = document.createElement("td");
    td.append(el);
    tr.append(td);
  }
  $("channel-rows").append(tr, sub);
}

// what the gateway holds: copies by user and the queue, never a message's text
async function refreshHeld() {
  if ($("held-view").hidden) return;
  const h = await api("held");
  $("copy-rows").replaceChildren(...h.copies.filter((c) => !c.picked).map((c) => {
    const tr = document.createElement("tr");
    [c.address, c.serial, c.from, c.subject, c.size, c.expires ? day(c.expires) : ""]
      .forEach((text, i) => {
        const td = document.createElement("td");
        td.textContent = text;
        if (i === 1 || i === 4) td.className = "n";
        if (i === 4 && c.held) td.title = "Too large to hold its text: listed only";
        tr.append(td);
      });
    return tr;
  }));
  $("queue-rows").replaceChildren(...h.queue.map((r) => {
    const tr = document.createElement("tr");
    const via = r.path && r.path.length ? `, via ${r.path.slice(0, -1).join(", ")}` : "";
    [when(r.started), r.operator + via, r.address, r.messages, r.pickups,
     r.error ? `${r.error} (${r.tries})` : r.done ? "not tried yet" : "under way"]
      .forEach((text, i) => {
        const td = document.createElement("td");
        td.textContent = text;
        if (i === 3 || i === 4) td.className = "n";
        tr.append(td);
      });
    return tr;
  }));
  $("reported").replaceChildren(...h.reported.slice().reverse().slice(0, 10).map((r) => {
    const li = document.createElement("li");
    li.textContent = `${when(r.at)}: ${r.address}, ${r.code ? `refused (${r.code})`
      : `${r.taken} taken${r.refused ? `, ${r.refused} refused` : ""}, ${r.pickups} picked up`}`;
    return li;
  }));
}

// the callers refused (wishlist W24), read at each call
async function refreshRefused() {
  const list = await api("refused");
  $("refused-count").textContent = list.length
    ? `${list.length} ${list.length === 1 ? "caller" : "callers"}: ${list.slice(0, 5).map((r) => r.caller).join(", ")}${list.length > 5 ? ` and ${list.length - 5} more` : ""}`
    : "None.";
  $("refused-rows").replaceChildren(...list.map((r) => {
    const tr = document.createElement("tr");
    [r.caller, r.note || "", r.at ? day(r.at) : ""].forEach((text, i) => {
      const td = document.createElement("td");
      td.textContent = text;
      if (i === 0) td.className = "mono";
      tr.append(td);
    });
    const td = document.createElement("td");
    const b = document.createElement("button");
    b.className = "quiet";
    b.textContent = "Carry again";
    b.title = `Carry ${r.caller}'s sessions again from its next call on`;
    b.addEventListener("click", () => act("refused", {remove: r.caller}, () => {
      toast(`${r.caller}'s sessions are carried again from its next call on.`);
      refreshRefused();
    }));
    td.append(b);
    tr.append(td);
    return tr;
  }));
  $("refused-rows").closest(".table-wrap").hidden = list.length === 0;
}

// partners (RP-7 §1): the queue goes to one by radio while the Internet is down
async function refreshPartners() {
  const list = await api("partners");
  $("partners-count").textContent = list.length
    ? list.map((p) => p.callsign).join(", ")
    : "None.";
  $("partner-rows").replaceChildren(...list.map((p) => {
    const tr = document.createElement("tr");
    const last = p.called ? `${when(p.called)}: ${p.said}` : "Never called";
    const held = p.forwards ? `; ${p.forwards} held for it` : "";
    const waiting = (p.records
      ? `${p.records}${p.messages ? ` (${p.messages} ${p.messages === 1 ? "message" : "messages"})` : ""}`
      : "0") + held;
    const at = Date.now() / 1000;
    const next = p.next_window == null
      ? "None published"
      : p.next_window <= at ? "Open now" : when(p.next_window);
    [p.callsign, waiting, next, last].forEach((text, i) => {
      const td = document.createElement("td");
      td.textContent = text;
      if (i === 0) td.className = "mono";
      if (i === 1) td.className = "n";
      tr.append(td);
    });
    const td = document.createElement("td");
    const go = document.createElement("button");
    go.textContent = "Forward now";
    go.title = `Call ${p.callsign} between sessions and hand it the sessions waiting to be reported`;
    go.disabled = !(state && state.running);
    go.addEventListener("click", () => act("forward", {partner: p.callsign}, (a) => toast(`${a.said[0].toUpperCase()}${a.said.slice(1)}.`)));
    const drop = document.createElement("button");
    drop.className = "quiet";
    drop.textContent = "Remove";
    drop.title = `No longer forward to ${p.callsign}, nor answer its forwarding calls`;
    drop.disabled = !!(state && state.running);
    drop.addEventListener("click", () => act("partners", {remove: p.callsign}, () => {
      toast(`${p.callsign} is no longer a partner.`);
      refreshPartners();
    }));
    td.className = "nowrap";
    td.append(go, " ", drop);
    tr.append(td);
    return tr;
  }));
  $("partner-rows").closest(".table-wrap").hidden = list.length === 0;
}

// the gateway's own windows (RP-7 §5): when its partners may call it
const EVERY = {3600: "Every hour", 10800: "Every 3 hours", 21600: "Every 6 hours",
  43200: "Every 12 hours", 86400: "Every day"};
const two = (n) => String(n).padStart(2, "0");

function schedule(w) {
  const every = EVERY[w.every_s] || `Every ${w.every_s} s`;
  const minutes = two(Math.floor(w.at_s / 60) % 60);
  const at = w.every_s === 3600
    ? `at :${minutes}`
    : `at ${two(Math.floor(w.at_s / 3600) % 24)}:${minutes} UTC`;
  return `${every} ${at}, for ${Math.round(w.length_s / 60)} min`;
}

async function refreshWindows() {
  const list = await api("windows");
  $("window-rows").replaceChildren(...list.map((w, i) => {
    const tr = document.createElement("tr");
    const c = w.channel;
    const channel = `${(c.dial_hz / 1e6).toFixed(4)} MHz ${c.mode}, ${c.modem} ${c.bandwidth_hz} Hz`;
    const published = w.why ? `No: ${w.why}` : "Yes";
    [channel, schedule(w), published].forEach((text) => {
      const td = document.createElement("td");
      td.textContent = text;
      tr.append(td);
    });
    const td = document.createElement("td");
    const drop = document.createElement("button");
    drop.className = "quiet";
    drop.textContent = "Remove";
    drop.title = "Take this window off; register again in Setup to publish the change";
    drop.disabled = !!(state && state.running);
    drop.addEventListener("click", () => act("windows", {remove: i + 1}, () => {
      toast("Window removed. Register again in Setup to publish the change.");
      refreshWindows();
    }));
    td.append(drop);
    tr.append(td);
    return tr;
  }));
  $("window-rows").closest(".table-wrap").hidden = list.length === 0;
}

async function refreshSessions() {
  const sessions = await api("sessions");
  shownSessions = sessions.length;
  $("rows").replaceChildren(...sessions.map((r) => {
    const tr = document.createElement("tr");
    const ended = r.result === "over" && !r.refused;
    tr.className = ended ? "" : "failed";
    const cells = [
      when(r.started), r.remote, r.account || "—",
      r.ended ? length(r.started, r.ended) : "",
      r.handed_in, r.picked_up,
      r.refused ? `refused: you refuse ${r.refused}`
        : ended ? "over" : r.result + (r.post_office ? `; ${postOffice(r.post_office)}` : ""),
    ];
    cells.forEach((text, i) => {
      const td = document.createElement("td");
      td.textContent = text;
      if (i === 4 || i === 5) td.className = "n";
      if (i === 3) td.className = "nowrap";
      tr.append(td);
    });
    return tr;
  }));
  $("empty").hidden = sessions.length > 0;
  $("rows").closest(".table-wrap").hidden = sessions.length === 0;
}

let logNext = 0;
async function refreshLog() {
  const {lines, next} = await api(`log?since=${logNext}`);
  if (!lines.length) return;
  logNext = next;
  const pre = $("log");
  const atEnd = pre.scrollTop + pre.clientHeight >= pre.scrollHeight - 4;
  pre.textContent += lines
    .map(([t, text]) => `${new Date(t * 1000).toLocaleTimeString()}  ${text}\n`).join("");
  if (atEnd) pre.scrollTop = pre.scrollHeight;
}

let lastLogged = -1;
async function tick() {
  try {
    const s = await api("state");
    render(s);
    if (s.status.logged !== lastLogged) {
      lastLogged = s.status.logged;
      await refreshLog();
      await refreshSessions();
      await refreshHeld();
      await refreshRefused();
      await refreshPartners();
      if (!$("partners-view").hidden) await refreshWindows();
    } else if (shownSessions < 0) {
      await refreshSessions();
    }
  } catch (e) {
    $("state").textContent = "No answer";
    $("state").dataset.state = "problem";
    $("state").title = `The gateway's panel does not answer: ${e.message}`;
  }
}

$("start").addEventListener("click", () => act("start", {}, render));
$("stop").addEventListener("click", async () => {
  if (state && state.status.session
      && !window.confirm(`A session with ${state.status.session.remote} is under way. Stop it?`)) {
    return;
  }
  $("stop").disabled = true;
  await act("stop", {}, render);
  $("stop").disabled = false;
});
$("updates-check").addEventListener("click", () => act("update/check", {}, render));
$("update-go").addEventListener("click", () => {
  if (state && state.update.can_install && state.running
      && !window.confirm("The gateway stops answering while it updates, and starts again after. Update now?")) {
    return;
  }
  act("update", {}, render);
});
$("check").addEventListener("click", async () => {
  $("check").disabled = true;
  await act("check", {}, render);
  $("check").disabled = false;
});
$("refused-open").addEventListener("click", () => {
  $("refused-view").hidden = !$("refused-view").hidden;
  $("refused-open").textContent = $("refused-view").hidden ? "Show" : "Hide";
  if (!$("refused-view").hidden) $("refuse-form").caller.focus();
});
$("refuse-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const f = e.currentTarget;
  const typed = f.caller.value.trim();
  act("refused", {caller: typed, note: f.note.value.trim()}, (list) => {
    const kept = list.find((r) => r.caller.toLowerCase() === typed.toLowerCase());
    toast(`Refusing ${kept ? kept.caller : typed} from its next call on.`);
    f.reset();
    refreshRefused();
  });
});
$("partners-open").addEventListener("click", () => {
  $("partners-view").hidden = !$("partners-view").hidden;
  $("partners-open").textContent = $("partners-view").hidden ? "Show" : "Hide";
  if (!$("partners-view").hidden) {
    refreshPartners();
    refreshWindows();
  }
});
$("window-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const f = e.currentTarget;
  const every = Number(f.every.value);
  const [h, m] = f.at.value.split(":").map(Number);
  const add = {
    dial_hz: Math.round(Number(f.dial.value.replace(",", ".")) * 1e6),
    mode: "USB",
    modem: "aether",
    bandwidth_hz: Number(f.bandwidth.value),
    every_s: every,
    at_s: (h * 3600 + m * 60) % every,
    length_s: Number(f.length.value) * 60,
  };
  act("windows", {add}, () => {
    toast("Window added. Register again in Setup to publish it.");
    refreshWindows();
  });
});
$("partner-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const f = e.currentTarget;
  const typed = f.callsign.value.trim().toUpperCase();
  act("partners", {add: typed}, () => {
    toast(`${typed} is a partner. Name this gateway on ${typed} too.`);
    f.reset();
    refreshPartners();
  });
});
$("held-open").addEventListener("click", () => {
  $("held-view").hidden = !$("held-view").hidden;
  $("held-open").textContent = $("held-view").hidden ? "Show" : "Hide";
  refreshHeld();
});
$("setup-open").addEventListener("click", () => show("setup"));
$("setup-go").addEventListener("click", () => show("setup"));
$("back").addEventListener("click", () => show("station"));
$("add-channel").addEventListener("click", () => addChannel());

$("gateway-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const f = e.target.elements;
  act("setup", {
    callsign: f.callsign.value, modem: f.modem.value, country: f.country.value,
    more_modems: f.more_modems.value.split("\n").map((m) => m.trim()).filter(Boolean),
    operator_checks_rules: f.operator_checks_rules.checked,
    answer_at_start: f.answer_at_start.checked,
    copies: f.copies_on.checked
      ? {users: Number(f.copies_users.value), days: Number(f.copies_days.value),
         max_air_bytes: Math.round(Number(f.copies_kb.value) * 1024)}
      : null,
  }, (s) => { render(s); toast(s.credential ? "Saved. Register again so the post office knows." : "Saved."); });
});

// a record's daily listening windows (RP-5 §4.4) as the form types them: 18:00-02:00, 08:00-10:00
function listensText(windows) {
  const clock = (s) => `${String(Math.floor(s / 3600)).padStart(2, "0")}:${String(Math.floor(s % 3600 / 60)).padStart(2, "0")}`;
  return (windows || []).filter((w) => w.every_s === 86400)
    .map((w) => `${clock(w.at_s)}-${clock((w.at_s + w.length_s) % 86400)}`).join(", ");
}

$("register-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const f = e.target.elements;
  const channels = [...$("channel-rows").querySelectorAll("tr.channel")].map((tr) => {
    const [dial, modem, bw] = [".ch-dial", ".ch-modem", ".ch-bw"].map((c) => tr.querySelector(c));
    const station = Object.fromEntries([...tr.stationRow.querySelectorAll("[data-k]")]
      .map((i) => [i.dataset.k, i.value.trim()]));
    return {dial_hz: Math.round(Number(dial.value) * 1e6), mode: modemNamed(modem.value).mode,
            modem: modem.value, bandwidth_hz: Number(bw.value), station};
  }).filter((c) => c.dial_hz > 0);
  act("register", {home: f.home.value, account: f.account.value, grid: f.grid.value, place: f.place.value,
                   hours: f.hours.value, listens: f.listens.value, channels,
                   site: {lat: f.site_lat.value, lon: f.site_lon.value, agl_m: f.site_agl.value},
                   scan: f.scan.checked ? channels.map((c) => c.dial_hz) : []},
      (a) => { render(a.state); toast(a.said); });
});

$("register-form").elements.account.addEventListener("change", planLine);
$("register-form").elements.home.addEventListener("change", (e) => {
  act("accounts", {home: e.target.value}, (a) => fillAccounts(a.accounts, state && state.registration));
});

// a second click within four seconds unregisters
$("unregister").addEventListener("click", (e) => {
  const b = e.currentTarget;
  if (!b.dataset.armed) {
    b.dataset.armed = "1";
    b.textContent = "Click again to unregister";
    setTimeout(() => { delete b.dataset.armed; b.textContent = "Unregister"; }, 4000);
    return;
  }
  delete b.dataset.armed;
  b.textContent = "Unregister";
  act("unregister", {}, (a) => { render(a.state); toast(a.said); });
});

$("keep-credential").addEventListener("click", () => {
  const hex = $("register-form").elements.credential.value;
  act("credential", {hex}, (s) => { render(s); toast("Credential kept."); });
});

$("po-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const f = e.target.elements;
  act("post-office", {domain: f.domain.value, url: f.url.value}, (s) => {
    render(s);
    f.domain.value = "";
    f.url.value = "";
  });
});

show("station");
await tick();
setInterval(tick, 1000);
