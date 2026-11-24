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
// the version the panels say they run: this tree's
const VERSION = readFileSync(join(ROOT, "core", "Cargo.toml"), "utf8")
  .match(/^version = "(.+)"$/m)[1];
const UPDATE = {current: VERSION, offer: null, checked: T - 3600, checking: false, error: null,
                can_install: false, installing: false, failed: null};
const TYPES = {".html": "text/html", ".js": "text/javascript", ".css": "text/css",
               ".svg": "image/svg+xml"};

// ── the client's example world ───────────────────────────────────────────────────────────────

const ME = {address: "ana@radiopost.org", operator: "N0CALL", expires: T + 71 * DAY};

const NO_FILES = {carried: 0, bytes: 0, held: 0, held_bytes: 0};
const msg = (id, m) => ({id, short: id.slice(0, 8), cc: [], priority: 0, attachments: NO_FILES,
                         ...m});
const INBOX = [
  msg("b3f1c9a2e7d4402f9a11c0d2e5b7a913", {from: "maria@example.com", to: [ME.address],
    subject: "Re: Field Day plans", date: T - 1500, station: "KK4ODA-1",
    attachments: {carried: 1, bytes: 412, held: 1, held_bytes: 412806}}),
  msg("4c2d8e1f90ab43c7b2a6d5e8f1c3a7b9", {from: "jim@hampost.org", to: [ME.address],
    subject: "Antenna party on Saturday", date: T - 5200, station: "KK4ODA-1"}),
  msg("e7a90b3c5d1f4e2a8c6b9d0f1a2b3c4d", {from: "postmaster@radiopost.org", to: [ME.address],
    subject: "Your attachments: tower-photo.jpg", date: T - DAY - 3000, station: "KK4ODA-1",
    attachments: {...NO_FILES, carried: 1, bytes: 48213}}),
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
  [INBOX[0].id]: {...INBOX[0], files: [{name: "directions.txt", type: "text/plain", size: 412}],
    held: [
    {name: "site-map.pdf", type: "application/pdf", size: 412806, asked: false}],
    body: "Hi Ana,\n\nGreat news that you can come. We set up on Friday at noon at the county\n"
      + "park, by the north pavilion. Directions and the site map are attached.\n\nBring the 40 m "
      + "dipole if you can, and a chair.\n\n73,\nMaria"},
};
const WAITING = {via: "KK4ODA-1", at: T - 1150, entries: [
  {serial: 41, from: "club-news@hampost.org", subject: "November club newsletter", size: 18400,
   priority: 0, fetch: false, held: false},
  {serial: 42, from: "maria@example.com", subject: "Photos from the hamfest", size: 61200,
   priority: 0, fetch: true, held: true},
  {serial: 43, from: "ken@example.net", subject: "Re: Battery box wiring", size: 2900,
   priority: 0, fetch: false, held: false},
]};
// where the station is: two places saved, and an iPhone's GPS over its own hotspot, read now
const HERE = {lat: 33.74901, lon: -84.38802, grid: "EM73tr", from: "gps", age_s: 2,
              freshness: "current"};
const PLACES = {
  places: [
    {id: "pl-home", name: "Home", lat: 33.72917, lon: -84.375, default: true, source: "grid",
     grid: "EM73tr", saved_at: T - 30 * DAY, square: "EM73tr", station: "st-home-ftdx10"},
    {id: "pl-field-day", name: "Field Day", lat: 34.10021, lon: -84.50113, default: false,
     source: "typed", saved_at: T - 5 * DAY, square: "EM74sc", station: "st-mobile-ts-480"},
  ],
  choice: {use: "automatic"},
  gps: {kind: "tcp", host: "172.20.10.1", port: 11123},
  status: {link: {state: "open", since_ms: (T - 600) * 1000}, hearing: {state: "positions"},
           fix: {lat: HERE.lat, lon: HERE.lon, alt_m: 318.2, quality: "gps", at_ms: (T - 2) * 1000},
           freshness: "current", age_s: 2, held: false, refused: 0,
           words: "reading: fix, 2 s ago"},
  here: HERE, note: null, broken: null, ports: ["/dev/ttyUSB0", "/dev/ttyACM0"],
  // two profiles: the mobile one in use (Field Day names it), the home one mostly assumed
  stations: [
    {id: "st-mobile-ts-480", name: "Mobile, TS-480", radio: "Kenwood TS-480", vehicle: true,
     hf: {power_w: 65, antenna: {kind: "whip", agl_m: 2.5}}, vhf: {antenna: {}}, noise: "residential",
     used: {
       hf: {power_w: {value: 65, said: "set"}, antenna: {value: "whip", said: "set"},
            agl_m: {value: 2.5, said: "set"}, gain_dbi: {value: 2.15, said: "assumed"},
            feedline_db: {value: 1, said: "assumed"}},
       vhf: {power_w: {value: 25, said: "assumed"}, antenna: {value: "whip", said: "assumed"},
             agl_m: {value: 2, said: "assumed"}, gain_dbi: {value: 0, said: "assumed"},
             feedline_db: {value: 2, said: "assumed"}},
       noise: {value: "residential", said: "set"}}},
    {id: "st-home-ftdx10", name: "Home, FTDX10", radio: "Yaesu FTDX10", vehicle: false,
     hf: {power_w: 50, antenna: {kind: "dipole", agl_m: 12}}, vhf: {antenna: {}},
     used: {
       hf: {power_w: {value: 50, said: "set"}, antenna: {value: "dipole", said: "set"},
            agl_m: {value: 12, said: "set"}, gain_dbi: {value: 2.15, said: "assumed"},
            feedline_db: {value: 1, said: "assumed"}},
       vhf: {power_w: {value: 25, said: "assumed"}, antenna: {value: "vertical", said: "assumed"},
             agl_m: {value: 5, said: "assumed"}, gain_dbi: {value: 0, said: "assumed"},
             feedline_db: {value: 2, said: "assumed"}},
       noise: {value: "residential", said: "assumed"}}},
  ],
  station: null, station_in_use: "st-home-ftdx10",
};
const GATEWAYS = {domain: "radiopost.org", fetched: T - 2 * DAY, others: ["hampost.org"],
  errors: {}, ambiguous: [], grid: "EM73", here: HERE, note: null, marks_error: null, gateways: [
    {callsign: "KK4ODA-1", post_office: "radiopost.org", place: "Atlanta, Georgia", grid: "EM73",
     km: 14, hours: "always", copies: {users: 50, days: 7}, updated: T - 12 * DAY,
     own: false, favourite: true, blocked: false, list_fetched: T - 2 * DAY, list_old: false,
     last: {at: T - 1300, class: "completed", words: "completed"}, worked: true,
     seen: T - 120, seen_old: false, listening: null,
     channels: [{dial_hz: 3591500, bandwidth_hz: 2300, scan_s: 20, favourite: true, blocked: false},
                {dial_hz: 7101000, bandwidth_hz: 2300, scan_s: 20, favourite: true, blocked: false}]},
    {callsign: "N0CALL-10", place: "The radio club", grid: "EM74", km: 46, hours: "evenings",
     own: true, favourite: false, blocked: false, list_fetched: null, list_old: false, last: null,
     worked: false,
     channels: [{dial_hz: 7103500, bandwidth_hz: 500, favourite: false, blocked: false}]},
    {callsign: "N2CALL-5", post_office: "radiopost.org", place: "Macon, Georgia", grid: "EM82",
     km: 128, hours: "1800-0200 UTC", updated: T - 40 * DAY, own: false, favourite: false,
     blocked: false, list_fetched: T - 2 * DAY, list_old: false, seen: T - 3 * DAY,
     seen_old: true, listening: {open: false, changes: T + 3 * 3600 + 36 * 60},
     last: {at: T - 3 * DAY, class: "no-answer", words: "no answer, nothing heard"}, worked: false,
     channels: [{dial_hz: 7103500, bandwidth_hz: 500, favourite: false, blocked: false}]},
    {callsign: "N1CALL-10", post_office: "hampost.org", place: "Austin, Texas", grid: "EM10",
     km: 1309, hours: "always", copies: {users: 20, days: 7}, updated: T - 5 * DAY, own: false,
     favourite: false, blocked: false, list_fetched: T - 2 * DAY, list_old: false, last: null,
     worked: false,
     channels: [{dial_hz: 7101000, bandwidth_hz: 2300, favourite: false, blocked: false},
                {dial_hz: 14109000, bandwidth_hz: 2300, favourite: false, blocked: true}]},
    {callsign: "W9OLD-1", post_office: "radiopost.org", place: "Chicago, Illinois", grid: "EN61",
     km: 945, hours: "always", own: false, favourite: false, blocked: true,
     list_fetched: T - 2 * DAY, list_old: false, last: null, worked: false,
     channels: [{dial_hz: 7101000, bandwidth_hz: 2300, favourite: false, blocked: false}]},
  ]};
