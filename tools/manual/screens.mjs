// SPDX-License-Identifier: MIT OR Apache-2.0
// The manual's screenshots, taken of the real panels with example data.
//
//     node tools/manual/screens.mjs [NAME…]
//
// It opens app/ui (the RadioPost client) and app/gateway/ui (RadioPost Gateway) in a headless
// Chromium through Playwright, answers their JSON API from the examples below instead of a running
// program, and writes PNGs at twice the screen's size into docs/manual/screens/. The pages are the
// ones in the tree, so a screenshot shows the panel as it is now; the data is made up and the
// same at every run (the clock is fixed). Run it after changing what a panel shows, look at the
// pictures, and commit them with the manual. With names, it takes only those shots. The shots
// named readme-… are the README's, written into docs/images/: one part of a window each, in a
// narrower window, so that they can be read at the README's width.
//
// Needs Node and Playwright with its Chromium (`npm i -g playwright`, `npx playwright install
// chromium`). The pictures depend on the fonts installed; the committed ones use Inter.

import {createRequire} from "node:module";
import {readFile} from "node:fs/promises";
import {mkdirSync, readFileSync} from "node:fs";
import {dirname, extname, join, resolve} from "node:path";
import {fileURLToPath} from "node:url";

const require = createRequire(import.meta.url);
const {chromium} = require("playwright");

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const OUT = join(ROOT, "docs", "manual", "screens");
const README_OUT = join(ROOT, "docs", "images");
const ORIGIN = "http://radiopost.test";
const NOW = new Date("2026-11-14T15:24:00-05:00");
const T = Math.floor(NOW.getTime() / 1000);
const DAY = 86400;
const TYPES = {".html": "text/html", ".js": "text/javascript", ".css": "text/css",
               ".svg": "image/svg+xml"};

// ── the client's example world ───────────────────────────────────────────────────────────────

const ME = {address: "ana@radiopost.org", operator: "N0CALL", expires: T + 71 * DAY};

const msg = (id, m) => ({id, short: id.slice(0, 8), cc: [], priority: 0, ...m});
const INBOX = [
  msg("b3f1c9a2e7d4402f9a11c0d2e5b7a913", {from: "maria@example.com", to: [ME.address],
    subject: "Re: Field Day plans", date: T - 1500, station: "KK4ODA-1"}),
  msg("4c2d8e1f90ab43c7b2a6d5e8f1c3a7b9", {from: "jim@hampost.org", to: [ME.address],
    subject: "Antenna party on Saturday", date: T - 5200, station: "KK4ODA-1"}),
  msg("e7a90b3c5d1f4e2a8c6b9d0f1a2b3c4d", {from: "postmaster@radiopost.org", to: [ME.address],
    subject: "Your attachments: tower-photo.jpg", date: T - DAY - 3000, station: "KK4ODA-1"}),
  msg("19f0e2d3c4b5a6978877665544332211", {from: "dad@example.com", to: [ME.address],
    subject: "Ice storm at the cabin", date: T - 3 * DAY, priority: 1, station: "N1CALL-10"}),
];
const OUTBOX = [
  msg("7d3e9f1a2b4c5d6e7f8091a2b3c4d5e6", {from: ME.address, to: ["maria@example.com"],
    subject: "Field Day: count me in", date: T - 420, state: "written"}),
  msg("2a4b6c8d0e1f2a3b4c5d6e7f8a9b0c1d", {from: ME.address, to: ["jim@hampost.org"],
    cc: ["sam@example.org"], subject: "Re: Antenna party on Saturday", date: T - DAY - 600,
    state: "queued", queued: {gateway: "N1CALL-10", at: T - DAY - 400}}),
];
const SENT = [
  msg("5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b", {from: ME.address, to: ["maria@example.com"],
    subject: "Field Day plans", date: T - 2 * DAY, state: "sent"}),
  msg("8f9a0b1c2d3e4f5a6b7c8d9e0f1a2b3c", {from: ME.address, to: ["dad@example.com"],
    subject: "Safe and warm here", date: T - 3 * DAY + 4000, state: "sent"}),
];
const MESSAGES = {
  [INBOX[0].id]: {...INBOX[0], files: [], held: [
    {name: "site-map.pdf", type: "application/pdf", size: 412806, asked: false}],
    body: "Hi Ana,\n\nGreat news that you can come. We set up on Friday at noon at the county\n"
      + "park, by the north pavilion. The site map is attached.\n\nBring the 40 m dipole if you "
      + "can, and a chair.\n\n73,\nMaria"},
};
const WAITING = {via: "KK4ODA-1", at: T - 1150, entries: [
  {serial: 41, from: "club-news@hampost.org", subject: "November club newsletter", size: 18400,
   priority: 0, fetch: false, held: false},
  {serial: 42, from: "maria@example.com", subject: "Photos from the hamfest", size: 61200,
   priority: 0, fetch: true, held: true},
  {serial: 43, from: "ken@example.net", subject: "Re: Battery box wiring", size: 2900,
   priority: 0, fetch: false, held: false},
]};
const GATEWAYS = {domain: "radiopost.org", fetched: T - 2 * DAY, others: ["hampost.org"],
  errors: {}, ambiguous: [], grid: "EM73", gateways: [
    {callsign: "KK4ODA-1", post_office: "radiopost.org", place: "Atlanta, Georgia", grid: "EM73",
     km: 14, hours: "always", copies: {users: 50, days: 7},
     channels: [{dial_hz: 3591500, bandwidth_hz: 500}, {dial_hz: 7101000, bandwidth_hz: 2300}]},
    {callsign: "N2CALL-5", post_office: "radiopost.org", place: "Macon, Georgia", grid: "EM82",
     km: 128, hours: "1800-0200 UTC",
     channels: [{dial_hz: 7103500, bandwidth_hz: 500}]},
    {callsign: "N1CALL-10", post_office: "hampost.org", place: "Austin, Texas", grid: "EM10",
     km: 1309, hours: "always", copies: {users: 20, days: 7},
     channels: [{dial_hz: 7101000, bandwidth_hz: 2300}, {dial_hz: 14109000, bandwidth_hz: 2300}]},
  ]};
