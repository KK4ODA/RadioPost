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

function toast(text, bad = false) {
  const t = $("toast");
  t.textContent = text;
  t.classList.toggle("bad", bad);
  t.hidden = false;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => { t.hidden = true; }, bad ? 7000 : 3000);
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
  for (const id of ["list-view", "reader", "composer", "setup"]) $(id).hidden = id !== view;
}

async function refresh() {
  const s = await api("state");
  $("who").textContent = s.me ? `${s.me.address}  ·  ${s.me.operator}` : "No credential yet";
  for (const name of ["inbox", "outbox", "sent"]) $(`n-${name}`).textContent = s.counts[name] || "";
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
                 session: `In session with ${a.remote || ""}`}[a.state];
  const state = $("state");
  state.dataset.state = a.state === "idle" && a.failed ? "failed" : a.state;
  state.textContent = a.state === "idle" && a.failed ? "Last session failed" : label;
  state.title = a.failed || "What the station is doing";
  const busy = a.state !== "idle";
  $("call").disabled = busy || !s.me;
  $("listen").disabled = busy || !s.me;
  $("stop").hidden = a.state !== "listening";
  $("write").disabled = busy || !s.me;
  $("device").textContent = s.device || "No key yet.";
  $("make-key").hidden = Boolean(s.device);
  $("cred-state").textContent = s.me
    ? `${s.me.address} (${s.me.operator}), valid until ${new Date(s.me.expires * 1000).toLocaleDateString()}`
    : "No credential yet.";
  $("trusted").textContent = s.trusted.length ? s.trusted.join(", ") : "None yet.";
  if (s.log !== logNext) await readLog();
  if (lastState !== "idle" && a.state === "idle") await openFolder(folder);
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

async function openFolder(name) {
  folder = name;
  for (const b of document.querySelectorAll("[data-folder]")) {
    b.setAttribute("aria-current", String(b.dataset.folder === name));
  }
  $("folder-title").textContent = name[0].toUpperCase() + name.slice(1);
  $("who-col").textContent = name === "inbox" ? "From" : "To";
  $("who-col").title = name === "inbox" ? "Who wrote it" : "Whom it is for";
  const rows = await api(`folder?name=${name}`);
  const body = $("rows");
  body.replaceChildren(...rows.map((m) => {
    const tr = document.createElement("tr");
    tr.className = ["", "priority", "emergency"][m.priority] || "";
    const who = name === "inbox" ? m.from : m.to.concat(m.cc).join(", ");
    for (const text of [who, m.subject || "(no subject)", when(m.date)]) {
      const td = document.createElement("td");
      td.textContent = text;
      tr.append(td);
    }
    tr.title = `${m.short}${m.station ? ` · via ${m.station}` : ""}`;
    tr.addEventListener("click", () => openMessage(m.id));
    return tr;
  }));
  $("empty").hidden = rows.length > 0;
  show("list-view");
}

async function openMessage(id) {
  const m = await api(`message?id=${id}`);
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

async function enroll(event) {
  event.preventDefault();
  const form = $("enroll");
  try {
    const r = await api("enroll", {
      address: form.address.value, operator: form.operator.value, code: form.code.value,
      url: form.url.value,
    });
    form.reset();
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
  $("setup-open").addEventListener("click", () => show("setup"));
  $("call").addEventListener("click", () => {
    const callsign = $("callsign").value.trim();
    if (!callsign) { toast("Whom to call?", true); return; }
    act("call", {callsign, station: $("direct").checked, adapter: adapter()});
  });
  $("listen").addEventListener("click", () => act("listen", {adapter: adapter()}));
  $("stop").addEventListener("click", () => act("stop", {}));
  $("make-key").addEventListener("click", () => act("key", {}));
  $("cred-save").addEventListener("click", async () => {
    await act("credential", {hex: $("cred-hex").value});
    $("cred-hex").value = "";
  });
  $("enroll").addEventListener("submit", enroll);
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