// each gateway's grid square's centre, for the map, and its bearing from here, as the client
// adds them (app/client/src/selection/directory.rs, app/client/src/grid.rs)
for (const gw of GATEWAYS.gateways) Object.assign(gw, placedFrom(HERE, gw.grid));
// each HF channel's prediction as the client adds it (app/client/src/selection/hf.rs), worked out
// by the real VOACAP engine at the shot's clock (hf-channels.json says how); a blocked channel has
// none
const HF_CHANNELS = JSON.parse(readFileSync(join(ROOT, "tools", "manual", "hf-channels.json"),
                                            "utf8")).channels;
// and what hf-predict heard toward two of them in the last hour (evidence.rs's words), made up:
// neither tier raises the band, so the engine's figures stand
const HEARD = {
  "N2CALL-5 7103500": {tier: "limited", stations: 1, best_snr_db: -14, examples: ["N8CALL"],
    hearing_stations: 0, hearing_you: 0,
    words: "FT8 from 1 station near it heard here on 40 m in the last hour, the strongest at -14 dB"},
  "N1CALL-10 7101000": {tier: "none", stations: 0, best_snr_db: null, examples: [],
    hearing_stations: 0, hearing_you: 0,
    words: "Nothing from near it heard here on 40 m in the last hour, in 25 min of listening"},
};
const GATEWAYS_HF = {...GATEWAYS, hf: {engine: "voacapl", kept: 5, heard: true},
  gateways: GATEWAYS.gateways.map((gw) => ({...gw, channels: gw.channels.map((c) => {
    const key = `${gw.callsign} ${c.dial_hz}`;
    const hf = HF_CHANNELS[key];
    return hf && !c.blocked ? {...c, hf: HEARD[key] ? {...hf, heard: HEARD[key]} : hf} : c;
  })}))};
// what the call records taught of KK4ODA-1's 40 m channel (learning.rs, hf.rs's
// through_learning), made up: the path 4 dB under the model coming in and 7 dB going out, the
// gateway answering 93 % of calls at this hour, the modem carrying 71 % of its model's rate; the
// model's own figures stay beside them
const K40 = HF_CHANNELS["KK4ODA-1 7101000"];
const widened = (w, sd) => ({...w, lower_db: Math.round(Math.hypot(w.lower_db, 1.28 * sd) * 10) / 10,
                             upper_db: Math.round(Math.hypot(w.upper_db, 1.28 * sd) * 10) / 10});
const K40_LEARNED = {
  chance: 0.92, chance_words: "likely to work",
  up: {...widened(K40.up, 2.7), snr_db: Math.round((K40.up.snr_db - 6.8) * 10) / 10, bps: 2310,
       rung: "QAM16-3/4", p_call: 1.0},
  down: {...widened(K40.down, 1.9), snr_db: Math.round((K40.down.snr_db - 4.1) * 10) / 10,
         bps: 2650, rung: "QAM64-1/2", p_call: 1.0},
  up_offset: {mean: -6.8, sd: 2.7}, down_offset: {mean: -4.1, sd: 1.9},
  there: 0.93, eta: 0.71, calls: 6.4,
  words: "Likely to work: links on 92 % of days at 20Z, about 2.3 kbit/s sending, 2.7 kbit/s "
    + "receiving · learned from your calls",
};
for (const gw of GATEWAYS_HF.gateways.filter((g) => g.callsign === "KK4ODA-1")) {
  gw.channels = gw.channels.map((c) => c.dial_hz === 7101000 && c.hf
    ? {...c, hf: {...c.hf, learned: K40_LEARNED}} : c);
}
GATEWAYS_HF.learning = {health: {p_fault: 0.01, failures: 0, gateways: 0, warning: null},
                        records: 14, forgotten: 3, direction: {mean: -2.7, sd: 2.2}};