const ADDRESSES = [
  {name: "Maria Perez", address: "maria@example.com", uses: 6, used: T - 420},
  {name: "Jim Ortega", address: "jim@hampost.org", uses: 3, used: T - DAY - 600},
  {name: "Dad", address: "dad@example.com", uses: 4, used: T - 3 * DAY + 4000},
  {name: "", address: "sam@example.org", uses: 1, used: T - DAY - 600},
];
const CLIENT_LOG = [
  "calling KK4ODA-1",
  "connected to KK4ODA-1",
  "the other station is KK4ODA-1, its credential and statement checked",
  "delivered 7d3e9f",
  "received b3f1c9 from maria@example.com",
  "received 4c2d8e from jim@hampost.org",
  "waiting: #41 1c5e02 from club-news@hampost.org, 18400 bytes: November club newsletter",
  "the session with KK4ODA-1 is over",
];

function clientState(over = {}) {
  return {
    me: ME, device: DEVICE,
    trusted: ["radiopost.org"], post_office: null, application: null,
    callsigns: [{id: 1, chosen: true, ...ME}],
    counts: {outbox: OUTBOX.length, inbox: INBOX.length, sent: SENT.length, trash: 0, reports: 0,
             files: 0, waiting: WAITING.entries.length},
    activity: {state: "idle", remote: null, failed: null},
    progress: null, update: null, log: CLIENT_LOG.length,
    ...over,
  };
}

// The post office desk, for the screenshots of its tabs: an operator's view of radiopost.org.
const DEVICE = "8f3c2a1b9d0e4f5a6b7c8d9e0f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c";
const MY_DEVICE = Buffer.from(DEVICE, "hex").toString("base64url");
const cred = (operator, serial, days, device, revoked = false) =>
  ({operator, serial, expires: T + days * DAY, device, revoked});
