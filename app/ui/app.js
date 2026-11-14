// SPDX-License-Identifier: MIT OR Apache-2.0
// The RadioPost client's panel: the folders, a reader, a composer, the station's calls and its
// setup, over the client's JSON API (app/client/src/panel.rs). No build step.

const $ = (id) => document.getElementById(id);
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
  for (const id of ["list-view", "waiting-view", "gateways-view", "addresses-view", "reader",
                    "composer", "setup", "office"]) {
    $(id).hidden = id !== view;
  }
  // the usual gateways are read afresh each time, for the callsign in use now
  if (view === "setup") loadUsual();
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

function showKind() {
  const gateway = kind() === "gateway";
  $("fetch-label").hidden = !gateway;
  $("gateways-open").hidden = !gateway;
  $("callsign").placeholder = gateway ? "KK4ODA-1" : "N0CALL";
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

async function refresh() {
  const s = await api("state");
  showUpdate(s.update, "RadioPost");
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
  const label = {idle: "Idle", calling: `Calling ${a.remote || ""}`, listening: "Listening",
                 session: `In session with ${a.remote || ""}`,
                 renewing: "Renewing credential"}[a.state];
  const state = $("state");
  state.dataset.state = a.state === "idle" && a.failed ? "failed" : a.state;
  state.textContent = a.state === "idle" && a.failed ? "Last session failed" : label;
  state.title = a.failed || "What the station is doing";
  const busy = a.state !== "idle";
  document.body.dataset.busy = String(busy);
  $("call").disabled = busy || !s.me;
  $("listen").disabled = busy || !s.me;
  $("stop").hidden = a.state !== "listening";
  $("write").disabled = busy || !s.me;
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
  // a session that ends shows what it brought; a renewal changes no folder, and must not close
  // a message being written
  if (lastState !== "idle" && lastState !== "renewing" && a.state === "idle") await openFolder(folder);
  lastState = a.state;
  return s;
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
  const writing = !$("composer").hidden;
  try {
    await api("callsigns", change);
  } catch (e) {
    toast(e.message, true);
    await refresh();
    return null;
  }
  forgetCallsign();
  const s = await refresh();
  if (s.me) probeOffice();
  if (change.add || !s.me) {
    show("setup");
    $("enroll").address.focus();
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
    tr.title = `#${e.serial}${e.third_party ? " · third-party traffic" : ""}`;
    return tr;
  }));
  show("waiting-view");
}

async function openGateways(refreshing = false) {
  markFolder("");
  try {
    const g = refreshing ? await api("gateways", {}) : await api("gateways");
    const others = (g.others || []).length;
    const missed = Object.entries(g.errors || {}).map(([d, why]) => `${d} (${why})`);
    $("gateways-note").textContent = g.domain
      ? `From ${g.domain}'s post office${others ? ` and ${others} it knows` : ""}, fetched `
        + `${new Date(g.fetched * 1000).toLocaleString()}. Kept here for when there is no Internet.`
        + (missed.length ? ` Not taken this time: ${missed.join("; ")}.` : "")
      : "No list yet: refresh it while you have the Internet. It is kept for when you do not.";
    const twice = new Set(g.ambiguous || []);
    if (!$("grid").matches(":focus")) $("grid").value = g.grid || "";
    $("gateway-rows").replaceChildren(...g.gateways.map((gw) => {
      const tr = document.createElement("tr");
      // the dial in MHz to the kilohertz, or finer when it needs it: 7.101, 3.5915
      const mhz = (hz) => (hz / 1e6).toFixed(4).replace(/0$/, "");
      const channels = (gw.channels || [])
        .map((c) => `${mhz(c.dial_hz)} MHz, ${c.bandwidth_hz} Hz`).join("\n");
      const choose = document.createElement("button");
      choose.textContent = "Choose";
      choose.title = `Call ${gw.callsign}: tune Aether to one of its channels first`;
      choose.addEventListener("click", () => {
        $("callsign").value = gw.callsign;
        document.querySelector("input[name=kind][value=gateway]").checked = true;
        keep("kind", "gateway");
        showKind();
        toast(`${gw.callsign} chosen. Tune Aether to ${channels.split("\n")[0]}, then Call.`);
      });
      const where = [gw.place, gw.grid].filter(Boolean).join(", ");
      // which post office lists it; a callsign two other post offices list is shown from each
      const by = el("div", {class: "note", text: gw.post_office || ""});
      if (twice.has(gw.callsign)) {
        by.append(" ", el("span", {class: "tag", text: "listed twice",
          title: "Two post offices other than yours list this callsign, each its own way"}));
      }
      const name = el("div", {}, el("span", {text: gw.callsign}), by);
      const cells = [name, where, gw.km === null ? "" : `${gw.km} km`, channels,
                     gw.hours || "", choose];
      cells.forEach((c, i) => {
        const td = document.createElement("td");
        if (typeof c === "string") td.textContent = c; else td.append(c);
        if (i === 2) td.className = "n";
        if (i === 0) td.className = "mono nowrap";
        if (i === 3) td.className = "lines";
        tr.append(td);
      });
      return tr;
    }));
    show("gateways-view");
  } catch (e) {
    toast(e.message, true);
  }
}