// KK4ODA-1's details (panel.rs's learned): the model, what was learned, what was measured
const LEARNED = {
  callsign: "KK4ODA-1",
  gateway: GATEWAYS_HF.gateways.find((g) => g.callsign === "KK4ODA-1"),
  channels: [
    {channel: "40 m", listed: {dial_hz: 7101000, bandwidth_hz: 2300, mode: "USB", scan_s: 20},
     model: {hf: {...K40, learned: K40_LEARNED}},
     learned: {down: {mean: -4.1, sd: 1.9}, up: {mean: -6.8, sd: 2.7},
               direction: {mean: -2.7, sd: 2.2}, there: 0.93, measured: 5.2, calls: 6.4,
               losses_per_hour: 0.6, efficiency: {mean: 0.71, n: 4.8}, drift: false},
     measured: {calls: 7, answered: 7, sessions: 7, lost: 1, down: {min: 24.6, max: 33.1, n: 7},
                up: {min: 22.0, max: 29.4, n: 6}, bps: {min: 1850, max: 3420, n: 6}}},
    {channel: "80 m", listed: {dial_hz: 3591500, bandwidth_hz: 2300, mode: "USB", scan_s: 20},
     model: {hf: HF_CHANNELS["KK4ODA-1 3591500"]}, learned: null,
     measured: {calls: 0, answered: 0, sessions: 0, lost: 0, down: null, up: null, bps: null}},
  ],
  availability: [{mean: 0.93, n: 7.1}, [0.91, 0.92, 0.93, 0.93, 0.94, 0.94, 0.93, 0.92]],
  service: {mean: 0.97, n: 6.2},
  recent: [
    {id: "a-5", at: T - 1600, kind: "gateway", remote: "KK4ODA-1", dial_hz: 7101000,
     words: "completed", completed: true, forgotten: false},
    {id: "a-1", at: T - 5 * DAY, kind: "gateway", remote: "KK4ODA-1", dial_hz: 7101000,
     words: "completed", completed: true, forgotten: false},
    {id: "a-0", at: T - 9 * DAY, kind: "gateway", remote: "KK4ODA-1", dial_hz: 7101000,
     words: "the link was lost", completed: false, forgotten: false},
  ],
  health: GATEWAYS_HF.learning.health, direction: GATEWAYS_HF.learning.direction, forgotten: false,
};
// the forgetting rules (panel.rs's forget): calls before a station change
const FORGET = {rules: [{what: "before", before: T - 40 * DAY}], records: 14, forgotten: 3};
function placedFrom(here, grid) {
  const g = grid.toUpperCase();
  let lon = (g.charCodeAt(0) - 65) * 20 - 180 + Number(g[2]) * 2;
  let lat = (g.charCodeAt(1) - 65) * 10 - 90 + Number(g[3]);
  if (g.length === 6) {
    lon += (g.charCodeAt(4) - 65) * 2 / 24 + 1 / 24;
    lat += (g.charCodeAt(5) - 65) / 24 + 0.5 / 24;
  } else {
    lon += 1;
    lat += 0.5;
  }
  const r = Math.PI / 180;
  const dl = (lon - here.lon) * r;
  const y = Math.sin(dl) * Math.cos(lat * r);
  const x = Math.cos(here.lat * r) * Math.sin(lat * r)
    - Math.sin(here.lat * r) * Math.cos(lat * r) * Math.cos(dl);
  const bearing = (Math.round(Math.atan2(y, x) / r) + 360) % 360;
  return {at: {lat, lon}, bearing};
}
// the calls placed, as the Recent view lists them
const RECENT = {recent: [
  {id: "a-5", at: T - 1600, kind: "gateway", remote: "KK4ODA-1", dial_hz: 7101000,
   bandwidth_hz: 2300, grid: "EM73tr", class: "completed", words: "completed", completed: true,
   took_s: 300, why: null},
  {id: "a-4", at: T - DAY - 7200, kind: "station", remote: "WD5EMA", dial_hz: 7101000,
   bandwidth_hz: 2300, grid: "EM73tr", class: "completed", words: "completed", completed: true,
   took_s: 95, why: null},
  {id: "a-3", at: T - 3 * DAY, kind: "gateway", remote: "N2CALL-5", dial_hz: 7103500,
   bandwidth_hz: 500, grid: "EM73tr", class: "no-answer", words: "no answer, nothing heard",
   completed: false, took_s: 76, why: "no answer"},
  {id: "a-2", at: T - 3 * DAY - 600, kind: "gateway", remote: "KK4ODA-1", dial_hz: 3591500,
   bandwidth_hz: 2300, grid: "EM73tr", class: "channel-busy", words: "the channel was busy",
   completed: false, took_s: 4, why: "busy"},
  {id: "a-1", at: T - 6 * DAY, kind: "gateway", remote: "KK4ODA-1", dial_hz: 7101000,
   bandwidth_hz: 2300, grid: "EM74sc", class: "link-lost", words: "the link was lost",
   completed: false, took_s: 410, why: "the link was lost"},
]};
// whom the call box offers under ▾ (GET /api/called): RECENT's calls, once each and newest first,
// a gateway with its record from the list in use, and a station called before RECENT's week
function calledOf(gateways) {
  const out = {gateway: [], station: []};
  const calls = [...RECENT.recent, {at: T - 12 * DAY, kind: "station", remote: "N4CALL",
                                    dial_hz: 3591500, bandwidth_hz: 500}];
  for (const a of calls) {
    if (out[a.kind].some((e) => e.callsign === a.remote)) continue;
    const e = {callsign: a.remote, at: a.at, dial_hz: a.dial_hz, bandwidth_hz: a.bandwidth_hz};
    if (a.kind === "gateway") {
      e.gateway = gateways.gateways.find((g) => g.callsign === a.remote) || null;
    }
    out[a.kind].push(e);
  }
  return out;
}
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

// alerts (RP-8): Ana's two circles, and what she sent and heard this afternoon
const answer = (kind, from, ago, text) => ({kind, from, time: T - ago, text: text ?? null});
const alert = (a) => ({out: false, to: null, level: "time-sensitive", quiet: false, sent_at: null,
                       tries: 1, answers: [], reply: null, why: null, alarm: false,
                       dial_hz: null, outlast_s: null, ...a});
const ALERT_LOG = [
  alert({tag: "a1", out: true, circle: "Riverside net", from: "N0CALL", to: null,
    text: "Shelter 3 needs cots: who has some?", time: T - 600, at: T - 600, state: "heard",
    dial_hz: 3591500,
    answers: [answer("heard", "N3CALL", 575), answer("heard", "KE4QCM", 571)]}),
  alert({tag: "a2", circle: "Family", from: "KK4ODA", to: "N0CALL", dial_hz: 7101000,
    text: "Please call me on 146.52 now", time: T - 2400, at: T - 2390, state: "replied",
    reply: "Calling you now"}),
  alert({tag: "a3", out: true, circle: "Family", from: "N0CALL", to: "KK4ODA",
    text: "Home safe, the roads are clear", time: T - 6000, at: T - 6000, state: "replied",
    dial_hz: 7101000, outlast_s: 49,
    answers: [answer("heard", "KK4ODA", 5975), answer("seen", "KK4ODA", 5900),
              answer("reply", "KK4ODA", 5880, "Good, thanks")]}),
  alert({tag: "a4", out: true, circle: "Riverside net", from: "N0CALL", to: "W4XYZ",
    text: "Net control tonight?", time: T - DAY, at: T - DAY, state: "not heard", tries: 2}),
];
const ALERT_CIRCLES = [
  {name: "Family", check: "9853 7abf", level: "time-sensitive", answer: true, watch_hz: 7101000},
  {name: "Riverside net", check: "8aec 0d0e", level: "important", answer: false,
   watch_hz: 3591500},
];
const ARRIVING = alert({tag: "a0", circle: "Family", from: "KK4ODA", to: "N0CALL",
  text: "Please call me on 146.52 now", time: T - 20, at: T - 4, state: "new", alarm: true,
  dial_hz: 7101000});