const DESK_GET = {
  "/office/status": {domains: ["radiopost.org"], accounts: 23, applications: 1, codes: 2,
    sessions: 0, operators: ["KK4ODA"], version: "0.1.0-beta.21", outgoing: 0, seen: 1,
    health: {fails: 0}},
  "/office/applications": {applications: [
    {application: "ap_example", status: "pending", name: "Pat Rivera", operator: "N2CALL",
     address: "pat@radiopost.org", contact: "pat.rivera@example.net", received: T - 2400,
     evidence: {verdict: "good", lines: [
       "FCC: N2CALL, General class, PAT RIVERA, Macon GA, licence good until 2034-05-02.",
       "The name on the application matches the licence."], flags: []}},
    {application: "ap_old", status: "approved", name: "Lee Chen", operator: "N3CALL",
     address: "lee@radiopost.org", contact: "lee@example.com", received: T - 3 * DAY,
     decided: T - 3 * DAY + 3600, told: {to: "lee@example.com"},
     mailbox: {made: "lee@radiopost.org"}},
  ]},
  "/office/accounts": {accounts: [
    {address: "ana@radiopost.org", name: "Ana Lopez", waiting: 3,
     credentials: [cred("N0CALL", 1204, 71, "Qm9hcmRtYW4tbGFwdG9wLWtleS1leGFtcGxl")]},
    {address: "club@radiopost.org", name: "Riverside Radio Club", waiting: 0,
     credentials: [cred("N1CALL", 1220, 88, "Q2x1Yi1zdGF0aW9uLWtleS1leGFtcGxlLTEy")]},
    {address: "facundo@radiopost.org", name: "Facundo", waiting: 0,
     credentials: [cred("KK4ODA", 1187, 63, MY_DEVICE),
                   cred("KK4ODA", 1011, 12, "TW9iaWxlLWxhcHRvcC1rZXktZXhhbXBsZS0w", true)]},
    {address: "lee@radiopost.org", name: "Lee Chen", waiting: 1,
     credentials: [cred("N3CALL", 1215, 86, "TGVlLWNoZW4tZGV2aWNlLWtleS1leGFtcGxl")]},
    {address: "shelter3@radiopost.org", name: "Shelter 3", waiting: 0, credentials: []},
  ]},
  "/office/outgoing": {outgoing: []},
  "/office/post-offices": {
    known: [{domain: "hampost.org", operator: "N1CALL", country: "US",
             contact: "postmaster@hampost.org", found: T - 3 * 3600, api: true, dnssec: true}],
    seen: [{domain: "qsl-mail.example.net", operator: "N4CALL", country: "US",
            how: ["door", "list:hampost.org"], last: T - DAY, api: true, dnssec: false}],
    blocked: [], looking_up: []},
  "/office/health": {at: T - 1400, lines: [
    {verdict: "ok", text: "_radiopost.radiopost.org: api https://po.radiopost.org/rp/v1"},
    {verdict: "ok", text: "https://po.radiopost.org/rp/v1/info: signed by key 37030f82"},
    {verdict: "ok", text: "radiopost.org: MX po.radiopost.org"},
    {verdict: "ok", text: "radiopost.org: SPF \"v=spf1 mx -all\""},
    {verdict: "ok", text: "_dmarc.radiopost.org: \"v=DMARC1; p=quarantine; rua=mailto:postmaster@radiopost.org\""},
    {verdict: "ok", text: "v1-rsa-20261110._domainkey.radiopost.org: a DKIM key, RSA"},
    {verdict: "ok", text: "the door answers, and the token is right"},
    {verdict: "ok", text: "disk: 41210 MB free (18% used)"},
    {verdict: "ok", text: "backups: last night's, radiopost.org-2026-11-14.rpbackup"},
    {verdict: "ok", text: "mailboxes: backed up today, stalwart-2026-11-14.tar.gz, 38.2 MB"},
    {verdict: "warn", text: "backups off the server: last fetched 9 days ago, by KK4ODA's computer shack-pc, with the mailboxes: open the RadioPost app on shack-pc, or download one"},
    {verdict: "note", text: "RadioPost 0.1.0-beta.22 is out: the desk's Server tab installs it"},
  ]},
  "/office/settings": {applications: "open", domains: ["radiopost.org"], operators: ["KK4ODA"],
    contact: "postmaster@radiopost.org", base_url: "https://po.radiopost.org/rp/v1",
    fcc: {on: true, count: 1606615, made: T - 2 * DAY}, qrz: {username: "kk4oda", password: true},
    mailboxes: {kind: "stalwart", url: "http://127.0.0.1:8080", key: true}, smtp: true,
    dkim: ["v1-rsa-20261110", "v1-ed25519-20261110"]},
  "/office/jobs": {runner: true, jobs: [
    {name: "update", what: "Install the newest RadioPost release, its signature checked, and restart the post office",
     last: {ok: true, finished: T - 6 * DAY, by: "KK4ODA"}},
    {name: "restart", what: "Restart the post office"},
    {name: "restart-mail", what: "Restart the mail server (Stalwart)"},
    {name: "services", what: "Whether the post office, the mail server and the web server run",
     last: {ok: true, finished: T - 2 * 3600, by: "KK4ODA",
            output: "radiopost-po: active (running)\nstalwart: active (running)\ncaddy: active (running)"}},
    {name: "log", what: "The post office's log, its last 300 lines"},
    {name: "disk", what: "Disk space"},
    {name: "mail-backup", what: "Back up the mail server's mailboxes now: Stalwart stopped for a moment, its data archived, started again; the last three kept (done each night)",
     last: {ok: true, finished: T - 6 * 3600}},
  ]},
  "/office/backups": {
    backups: [{name: "radiopost.org-2026-11-14.rpbackup", bytes: 2184000, at: T - 9 * 3600},
              {name: "radiopost.org-2026-11-13.rpbackup", bytes: 2151000, at: T - 33 * 3600}],
    mail: [{name: "stalwart-2026-11-14.tar.gz", bytes: 38200000, at: T - 6 * 3600}],
    downloaded: {at: T - 9 * DAY, to: "shack-pc", by: "KK4ODA", mail: "stalwart-2026-11-05.tar.gz"},
    keys: [{id: "k1", label: "shack-pc", added_at: T - 12 * DAY, added_by: "KK4ODA",
            fetched_at: T - 9 * DAY, fetched_bytes: 40100000}]},
  "/office/dkim": {next: "start", rotate_after_days: 180, rotation: null, keys: [
    {id: "a", selector: "v1-rsa-20261110", algorithm: "rsa", stage: "active",
     created: "2026-11-10T14:02:00Z"},
    {id: "b", selector: "v1-ed25519-20261110", algorithm: "ed25519", stage: "active",
     created: "2026-11-10T14:02:00Z"}]},
};

function deskAnswer(method, path, body) {
  if (method === "POST" && path === "/office/codes") {
    return {code: "H7KQ-2MXD", address: body.address, operator: body.operator,
            expires: T + 7 * DAY};
  }
  if (path.startsWith("/office/lookup/")) {
    return {verdict: "good", lines: [
      "FCC: N4CALL, Extra class, SAM OKAFOR, Athens GA, licence good until 2033-08-19.",
      "The name matches the licence."], flags: []};
  }
  return DESK_GET[path] || {};
}

// the forms the panel holds, as /api/forms gives them (RP-2 §8): ICS-213, its fingerprint from
// vectors/forms.json
const ICS_213 = JSON.parse(readFileSync(join(ROOT, "forms", "ics-213.json"), "utf8"));
const FORMS = [{
  template: JSON.parse(readFileSync(join(ROOT, "vectors", "forms.json"), "utf8"))
    .templates[0].fingerprint_hex,
  name: ICS_213.name, note: ICS_213.note,
  fields: ICS_213.fields.map((f) => ({required: false, choices: [], bound: null, max: null, ...f})),
}];

function clientApi(world) {
  return (method, path, query, body) => {
    const s = world.state;
    switch (path) {
      case "state": return s;
      case "log": return {lines: Number(query.get("since")) ? [] : world.log, next: world.log.length};
      case "folder": return {inbox: INBOX, outbox: OUTBOX, sent: SENT, trash: []}[query.get("name")];
      case "message": return MESSAGES[query.get("id")] || {...INBOX[1], files: [], held: [], body: ""};
      case "waiting": return WAITING;
      case "gateways": return GATEWAYS;
      case "settings": return {usual_gateways: ["KK4ODA-1"], radio_skips_read: false};
      case "accept": return {entries: ["maria@example.com", "jim@hampost.org", "@hampost.org"]};
      case "addresses": return {entries: ADDRESSES};
      case "forms": return FORMS;
      case "modem": return {};
      case "backups": return {on: true, computer: "shack-pc", folder: "D:\\RadioPost backups",
        key: "k1", key_file: "C:\\Users\\you\\.radiopost\\backup.key",
        fetched: {at: T - 9 * DAY, name: "radiopost.org-2026-11-05.rpbackup", bytes: 40100000,
                  mail: true}};
      case "office": {
        if (!world.operator) return {status: 403, body: {title: "Forbidden", name: "forbidden"}};
        return {status: 200, body: deskAnswer(body.method, body.path, body.body || {})};
      }
      default: return {error: `no ${method} ${path}`, status: 404};
    }
  };
}

