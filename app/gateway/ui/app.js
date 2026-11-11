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

function render(s) {
  state = s;
  showUpdate(s.update, "RadioPost Gateway");
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

  $("modem").textContent = settings ? settings.modem : "—";
  $("modem-state").textContent = st.problem
    || (st.state === "listening" || st.state === "session"
      ? `Listening as ${settings.callsign} since ${when(st.since)}`
      : st.state === "stopped" ? "Not answering." : "");

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
        ? `The credential runs out on ${day(s.credential.expires)}: register again soon (Setup).`
        : `Registered under ${s.credential.address}: credential ${s.credential.serial}, until ${day(s.credential.expires)}.`;
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

  $("setup-open").hidden = !settings;
  if (!formsFilled) fillForms(s);
  renderPostOffices(s);
  $("key-line").hidden = !s.key;
  $("key").textContent = s.key || "";
  $("enrolled").textContent = s.client.enrolled
    ? `The RadioPost app on this computer is enrolled as ${s.client.enrolled.operator}, ${s.client.enrolled.address}.`
    : "The RadioPost app on this computer is not enrolled yet: enroll it first (its Setup), or give the home of one that is.";
  $("where").textContent = `The gateway's files: ${s.dir}. RadioPost Gateway ${s.version}.`;
  $("held").hidden = !answering;
  for (const form of ["gateway-form", "register-form", "po-form"]) {
    for (const el of $(form).elements) el.disabled = answering;
  }
}

function fillForms(s) {
  formsFilled = true;
  const g = $("gateway-form").elements;
  const settings = s.settings;
  g.callsign.value = settings ? settings.callsign : "";
  g.modem.value = settings ? settings.modem : s.aether;
  g.country.value = settings ? settings.country : "US";
  g.operator_checks_rules.checked = settings ? settings.operator_checks_rules : false;
  g.answer_at_start.checked = settings ? settings.answer_at_start : true;
  $("register-form").elements.home.value = s.client.home;
  if (!$("channel-rows").children.length) addChannel(7.101, 2300);
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

function addChannel(mhz = "", bandwidth = 2300) {
  const tr = document.createElement("tr");
  const dial = document.createElement("input");
  dial.type = "number";
  dial.step = "0.0001";
  dial.min = "1.8";
  dial.placeholder = "7.101";
  dial.value = mhz;
  dial.title = "The dial frequency, in MHz, upper sideband";
  const bw = document.createElement("select");
  bw.className = "narrow";
  for (const hz of [2300, 500]) {
    const o = document.createElement("option");
    o.value = hz;
    o.textContent = `${hz} Hz`;
    bw.append(o);
  }
  bw.value = bandwidth;
  bw.title = "Aether's bandwidth on this channel";
  const remove = document.createElement("button");
  remove.type = "button";
  remove.className = "quiet";
  remove.textContent = "Remove";
  remove.addEventListener("click", () => tr.remove());
  for (const el of [dial, bw, remove]) {
    const td = document.createElement("td");
    td.append(el);
    tr.append(td);
  }
  $("channel-rows").append(tr);
}

async function refreshSessions() {
  const sessions = await api("sessions");
  shownSessions = sessions.length;
  $("rows").replaceChildren(...sessions.map((r) => {
    const tr = document.createElement("tr");
    const ended = r.result === "over";
    tr.className = ended ? "" : "failed";
    const cells = [
      when(r.started), r.remote, r.account || "—",
      r.ended ? length(r.started, r.ended) : "",
      r.handed_in, r.picked_up,
      ended ? "over" : r.result + (r.post_office ? `; ${postOffice(r.post_office)}` : ""),
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
$("setup-open").addEventListener("click", () => show("setup"));
$("setup-go").addEventListener("click", () => show("setup"));
$("back").addEventListener("click", () => show("station"));
$("add-channel").addEventListener("click", () => addChannel());

$("gateway-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const f = e.target.elements;
  act("setup", {
    callsign: f.callsign.value, modem: f.modem.value, country: f.country.value,
    operator_checks_rules: f.operator_checks_rules.checked,
    answer_at_start: f.answer_at_start.checked,
  }, (s) => { render(s); toast("Saved."); });
});

$("register-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const f = e.target.elements;
  const channels = [...$("channel-rows").children].map((tr) => {
    const [dial, bw] = tr.querySelectorAll("input, select");
    return {dial_hz: Math.round(Number(dial.value) * 1e6), mode: "USB", modem: "aether",
            bandwidth_hz: Number(bw.value)};
  }).filter((c) => c.dial_hz > 0);
  act("register", {home: f.home.value, grid: f.grid.value, place: f.place.value,
                   hours: f.hours.value, channels},
      (a) => { render(a.state); toast(a.said); });
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