const NOTIFY = {
  sounds: {on: true, volume: 60, events: {}},
  quiet: {on: true, from: "22:30", to: "06:30"},
  mail: {default: "normal", levels: {"address:dad@example.com": "time-sensitive",
                                     "group:Shelter team": "important"}},
  notify: true,
};

function clientState(over = {}) {
  return {
    me: ME, device: DEVICE,
    trusted: ["radiopost.org"], post_office: null, application: null,
    callsigns: [{id: 1, chosen: true, ...ME}],
    counts: {outbox: OUTBOX.length, inbox: INBOX.length, sent: SENT.length, trash: 0, reports: 0,
             files: 0, waiting: WAITING.entries.length},
    activity: {state: "idle", remote: null, failed: null, listen: false, held: null},
    progress: null, update: UPDATE, log: CLIENT_LOG.length,
    alerts: {seq: 1, alarming: false, circles: ALERT_CIRCLES.length},
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
    {address: "ana@radiopost.org", name: "Ana Lopez", waiting: 3, mailbox: true,
     credentials: [cred("N0CALL", 1204, 71, "Qm9hcmRtYW4tbGFwdG9wLWtleS1leGFtcGxl")]},
    {address: "club@radiopost.org", name: "Riverside Radio Club", waiting: 0, mailbox: true,
     credentials: [cred("N1CALL", 1220, 88, "Q2x1Yi1zdGF0aW9uLWtleS1leGFtcGxlLTEy")]},
    {address: "facundo@radiopost.org", name: "Facundo", waiting: 0, mailbox: true,
     credentials: [cred("KK4ODA", 1187, 63, MY_DEVICE),
                   cred("KK4ODA", 1011, 12, "TW9iaWxlLWxhcHRvcC1rZXktZXhhbXBsZS0w", true)]},
    {address: "lee@radiopost.org", name: "Lee Chen", waiting: 1, mailbox: true,
     credentials: [cred("N3CALL", 1215, 86, "TGVlLWNoZW4tZGV2aWNlLWtleS1leGFtcGxl")]},
    {address: "shelter3@radiopost.org", name: "Shelter 3", waiting: 0, mailbox: false,
     credentials: []},
  ]},
  "/office/outgoing": {outgoing: []},
  // the gateways registered here (RP-5 §6.4): N3CALL-1 by mistake, under the shelter's account
  "/office/gateways": {gateways: [
    {callsign: "KK4ODA-1", operator: "KK4ODA", address: "facundo@radiopost.org",
     record: {place: "Atlanta, Georgia", grid: "EM73"}, placed: 2, last_poll: T - 40,
     cut_off: false, credentials: [{serial: 1188, expires: T + 63 * DAY, revoked: false}]},
    {callsign: "N3CALL-1", operator: "N3CALL", address: "shelter3@radiopost.org",
     record: {place: "Shelter 3, Decatur", grid: "EM73"}, placed: 0, last_poll: null,
     cut_off: false, credentials: [{serial: 1231, expires: T + 89 * DAY, revoked: false}]},
  ]},
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
            expires: T + 7 * DAY,
            mailbox: {address: body.address, made: `mailbox ${body.address} made in Stalwart`,
                      password: "Fq7Rk2mWzX9pTa4NcY3e"}};
  }
  if (path.startsWith("/office/lookup/")) {
    return {verdict: "good", lines: [
      "FCC: N4CALL, Extra class, SAM OKAFOR, Athens GA, licence good until 2033-08-19.",
      "The name matches the licence."], flags: []};
  }
  return DESK_GET[path] || {};
}

// a VHF gateway on Stone Mountain (made up), and its path from the GPS's fix downtown as the client
// worked it out over the real Copernicus tile N33W085 (terrain-path.json says how)
const TERRAIN_PATH = JSON.parse(readFileSync(join(ROOT, "tools", "manual", "terrain-path.json"), "utf8"));
const VHF_GATEWAY = {callsign: "N3CALL-10", post_office: "radiopost.org", place: "Stone Mountain, Georgia",
  grid: "EM73", km: 23, hours: "always", updated: T - 20 * DAY, own: false, favourite: false,
  blocked: false, list_fetched: T - 2 * DAY, list_old: false, last: null, worked: false,
  seen: T - 300, seen_old: false, listening: null,
  site: {lat: 33.8053, lon: -84.1455, agl_m: 15},
  channels: [{dial_hz: 145050000, mode: "FM", bandwidth_hz: 12500, modem: "aether-fm", favourite: false, blocked: false,
              station: {power_w: 50, antenna: "collinear"}}],
  terrain: {state: "judged", dial_hz: 145050000, chance: TERRAIN_PATH.chance, words: TERRAIN_PATH.words}};