// ── the gateway's example world ─────────────────────────────────────────────────────────────

const GW_SETTINGS = {callsign: "KK4ODA-1", modem: "ws://127.0.0.1:8515/v1", country: "US",
  operator_checks_rules: false, answer_at_start: true,
  copies: {users: 50, days: 7, max_air_bytes: 16384}, post_offices: {}};
const GW_SESSIONS = [
  {started: T - 1300, ended: T - 1148, remote: "N0CALL", account: "ana@radiopost.org",
   handed_in: 1, picked_up: 2, result: "over"},
  {started: T - 9800, ended: T - 9512, remote: "KK4ODA", account: "facundo@radiopost.org",
   handed_in: 3, picked_up: 0, result: "over"},
  {started: T - 15400, ended: T - 15100, remote: "N2CALL", account: "pat@radiopost.org",
   handed_in: 0, picked_up: 0, result: "nothing heard for 300 seconds"},
  {started: T - DAY - 600, ended: T - DAY - 588, remote: "N3CALL", account: null,
   handed_in: 0, picked_up: 0, result: "refused", refused: "N3CALL"},
];
const GW_LOG = [
  [T - 2 * 3600 + 2, "listening as KK4ODA-1 on ws://127.0.0.1:8515/v1"],
  [T - 9800, "KK4ODA called"],
  [T - 9512, "the session with KK4ODA is over"],
  [T - 1300, "N0CALL called"],
  [T - 1148, "the session with N0CALL is over"],
];

function gatewayState(over = {}) {
  return {
    settings: GW_SETTINGS,
    credential: {address: "facundo@radiopost.org", serial: 1187, expires: T + 63 * DAY},
    running: true,
    status: {state: "listening", since: T - 2 * 3600 + 2, problem: null, session: null,
      logged: GW_LOG.length,
      checks: [
        {domain: "radiopost.org", ok: true, at: T - 240, url: "https://po.radiopost.org/rp/v1"},
        {domain: "hampost.org", ok: true, at: T - 240, url: "https://po.hampost.org/rp/v1"}]},
    held: {copies: 5, users: 3, records: 0, queued: 0},
    key: "5a1f0e9d8c7b6a5f4e3d2c1b0a9f8e7d6c5b4a3f2e1d0c9b8a7f6e5d4c3b2a19",
    client: {home: "C:\\Users\\you\\.radiopost",
             enrolled: {operator: "KK4ODA", address: "facundo@radiopost.org"}},
    dir: "C:\\Users\\you\\.radiopost-gateway", version: "0.1.0-beta.20",
    aether: "ws://127.0.0.1:8515/v1", update: null,
    ...over,
  };
}

function gatewayApi(world) {
  return (method, path, query) => {
    switch (path) {
      case "state": return world.state;
      case "log": {
        const log = world.log ?? GW_LOG;
        return {lines: Number(query.get("since")) ? [] : log, next: log.length};
      }
      case "sessions": return world.sessions;
      case "held": return {
        copies: [
          {address: "ana@radiopost.org", serial: 41, from: "club-news@hampost.org",
           subject: "November club newsletter", size: 18400, expires: T + 6 * DAY, held: true},
          {address: "ana@radiopost.org", serial: 43, from: "ken@example.net",
           subject: "Re: Battery box wiring", size: 2900, expires: T + 6 * DAY},
          {address: "pat@radiopost.org", serial: 7, from: "lee@radiopost.org",
           subject: "Net control schedule", size: 1800, expires: T + 5 * DAY},
        ],
        queue: [], reported: [
          {at: T - DAY, address: "ana@radiopost.org", taken: 2, refused: 0, pickups: 1}]};
      case "refused": return world.refused ?? [
        {caller: "N3CALL", note: "Sent business mail through the gateway", at: T - 5 * DAY}];
      case "partners": return world.partners ?? [
        {callsign: "N1CALL-10", records: 0, messages: 0, forwards: 0, called: T - 4 * DAY,
         said: "nothing to forward", next_window: T + 51 * 60}];
      case "windows": return world.windows ?? [
        {channel: {dial_hz: 7101000, mode: "USB", modem: "aether", bandwidth_hz: 2300,
                   occupied_hz: [7101240, 7103760]},
         every_s: 3600, at_s: 900, length_s: 600, why: null}];
      default: return {error: `no ${method} ${path}`, status: 404};
    }
  };
}

// ── serving a page with its API answered here ───────────────────────────────────────────────

async function open(browser, {ui, api, width = 1280, height = 800}) {
  const context = await browser.newContext({viewport: {width, height}, deviceScaleFactor: 2,
    colorScheme: "light", locale: "en-US", timezoneId: "America/New_York"});
  const page = await context.newPage();
  await page.clock.setFixedTime(NOW);
  await page.route(`${ORIGIN}/**`, async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname.startsWith("/api/")) {
      const path = url.pathname.slice(5);
      const body = route.request().postDataJSON?.() ?? null;
      const answer = api(route.request().method(), path, url.searchParams, body);
      const status = answer && answer.status && answer.error ? answer.status : 200;
      await route.fulfill({status, contentType: "application/json", body: JSON.stringify(answer)});
      return;
    }
    const file = join(ui, url.pathname === "/" ? "index.html" : url.pathname.slice(1));
    try {
      await route.fulfill({body: await readFile(file),
                           contentType: TYPES[extname(file)] || "application/octet-stream"});
    } catch {
      await route.fulfill({status: 404, body: ""});
    }
  });
  await page.goto(`${ORIGIN}/`);
  await page.waitForTimeout(400);
  return {page, context};
}