// a row's buttons: the trash can, or in the trash put back and delete for good
function rowActions(m, name) {
  const td = document.createElement("td");
  td.className = "acts";
  const button = (glyph, title, run, extra = "") => {
    const b = document.createElement("button");
    b.type = "button";
    b.className = `act needs-idle ${extra}`;
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
    button("trash", "Move it to the trash", () => toTrash(m.id, name));
  }
  return td;
}

async function openFolder(name) {
  if (name === "waiting") return openWaiting();
  if (name === "addresses") return openAddresses();
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

async function loadAddresses() {
  try { addressBook = (await api("addresses")).entries; } catch { /* kept as it was */ }
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
  $("n-addresses").textContent = addressBook.length || "";
  return r;
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
    input.value = `${head}${head ? " " : ""}${writtenAs(e)}, `;
    close();
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
  };
  const render = () => {
    const [head, typed] = split();
    const taken = head.toLowerCase();
    shown = !typed ? [] : addressBook
      .filter((e) => !taken.includes(e.address))
      .filter((e) => e.address.startsWith(typed) || e.address.includes(typed)
        || (e.name || "").toLowerCase().split(/\s+/).some((w) => w.startsWith(typed)))
      .sort((a, b) => (b.uses - a.uses) || (b.used - a.used))
      .slice(0, 6);
    if (!shown.length) { close(); return; }
    active = Math.min(Math.max(active, 0), shown.length - 1);
    list.replaceChildren(...shown.map((e, i) => {
      const li = document.createElement("li");
      li.setAttribute("role", "option");
      li.setAttribute("aria-selected", String(i === active));
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
function newComposer() {
  const form = $("composer");
  form.reset();
  answering = null;
  $("c-title").textContent = "New message";
  $("c-files").replaceChildren();
  $("c-held").replaceChildren();
  $("c-original").hidden = true;
  return form;
}

// Reply, Reply all or Forward: the composer filled in from the message read. A reply quotes
// nothing (every byte goes on the air); the original shows under the composer for reference.
async function answer(how) {
  let a;
  try {
    a = await api(`answer?id=${reading.id}&how=${how}`);
  } catch (e) {
    toast(e.message, true);
    return;
  }
  const form = newComposer();
  answering = {how, id: a.original.id, replyTo: a.reply_to};
  $("c-title").textContent = ANSWER_TITLES[how];
  form.to.value = a.to.length ? `${a.to.join(", ")}, ` : "";
  form.cc.value = a.cc.length ? `${a.cc.join(", ")}, ` : "";
  form.subject.value = a.subject;
  form.body.value = how === "forward" ? `\n\n${a.body}` : "";
  $("c-files").replaceChildren(...a.files.map((f, i) => {
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
  }));
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
  $("c-original-body").textContent = o.body;
  show("composer");
  const first = how === "forward" ? form.to : form.body;
  first.focus();
  if (first === form.body) form.body.setSelectionRange(0, 0);
}

async function openMessage(id, from = {folder}) {
  const m = await api(`message?id=${id}`);
  reading = {id, folder: from.folder, was: from.was};
  const trash = from.folder === "trash";
  // a reply answers a message received; anything may be forwarded
  const received = from.folder === "inbox" || (trash && from.was === "inbox");
  $("r-reply").hidden = $("r-reply-all").hidden = !received;
  $("r-delete").hidden = trash || !FOLDER_NAMES[from.folder];
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
  $("r-files").replaceChildren(...m.files.map((f) => {
    const li = document.createElement("li");
    li.textContent = `${f.name} (${f.type}, ${f.size} bytes)`;
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
  $("r-body").textContent = m.body;
  show("reader");
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
  if (answering?.replyTo) message.reply_to = answering.replyTo;
  if (answering?.how === "forward") {
    message.forward = answering.id;
    message.carry = [...$("c-files").querySelectorAll("input[data-carry]")]
      .filter((b) => b.checked).map((b) => Number(b.dataset.carry));
    // the note written above the message forwarded, without the blank lines left before it
    message.body = message.body.replace(/^\s*\n/, "");
  }
  try {
    await api("compose", message);
    newComposer();
    loadAddresses();
    toast("In the outbox: the next session carries it.");
    await refresh();
    await openFolder("outbox");
  } catch (e) {
    toast(e.message, true);
  }
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
    const known = new Map(list.gateways.map((g) => [g.callsign, g]));
    const names = [...new Set([...list.gateways.map((g) => g.callsign), ...usualChosen])];
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
    return el("tr", {}, el("td", {class: "mono", text: a.address}), name, devices,
      el("td", {class: "n", text: String(a.waiting || 0)}));
  }));
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
    ? `Last downloaded ${day(d.at)} by ${d.by}, ${d.mail ? "with the mailboxes" : "without the mailboxes"}.`
    : "None downloaded yet: a backup that stays on the server is lost with it.";
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
    // a reply left half written stays; a new message starts empty
    if (answering) newComposer();
    show("composer");
  });
  wireSuggestions($("composer").to);
  wireSuggestions($("composer").cc);
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
  $("address-new").addEventListener("keydown", (e) => { if (e.key === "Enter") addAddress(); });
  $("cancel").addEventListener("click", () => { newComposer(); openFolder(folder); });
  $("composer").addEventListener("submit", send);
  $("back").addEventListener("click", () => openFolder(folder));
  $("r-reply").addEventListener("click", () => answer("reply"));
  $("r-reply-all").addEventListener("click", () => answer("reply-all"));
  $("r-forward").addEventListener("click", () => answer("forward"));
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
  $("grid-save").addEventListener("click", async () => {
    try {
      await api("grid", {grid: $("grid").value});
      await openGateways();
    } catch (e) {
      toast(e.message, true);
    }
  });
  $("call").addEventListener("click", () => {
    const callsign = $("callsign").value.trim();
    if (!callsign) { toast("Whom to call?", true); return; }
    const fetchRule = $("fetch").value;
    act("call", {
      callsign, station: kind() === "station", adapter: adapter(),
      fetch: /^[0-9]+$/.test(fetchRule) ? Number(fetchRule) : fetchRule,
    });
  });
  $("listen").addEventListener("click", () => act("listen", {adapter: adapter()}));
  $("stop").addEventListener("click", () => act("stop", {}));
  $("update-go").addEventListener("click", () => act("update", {}));
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
  await loadAddresses();
  const s = await refresh();
  if (!s.device || !s.me) show("setup");
  else await openFolder("inbox");
  setInterval(() => refresh().catch(() => {}), 1000);
  if (s.me) probeOffice();
  // a new application shows on the button within a minute
  setInterval(() => { if (meAddress) probeOffice(); }, 60000);
}

start().catch((e) => toast(e.message, true));