Object.assign(VHF_GATEWAY, placedFrom(HERE, VHF_GATEWAY.grid));
// four more made up round Atlanta, two on public summits and two low, with the terms the client
// worked out for each over the real tiles N33W085, N33W084, N34W085 and N34W084: the terrain map's
// gateways. Its background, terrain-hillshade.jpg, is the client's own hillshade of that area
function vhf(callsign, place, site, dial, modem, terrain) {
  const gw = {callsign, post_office: "radiopost.org", place, grid: "EM73", hours: "always",
    own: false, favourite: false, blocked: false, list_fetched: T - 2 * DAY, list_old: false,
    last: null, worked: false, seen: T - 600, seen_old: false, listening: null, site,
    km: Math.round(terrain.length_km),
    channels: [{dial_hz: dial, mode: "FM", bandwidth_hz: 12500, modem, favourite: false,
                blocked: false}],
    terrain: {...terrain, dial_hz: dial}};
  return Object.assign(gw, placedFrom(HERE, gw.grid));
}
const MORE_VHF = [
  vhf("N4CALL-10", "Kennesaw Mountain, Georgia", {lat: 33.9763, lon: -84.5779, agl_m: 10}, 145070000,
      "ax25", {state: "judged", chance: "likely to work", length_km: 30.8,
               class: {kind: "partly", at_km: 1.2}, class_words: "Fresnel zone partly obstructed at 1.2 km",
               words: "Fresnel zone partly obstructed at 1.2 km · diffraction about 8 dB (P.526) · margin about 43 dB · likely to work"}),
  vhf("N5CALL-10", "Hapeville, Georgia", {lat: 33.6407, lon: -84.4277, agl_m: 12}, 145090000,
      "aether-fm", {state: "judged", chance: "likely to work", length_km: 12.6,
                    class: {kind: "partly", at_km: 8.8}, class_words: "Fresnel zone partly obstructed at 8.8 km",
                    words: "Fresnel zone partly obstructed at 8.8 km · diffraction about 16 dB (P.526) · margin about 43 dB · likely to work"}),
  vhf("N6CALL-10", "Stockbridge, Georgia", {lat: 33.60, lon: -84.20, agl_m: 6}, 440200000, "ax25",
      {state: "judged", chance: "may work", length_km: 24.0,
       class: {kind: "blocked", at_km: 20.2, above_ray_m: 42},
       class_words: "Blocked by terrain 42 m above the direct ray at 20.2 km",
       close: "ground 0.1 km from the gateway's antenna, 13° up", spread_db: 14,
       words: "Blocked by terrain 42 m above the direct ray at 20.2 km · diffraction about 40 dB (P.526) · margin about 4 dB · may work · less certain: ground 0.1 km from the gateway's antenna, 13° up"}),
  vhf("N7CALL-10", "Austell, Georgia", {lat: 33.62, lon: -84.60, agl_m: 8}, 145010000, "vara-fm",
      {state: "judged", chance: "likely to work", length_km: 24.3,
       class: {kind: "blocked", at_km: 6.1, above_ray_m: 8},
       class_words: "Blocked by terrain 8 m above the direct ray at 6.1 km",
       words: "Blocked by terrain 8 m above the direct ray at 6.1 km · diffraction about 25 dB (P.526) · margin about 29 dB · likely to work"}),
];
const GATEWAYS_VHF = {...GATEWAYS, gateways: [VHF_GATEWAY, ...MORE_VHF, ...GATEWAYS.gateways]};
const HILLSHADE = readFileSync(join(ROOT, "tools", "manual", "terrain-hillshade.jpg"));
// Setup's terrain: a few tiles held, the area being prepared, the radii as the client gives them
// from downtown
const NOTICE_90 = TERRAIN_PATH.notice;
const NOTICE_30 = NOTICE_90.replaceAll("WorldDEM™-90", "WorldDEM-30");
const TERRAIN_STATUS = {dir: "C:\\Users\\you\\AppData\\Local\\RadioPost\\terrain",
  held: {glo30: 1, glo90: 3}, held_mb: 61.8,
  fetching: {tile: "N34W085", read: 2100000, of: 5300000}, queued: 1, done: 2, failed: [],
  notices: [{data: "Copernicus GLO-30", notice: NOTICE_30}, {data: "Copernicus GLO-90", notice: NOTICE_90}],
  around: [
    {radius_km: 25, tiles: 1, mb: {glo90: 5, glo30: 45}},
    {radius_km: 50, tiles: 4, mb: {glo90: 20, glo30: 180}},
    {radius_km: 100, tiles: 8, mb: {glo90: 40, glo30: 360}},
    {radius_km: 150, tiles: 14, mb: {glo90: 70, glo30: 630}}]};

// the forms the panel holds, as /api/forms gives them (RP-2 §8): every published template, in the
// order vectors/forms.json lists them, with its fingerprint from there
const formField = (f) => ({required: false, choices: [], bound: null, max: null, scale: null,
  unit: null, ...f, columns: (f.columns || []).map(formField)});
const FORMS = JSON.parse(readFileSync(join(ROOT, "vectors", "forms.json"), "utf8")).templates
  .map((v) => {
    const t = JSON.parse(readFileSync(join(ROOT, v.file), "utf8"));
    return {template: v.fingerprint_hex, name: t.name, note: t.note, fields: t.fields.map(formField)};
  });

// the reports (/api/reports): damage reports around Atlanta after a storm, and two storm reports,
// as the panel gives them: each place with its text and map link, and the choices made
const fpOf = (file) => JSON.parse(readFileSync(join(ROOT, "vectors", "forms.json"), "utf8"))
  .templates.find((v) => v.file === `forms/${file}.json`).fingerprint_hex;
const placeOf = (lat, lon, label = "Location") => {
  const deg = (d) => {
    const t = d.toFixed(5).replace(/0+$/, "");
    return t.endsWith(".") ? `${t}0` : t;
  };
  return {label, lat, lon, within: null,
    text: `${deg(lat)}, ${deg(lon)}`,
    osm: `https://www.openstreetmap.org/?mlat=${deg(lat)}&mlon=${deg(lon)}#map=17/${deg(lat)}/${deg(lon)}`};
};
const REPORTS = [
  ["N3CALL", "shelter3@radiopost.org", 33.7812, -84.3876, 3, "Roof gone, 12 Peachtree Pl"],
  ["N4CALL", "n4call@radiopost.org", 33.7629, -84.4012, 1, "Fence down"],
  ["N5CALL", "n5call@radiopost.org", 33.7955, -84.3561, 4, "House destroyed, Elm St"],
  ["N6CALL", "n6call@hampost.org", 33.7480, -84.3698, 2, "Windows out, car port"],
  ["N7CALL", "n7call@radiopost.org", 33.8102, -84.4170, 0, "No damage"],
  ["N8CALL", "n8call@radiopost.org", 33.7701, -84.3305, 3, "Tree through the roof"],
  ["N9CALL", "n9call@hampost.org", 33.7344, -84.4256, 2, "Shed flattened"],
].map(([call, from, lat, lon, damage, subject], i) => ({
  id: `damage${i}`.padEnd(20, "a"), folder: "inbox", from, to: [ME.address],
  date: T - 600 * (i + 1), subject, template: fpOf("damage-report"), form: "Damage Report",
  places: [placeOf(lat, lon)], choices: {"5": damage, "4": 0},
})).concat([
  {id: "storm0".padEnd(20, "a"), folder: "inbox", from: "n4call@radiopost.org",
   to: [ME.address], date: T - 5400, subject: "Hail and wind", template: fpOf("storm-report"),
   form: "Storm Report", places: [placeOf(33.7590, -84.3880)], choices: {"7": 0}},
]);

// a form's position field, filled from the GPS in use (/api/form-position), and what it goes as
const FORM_POSITION = {text: "33.78123, -84.38765", says: "From the GPS, 4 s ago."};
const FORM_POSITION_SHOWN = {value: [337812300, -843876500], shown: "33.78123, -84.38765 (EM73ts)"};