// numbered markers beside the parts a manual's list then names: [selector, number, where]. By
// default a marker sits just after the element's own text; "inside" puts it at the element's
// right end, inside it; "before" just before it; "corner" on its top right corner.
async function mark(page, marks) {
  await page.evaluate((list) => {
    for (const [selector, n, where] of list) {
      const target = document.querySelector(selector);
      if (!target) throw new Error(`no ${selector} to mark`);
      const box = target.getBoundingClientRect();
      let r = box;
      if (!where) {
        // the text's own extent, not the element's: the first line of what it says
        const walker = document.createTreeWalker(target, NodeFilter.SHOW_TEXT);
        let node = walker.nextNode();
        while (node && !node.textContent.trim()) node = walker.nextNode();
        if (node) {
          const range = document.createRange();
          range.selectNodeContents(node);
          const rects = [...range.getClientRects()];
          if (rects.length) r = rects[0];
        }
      }
      const size = 24;
      const cy = where === "corner" ? box.top - size / 2 : r.top + r.height / 2 - size / 2;
      const x = where === "inside" ? box.right - size - 8
        : where === "before" ? box.left - size - 6
          : where === "corner" ? box.right - size / 2
            : r.right + 8;
      const dot = document.createElement("div");
      dot.textContent = n;
      Object.assign(dot.style, {position: "fixed", left: `${Math.max(2, x)}px`,
        top: `${Math.max(2, cy)}px`, width: `${size}px`, height: `${size}px`, borderRadius: "50%",
        background: "#c2410c", color: "#fff", font: "700 13px/24px Inter, system-ui, sans-serif",
        textAlign: "center", zIndex: 9999, boxShadow: "0 0 0 2px #fff, 0 1px 4px rgba(0,0,0,.35)"});
      document.body.append(dot);
    }
  }, marks);
}

// The client's log shows the time it read each line; read at once, the example's lines would all
// show one time. Give them the times a session would have.
async function spreadLog(page, id, first, step) {
  await page.evaluate(([logId, from, gap]) => {
    const pre = document.getElementById(logId);
    const lines = pre.textContent.split("\n").filter(Boolean);
    pre.textContent = lines.map((line, i) => {
      const t = new Date((from + i * gap) * 1000);
      return line.replace(/^\S+ \S+ {2}/, `${t.toLocaleTimeString("en-US")}  `);
    }).join("\n") + "\n";
    pre.scrollTop = pre.scrollHeight;
  }, [id, first, step]);
}

async function shot(page, name, clip) {
  const path = join(OUT, `${name}.png`);
  if (clip) {
    const box = await page.locator(clip).boundingBox();
    const pad = 12;
    await page.screenshot({path, clip: {x: Math.max(0, box.x - pad), y: Math.max(0, box.y - pad),
      width: box.width + 2 * pad, height: box.height + 2 * pad}});
  } else {
    await page.screenshot({path});
  }
  console.log(`docs/manual/screens/${name}.png`);
}

// ── the shots ───────────────────────────────────────────────────────────────────────────────

const CLIENT_UI = join(ROOT, "app", "ui");
const GATEWAY_UI = join(ROOT, "app", "gateway", "ui");

function clientWorld(over = {}) {
  return {state: clientState(over.state || {}), log: over.log || CLIENT_LOG,
          operator: over.operator || false};
}

// the desk on one tab, as KK4ODA, an operator of radiopost.org
async function openDesk(browser, tab, height = 900, width = 1280) {
  const world = clientWorld({state: {me: {address: "facundo@radiopost.org", operator: "KK4ODA",
                                          expires: T + 63 * DAY}},
                             operator: true});
  const {page} = await open(browser, {ui: CLIENT_UI, api: clientApi(world), height, width});
  await spreadLog(page, "log", T - 1300, 19);
  await page.waitForTimeout(300);
  await page.click("#office-open");
  await page.waitForTimeout(300);
  await page.click(`[data-otab=${tab}]`);
  await page.waitForTimeout(500);
  return page;
}

// the desk alone, without the folders and the log: larger on the page
async function shotMain(page, name) {
  await page.screenshot({path: join(OUT, `${name}.png`), clip: await unionClip(page, ["#office"])});
  console.log(`docs/manual/screens/${name}.png`);
}

// a call to KK4ODA-1 under way, the gateway chosen from the list as the manual's §8 says: its
// callsign in Call and its channels in Channel
async function openInSession(browser, width = 1280, height = 900) {
  const world = clientWorld({state: {
    activity: {state: "session", remote: "KK4ODA-1", failed: null},
    progress: {sending: {messages: 2, whole: 1, bytes: 3400, crossed: 2300},
               receiving: {messages: 3, whole: 1, bytes: 9600, crossed: 4100},
               receiving_now: true, rate_bps: 900}},
    log: CLIENT_LOG.slice(0, 5)});
  const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(world), width, height});
  await page.click("#gateways-open");
  await page.waitForTimeout(200);
  await page.locator("#gateway-rows tr", {hasText: "KK4ODA-1"})
    .getByRole("button", {name: "Choose"}).click();
  await page.click("button[data-folder=inbox]");
  await page.evaluate(() => { document.getElementById("toast").hidden = true; });
  await page.locator("#callsign").blur();
  await spreadLog(page, "log", T - 110, 17);
  await page.waitForTimeout(1200);
  return {page, context};
}

