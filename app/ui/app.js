// SPDX-License-Identifier: MIT OR Apache-2.0
// The RadioPost client's panel: the folders, a reader, a composer, the station's calls and its
// setup, over the client's JSON API (app/client/src/panel.rs). No build step.

const $ = (id) => document.getElementById(id);
let folder = "inbox";
let logNext = 0;
let lastState = "idle";

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
  for (const id of ["list-view", "waiting-view", "gateways-view", "reader", "composer", "setup"]) {
    $(id).hidden = id !== view;
  }
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
  $("who").textContent = s.me ? `${s.me.address}  ·  ${s.me.operator}` : "No credential yet";
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
  $("make-key").hidden = Boolean(s.device);
  showEnrollment(s.me);
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
    $("gateways-note").textContent = g.domain
      ? `From ${g.domain}'s post office, fetched ${new Date(g.fetched * 1000).toLocaleString()}. `
        + "Kept here for when there is no Internet."
      : "No list yet: refresh it while you have the Internet. It is kept for when you do not.";
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
      const cells = [gw.callsign, where, gw.km === null ? "" : `${gw.km} km`, channels,
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

// what the reader's buttons act on: the folder the message was opened from
let reading = null;

async function openMessage(id, from = {folder}) {
  const m = await api(`message?id=${id}`);
  reading = {id, folder: from.folder, was: from.was};
  const trash = from.folder === "trash";
  $("r-delete").hidden = trash || !FOLDER_NAMES[from.folder];
  $("r-restore").hidden = !trash;
  $("r-purge").hidden = !trash;
  $("r-restore").title = trash ? `Put it back in the ${from.was}` : "";
  $("r-subject").textContent = m.subject || "(no subject)";
  $("r-from").textContent = m.from;
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
  try {
    await api("compose", {
      to: form.to.value, cc: form.cc.value, subject: form.subject.value,
      priority: Number(form.priority.value), body: form.body.value, files,
    });
    form.reset();
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
  const open = !me || expired || enrollOpen;
  $("enroll-title").textContent = me && !expired ? "Enrolled with your post office" : "Enroll with your post office";
  $("enroll-fields").hidden = !open;
  $("enroll-again").hidden = open;
  $("enroll-go").textContent = me ? "Enroll again" : "Enroll";
  const form = $("enroll");
  if (me && open && !form.contains(document.activeElement)) {
    if (!form.address.value) form.address.value = me.address;
    if (!form.operator.value) form.operator.value = me.operator;
  }
}

async function enroll(event) {
  event.preventDefault();
  const form = $("enroll");
  try {
    const r = await api("enroll", {
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

function wire() {
  try {
    const kept = localStorage.getItem("radiopost.adapter");
    if (kept) $("adapter").value = kept;
  } catch { /* not kept */ }
  for (const b of document.querySelectorAll("[data-folder]")) {
    b.addEventListener("click", () => openFolder(b.dataset.folder));
  }
  $("write").addEventListener("click", () => show("composer"));
  $("cancel").addEventListener("click", () => { $("composer").reset(); openFolder(folder); });
  $("composer").addEventListener("submit", send);
  $("back").addEventListener("click", () => openFolder(folder));
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
  $("t-save").addEventListener("click", async () => {
    await act("trust", {domain: $("t-domain").value, public: $("t-key").value});
    $("t-key").value = "";
  });
}

async function start() {
  wire();
  const s = await refresh();
  if (!s.device || !s.me) show("setup");
  else await openFolder("inbox");
  setInterval(() => refresh().catch(() => {}), 1000);
}

start().catch((e) => toast(e.message, true));