function clientApi(world) {
  return (method, path, query, body) => {
    const s = world.state;
    switch (path) {
      case "state": return s;
      case "log": return {lines: Number(query.get("since")) ? [] : world.log, next: world.log.length};
      case "folder": return {inbox: INBOX, outbox: OUTBOX, sent: SENT, trash: []}[query.get("name")];
      case "message": return MESSAGES[query.get("id")] || {...INBOX[1], files: [], held: [], body: ""};
      case "waiting": return WAITING;
      case "gateways": return world.vhf ? GATEWAYS_VHF : world.hf ? GATEWAYS_HF : GATEWAYS;
      case "terrain": return TERRAIN_STATUS;
      case "terrain/path": return TERRAIN_PATH;
      case "places": return PLACES;
      case "attempts": return RECENT;
      // only the shots of the Station box show the calls made before; the others keep it empty
      case "called": return world.called
        ? calledOf(world.vhf ? GATEWAYS_VHF : world.hf ? GATEWAYS_HF : GATEWAYS)
        : {gateway: [], station: []};
      case "learned": return LEARNED;
      case "p2p-chance": return world.p2p || null;
      case "forget": return FORGET;
      case "settings": return {usual_gateways: ["KK4ODA-1"], radio_skips_read: false};
      case "accept": return {entries: ["maria@example.com", "jim@hampost.org", "@hampost.org"]};
      case "addresses": return {entries: ADDRESSES};
      case "forms": return FORMS;
      case "form-position": return body ? FORM_POSITION_SHOWN : FORM_POSITION;
      case "reports": return REPORTS;
      case "modem": return {};
      // a channel picked is tuned at once (W14): moved there, or, for a radio Aether keys with no
      // CAT, the dial to set by hand
      case "tune": {
        const mhz = (body.dial_hz / 1e6).toFixed(4).replace(/0$/, "");
        return world.byHand
          ? {tuned: "by_hand", dial_hz: body.dial_hz, tone: true,
             said: `set the radio's dial to ${mhz} MHz (USB) yourself: the modem cannot set it`}
          : {tuned: "moved", dial_hz: body.dial_hz, tone: true,
             said: `tuned the radio to ${mhz} MHz`};
      }
      case "tone": return {on: body.on, seconds: body.on ? 10 : 0};
      case "alerts": case "circles":
        return {seq: s.alerts.seq, circles: ALERT_CIRCLES, log: world.alerts,
                alarming: world.alerts.some((e) => e.alarm),
                radio_hz: 7101000, scanned_hz: []};
      case "notify": return NOTIFY;
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
  operator_checks_rules: false, answer_at_start: true, report_status: true,
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

// the RadioPost client's accounts on the gateway's computer (W46): the gateway registers with one
const GW_ACCOUNTS = [
  {id: 1, home: "C:\\Users\\you\\.radiopost", address: "facundo@radiopost.org", operator: "KK4ODA",
   domain: "radiopost.org", chosen: true},
  {id: 2, home: "C:\\Users\\you\\.radiopost\\callsigns\\2", address: "kk4oda@hampost.org",
   operator: "KK4ODA", domain: "hampost.org", chosen: false},
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
        {domain: "hampost.org", ok: true, at: T - 240, url: "https://po.hampost.org/rp/v1"}],
      // what it last told its post office about being on the air (RP-5 §5.9)
      reported: {off: false, to: "radiopost.org", state: "listening", at: T - 95, error: null}},
    held: {copies: 5, users: 3, records: 0, queued: 0},
    key: "5a1f0e9d8c7b6a5f4e3d2c1b0a9f8e7d6c5b4a3f2e1d0c9b8a7f6e5d4c3b2a19",
    client: {home: "C:\\Users\\you\\.radiopost",
             enrolled: {operator: "KK4ODA", address: "facundo@radiopost.org"},
             accounts: GW_ACCOUNTS},
    registration: {callsign: "KK4ODA-1", address: "facundo@radiopost.org", domain: "radiopost.org",
                   serial: 1187, expires: T + 63 * DAY, operator: "KK4ODA",
                   post_office: "https://po.radiopost.org/rp/v1",
                   home: "C:\\Users\\you\\.radiopost"},
    last_record: {channels: [
      {dial_hz: 3591500, mode: "USB", modem: "aether", bandwidth_hz: 500},
      {dial_hz: 7101000, mode: "USB", modem: "aether", bandwidth_hz: 2300,
       station: {power_w: 100, antenna: "dipole", agl_m: 12}}],
      grid: "EM73", place: "Atlanta, Georgia", hours: "always", internet: true,
      site: {lat: 33.79, lon: -84.33, agl_m: 12}},
    dir: "C:\\Users\\you\\.radiopost-gateway", version: VERSION,
    aether: "ws://127.0.0.1:8515/v1", update: UPDATE,
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
      case "accounts": return {home: world.state.client.home, accounts: GW_ACCOUNTS};
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
  // the clock and the API are the context's, so a window the page opens has them too
  await context.clock.setFixedTime(NOW);
  await context.route(`${ORIGIN}/**`, async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname === "/api/terrain/hillshade") {
      await route.fulfill({body: HILLSHADE, contentType: "image/jpeg"});
      return;
    }
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
  const page = await context.newPage();
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
          operator: over.operator || false, byHand: over.byHand || false, vhf: over.vhf || false,
          hf: over.hf || false, called: over.called || false,
          alerts: over.alerts || ALERT_LOG};
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
    activity: {state: "session", remote: "KK4ODA-1", failed: null, listen: false, held: null},
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

// a Storm Report written: the position from the GPS, two events ticked, the hail's size and the
// wind's speed with their units, what was damaged
async function fillReport(page) {
  await page.click("#write");
  const form = page.locator("#composer");
  await form.locator("input[name=to]").fill("ffc-reports@example.gov");
  await form.locator("input[name=subject]").fill("Hail and wind, Atlanta");
  await form.locator("select[name=form]").selectOption({label: "Storm Report"});
  await page.waitForTimeout(400);
  const field = (n) => page.locator(`#c-form-fields [data-field="${n}"]`);
  await field(2).fill("2026-11-14T20:10");
  const events = field(3).locator("input[type=checkbox]");
  await events.nth(3).check();
  await events.nth(4).check();
  await field(4).fill("NE at 30 mph");
  await field(5).fill("1.75");
  await field(6).fill("60");
  await field(7).selectOption({label: "Measured"});
  await field(11).fill("Two large limbs down on Peachtree St.\nCar windows broken.");
  await field(14).fill("KK4ODA");
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

  // the call box with KK4ODA-1 chosen: its first channel tuned at once, the tuner tone, Call
  async "client-call"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI,
                                                 api: clientApi(clientWorld({called: true})),
                                                 height: 1100});
    await spreadLog(page, "log", T - 1300, 19);
    await page.click("#gateways-open");
    await page.waitForTimeout(200);
    await page.locator("#gateway-rows tr", {hasText: "KK4ODA-1"})
      .getByRole("button", {name: "Choose"}).click();
    await page.click("button[data-folder=inbox]");
    await page.waitForTimeout(300);
    await page.evaluate(() => { document.getElementById("toast").hidden = true; });
    await shot(page, "client-call", ".station");
    await context.close();
  },

  // a station call: its dial and bandwidth, on a radio whose dial the modem cannot set
  async "client-call-station"(browser) {
    const world = clientWorld({byHand: true, called: true});
    // N0CALL's gateway is listed: 40 m predicted poor at the hour, 80 m better
    world.p2p = {gateway: "N0CALL-10",
                 chosen: {band: "40m", distance_km: 95, chance: 0.12, chance_words: "unlikely to work",
                          words: "Unlikely to work: links on 12 % of days at 00Z"},
                 better: {dial_hz: 3_590_000,
                          hf: {band: "80m", chance: 0.86, chance_words: "likely to work"}}};
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(world),
                                                 height: 900});
    await spreadLog(page, "log", T - 1300, 19);
    await page.check("input[name=kind][value=station]");
    await page.fill("#callsign", "N0CALL");
    await page.fill("#dial", "7.103");
    await page.selectOption("#bandwidth", "500");
    await page.locator("#dial").dispatchEvent("change");
    await page.locator("#dial").blur();
    await page.waitForTimeout(1000);
    await page.evaluate(() => { document.getElementById("toast").hidden = true; });
    await shot(page, "client-call-station", ".station");
    await context.close();
  },

  // P2P chosen: the last station called back in Call with its dial and bandwidth, and ▾ open on
  // the stations called before
  async "client-called"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI,
                                                 api: clientApi(clientWorld({called: true})),
                                                 height: 900});
    await spreadLog(page, "log", T - 1300, 19);
    await page.waitForTimeout(300);
    await page.check("input[name=kind][value=station]");
    await page.click("#called-open");
    await page.waitForTimeout(300);
    await page.screenshot({path: join(OUT, "client-called.png"),
                           clip: await unionClip(page, [".station", "#called-list"])});
    console.log("docs/manual/screens/client-called.png");
    await context.close();
  },

  // Listen for P2P switched on: sunk in, its lamp lit, reading Listening for P2P; Call greyed
  async "client-listening"(browser) {
    const world = clientWorld({state: {activity: {state: "listening", remote: null, failed: null,
                                                  listen: true, held: null,
                                                  listening_at: {dial_hz: 7_103_000,
                                                                 answers: [500, 2300],
                                                                 scanning: []}}}});
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(world),
                                                 height: 900});
    await spreadLog(page, "log", T - 1300, 19);
    await page.waitForTimeout(300);
    await shot(page, "client-listening", ".station");
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

  // a narrow window: the panel stacked in one column, and Write's composer in a window of its own
  // beside it, the two drawn side by side as on a screen
  async "client-compose-window"(browser) {
    const panelSize = {width: 600, height: 820};
    const composerSize = {width: 660, height: 820};
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld()),
                                                 ...panelSize});
    await spreadLog(page, "log", T - 1300, 19);
    const opened = context.waitForEvent("page");
    await page.click("#write");
    const w = await opened;
    await w.setViewportSize(composerSize);
    await w.waitForTimeout(400);
    const form = w.locator("#composer");
    await form.locator("input[name=to]").fill("maria@example.com");
    await form.locator("input[name=to]").press("Escape");
    await form.locator("input[name=subject]").fill("Field Day: count me in");
    await form.locator("textarea[name=body]").fill(
      "Hi Maria,\n\nCount me in for Friday. I will bring the 40 m dipole, a chair and\n"
      + "the spare battery box.\n\n73,\nAna, N0CALL");
    await w.evaluate(() => document.activeElement.blur());
    const panel = (await page.screenshot()).toString("base64");
    const composer = (await w.screenshot()).toString("base64");
    const frame = (title, png, size) => `<figure><figcaption>${title}</figcaption>`
      + `<img src="data:image/png;base64,${png}" width="${size.width}" height="${size.height}"></figure>`;
    const both = await context.newPage();
    const gap = 28;
    await both.setViewportSize({width: panelSize.width + composerSize.width + 3 * gap + 4,
                                height: panelSize.height + 2 * gap + 32});
    await both.setContent(`<style>
      body { margin: 0; padding: ${gap}px; display: flex; gap: ${gap}px; align-items: flex-start;
             background: #d5dae0; font: 13px/30px Inter, system-ui, sans-serif; }
      figure { margin: 0; border: 1px solid #9aa3ad; border-radius: 8px; overflow: hidden;
               background: #fff; box-shadow: 0 10px 30px rgb(0 0 0 / .18); }
      figcaption { height: 30px; padding: 0 12px; background: #eef0f2; color: #1b1f23;
                   border-bottom: 1px solid #d9dde1; }
      img { display: block; }
    </style>${frame("RadioPost", panel, panelSize)}${frame("New message · RadioPost", composer,
      composerSize)}`);
    await both.screenshot({path: join(OUT, "client-compose-window.png")});
    console.log("docs/manual/screens/client-compose-window.png");
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

  async "client-form-report"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld()),
                                                 height: 1300});
    await spreadLog(page, "log", T - 1300, 19);
    await fillReport(page);
    await page.locator("body").click({position: {x: 1200, y: 1200}});
    await shot(page, "client-form-report");
    await context.close();
  },

  async "client-reports"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld()),
                                                 height: 1100});
    await spreadLog(page, "log", T - 1300, 19);
    await page.click('[data-folder="reports"]');
    await page.waitForTimeout(300);
    await page.locator("#report-form").selectOption({label: "Damage Report"});
    await page.waitForTimeout(300);
    await page.locator("#report-rows tr").first().click();
    await page.waitForTimeout(300);
    await shot(page, "client-reports");
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
    const {page, context} = await open(browser, {ui: CLIENT_UI,
                                                 api: clientApi(clientWorld({hf: true})),
                                                 height: 900});
    await spreadLog(page, "log", T - 1300, 19);
    await page.click("#gateways-open");
    await page.evaluate(() => { document.getElementById("map-card").open = false; });
    await page.waitForTimeout(200);
    await shot(page, "client-gateways");
    await context.close();
  },

  // a gateway's details: the model, what was learned and what was measured, side by side
  async "client-gateway-details"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI,
                                                 api: clientApi(clientWorld({hf: true})),
                                                 height: 1100});
    await page.click("#gateways-open");
    await page.waitForTimeout(300);
    await page.locator("#gateway-rows tr", {hasText: "KK4ODA-1"})
      .getByRole("button", {name: "Details"}).click();
    await page.waitForTimeout(300);
    await shot(page, "client-gateway-details", "#details-card");
    await context.close();
  },

  // the map: the gateways of the view round Atlanta, N1CALL-10 picked in the list, its great
  // circle drawn and what it is under the map
  async "client-gateway-map"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld()),
                                                 height: 1000});
    await page.click("#gateways-open");
    await page.waitForTimeout(300);
    await page.locator("#gateway-rows tr", {hasText: "N1CALL-10"}).locator("td").nth(1).click();
    // the click scrolled down to the row: the map back in view
    await page.locator("#map-card").evaluate((card) => card.scrollIntoView({block: "start"}));
    await page.waitForTimeout(300);
    await shot(page, "client-gateway-map", "#map-card");
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
    // the pointer on the first message, so its Edit and trash can show
    await page.hover("#rows tr:nth-child(1)");
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

  // the Alerts view: what Ana sent and heard, and her two circles with their checks
  async "client-alerts"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI,
                                                 api: clientApi(clientWorld({hf: true})),
                                                 height: 1700});
    await page.click("button[data-folder=alerts]");
    await page.waitForTimeout(400);
    await page.fill("#alert-to", "KK4ODA");
    await page.locator("#alert-to").dispatchEvent("input");
    await page.fill("#alert-text", "Net in 10 minutes on 3.590");
    await page.locator("#alert-text").dispatchEvent("input");
    await page.locator("body").click({position: {x: 1250, y: 30}});
    await shot(page, "client-alerts", "#alerts-view");
    await context.close();
  },

  // an alert from KK4ODA arriving: the alarm over the window
  async "client-alarm"(browser) {
    const world = clientWorld({state: {alerts: {seq: 2, alarming: true, circles: 2}},
                               alerts: [ARRIVING, ...ALERT_LOG]});
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(world)});
    await spreadLog(page, "log", T - 1300, 19);
    await page.waitForTimeout(1500);
    await shot(page, "client-alarm");
    await context.close();
  },

  // Setup's Sounds and notifications
  async "client-sounds"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld()),
                                                 height: 2200});
    await page.click("#setup-open");
    await page.waitForTimeout(400);
    await page.evaluate(() => document.getElementById("sounds-card").scrollIntoView());
    await shot(page, "client-sounds", "#sounds-card");
    await context.close();
  },

  async "client-recent"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld()),
                                                 height: 900});
    await page.click("#gateways-open");
    await page.waitForTimeout(200);
    await page.click("[data-gview=recent]");
    await page.waitForTimeout(200);
    await shot(page, "client-recent", "#gateways-view");
    await context.close();
  },

  // a VHF gateway's line in the list, and its terrain: the profile, the budget each way
  async "client-terrain"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld({vhf: true})),
                                                 height: 1900});
    await page.click("#gateways-open");
    // both maps folded away, as the operator can, so the card and the row fit
    await page.evaluate(() => {
      document.getElementById("map-card").open = false;
      document.getElementById("area-card").open = false;
    });
    await page.waitForTimeout(200);
    await page.locator("#gateway-rows tr", {hasText: "N3CALL-10"}).getByRole("button", {name: "Terrain"}).click();
    await page.waitForTimeout(400);
    await page.screenshot({path: join(OUT, "client-terrain.png"), clip: await unionClip(page,
      ["#terrain-card", "#gateway-rows tr:first-child"])});
    console.log("docs/manual/screens/client-terrain.png");
    await context.close();
  },

  // the terrain map: five VHF gateways round Atlanta over the hillshade, N6CALL-10 picked
  async "client-area"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld({vhf: true})),
                                                 height: 1500});
    await page.click("#gateways-open");
    await page.evaluate(() => { document.getElementById("map-card").open = false; });
    await page.waitForTimeout(400);
    await page.locator("#area-map .a-gateway", {hasText: "N6CALL-10"}).click();
    await page.evaluate(() => document.activeElement.blur());
    await page.locator("#area-card").evaluate((card) => card.scrollIntoView({block: "start"}));
    await page.waitForTimeout(400);
    await shot(page, "client-area", "#area-card");
    await context.close();
  },

  async "client-terrain-setup"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld()),
                                                 height: 1800});
    await page.click("#setup-open");
    await page.waitForTimeout(400);
    await page.evaluate(() => document.getElementById("terrain-setup").scrollIntoView());
    await shot(page, "client-terrain-setup", "#terrain-setup");
    await context.close();
  },

  async "client-places"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld()),
                                                 height: 1500});
    await page.click("#setup-open");
    await page.waitForTimeout(400);
    await page.evaluate(() => document.getElementById("places-card").scrollIntoView());
    await shot(page, "client-places", "#places-card");
    await context.close();
  },

  async "client-stations"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI, api: clientApi(clientWorld()),
                                                 height: 1500});
    await page.click("#setup-open");
    await page.waitForTimeout(400);
    await page.evaluate(() => document.getElementById("stations-card").scrollIntoView());
    await shot(page, "client-stations", "#stations-card");
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

  async "desk-gateways"(browser) {
    const page = await openDesk(browser, "gateways", 700);
    await shotMain(page, "desk-gateways");
    await page.context().close();
  },

  async "desk-code"(browser) {
    const page = await openDesk(browser, "code", 1000);
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
    // the gateway scans its two channels (W50)
    const state = gatewayState();
    state.status.scan = {dials: [3591500, 7101000], scan_s: 20, at_hz: 7101000, not: null};
    const world = {state, sessions: GW_SESSIONS};
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
      held: {}, key: null, registration: null, last_record: null}), sessions: [], log: [],
      refused: [], partners: []};
    const {page, context} = await open(browser, {ui: GATEWAY_UI, api: gatewayApi(world), height: 720});
    await page.waitForTimeout(400);
    await shot(page, "gateway-first");
    await context.close();
  },

  async "gateway-setup"(browser) {
    const world = {state: gatewayState({running: false,
      status: {state: "stopped", since: T - 60, problem: null, session: null, logged: 0,
               checks: []}}), sessions: GW_SESSIONS};
    const {page, context} = await open(browser, {ui: GATEWAY_UI, api: gatewayApi(world), height: 2300});
    await page.waitForTimeout(400);
    // registered with radiopost.org: the form filled from the record it registered
    await page.click("#setup-open");
    await page.waitForTimeout(200);
    await page.locator("body").click({position: {x: 1270, y: 10}});
    // the second channel's station open, to show what it holds
    await page.locator("#channel-rows tr.channel").nth(1).locator(".station-open").click();
    await page.waitForTimeout(100);
    await page.screenshot({path: join(OUT, "gateway-setup.png"), clip: await unionClip(page,
      ["#setup h1", "#po-form", "#updates-check"], 1100)});
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

  // the Gateways page down to its map, N1CALL-10 picked in the list
  async "readme-gateways"(browser) {
    const {page, context} = await open(browser, {ui: CLIENT_UI,
                                                 api: clientApi(clientWorld({hf: true})),
                                                 width: 1280, height: 1000});
    await page.click("#gateways-open");
    await page.waitForTimeout(300);
    await page.locator("#gateway-rows tr", {hasText: "N1CALL-10"}).locator("td").nth(1).click();
    await page.locator("#gateways-view").evaluate((view) => view.scrollIntoView({block: "start"}));
    await page.waitForTimeout(300);
    await readmeShot(page, "readme-gateways",
                     ["#gateways-view > h1", "#gateways-view > .tabs", "#map-card"]);
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