// RadioPost Gateway answering N0CALL
function gatewayInSession() {
  return {state: gatewayState({status: {state: "session", since: T - 95, problem: null,
    logged: GW_LOG.length, checks: gatewayState().status.checks,
    session: {remote: "N0CALL", account: "ana@radiopost.org", started: T - 95, handed_in: 1,
              picked_up: 1}}}), sessions: GW_SESSIONS};
}

// a new message with ICS-213 chosen and its fields filled in, as the manual and the README show it
async function fillForm(page) {
  await page.click("#write");
  const form = page.locator("#composer");
  await form.locator("input[name=to]").fill("eoc@radiopost.org");
  await form.locator("input[name=subject]").fill("Cots and water for tonight");
  await form.locator("select[name=form]").selectOption({label: "ICS-213 General Message"});
  const field = (n) => page.locator(`#c-form-fields [data-field="${n}"]`);
  await field(1).fill("Storm response, Atlanta");
  await field(2).fill("Duty officer, county EOC");
  await field(3).fill("J. Park, Shelter 3 manager");
  await field(7).fill("140 people tonight, 30 more expected by 20:00.\n"
    + "Need 40 cots and 300 litres of drinking water.\nGenerator fuel for 12 hours.");
  await field(8).fill("J. Park, Shelter 3 manager");
}

// a README picture: the whole window, or the box around the elements named
async function readmeShot(page, name, selectors) {
  const path = join(README_OUT, `${name}.png`);
  await page.screenshot(selectors ? {path, clip: await unionClip(page, selectors)} : {path});
  console.log(`docs/images/${name}.png`);
}

const SHOTS = {
  async "client-window"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld()),
                                                 height: 900});
    await spreadLog(page, "log", T - 1300, 19);
    await mark(page, [["#who", "1"], ["#state", "2", "before"], ["#write", "3", "inside"],
                      ["[data-folder=inbox]", "4"], [".station h2", "5"],
                      ["#list-view h1", "6"], ["#setup-open", "7"], ["#log", "8", "inside"]]);
    await shot(page, "client-window");
    await context.close();
  },

  async "client-compose"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld()),
                                                 height: 900});
    await spreadLog(page, "log", T - 1300, 19);
    await page.click("#write");
    const form = page.locator("#composer");
    await form.locator("input[name=to]").fill("maria@example.com, Jim Ortega <jim@hampost.org>");
    await form.locator("input[name=subject]").fill("Field Day: count me in");
    await form.locator("textarea[name=body]").fill(
      "Hi Maria,\n\nCount me in for Friday. I will bring the 40 m dipole, a chair and\n"
      + "the spare battery box.\n\n73,\nAna, N0CALL");
    await page.locator("body").click({position: {x: 1200, y: 760}});
    await shot(page, "client-compose");
    await context.close();
  },

  async "client-form"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld()),
                                                 height: 1300});
    await spreadLog(page, "log", T - 1300, 19);
    await fillForm(page);
    await page.locator("body").click({position: {x: 1200, y: 1200}});
    await shot(page, "client-form");
    await context.close();
  },

  async "client-reader"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld()),
                                                 height: 900});
    await spreadLog(page, "log", T - 1300, 19);
    await page.locator("#rows tr").first().click();
    await page.waitForTimeout(200);
    await shot(page, "client-reader");
    await context.close();
  },

  async "client-gateways"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld()),
                                                 height: 900});
    await spreadLog(page, "log", T - 1300, 19);
    await page.click("#gateways-open");
    await page.waitForTimeout(200);
    await shot(page, "client-gateways");
    await context.close();
  },

  async "client-session"(browser) {
    const {page, context} = await openInSession(browser);
    await shot(page, "client-session");
    await context.close();
  },

  async "client-waiting"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld()),
                                                 height: 900});
    await spreadLog(page, "log", T - 1300, 19);
    await page.click("[data-folder=waiting]");
    await page.waitForTimeout(200);
    await shot(page, "client-waiting");
    await context.close();
  },

  async "client-outbox"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld()),
                                                 height: 900});
    await spreadLog(page, "log", T - 1300, 19);
    await page.click("[data-folder=outbox]");
    await page.waitForTimeout(200);
    await shot(page, "client-outbox");
    await context.close();
  },

  async "client-enroll"(browser) {
    const world = clientWorld({state: {me: null, device: null, trusted: [], callsigns: [{id: 1, chosen: true}],
      counts: {outbox: 0, inbox: 0, sent: 0, trash: 0, reports: 0, files: 0, waiting: 0}}, log: []});
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(world), height: 900});
    const form = page.locator("#enroll");
    await form.locator("input[name=address]").fill("ana@radiopost.org");
    await form.locator("input[name=operator]").fill("N0CALL");
    await form.locator("input[name=code]").fill("7Q4F-AZ9K");
    await form.locator("input[name=code]").blur();
    await shot(page, "client-enroll", "#enroll");
    await context.close();
  },

  async "client-apply"(browser) {
    const world = clientWorld({state: {me: null, callsigns: [{id: 1, chosen: false, ...ME}, {id: 2, chosen: true}],
      counts: {outbox: 0, inbox: 0, sent: 0, trash: 0, reports: 0, files: 0, waiting: 0}}, log: []});
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(world), height: 900});
    await page.click("#setup-open");
    await page.click("input[name=join][value=apply]");
    const form = page.locator("#enroll");
    await form.locator("input[name=address]").fill("club@radiopost.org");
    await form.locator("input[name=operator]").fill("N1CALL");
    await form.locator("input[name=name]").fill("Ana Lopez");
    await form.locator("input[name=contact]").fill("ana.lopez@example.com");
    await form.locator("input[name=contact]").blur();
    await mark(page, [["#who-pick", "1", "corner"],
                      ["label:has(> input[name=join][value=apply])", "2"],
                      ["#enroll-go", "3", "corner"]]);
    await shot(page, "client-apply");
    await context.close();
  },

  async "client-applied"(browser) {
    const world = clientWorld({state: {me: null,
      application: {status: "pending", address: "club@radiopost.org", operator: "N1CALL",
                    name: "Ana Lopez", contact: "ana.lopez@example.com"},
      callsigns: [{id: 1, chosen: false, ...ME},
                  {id: 2, chosen: true, application: {address: "club@radiopost.org",
                   operator: "N1CALL", status: "pending"}}],
      counts: {outbox: 0, inbox: 0, sent: 0, trash: 0, reports: 0, files: 0, waiting: 0}}, log: []});
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(world),
                                                 height: 900});
    await page.click("#setup-open");
    await page.waitForTimeout(200);
    await shot(page, "client-applied");
    await context.close();
  },

  async "client-setup"(browser) {
    const world = clientWorld({state: {callsigns: [{id: 1, chosen: true, ...ME},
      {id: 2, chosen: false, address: "club@radiopost.org", operator: "N1CALL",
       expires: T + 88 * DAY}]}});
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(world), height: 1300});
    await page.click("#setup-open");
    await page.waitForTimeout(400);
    await page.screenshot({path: join(OUT, "client-setup.png"), clip: await unionClip(page,
      ["#setup h1", "#usual-save"], 1100)});
    console.log("docs/manual/screens/client-setup.png");
    await context.close();
  },

  async "desk-applications"(browser) {
    const page = await openDesk(browser, "applications");
    await page.click("#o-decided-box summary");
    await page.waitForTimeout(200);
    await shotMain(page, "desk-applications");
    await page.context().close();
  },

  async "desk-accounts"(browser) {
    const page = await openDesk(browser, "accounts");
    await shotMain(page, "desk-accounts");
    await page.context().close();
  },

  async "desk-code"(browser) {
    const page = await openDesk(browser, "code");
    const form = page.locator("#o-code");
    await form.locator("input[name=address]").fill("sam@radiopost.org");
    await form.locator("input[name=operator]").fill("N4CALL");
    await form.locator("input[name=name]").fill("Sam Okafor");
    await page.click("#o-code-check");
    await page.waitForTimeout(300);
    const evidence = await page.locator("#o-code-evidence").innerHTML();
    await form.locator("button[type=submit]").click();
    await page.waitForTimeout(300);
    // the form empties once the code is made: show what was typed with the code
    await page.evaluate((html) => {
      const f = document.getElementById("o-code");
      f.address.value = "sam@radiopost.org";
      f.operator.value = "N4CALL";
      f.name.value = "Sam Okafor";
      document.getElementById("o-code-evidence").innerHTML = html;
    }, evidence);
    await shotMain(page, "desk-code");
    await page.context().close();
  },

  async "desk-others"(browser) {
    const page = await openDesk(browser, "others");
    await shotMain(page, "desk-others");
    await page.context().close();
  },

  async "desk-health"(browser) {
    const page = await openDesk(browser, "health");
    await shotMain(page, "desk-health");
    await page.context().close();
  },

  async "desk-server"(browser) {
    const page = await openDesk(browser, "server", 1500);
    await page.locator("#o-jobs details summary").first().click();
    await page.waitForTimeout(200);
    await page.screenshot({path: join(OUT, "desk-server.png"), clip: await unionClip(page,
      ["#office > .head", "#o-server > .card:nth-child(2)"])});
    console.log("docs/manual/screens/desk-server.png");
    await page.context().close();
  },

  async "desk-backups"(browser) {
    const page = await openDesk(browser, "server", 2000);
    await page.screenshot({path: join(OUT, "desk-backups.png"), clip: await unionClip(page,
      ["#o-server > .card:nth-child(3)", "#o-server > .card:nth-child(4)"])});
    console.log("docs/manual/screens/desk-backups.png");
    await page.context().close();
  },

  async "desk-settings"(browser) {
    const page = await openDesk(browser, "settings", 1400);
    await page.screenshot({path: join(OUT, "desk-settings.png"), clip: await unionClip(page,
      ["#office > .head", "#o-settings"])});
    console.log("docs/manual/screens/desk-settings.png");
    await page.context().close();
  },

  async "gateway-window"(browser) {
    const world = {state: gatewayState(), sessions: GW_SESSIONS};
    const {page, context} = await open(browser, {ui: GATEWAY_UI, api: gatewayApi(world), height: 860});
    await page.waitForTimeout(600);
    await mark(page, [["#who", "1"], ["#state", "2", "before"], ["#stop", "3", "corner"],
                      ["#setup-open", "4", "corner"],
                      ["[aria-label=Modem] h2", "5"], ["[aria-label='Post offices'] h2", "6"],
                      ["[aria-label=Registration] h2", "7"],
                      ["[aria-label='For when the Internet is down'] h2", "8"],
                      ["[aria-label='Refused callers'] h2", "9"],
                      [".cards [aria-label=Partners] h2", "10"],
                      ["#station-view > h1", "11"], ["#log", "12", "inside"]]);
    await shot(page, "gateway-window");
    await context.close();
  },

  async "gateway-first"(browser) {
    const world = {state: gatewayState({settings: null, credential: null, running: false,
      status: {state: "stopped", since: T - 60, problem: null, session: null, logged: 0, checks: []},
      held: {}, key: null}), sessions: [], log: [], refused: [], partners: []};
    const {page, context} = await open(browser, {ui: GATEWAY_UI, api: gatewayApi(world), height: 720});
    await page.waitForTimeout(400);
    await shot(page, "gateway-first");
    await context.close();
  },

  async "gateway-setup"(browser) {
    const world = {state: gatewayState({running: false,
      status: {state: "stopped", since: T - 60, problem: null, session: null, logged: 0,
               checks: []}}), sessions: GW_SESSIONS};
    const {page, context} = await open(browser, {ui: GATEWAY_UI, api: gatewayApi(world), height: 2100});
    await page.waitForTimeout(400);
    await page.click("#setup-open");
    const reg = page.locator("#register-form");
    await reg.locator("input[name=grid]").fill("EM73");
    await reg.locator("input[name=place]").fill("Atlanta, Georgia");
    const dial = page.locator("#channel-rows input").first();
    await dial.fill("3.5915");
    await page.locator("#channel-rows tr").first().locator("select").nth(1).selectOption("500");
    await page.click("#add-channel");
    await page.locator("#channel-rows input").nth(1).fill("7.101");
    await page.locator("body").click({position: {x: 1270, y: 10}});
    await page.screenshot({path: join(OUT, "gateway-setup.png"), clip: await unionClip(page,
      ["#setup h1", "#po-form"], 1100)});
    console.log("docs/manual/screens/gateway-setup.png");
    await context.close();
  },

  async "gateway-held"(browser) {
    const world = {state: gatewayState(), sessions: GW_SESSIONS};
    const {page, context} = await open(browser, {ui: GATEWAY_UI, api: gatewayApi(world), height: 1900});
    await page.waitForTimeout(400);
    await page.click("#held-open");
    await page.click("#refused-open");
    await page.click("#partners-open");
    await page.waitForTimeout(400);
    await page.screenshot({path: join(OUT, "gateway-held.png"), clip: await unionClip(page,
      ["#partners-view", "#held-view"])});
    console.log("docs/manual/screens/gateway-held.png");
    await context.close();
  },

  async "gateway-session"(browser) {
    const {page, context} = await open(browser, {ui: GATEWAY_UI, api: gatewayApi(gatewayInSession()),
                                                 height: 860});
    await page.waitForTimeout(600);
    await page.screenshot({path: join(OUT, "gateway-session.png"), clip: await unionClip(page,
      ["body > header", "#session"])});
    console.log("docs/manual/screens/gateway-session.png");
    await context.close();
  },

  // ── the README's ─────────────────────────────────────────────────────────────────────────

  async "readme-session"(browser) {
    const {page, context} = await openInSession(browser, 1100, 900);
    await readmeShot(page, "readme-session");
    await context.close();
  },

  async "readme-reader"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld()),
                                                 width: 1000, height: 760});
    await page.locator("#rows tr").first().click();
    await page.waitForTimeout(200);
    await readmeShot(page, "readme-reader", ["#reader > *"]);
    await context.close();
  },

  async "readme-form"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld()),
                                                 width: 1000, height: 1300});
    await fillForm(page);
    await page.evaluate(() => document.activeElement.blur());
    // down to the last field filled in: the rest of the form is empty boxes
    await readmeShot(page, "readme-form", ["#c-title", "#composer input[name=to]",
                                           '#c-form-fields [data-field="8"]']);
    await context.close();
  },

  async "readme-gateways"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld()),
                                                 width: 1100, height: 700});
    await page.click("#gateways-open");
    await page.waitForTimeout(200);
    await readmeShot(page, "readme-gateways", ["#gateways-view > *"]);
    await context.close();
  },

  async "readme-gateway"(browser) {
    const {page, context} = await open(browser, {ui: GATEWAY_UI, api: gatewayApi(gatewayInSession()),
                                                 width: 1100, height: 860});
    await page.waitForTimeout(600);
    await readmeShot(page, "readme-gateway", ["body > header", "#session"]);
    await context.close();
  },

  async "readme-desk"(browser) {
    const page = await openDesk(browser, "accounts", 760, 1100);
    await readmeShot(page, "readme-desk", ["#office"]);
    await page.context().close();
  },
};

// the box that holds every element named, padded, at most `width` wide
async function unionClip(page, selectors, width = 0) {
  const boxes = [];
  for (const s of selectors) {
    for (const b of await page.locator(s).all()) {
      const box = await b.boundingBox();
      if (box) boxes.push(box);
    }
  }
  const pad = 12;
  const x = Math.max(0, Math.min(...boxes.map((b) => b.x)) - pad);
  const y = Math.max(0, Math.min(...boxes.map((b) => b.y)) - pad);
  const right = Math.max(...boxes.map((b) => b.x + b.width)) + pad;
  const bottom = Math.max(...boxes.map((b) => b.y + b.height)) + pad;
  return {x, y, width: width ? Math.min(width, right - x) : right - x, height: bottom - y};
}

const wanted = process.argv.slice(2);
mkdirSync(OUT, {recursive: true});
mkdirSync(README_OUT, {recursive: true});
const browser = await chromium.launch();
try {
  for (const [name, take] of Object.entries(SHOTS)) {
    if (wanted.length && !wanted.includes(name)) continue;
    await take(browser);
  }
} finally {
  await browser.close();
}
