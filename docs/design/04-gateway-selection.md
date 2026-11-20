# RadioPost: choosing a gateway (a proposal)

| | |
|---|---|
| **Status** | Accepted, in progress |
| **Version** | 0.3.1 |
| **Updated** | 2026-11-20 |
| **Editor** | KK4ODA |
| **Licence** | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| **Follows** | [The wishlist](02-wishlist.md), W17; [the concept](../concept/00-concept.md), §9 and §10 |

The question this document answers is the operator's:

> Given where I am, the terrain, my station, the conditions, what has worked before and what I have
> to send, which gateway and channel will get my mail through, and how soon?

Today the client answers it by distance from a typed grid square. This document proposes how to
answer it properly, for HF and for VHF and UHF, and how hf-predict should change to help. It is a
proposal the author accepted on 2026-11-20, with the ten answers in §21; no specification has
changed yet. The wishlist carries it as W17 and W55 to W58. Phase 1 is in progress (§18).

**How facts are marked.** Facts about RadioPost, Aether and hf-predict were read in their
repositories on 2026-11-20. Outside facts were researched the same day: **[P]** checked on a primary
source, **[S]** found only secondhand, **[T]** fetched and measured. Assumptions are called
assumptions; numbers to be measured are called that.

---

## 1. Summary

**The answer, in one paragraph.** Rank *candidates* (a gateway on one channel with one modem
setting, from here, now) by **how soon this transaction is likely to be done** (the time by which it
is done four times in five), computed from separate estimates of whether the gateway is there,
whether the path carries a call each way, how fast the modem moves data on it, and how often links
break. Each estimate starts from physics (VOACAP both ways at HF, terrain and a link budget at VHF),
is turned into modem terms by the modem's own measured ladder, and is corrected by what this station
has measured, failures included and classified. Everything runs on the client, offline, with no
central service; optional additions to RP-4 and RP-5 would sharpen it.

| Topic | Recommendation | § |
|---|---|---|
| What is ranked | Gateway × channel × modem setting × position × station × time | 5 |
| The score | Time by which the transaction is done 4 times in 5 (T80); priority traffic: probability of being done by a deadline | 13 |
| HF physics | VOACAP (hf-predict's engine), both directions, batched and cached; SNR deciles turned into each rung's probability by the modem's ladder | 10 |
| VHF physics | Copernicus GLO-30 terrain; clearance and Fresnel shown; loss by ITU-R P.526 §4.5; a link budget each way; ITM only if records show it ranks better | 8, 9 |
| Learning | Per-path SNR offsets against the model (a grid posterior that treats failed calls as censored evidence), availability, service, modem efficiency and link losses; pooled, forgotten slowly, each explainable in a sentence | 12 |
| Failures | Classified from the evidence the client holds; each class updates only what it bears on; station faults detected and kept out of gateways' records | 12.3 |
| Position | Saved places; GPS by serial, TCP, UDP and gpsd (covers USB, Bluetooth, GPS2IP on an iPhone, Android apps); stale positions never used as current | 6 |
| Directory | RP-5's signed lists, as now; optional record members for site, station, listening windows and the post office's last contact; local entries for the operator | 7 |
| hf-predict | Its domain code moved into Tauri-free crates, relicensed MIT OR Apache-2.0, embedded in both apps; one small local API for today's FT8 evidence | 11 |
| Records | One line of JSON per attempt, never rewritten; everything learned recomputed from them | 15 |
| Order of work | Position, directory and **attempt records first**; then VHF terrain and HF in either order; then learning, then transaction ranking; after a failure the client suggests the next candidate and never calls by itself | 18 |
| Proof | Method against method on RadioPost's own records and a randomised field study; Winlink's published method as one baseline; no claim before the numbers | 19 |

**What is new against Winlink Express and Pat** (§4): their HF prediction is VOACAP at one required
SNR for any data signal (40 dB-Hz in Winlink Express, 20 in Pat); here it is the modem's own ladder,
so connecting and carrying data are predicted separately. Winlink Express uses connection history
only in auto-connect, with unpublished weights; here history enters every ranking, every weight is
written down, and failures are classified before they count. Neither looks at terrain at VHF.

---

## 2. RadioPost today

### 2.1 What exists

| Part | What it does now | Files |
|---|---|---|
| Gateway records | Each post office publishes its gateways: callsign, operator, a 4- or 6-character grid, a place name, channels (`dial_hz`, `mode` USB or FM, `modem`, `bandwidth_hz`, `scan_s`), free-text `hours`, `internet`, copies, forwarding windows, `updated` (RP-5 §4.4). | `spec/rp-0005-post-office-api.md`, `core/radiopost/src/gateway_list.rs` |
| The client's list | Fetched from the home post office and every post office it names, checked, merged, kept in `state.json` for use offline, refreshed every 6 hours while online (D36). | `app/client/src/online.rs`, `app/client/src/store.rs` (`gateway_list`) |
| Ranking | **Distance only**: from one grid square the operator types to each gateway's grid square, centre to centre, nearest first (`Store::gateways_ranked`). | `app/client/src/grid.rs`, `app/client/src/store.rs` |
| Choosing | The Gateways view lists callsign, place, distance, channels and hours; *Choose* puts the callsign and channel in the call box and tunes the radio (W14). | `app/ui/app.js` (`openGateways`), `app/ui/index.html` |
| The call | `session::call` opens the modem, tunes, places `connect` (with `call_s` for a scanning gateway), runs the session engine, and returns a `Report`: `remote`, `transmissions`, `received`, `delivered`, `queued`, and `failed` as a sentence. | `app/client/src/session.rs` |
| What the modem says | RPMI gives the host `connected` (with bandwidth), `failed` (`timeout`, `refused`, `busy`, `aborted`), `disconnected` (`local`, `remote`, `timeout`, `failure`), `quality` (`snr_db`, `rate_bps`, `receiving`), `busy`, `heard` (`callsign`, `snr_db`), `queued`, `delivered`, `dial` (RP-4 §5). | `spec/rp-0004-modem-interface.md`, `core/rpmi/src/host.rs` |
| Live progress | `Live` follows `queued`, `delivered` and `quality` during a session, for the window (W48). | `app/client/src/session.rs` |
| The log | Dated lines in `radiopost.log` in the first home, shared in diagnostic zips, written without correspondents' addresses. | `core/radiopost/src/logfile.rs` |

### 2.2 What is missing for this design

* **No position but a typed grid square.** No saved locations, no GPS, no station description.
  A 4-character grid is about 111 by 185 km at Atlanta's latitude; a 6-character one about 4.6 by
  7.7 km. That is enough to rank HF gateways by distance and far too coarse for a VHF terrain
  profile, whose answer is decided mostly by the terrain near each end.
* **No memory of attempts.** A call's outcome lives in the window and the log line. Nothing
  records, in a form a program can read back, which gateway was called on which channel, from
  where, at what time, with what result, how long it took, or how fast the data moved. The
  `Report` drops the modem's SNR and rate once the session ends.
* **Failures are sentences.** `failed: Some("the call failed: timeout")` mixes the modem's code
  with text. The information to classify most failures exists (RPMI's codes, RP-3's `ERROR`
  codes, the order of events) but is not kept as data.
* **Gateway records say little about the station.** No precise site, no antenna height, no
  power, and `hours` is text for people. A reader ignores members it does not know (RP-0 §4.2), so
  all of these can be added as optional members (§7).
* **The modem interface says nothing about the other direction.** `quality` carries the SNR this
  station hears. Aether also knows how well the other station hears this one (its `peer_snr_db`)
  and whether anything was heard of the called station during a call that failed. RPMI carries
  neither (§12.2).

### 2.3 What helps

* **Resume is built in** (RP-3 §7). A broken link loses at most the PDU in flight; the next session,
  through the same gateway or another, carries on. So an interruption costs a reconnection, not the
  transfer. The transaction model (§13) depends on this.
* **Messages are named by content** (D3). The outbox's air forms are known exactly before a call,
  and the waiting list from the last call gives the size of each message waiting (`Store::waiting`).
  Outbound volume is known; inbound volume is partly known.
* **The dial is in RPMI** (RP-4 §4.1, W14), so a choice can tune the radio, and every attempt can
  record the dial it was made on.
* **Aether already keeps a session history** of its own (`sessions.json`: remote, role, bandwidth,
  frequency, bytes sent, acknowledged and received, end reason, SNR both ways, top rungs) and a
  list of stations heard with their SNR (`heard.json`). Through the control API these could enrich
  a RadioPost attempt record; through RPMI they cannot today.
* **The modem's own ladder is published and measured.** Aether's air-interface specification lists
  every rung's rate and the SNR it needs on AWGN and the ITU channels (its §4, §4.1, §10). That is
  what turns a propagation prediction into a data-rate prediction (§10.3).

---

## 3. hf-predict today

hf-predict (v0.14.0, Apache-2.0, Tauri 2 with a Rust core and a React interface) predicts, measures
and compares HF propagation. Its own [engineering
assessment](https://github.com/KK4ODA/hf-predict/blob/master/docs/engineering-assessment.md)
records each phase's findings. What matters here:

| Area | What it has | Files (under `src-tauri/src/`) | Fit for RadioPost |
|---|---|---|---|
| Engine | Real VOACAP (`voacapl`, the gfortran port) run as a subprocess, built for Windows, macOS and Linux by one script; output identical between Windows and Linux; method 30; about 9 MB with its data. | `engine.rs`, `voacap/deck.rs`, `voacap/output.rs`, `engines/voacapl/build.sh` | **Reuse as is.** The hard part (a portable, validated engine) is done. |
| Engine interface | `PropagationEngine` with `predict` and `predict_hour_to_many`; typed `PredictionRequest` and `Prediction` with every VOACAP row, including the SNR deciles and `MUFday`. | `propagation.rs` | **Reuse.** The deciles are what a modem-rate prediction needs. |
| Station model | Power, one of five antennas (VOACAP antenna files), noise category, minimum angle; required SNR from three modes (SSB, CW, FT8). | `station.rs` | **Extend**: a required SNR from a modem's ladder, gateway profiles, antenna height. |
| Prediction | Short and long path, power comparison, MUF, FOT and LUF from a frequency sweep; coverage grids at 4°, cached by month and hour. | `predictor.rs`, `coverage.rs` | **Reuse** the coverage grid as a fast first pass over many gateways. |
| Solar input | Bundled smoothed sunspot table from SWPC; NOAA products by Internet or by Winlink request. | `solar.rs`, `spacewx/` | **Reuse.** |
| Observations | WSJT-X UDP listener and `ALL.TXT` import into SQLite (`observations`, `listening_intervals`), origin and provider on every row, a cache of predicted reliability. | `observations.rs`, `wsjtx/` | **Use as evidence** of propagation toward a gateway (§10.5), through an interface. |
| Comparison | A rule table of predicted tier against observed tier (HIGH PRIORITY, TRY, INVESTIGATE …). | `compare.rs` | **Reuse the tiers** as one input; the table itself is FT8's. |
| Calibration | Predicted reliability binned against the rate of hearing, from the station's own log: the model orders paths correctly (9 % to 39 % heard on 20 m across the bins), the absolute rate is not meaningful. | `calibration.rs` | **Evidence that VOACAP is a usable prior**, not a forecast. |
| Locations | A name and the text the user typed. No GPS. | `userdata.rs` | **Replace** with the shared location model (§6). |
| Packaging | One crate, `hf_predict_lib`, which depends on Tauri; the domain modules are separate files but not a library another program can take. | `Cargo.toml`, `lib.rs` | **Split** (§11). |

Limits that matter for this design:

1. **Everything is in one Tauri crate.** RadioPost cannot depend on the engine runner, the deck
   writer or the output parser without pulling in Tauri and the whole app.
2. **The station model knows three human modes.** A data modem's needs (a ladder of rungs, each with
   a threshold that depends on the channel's fading) have no place in it.
3. **One direction at a time, one station profile for both ends.** A gateway path needs both
   directions, each with its own transmitter, antenna and noise.
4. **Monthly climatology only.** VOACAP gives the share of days in the month a path works at an
   hour. That is the right prior, but it is not today's forecast, and nothing adjusts it with what
   was measured (the assessment says so, and labels predictions accordingly).
5. **Calibration is descriptive.** It bins and shows; it does not fit an offset that a later
   prediction uses, and it scores no probability (no Brier score or log loss).
6. **Licence.** Apache-2.0 only, where RadioPost's code is MIT OR Apache-2.0. A RadioPost binary
   that links Apache-2.0-only crates is still distributable, but the MIT option is lost for those
   parts. The author is hf-predict's only committer, and chose MIT OR Apache-2.0 for the extracted
   crates (§21, Q1).

---

## 4. How others choose

Researched on 2026-11-20. **[P]** marks a fact checked on a primary source (the program's source,
an API's own metadata, the developer's own material); **[S]** one found only secondhand (club
guides, forums). winlink.org refused automated access that day, so Winlink Express's help file was
not read; its developer's slides and a club's copy of its revision history stood in.

### 4.1 Winlink Express

* **HF channel selector.** Columns: callsign, frequency, mode, grid square, hours, group, distance,
  bearing, *Path Reliability (Estimate)* and *Path Quality (Estimate)*; sortable by any column,
  reliability first by default [S]. *Path Quality* is not defined anywhere found.
* **The estimate is VOACAP, run locally** on the user's PC; older versions needed NTIA's ITSHF
  package installed [S]. hf-predict's assessment records that Winlink Express has shipped DVOACAP,
  a re-implementation, since 2018. Inputs, from the `[Propagation]` settings of version 1.3.7.0 [S]:
  method 30, CCIR coefficients, 50 W, required SNR 40, required reliability 90 %, noise 143,
  take-off angle 5°, and the solar flux weighted 80 % against a smoothed sunspot table. No antenna
  input was found. A *Forecast* button shows 24 hours by channel [P, developer's slides].
* **Updating the list** over the Internet (Winlink's web service) or by radio, a request answered by
  a message the program applies itself [S].
* **Favourites**: day and night sets of favourite channels [P].
* **Auto-connect is the one place history is used** [P, developer's slides]: "channel connection
  order is based on more than propagation quality estimate": favourites get a boost; success and
  failure history; relative transfer speed against other gateways used; only the best two channels
  per gateway; after a busy frequency, nothing within ±500 Hz; filters for service code, minimum
  distance, blocked gateways, PACTOR level, VARA bandwidth, radio-only. **The weights are not
  published.**
* **VHF and UHF** (packet, VARA FM): sorted by distance from the grid square, with bearing [S].
  **No terrain or line-of-sight analysis** was found in any source, although Winlink's directory
  holds antenna height, gain, power and a "radio range" for each channel (below).

### 4.2 Winlink's directory

Winlink's web service is one central directory [P, its own metadata, CMS Web Services v5.0.9739]:

* A **channel record** carries timestamp, callsign, grid square, frequency, mode, baud, **power
  (W), antenna height above average terrain, gain (dB), direction** (0 for omnidirectional), hours
  and service code.
* The status list Pat uses carries each gateway's **latitude and longitude** and, per channel, a
  **"radio range"** and an antenna description [P, Pat's source].
* **A gateway appears only if it reported within a window** (`HistoryHours`, 6 by default, up to
  48): the directory doubles as an availability signal.
* Gateways report **every session** to the servers (grids, mode, frequency, distance, bytes,
  messages, time) [P]. Winlink therefore holds a large history of real outcomes. No public endpoint
  was found that ranks gateways with it.

### 4.3 Pat

Pat (open source, checked at commit 2ad31e8 of 2026-11-01) [P]:

* `pat rmslist` sorts by distance or by *link quality*, which needs VOACAP (a local `voacapl`, a
  Windows VOACAP, or a web service).
* Inputs: both grids, the frequency, the month's predicted sunspot number from SILSO, **50 W fixed**
  (marked TODO in the code), required SNR 20, take-off angle 3°, noise 145.
* *Link quality* (0–100) is a weighted product of VOACAP outputs: reliability (weight 2), SNR (1),
  take-off angle (0.3), MUFday (0.2), propagation mode (1.0), signal power (0.6).
* No history, no ranking by results. The list is cached until the user forces a download.
* GPS through gpsd, the locator updated hourly when asked.

### 4.4 Others

SailMail and Airmail offer a propagation tool, a prediction only [P]. JS8Call relays are chosen by
hand. APRS-IS clients rotate among servers by DNS. Nothing found ranks by measured outcomes except
Winlink Express's auto-connect.

### 4.5 Where RadioPost can do better, and where the comparison is unfair

| Winlink Express / Pat | This design |
|---|---|
| One prediction for "a data signal": a single required SNR (40 in Winlink Express, 20 in Pat) | The modem's own ladder: the probability of each rung each way, so connection and rate are predicted separately (§10.3) |
| Path reliability as the ranking | Completion time for this transaction (§13) |
| History used only in auto-connect, with unpublished weights | History in every ranking, with every weight written down and every estimate explainable (§12) |
| Failures counted, causes unknown | Failures classified; only the right quantity updated (§12.3) |
| VHF by distance | Terrain, Fresnel clearance, diffraction and a link budget (§8) |
| A typed grid square, or gpsd (Pat) | Saved places, serial, network and gpsd GPS, freshness checked (§6) |
| One directory, one operator; availability from recent reports | Many post offices, no centre; availability from the post office's own contact with the gateway (§7.4) |

**Fairness.** Winlink's network is two thousand gateways; RadioPost's is a pilot. A better
*method* does not make a better *network*, and no claim about completed transactions can be made
across two networks with different gateways and modems. The comparison this design can make
honestly is of methods, on RadioPost's own records: Winlink Express's method (VOACAP reliability at
SNR 40, then distance) as one of the baselines, replayed against the same attempts (§19.1).

---

## 5. The selection problem

### 5.1 What is ranked

The unit is a **candidate**: one gateway, on one of its channels, with one modem setting, from where
the station is now, with the station it has now:

```
candidate = (gateway callsign, channel: dial_hz + mode + modem + bandwidth_hz,
             station profile, position, time)
```

A gateway with three channels is three candidates. A channel run at 500 or 2300 Hz by Aether is two
when the gateway's record lists both bandwidths. The same gateway can be first on 40 m at noon and
last on 40 m at midnight. Ranking callsigns hides exactly the differences that decide the outcome.

### 5.2 Eight quantities, kept apart

| # | Quantity | What it is | Main evidence |
|---|---|---|---|
| Q1 | RF path viability | Can a signal get there and back at all, each way? | Propagation (HF), terrain and link budget (VHF), signals heard |
| Q2 | Connection probability | Does a call end in a link? | Q1, the gateway being there and listening, the channel being free, the modem's call |
| Q3 | Link stability | How often does a link that is up break before the work is done? | Session histories |
| Q4 | Effective throughput | Bytes of mail a second, after acknowledgements, retransmissions and turnarounds, each way | The modem's ladder and the SNR (prior), measured sessions |
| Q5 | Service availability | Is the gateway program answering and reaching its post office? | The post office's view (§7.4), `ERROR` codes in sessions |
| Q6 | Completion probability | Does *this* transaction finish within the time the operator gives it? | Q2–Q5 and the transaction's size |
| Q7 | Expected completion time | Including failed calls, reconnections and overhead | Q2–Q5 and the size |
| Q8 | Confidence | How much evidence stands behind each of the above | The amount, age and relevance of the evidence |

Q1 to Q5 are learned and predicted separately because each has different evidence, changes at a
different speed, and fails for different reasons. Q6 and Q7 are computed from them (§13). Q8 is
carried through everything and shown, never folded into the score where it cannot be seen.

### 5.3 The target

The question the ranking answers is the operator's:

> P(this transaction completes | candidate, position, station, conditions now, transaction)

and, with it, how long that is likely to take. A strong predicted signal is worth something only
through these. A gateway whose path is excellent but which answers half the time, or whose links
break every five minutes, is ranked by what that does to the transaction.

### 5.4 What is not ranked on

* **Distance itself.** It is a proxy for Q1 and is used only where nothing better exists: the cold
  start, and as a weak prior at VHF without terrain.
* **The best rung the modem could reach.** A path that peaks at 64-QAM for ten minutes a day and
  fades below the floor the rest is a poor choice for a 50 kB transfer, whatever its median.
* **Favourites.** They are the operator's preference. They order equals; they never hide a
  candidate's problems or lift a poor one over a good one (§14.5).

---

## 6. Location and station

### 6.1 Saved places

| Field | Notes |
|---|---|
| Name | "Home", "Mobile", "Field Day", "EOC" |
| Latitude, longitude | Degrees, WGS 84. The grid square is computed and shown, never stored as the truth |
| Ground elevation | Optional; filled from the elevation data when it is present (§9) |
| Default | One place is the default |
| Station profile | Optional link: picking the place picks the profile (§6.5) |
| Source | Typed, picked on the map, from a grid square (its centre, marked as such), or saved from the GPS |

Entering a grid square still works and is the quickest way in; the place then says it is a square's
centre, and the VHF analysis says its precision limits it (a 6-character square is about 4.6 by
7.7 km at Atlanta's latitude, enough to place a ridge on the wrong side).

### 6.2 GPS

Three readers, sharing one NMEA parser, cover every receiver and phone found in the research of 2026-11-20, on all
three desktop systems, with a fourth for Linux stations already running gpsd:

| Reader | Covers |
|---|---|
| **Serial NMEA** (a port and a speed, or *find*) | USB receivers (COMn, `/dev/ttyUSB*`, `/dev/ttyACM*`, `/dev/cu.usbmodem*`); Bluetooth receivers paired as serial ports (Windows' outgoing COM port, Linux `rfcomm`, macOS `/dev/cu.*`); Android apps that offer Bluetooth serial (Share GPS, GPS NMEA Tether, Bluetooth GPS Output) |
| **TCP client** (host and port) | GPS2IP's socket mode on the iPhone (port 11123 in its examples; `172.20.10.1:11123` over the phone's own hotspot); Share GPS, gpsdRelay and NetGPS on Android |
| **UDP listener** (a port) | GPS2IP's UDP client mode (unicast; broadcast and multicast do not work over the iPhone's hotspot); gpsdRelay, Share GPS, GPSd Forwarder |
| **gpsd** (host and port, 2947) | A Linux station where gpsd already holds the receiver; Share GPS's gpsd server. Not a native Windows option |

Left out: the operating systems' location services. Windows' location API does not see a receiver
on a COM port, and macOS's needs permission prompts; neither adds a source the four readers lack.

**Parsing, from the NMEA 0183 rules and what the phone apps actually send:**

* A line counts only with a valid checksum (XOR of the characters between `$` and `*`).
* A position counts only from `RMC` with status `A` and `GGA` with fix quality 1 or more. Any talker
  (`GP`, `GN`, `GL`, `GA`, `GB`/`BD`, `GQ`) is accepted; the sentence type decides.
* Satellite count, HDOP and `GSA` are never used as quality: GPS2IP sends a fixed HDOP of 0.9 and
  eight satellites, because iOS does not give them.
* A datagram may hold several sentences; split on CR LF.
* *Find* for a serial port tries 4 800, 9 600, 38 400 and 115 200 baud, keeps the one that gives
  valid checksums, and raises DTR (some Bluetooth receivers send nothing without it).
* GPS2IP Lite stops after four minutes; the reader says "the phone stopped sending" rather than
  silently keeping the last fix.

### 6.3 Freshness

A position is **current** for 10 seconds after its fix, **recent** up to 2 minutes (shown with its
age), and **stale** after that. A stale fix is never used as the present position. The selector
falls back, in order, to the place last saved from this GPS, then to the default place, and says
which on screen ("GPS silent for 6 minutes: using Home"). A fix that jumps more than plausible
speed allows (500 km/h) is held until a second fix confirms it.

### 6.4 Choosing the source

The operator picks a place, a GPS reader, or **automatic**: the GPS when it is current, otherwise
the place most recently saved within 2 km of the last good fix, otherwise the default place. A GPS
reader that stays silent for a minute is reported once, not every second.

**Moving.** Rankings are recomputed when the position has moved far enough to matter, not on every
fix: at HF, when it moves 10 km or crosses into another 6-character square; at VHF, 100 m or 1 % of
the shortest candidate path, whichever is more, no more often than every 10 seconds, and only for the
candidates near the top (§8.9).

**Privacy.** The precise position stays on the computer. The log and shared diagnostics carry the
6-character grid; nothing is sent to a post office or gateway (RP-3 already carries no position).

### 6.5 Station profiles

A profile describes the station, so the models need not assume one:

| Field | Used by | Default when missing |
|---|---|---|
| Radio, power (W) | HF and VHF link budgets | 100 W HF, 25 W VHF |
| Antenna: type, height above ground, gain, azimuth for a beam | Both: VOACAP antenna file (HF); height and gain (VHF) | HF: dipole at 10 m; VHF: 0 dBi at 2 m (a vehicle) or 5 m (a mast), by place type |
| Feed-line loss (dB) | VHF | 1 dB HF, 2 dB VHF |
| Local noise | HF (VOACAP's man-made noise categories) | Residential |
| Modems and bandwidths | Eligibility | Whatever the adapter's `hello` lists |
| Bands the radio and antenna cover | Eligibility | All the modem allows |

None is required. Every value used is shown as *set*, *measured* (ground elevation from the
terrain data; an offset learned, §12.5) or *assumed*, and the explanation names assumptions that
matter ("assumed 2 m antenna: raising it may change this"). Learned offsets (§12.5) absorb the
error of a wrong default over time.

Profiles and places are separate lists linked by the place's optional default profile, so a radio
moved from car to field keeps one profile.

### 6.6 Shared with hf-predict

Places, GPS readers and the station profile are what hf-predict also needs (its assessment lists
GPS by serial NMEA or gpsd as a wanted input; it has none today). They belong in the shared
libraries (§11), with each program keeping its own settings file.

---

## 7. The gateway directory

### 7.1 What there is, and what stays

The directory is the one RadioPost already has: each post office's signed list of the gateways
registered with it (RP-5 §4.4), read from the home post office and every post office it names,
checked with their keys, merged with the home's record winning, kept for use offline and refreshed
every six hours (D36). There is no central directory, and this design adds none. Everything below
is either an **optional member** of the existing record, which a reader that does not know it
ignores (RP-0 §4.2), or **local** to the client.

### 7.2 What the record lacks, and optional members to add

| Needed for | Missing today | Proposed optional member | Who fills it |
|---|---|---|---|
| VHF terrain (§8) | A position better than a 6-character square; antenna height | `site`: `{"lat", "lon", "agl_m"}`, at the precision the operator chooses (2 decimal places is about 1 km, 4 about 11 m) | Gateway operator, at registration (RP-5 §6.4) |
| Both link budgets | Power, antenna, gain | In each channel, `station`: `{"power_w", "antenna", "agl_m", "gain_dbi", "azimuth_deg"}`, all optional; `antenna` one of a short registered list (`vertical`, `dipole`, `yagi`, `j-pole`, `collinear`, `loop`, `nvis`, `other`) | Gateway operator |
| Eligibility by time (§14.2.1) | `hours` is text for people | `listens`: windows in RP-7 §5's form (`every_s`, `at_s`, `length_s`), any number; `hours` stays for people | Gateway operator |
| Availability (§12.4) | Nothing says whether the gateway is running | `seen`: when the gateway last reached its post office (a login or a poll for copies, RP-5 §3, §5.8), to the minute | **The post office**, when it serves the list |

Winlink's directory already carries power, antenna height, gain and direction for each channel, and
a latitude and longitude for each gateway (§4.2): gateway operators there are used to giving them.

These are MINOR additions to a Stable specification (RP-0 §8.2), announced as §8.4 asks, with
vectors. They change nothing for a reader that ignores them, nothing for a gateway that leaves them
out, and nothing on the air. `seen` adds a small privacy cost (when a gateway's computer is on);
`site` is the operator's choice of precision. The author chose to propose them in phases: `seen`
and `listens` with Phase 1, `site` and `station` with Phase 2 (§21, Q3).

**Not proposed:** a single "range" figure, as Winlink's records have. It folds power, antenna,
terrain and the remote station into one number that is wrong for most callers; the terrain
analysis computes what it would guess. And no gateway-published measurements of its callers: the
wishlist (W17) set them aside until a prediction proves too poor, and they carry callers' positions.

### 7.3 What the client adds locally

* **Gateways of the operator's own**: a station not in any list (a partner for station-to-station
  mail, a test gateway, a club's station before it registers), with callsign, channels and, if
  known, site and station. Marked "yours" everywhere.
* **Corrections**: the operator's own values for a listed gateway (its real antenna height, a
  channel it has stopped using), kept beside the record and marked as the operator's, never
  replacing it silently.
* **Favourites and blocks**, by gateway or by channel.
* **Ages shown, nothing hidden.** A record shows how old it is (`updated`), how old the list is
  (fetched), and, with `seen`, when the gateway last reached its post office. A gateway whose list
  could not be fetched for a week, or whose `seen` is days old, is dimmed with the reason, never
  dropped.

### 7.4 Two kinds of "available", kept apart

* **Reachable by radio** (Q1): the path. Learned from attempts and signals heard (§12).
* **In service** (Q5): the gateway program running and reaching its post office. Evidence:
  `seen` (§7.2) from the post office; `internet` in the record; and the outcome of sessions that
  linked (§12.3: refused, service failed). A gateway cut off from the Internet still serves by
  radio (RP-7), so an old `seen` with `internet` true lowers the availability prior; it does not
  remove the gateway.

The availability prior (§12.4) is 0.9 when `seen` is within the hour, 0.7 within a day, 0.5 when
unknown, and 0.3 when the post office has not heard from it for a week, each worth two attempts.
These are starting values, to be fitted to outcomes in Phase 4.

---

## 8. VHF and UHF: terrain-aware paths

At VHF and UHF the ionosphere drops out and the ground decides. A gateway 15 km away behind a ridge
can be unreachable while one at 60 km on a hilltop is solid. This section says how the client
judges a path from terrain; §9 says where the terrain comes from.

### 8.1 Distance first, always

Without terrain data, or before it has arrived, VHF candidates are ranked by distance with bearing
shown, adjusted by history (§12). The **radio horizon** sets what is plausible: with the standard
4/3 effective Earth radius it is about 4.12 (√h₁ + √h₂) km for antenna heights in metres. A car at
2 m and a gateway mast at 30 m: 4.12 × (1.4 + 5.5) ≈ 28 km. Diffraction carries well past that at
VHF; candidates beyond three times the horizon, or 150 km, are listed but not analysed until the
operator asks.

### 8.2 The profile

For each candidate the client samples ground height along the great circle from the station to the
gateway's site, at the elevation data's own spacing (30 m for Copernicus GLO-30), interpolating
bilinearly. A 50 km path is about 1 700 points: a fraction of a millisecond once the data is in
memory.

Each end's height is ground plus antenna height above ground: the station profile's (§6.5) and the
record's `site.agl_m` (§7.2), else defaults, marked as assumed. The gateway's ground height comes
from the terrain at its site, not from the record, so the two ends are on one datum.

### 8.3 Geometry

With *d* the distance from the station, *D* the path length, *z*(d) the ground height and *a*ₑ =
*k* · 6 371 km (*k* = 4/3, ITU-R P.530's standard-atmosphere median; ITU-R P.310):

```
earth bulge     b(d)  = d (D − d) / (2 aₑ)
direct ray      L(d)  = H₁ + (H₂ − H₁) · d / D           H = ground + antenna, at each end
clearance       c(d)  = L(d) − (z(d) + b(d))
first Fresnel   F₁(d) = 17.3 · √( d (D − d) / (f · D) )  metres; d, D in km, f in GHz
```

At 145 MHz on a 30 km path the first Fresnel zone's radius at mid-path is 17.3 × √(225 / (0.145 ×
30)) ≈ 125 m; at 440 MHz, about 71 m. These are large against VHF terrain, which matters below.

### 8.4 What the path is called

| Class | Rule | Said as |
|---|---|---|
| Clear | c(d) ≥ 0.6 F₁(d) everywhere | "Clear terrain profile" |
| Partly obstructed | c(d) ≥ 0 everywhere, but under 0.6 F₁ somewhere | "Fresnel zone partly obstructed at 8 km" |
| Blocked | c(d) < 0 somewhere: the terrain cuts the direct ray | "Blocked by a ridge 12 km out" |
| Not enough data | Elevation data missing for part of the path, or a site known only to a grid square | "Terrain not checked: …" |

The class explains; it does not decide. **At VHF the 60 % rule is not a usability test.** ITU-R
P.530 states it as the clearance for *free-space* conditions and frames its procedure for links
above about 2 GHz, noting that smaller fractions may be needed below that to avoid unreasonable
antenna heights. With Fresnel zones of 70 to 125 m, most VHF paths of tens of kilometres are "partly
obstructed" and work perfectly. What decides is the loss, against the link budget (§8.6): the
classes are shown beside it, never instead of it.

### 8.5 Diffraction loss

| Model | What it does | Cost | Verdict |
|---|---|---|---|
| Single knife edge (ITU-R P.526 §4.1) | Loss of the one highest obstacle, from v = h √(2/λ · (1/d₁ + 1/d₂)): J(v) = 6.9 + 20 log₁₀(√((v − 0.1)² + 1) + v − 0.1) dB for v > −0.78 | Trivial | Good for explaining; wrong for several ridges |
| Two isolated edges, the predominant edge first (P.526 §4.3) | The classic Deygout-style construction | Small | Better on two ridges; no smooth-earth term |
| **General terrestrial path (P.526-16 §4.5)** | A Bullington construction on the actual profile and on a smooth fitted profile, combined with spherical-Earth diffraction (P.526 §3.2); "for a completely smooth path, the result will be the same as the spherical Earth model". Commonly called delta-Bullington | O(n) on the profile | **Recommended first model**: one standard, deterministic, no tuning, smooth across line-of-sight and over the horizon, and it reduces to one equivalent obstacle that can be drawn and explained |
| ITS Irregular Terrain Model, point to point (NTIA's C++ v1.3, public domain in the US and permissively licensed elsewhere) | Reference attenuation from the profile with horizons, effective heights, climate and refractivity; time, location and situation variability | Small; a C++ dependency or a port | **Second model**, for comparison and for its variability figures; adopted as primary only if outcomes show it ranks better (§19.5) |
| ITU-R P.1812, P.452 | Path-specific prediction with clutter and ducting | Needs ITU's digital maps, which may not be redistributed without ITU's written permission | Not used |

No reference implementation of P.526 §4.5 was found; it is written from the text, tested against
hand-worked profiles and against ITM on a set of paths (§18, Phase 2).

### 8.6 The link budget

```
received power  P_rx = P_tx + G_tx − L_tx − (FSPL + L_diff + L_clutter) + G_rx − L_rx      dBm
free space      FSPL = 32.45 + 20 log₁₀ f_MHz + 20 log₁₀ d_km                              dB
margin          M    = P_rx − threshold(modem, rate)
```

Each direction has its own budget: the user's power and the gateway's antenna one way, the gateway's
power and the user's antenna the other.

An example shows why the budget, not the clearance class, decides. 2 m, 40 km, a 50 W mobile at 0
dBi net, the gateway's antenna 6 dBi with 2 dB of cable: FSPL is 32.45 + 43.2 + 32.0 ≈ 107.7 dB.
Through a ridge costing 20 dB the gateway receives 47 − 127.7 + 6 − 2 ≈ −77 dBm. A good FM receiver's
12 dB SINAD point is around −122 dBm (an IC-9700 is specified under 0.18 µV); even with tens of
decibels allowed for a data modem and for location variability, that path works. Behind a mountain
costing 50 dB more, it does not. "Blocked" and "fine" are both true of the first path.

**Modem thresholds at VHF are not published as measurements.** No measured SNR or SINAD threshold
was found for 1 200 baud AFSK or 9 600 baud G3RUH packet; G3RUH himself declines to give one in
microvolts. The client therefore starts each FM modem's threshold as a labelled assumption with a
wide spread (a prior a few decibels above the radio's 12 dB SINAD point for 1 200 baud, several
more for 9 600), learns the offset from outcomes (§12.5), and asks for a bench measurement: Aether
FM's own plan already calls for a signal generator, a dummy load and an attenuator, and the same
setup measures packet's thresholds through the same radios.

**Location variability.** Terrain models predict the median over a small area; a vehicle sees
fades of several to ten decibels from buildings and multipath over metres. The margin is read
against a spread (10 dB to start, §12.5): P(viable) = Φ(M / σ), shown in words.

### 8.7 Vegetation, buildings and other obstructions

The elevation data is either the bare earth or the surface as a satellite saw it, trees and
buildings included (§9). Neither is right for VHF: a forest is not a solid wall at 145 MHz, and
bare earth ignores the building beside a mobile. The plan:

1. **Phase 2:** one consistent surface model (Copernicus GLO-30) everywhere, labelled "surface
   model: trees and buildings as the radar saw them in 2010–2015". Its systematic optimism or pessimism for a
   place is what the place term in §12.5 learns.
2. **The operator's own obstruction**: "a building 15 m high, 30 m to the north-east", added to the
   profile at their end, where it matters most and where no dataset is good enough.
3. **Later, where data exists (§9.2):** bare earth with canopy height and a foliage loss model, or
   building heights, compared against the surface model on recorded outcomes before either
   replaces it.

The panel always says which is used, and never claims to know a building it has no data for.

### 8.8 The antenna-height question

The profile is cached; only the clearance and loss depend on the antenna heights. Changing a height
recomputes in microseconds, so the profile view (§16.3) has a slider: "raising your antenna from 2
to 10 m clears the ridge at 7 km: about 18 dB better". This is the most useful thing a field
operator can learn from the terrain, and it costs nothing once the profile is there.

### 8.9 Doing it for many gateways, and on the move

1. **Distance and horizon** for every VHF candidate (microseconds each).
2. **Terrain from the cache**; tiles fetched for the area of the plausible candidates, in the
   background, and kept (§9.3).
3. **Profiles and P.526 loss** for the candidates within reach: about 1 ms each.
4. **ITM** (when built) only for the top ten, or on request.
5. **Cache** each profile by both ends' positions rounded to the data's spacing, the data source's
   version, and the gateway record's `updated`.

Recomputed when the station has moved 100 m or 1 % of the shortest candidate path (whichever is
more), no more than every 10 seconds; when antenna heights change (geometry only); when a gateway's
record changes; or when better data arrives. A vehicle driving through a town re-ranks every few
seconds at negligible cost once the tiles are local.

### 8.10 Validating it

Terrain modelling earns its place only if it ranks VHF gateways better than distance does. The
test, on recorded attempts, is in §19.5: connection rate of the top choice, false "unreachable"
(predicted blocked, connected) and false "viable" (predicted fine, failed), with complete and with
missing antenna information, against distance alone. If it does not beat distance, it is shown as
information and not used in the score.

---

## 9. Elevation and obstruction data

Checked on 2026-11-20 (sizes and listings fetched where marked [T]; licences read on the primary
source [P]; [S] secondhand).

### 9.1 Elevation

| Dataset | Grid, coverage | Surface or bare earth | Licence | Fetch with no account | One 1° tile (N33W100) |
|---|---|---|---|---|---|
| **Copernicus GLO-30** | 1″ (~30 m), the world's land | Surface (radar, 2010–2015), EGM2008; < 4 m absolute (LE90) [P] | Free, worldwide, adaptation and redistribution allowed, with Copernicus's notice and its liability sentence in the redistributor's terms [P] | Yes: AWS Open Data `copernicus-dem-30m`, Cloud-Optimized GeoTIFF [P][T] | 39.6 MB [T] |
| **Copernicus GLO-90** | 3″ (~90 m), the world's land | As GLO-30 | As GLO-30 | Yes: `copernicus-dem-90m` [P][T] | 4.7 MB [T] |
| SRTM GL1 | 1″, 60° N to 56° S | Radar surface | US Government, open | Yes, through OpenTopography's public bucket [T]; NASA's own needs a login | 8.7 MB [T] |
| NASADEM | 1″, 60° N to 56° S | Reprocessed SRTM | Any purpose [P] | No: NASA Earthdata login [T] | 6.7 MB |
| USGS 3DEP 1/3″ | ~10 m, the United States | **Bare earth**, NAVD88 [T] | Public domain [P] | Yes: `prd-tnm` [T]; Cloud-Optimized, 512-pixel blocks [T] | **407 MB** [T] |
| USGS 3DEP 1 m | Lidar areas of the US | Bare earth | Public domain | Yes, through the TNM API [T] | ~400 MB a 1 m tile |
| ALOS AW3D30 | 1″, the world but Japan and Antarctica | Surface | Free with JAXA's terms [S] | No: registration | — |
| FABDEM | 1″, the world | Copernicus with forests and buildings removed | **Non-commercial** (CC BY-NC-SA 4.0) [P] | — | — |
| Terrain Tiles (Mapzen, AWS) | Mixed sources by area | Mixed | Each source's attribution [P] | Yes: `elevation-tiles-prod` [P][T] | 4.2 MB (Skadi `.hgt.gz`) [T] |
| OpenTopography API | — | — | — | Key needed; 50 calls a day for non-academic users [P] | — |

**Recommendation**, which the author took (§21, Q5):

* **Copernicus GLO-30 as the one source, everywhere**, with **GLO-90 for the first pass** and where
  a GLO-30 tile is missing (the AWS copy has none over Armenia and Azerbaijan [T]). It is the only
  global set that needs no account, allows redistribution, and is one consistent model with a
  stated accuracy. The notice and the liability sentence go in the client's About and licence
  files, and the profile view names the source.
* **Not FABDEM**: its licence forbids commercial use, and RadioPost's code is MIT OR Apache-2.0,
  which allows it. **Not NASADEM, AW3D30 or OpenTopography**: each needs an account or a key.
* **3DEP later, in the US, as an option** to test (§8.7): bare earth at 10 m is better for knowing
  where the ground is, but its tiles are 407 MB and it has no trees or buildings, so it needs a
  clutter layer (§9.2) and range reads of its internal blocks rather than whole tiles.
* **Terrain Tiles' Terrarium PNGs for drawing** (§16.2), not for analysis: light, but their sources
  and datums vary by area.

### 9.2 Obstructions beyond terrain

| Dataset | What | Licence | Use |
|---|---|---|---|
| Meta/WRI canopy height | 1 m, worldwide; MAE ~2.8 m by Meta's figure [S], unverified for tall trees [P] | CC BY 4.0 [P] | Later, with bare earth (3DEP), for a foliage-loss model |
| ETH canopy height 2020 | 10 m, worldwide (Lang et al. 2023) | CC BY 4.0 [P] | As above, lighter |
| Microsoft building footprints | 1.4 billion; heights only where measured (174 million in 2023) [P] | CDLA Permissive 2.0 [P] | Buildings near the station, where heights exist |
| Google Open Buildings 2.5D | Height above ground, ~4 m effective, Africa, Asia, Latin America; MAE 1.5 m [P] | CC BY 4.0 or ODbL [P] | As above, where covered |
| Overture buildings | Merged sources with `height`; no coverage share published | ODbL [P] | Fetched at run time only, if at all |
| OpenStreetMap | 3.3 % of buildings have `height`, 5.7 % `building:levels` [T] | ODbL | Too sparse to rely on |

None is needed for Phase 2. Each is an experiment for Phase 6, judged on recorded outcomes (§19.3),
because each adds data to download and a model of loss through trees or past buildings that itself
needs validating. The operator's own obstruction (§8.7) comes first.

### 9.3 Fetching and keeping it

* **The tile is the unit**: one 1° tile, fetched over HTTPS from the public bucket, converted once
  to a compact local form (16-bit heights in decimetres, compressed in blocks), and kept in the
  operating system's cache directory under the source's name and version. A 1° GLO-30 tile becomes
  roughly 10 to 25 MB locally (to be measured); a 150 km radius around a station needs four to nine.
* **Fetched on first need**, in the background, with progress, and never on a metered connection
  without asking. An area can be fetched ahead ("prepare this region for Field Day") for use with no
  Internet.
* **Cloud-Optimized GeoTIFF range reads** fetch only the blocks a set of profiles crosses. That
  cuts the first download for a few paths several times over; worth building once the whole-tile
  path works and its sizes are measured.
* **Offline**: whatever is in the cache is used; a path crossing a missing tile is "not enough
  data", ranked by distance and history, and says which area to fetch.
* **Attribution** travels with every view that uses the data.

---

## 10. HF prediction

### 10.1 The engine, and what it gives

VOACAP, as hf-predict runs it (§3): `voacapl`, real VOACAP, local, offline, on all three systems.
For a circuit, an hour and a frequency it gives the median SNR in 1 Hz, its lower and upper decile
spreads across the days of the month, `MUFday` (the share of days the path supports the frequency
at all), the propagation mode, take-off angle and the probability of multipath. Those are exactly
the inputs this design needs; nothing else is required of the engine.

ITURHFProp (ITU-R P.533-14) is a second engine to keep in view: its repository says the code is
"free from any copyright assertions" [P], a clearer position than hf-predict's assessment recorded.
It is optional and later (§11).

Real-time ionospheric maps are not used. KC2G's maps interpolate ionosonde data and publish no API
terms [P]; GIRO's ionosonde data is for non-commercial research by account holders only [P].
Neither can be built into a program others may use commercially. Today's conditions enter through
measurements instead (§10.5).

### 10.2 Both ways, for each channel, cheaply

* **Both directions.** A gateway path is two circuits: the user's power, antenna and noise one way,
  the gateway's the other. VOACAP's loss is reciprocal, but the noise at each receiver differs with
  place and time, so each direction is its own run.
* **Batched.** A VOACAP deck runs one circuit to many receivers with only the `CIRCUIT` card
  changing (hf-predict's `write_deck_for_receivers`), and a run takes up to eleven frequencies. With
  default gateway stations, the user-to-gateways direction is one deck per hour; the reverse, one
  more.
* **Staged.** First, the coverage grid at the hour for each band the candidates use (hf-predict's
  coverage: 1.1 s a band at 10° × 15°, measured, 6.6 to 7.2 s at 4° on four cores), which puts every
  gateway in a cell. Then point-to-point runs, both ways, for the candidates that pass. hf-predict's
  assessment infers, unverified, about 90 ms a circuit per core for a 24-hour deck; a hundred gateways both
  ways is of the order of ten seconds on four cores, once a month per place, from then on read from
  the cache.
* **Cached** by place (6-character square), month, gateway, station profiles, sunspot number and
  engine version. Moving more than 10 km or to another square re-runs it (§6.4).

### 10.3 From SNR to what a modem does: the modem profile

A propagation prediction answers "how strong", and the operator needs "how fast, and will it
connect". The bridge is a **modem profile**: for each rung of the modem's ladder, the SNR it needs
on each channel class and the rate it carries; and the thresholds of its call and answer.

For Aether the profile is its published, measured ladder (its air-interface specification §4,
§4.1 and §10, all in a 3 kHz noise bandwidth). Its call and acceptance go on the tone floor, whose
control frame needs about −19.5 dB on AWGN and −11 to −16 dB on the ITU fading channels; its
fastest rungs need +17 dB on AWGN at 2 300 Hz.

| Requirement | dB in 3 kHz | dB-Hz (add 34.8) |
|---|---|---|
| FT8 decode (−21 dB in 2 500 Hz) | −21.8 | 13 |
| Aether's tone-floor call, AWGN | −19.5 | 15.3 |
| Aether's tone-floor call, ITU Moderate | −14.5 | 20.3 |
| Aether's first OFDM rung at 2 300 Hz (197 bit/s), AWGN | −5.1 | 29.7 |
| Pat's required SNR | | 20 |
| SSB voice (VOACAP's usual figure) | | 38–45 |
| Winlink Express's required SNR | | 40 |

So a ranking by "path reliability at SNR 40", as Winlink Express shows it, calls a path closed
where Aether still connects 20 to 25 dB lower, and treats a path that connects but crawls the same
as one that carries 4 kbit/s. Ranking by the modem's own ladder separates the two.

From VOACAP's median μ and decile spreads (lower Δ_L, upper Δ_U), each day's SNR is taken as a split
normal (σ_L = Δ_L / 1.28 below the median, σ_U = Δ_U / 1.28 above), shifted by the learned offset δ
(§12.5). Then, for each direction:

```
P(connect)  = P(SNR_up ≥ θ_call) · P(SNR_down ≥ θ_answer) · MUFday           (with availability, §12)
g(SNR)      = net rate of the fastest rung whose threshold + margin ≤ SNR, on the path's class
rate        = η · E[ g(SNR) ]                                                (η learned, §12.6)
```

**The channel class** decides which thresholds apply; Aether's differ by 4 to 11 dB between AWGN
and the ITU fading channels. VOACAP does not give a fading class, so a first mapping, to be checked against
outcomes:

| Path, from VOACAP's mode and geometry | Class |
|---|---|
| Near-vertical (under 500 km, take-off angle above 60°) | Poor, and the NVIS echo limits Aether measured (its ADR-0087) |
| One F hop, mid-latitude, multipath probability low | Moderate |
| One hop with high multipath probability, two or more hops, high latitude | Poor |
| Ground wave (a few tens of km) | Good |

**Other modems.** Mercury and ARDOP are open; their published modes can make profiles once someone
measures them, and until then they get a generic profile (a floor and a rate ceiling) whose
efficiency is learned. VARA is a black box by the project's rule: only its published commands are
used, so its profile is generic and everything about it is learned from this station's own
sessions. The ranking says so ("VARA: no modem model; history only").

### 10.4 What VOACAP is, and is not

VOACAP predicts a month's climate at an hour: the share of days a path works, not whether it works
today. hf-predict's own calibration against nine months of FT8 reception shows it **orders paths
correctly** (on 20 m the share of listening hours a locator was heard rises from 9 % to 39 % across
the predicted bins), while the absolute level depends on who was transmitting. Published comparisons
put its SNR error at 3 to 9 dB RMS on studied paths (McNamara, cited in Walden 2012, Radio Science
doi:10.1029/2011RS004914). No published work was found that calibrates VOACAP against WSPR or FT8
statistically. Hence the design: VOACAP is the **prior and the clock** (it carries the hour, the
season and the cycle), the station's own measurements correct its level (§12.5), and today's
conditions come from observations (§10.5).

**Solar input** is the bundled smoothed sunspot table, as in hf-predict. A geomagnetic storm (Kp 5
or more) does not move the predictions, for no validated model says by how much; it widens the
day-to-day spread for high-latitude and long paths and puts a warning on them.

### 10.5 Today's evidence

| Evidence | Says | Weight |
|---|---|---|
| The gateway itself heard (RPMI `heard`, its identification or another session) | The downlink's SNR now, measured | Strong: a direct measurement of δ_down (§12.5) |
| An operator's probe (§12.2) | Both directions' SNR now | Strong: both offsets |
| FT8 from stations near the gateway, heard here in the last hour (hf-predict's observations) | The band is open in that direction now, for some power | Moderate: raises P(open) for that band; not the SNR (their power is unknown) |
| PSK Reporter: who heard this station's FT8 (optional, online, at most one query every five minutes as its author asks) | The reverse direction is open | Moderate |
| NOAA's indices (flux, Kp) | Disturbed conditions | Warnings and wider spreads only |

Hearing FT8 from near the gateway is evidence of propagation, not of a data link: FT8 decodes at 13
dB-Hz, Aether's floor needs 15 to 20, and its useful rates far more. So it moves the probability that
the band is open toward the gateway, never the predicted rate. The size of that move starts as a
fixed, labelled ratio (2 : 1) and is replaced, in Phase 4, by what this station's records show
(how often an FT8 "open" preceded a RadioPost link).

hf-predict holds these observations; RadioPost reads them only through an interface (§11.4), and
works the same without them.

### 10.6 Another time

The same predictions answer "which HF gateway tonight at 20:00": VOACAP at that hour and month,
learned offsets as they stand (not tied to the hour), availability for that block of the day, and
the panel's note of what is assumed: monthly climatology, no storm forecast, the position as saved.
NOAA's three-day forecast, when held, adds its storm warning to the evening's rows.

---

## 11. hf-predict: what it should become

### 11.1 The choices

| | Architecture | For | Against |
|---|---|---|---|
| 1 | hf-predict stays an app; a shared propagation library beside it | Each app embeds what it needs; works offline; independent releases | The library needs its own care (versions, a changelog) |
| 2 | hf-predict becomes a local prediction service with a documented API | One engine install; other programs (RadioPostKit, loggers) could use it | RadioPost's HF ranking would need a second program installed and running, in the field, at the moment of choosing; release coupling through an API |
| 3 | One prediction engine embedded in both | As 1 | Is 1, narrower: says nothing of observations or terrain |
| 4 | A modular framework: HF, VHF terrain and reliability statistics | Everything shared | The statistics are about RadioPost sessions (gateways, channels, transactions); hf-predict has none. Sharing them couples the two apps' domains for no reuse |
| 5 | Everything in RadioPost's repository | One place | hf-predict loses its independence, and RadioPost's protocol repository fills with science code |

### 11.2 Recommendation: shared Rust libraries, embedded; one narrow local API

**hf-predict's repository becomes a Cargo workspace of libraries plus the app** (the author's
choice, §21, Q2). Its domain code
moves out of the Tauri crate, unchanged at first, into crates with no Tauri dependency. RadioPost
depends on them by git tag. Each app embeds them and works alone.

| Crate (names provisional) | Holds | From hf-predict's | Used by |
|---|---|---|---|
| `radioprop-geo` | Positions, Maidenhead (4 to 10 characters), great circle, bearing | `geo.rs`, and RadioPost's `grid.rs` (one copy) | Both |
| `radioprop-place` | Saved places, station profiles, GPS readers (serial, TCP, UDP, gpsd), the NMEA parser, freshness | `userdata.rs`, `station.rs`; new | Both |
| `radioprop-hf` | The engine runner, decks, output parser, `PropagationEngine`, the sunspot table, coverage grids, the predictor; **both-way runs, per-gateway stations, any required SNR**; modem profiles (§10.3) | `engine.rs`, `voacap/`, `propagation.rs`, `predictor.rs`, `coverage.rs`, `solar.rs` | Both |
| `radioprop-terrain` | Elevation tiles (fetch, convert, cache), profiles, clearance, Fresnel, P.526 §4.5, ITM later, link budgets | New | RadioPost; hf-predict could add a VHF path view |
| `radioprop-observe` | Observation records and their origin, the WSJT-X UDP parser, `ALL.TXT` reader, the observation store's read interface, the predicted-and-observed tiers | `observations.rs`, `wsjtx/`, `compare.rs` | hf-predict; RadioPost reads evidence through §11.4 |

**What stays in each app.** hf-predict: its views, the scanner and radio control, space-weather
fetching, its SQLite store. RadioPost: the gateway directory, attempt records, failure
classification, learning, the transaction model, the ranking and its panel. The statistics of §12
are RadioPost's alone.

**The engine binary** keeps its single build (hf-predict's `engines/voacapl/build.sh`, already
building on all three systems in CI). hf-predict's release attaches the engine for each platform as
an artifact with its checksum; RadioPost's release fetches the version it pins, checks it, and
bundles it (about 9 MB installed; its data compresses to 1.4 MB). Bundled rather than downloaded
on first use, because HF ranking must work at a Field Day site with no Internet (the author's choice, §21,
Q7).

**The licence.** The crates are relicensed MIT OR Apache-2.0, matching RadioPost; hf-predict
the app may stay Apache-2.0. The author, its only committer, chose this (§21, Q1), so RadioPost's
binaries keep one licence choice throughout.

### 11.3 How to get there without breaking hf-predict

1. **Workspace, no behaviour change.** Move the modules into crates; the Tauri crate depends on
   them; every existing test, the engine reference cases and the release build pass unchanged.
2. **The station model generalised.** A required SNR in dB-Hz from any source (a human mode, or a
   modem rung) replaces the three-value `Mode` internally; the app's choices stay as they are.
3. **Both ways and many stations.** A circuit names a station at each end; batched decks keep
   their speed.
4. **Places and GPS.** The new crate serves hf-predict first (it lists GPS as a wanted input), then
   RadioPost.
5. **Tagged releases of the crates**, with a changelog, independent of the app's versions.

Step 1 is a few days' work and is checked entirely by the existing tests. Nothing in RadioPost
waits on more than steps 1 to 3 for HF (§18, Phase 3).

### 11.4 The one local API: today's observations

When hf-predict is running, it holds the station's recent FT8 observations. RadioPost asks for a
summary through a small read-only API on loopback (hf-predict's own, documented in its repository):

```
GET http://127.0.0.1:<port>/v1/evidence?toward=<lat>,<lon>&since=<unix>&bands=40m,30m
→ {"bands": [{"band": "40m", "tier": "moderate", "stations": 7, "periods_listened": 40,
             "nearest_km": 120, "median_snr_db": -12, "provider": "wsjtx-udp"}], "as_of": …}
```

RadioPost works identically without it; the row's explanation simply has no observation line. An
API rather than reading hf-predict's database file, because the file's schema is hf-predict's to
change, and an API keeps that freedom.

### 11.5 Changes that RadioPost needs, and changes worth making anyway

| Change | For RadioPost | For hf-predict itself |
|---|---|---|
| Crates without Tauri | Needed | Cleaner tests, faster builds |
| Required SNR from a modem profile | Needed | Lets it predict for digital modes (JS8, VARA, Aether) |
| Both directions, a station each end | Needed | A real "can they hear me" view |
| Places with GPS | Needed | Portable operation (in its own brief) |
| Learned offsets per path (§12.5's method) | — (RadioPost learns its own) | Turns its descriptive calibration into a prediction correction for its FT8 paths |
| Probability scores (Brier, log loss, reliability diagram) | Used in §19 | Says how good its predictions are, not only whether they order paths |
| ITURHFProp as a second engine | Optional | Disagreements between engines are information |
| The evidence API (§11.4) | Used | Other programs can use it too |
| PSK Reporter (already its next step) | Optional input (§10.5) | Its planned "heard you" column |

---

## 12. Telemetry, failure classification and learning

The client learns from every attempt it makes, on the computer, with no service anywhere. This
section says what it records (§12.1–12.2), how it decides why an attempt failed (§12.3), what it
learns and how (§12.4–12.8), and how the operator sees and resets it (§12.9).

### 12.1 The attempt record

Every call is recorded, whatever its end, as one line of JSON appended to a file (§15.2). The line
is written in two steps: a stub when the call starts, completed when it ends, so a crash leaves a
record that says "ended unknown" rather than nothing. Raw records are never rewritten; everything
learned is computed from them and can be computed again (§12.9).

| Layer | Fields | Source |
|---|---|---|
| Context | UTC start; the callsign called and called as; candidate (dial, mode, modem name and version, bandwidth); position, its source and its age; station profile ID; the prediction made for it at the time (§14), with the model's name and version | Client |
| Modem | `busy` at the call; call duration; `failed` code and text, or time to `connected`; `call_s`; whether the called station was heard during the call, and at what SNR (§12.2); `disconnected` reason; every `quality` reading (time, `snr_db`, `rate_bps`, `receiving`); stalls (`receiving` false with nothing arriving for 30 s or more) | RPMI (RP-4 §5) |
| Transport | Bytes handed over, `delivered` times and bytes, `queued`; bytes received; link time; time spent receiving and sending | RPMI |
| Protocol | `WELCOME`'s `online`; the `NOTICE` and `ERROR` codes and which side sent them; whether the credential was sent whole; resume offsets used (RP-3 §7) | Session engine |
| Application | The transaction as planned at the call (bytes out and in, by message); what crossed; what was left; whether the plan completed | Client |

Positions are kept at full precision in this file, because VHF learning needs them (§12.5). The
log and anything shared (diagnostic zips, a contributed data set) carry the 6-character grid only.
An option keeps only the 6-character grid in the attempt file too, at the cost of VHF learning by
place.

### 12.2 What the modem interface should add

RPMI already carries most of what is needed. Three optional, MINOR additions to RP-4 (RP-0 §8.2,
announced as §8.4 asks; an adapter or host that ignores them changes nothing) would close the gaps.
Aether already knows each of them.

| Addition | Why | Aether's source |
|---|---|---|
| `quality.peer_snr_db` | The other direction. A gateway path is two paths; RPMI reports only the one this station hears. | `metrics.peer_snr_db` (from the other station's acknowledgements) |
| `failed.heard`, `failed.snr_db` | Whether the called station's signal was heard during a call that failed. "Nothing heard" and "heard, but no link" point to different causes (§12.3). | Its arrivals and frame reports during the call |
| `probe` request, `probed` event, `caps.probe` | An operator's measurement of both directions in two short frames each way (about 11 s), without a session. | `probe {remote}` and its result (Aether's ADR-0006) |

None is needed to start (§18): Phases 1 to 4 work with RP-4 as it is, and these make the learning
sharper when present. The author chose to propose them in phases: the first two with Phase 4, the
probe with Phase 6 (§21, Q4).

### 12.3 Failure classification

A failed attempt teaches something only when its cause is known well enough to blame the right
thing. Blaming the gateway's path for a dead microphone cable, or for the operator's own cancel,
would teach the opposite of the truth. So each attempt gets a class from the evidence the client
holds, and each class updates only the quantities it bears on.

| Class | Evidence | Updates | Confidence |
|---|---|---|---|
| Cancelled | The operator aborted (`failed` `aborted`, or the panel's stop) | Nothing | Certain |
| Local station | Adapter unreachable, `no-modem`, a modem `error`, the modem's rules check refused the dial, the modem warned it may not be transmitting | Station health only | High |
| Channel busy | `failed` `busy`, or `busy` true when the call was to start | The channel's occupancy only | High |
| No answer, nothing heard | `failed` `timeout`, and nothing heard of the called station during the call | Shared between availability and both path directions, by their current odds (below) | Low: ambiguous by nature |
| No answer, station heard | `failed` `timeout`, but the called station was heard (`heard`, or `failed.heard`) | Availability up (it is there); downlink up; uplink down | Medium |
| Refused | `failed` `refused`, or `ERROR` `refused-by-policy`, `not-authorized`, a credential code | Service (policy) only; the path counts as working | High |
| Service failed | `ERROR` `internal`, `busy`, `post-office-unreachable` with no `NOTICE` that the gateway works offline | Service only; the path counts as working | High |
| Link lost | `disconnected` `timeout` or `failure` before the session's end | Stability; and the path, if the SNR fell toward the floor first | Medium |
| Protocol error | `malformed`, or an exchange out of RP-3's order | Nothing about the path; noted against the gateway's software | High |
| Completed | Both `DONE`s, the plan done | Everything: a measured success | Certain |
| Completed in part | A normal end with work left by choice (the fetch rule) | As completed | Certain |
| Unknown | The record ended without evidence (a crash) | Nothing | — |

**Sharing an ambiguous failure.** "No answer, nothing heard" is explained by the gateway being
away, or by either path direction being too weak. The record is shared between those causes in
proportion to how likely each is just now, given everything else known (the gateway's recent
availability, the predicted SNR each way). This is the ordinary Bayesian update with a hidden cause
(§12.5 does it exactly on a grid). A gateway that answered every call this week loses little
availability from one silent call; a path predicted near the floor takes most of the blame.

**Station health.** Some failures are the station's. When three or more recent attempts, to at
least two gateways or two bands, all failed with nothing heard, and each was predicted to connect
with probability 0.7 or more, the client says so ("Your last four calls failed on three gateways
that usually answer. Check the radio, antenna and audio before blaming the gateways.") and gives
those failures less weight in each gateway's learning. The weight is one less the probability that
the station was at fault, from a prior of 0.05 and the likelihood ratio of each such failure:
almost certain to fail with a fault (0.95), and failing with probability 1 − p̂ without one. The
evidence stays recorded; only its weight in the gateways' estimates changes.

**What the client will not do** is invent a cause. An attempt whose evidence fits several classes is
recorded with the evidence and the most specific class it supports, and the explanation says
"unknown" when that is the truth.

### 12.4 What is learned

| Quantity | For each | Form | Prior | Forgetting (half-life) |
|---|---|---|---|---|
| Availability *A* | gateway; by three-hour block of the day, pooled toward the gateway | Beta | From the post office's last contact with the gateway (§7.4); 0.5 without | 14 days |
| SNR offset δ, each way | gateway and band (HF), gateway and channel (VHF), with a place term at VHF | Grid posterior, in dB | The physics model's error for the band or class, centred on the offset learned for all gateways together | 60 days HF, 365 days VHF |
| Service *S* | gateway | Beta | 0.95, worth 2 attempts | 7 days |
| Modem efficiency η | modem and bandwidth; pooled toward it for each gateway | Log-normal ratio, measured goodput over the modem model's at the measured SNR | 0.6 for Aether, 0.5 otherwise: starting values, wide (§13.6) | 90 days |
| Link loss rate λ | gateway and band | Gamma–Poisson: losses per second of link | From all sessions, as a function of the SNR margin | 60 days |
| Occupancy *O* | dial, by hour of the day | Beta | 0.1 | 30 days |
| Station health *H* | the station | As §12.3 | 0.05 | Hours: the last day only |
| Overheads | modem, bandwidth, scanning or not | Medians of time to connect, a failed call's length, handshake and ending | From the first sessions; defaults from RP-3 §11's sizes and the modem's call design | 90 days |

**Why these forms.** Attempts are few (tens a month for an active user), conditions change, and the
operator must be able to see why a ranking moved. Counts with a prior (Beta), a rate with a prior
(Gamma), and a one-number offset on a grid each update by arithmetic, need no fitting step, can
never fail to converge, and explain themselves: "2 of 3 from here; 14 of 20 for this gateway from
your area; the model said 70 %." Alternatives were weighed and set aside:

| Method | Why not as the base |
|---|---|
| Exponentially weighted averages alone | No notion of how much evidence there is: two successes and two hundred look alike. |
| Online logistic regression | Tens of observations for many features: it overfits, and its coefficients do not explain one gateway. Kept as an option for the prior's calibration only (§12.6). |
| A full hierarchical model fitted by MCMC | The right model in principle; slow, opaque to the operator, and a large dependency for a desktop client. The pooling below gets most of its benefit. |
| Bandits (Thompson sampling, UCB) as the ranking | They choose by sampling, so the list would change from one look to the next, and they spend the operator's air time on exploring. Used only when the operator asks for something to try (§12.8). |

**Pooling.** Each Beta estimate borrows from the level above it until it has its own evidence:

```
p_level = (successes + n0 * p_parent) / (attempts + n0)        n0 = 2 to 4
```

The chain for availability is: gateway in this block of the day → gateway → all gateways. For
service: gateway → all gateways. A gateway with two successes and no failures, under a parent of
0.6 and n0 = 2, is estimated at (2 + 1.2) / 4 = 0.80; one with 45 in 50 at (45 + 1.2) / 52 = 0.89.
Two lucky sessions do not outrank a long record.

**Weights.** Each record's weight is the product of: its age, as 2^(−age / half-life); one less the
probability that the station was at fault (§12.3); and one over the number of attempts in its
episode (calls to the same candidate within ten minutes of each other, which fail together for the
same reason and are one piece of evidence about the path, not several).

### 12.5 The SNR offset: how the physics model is calibrated

Propagation changes by the hour and the season; a gateway's power, antenna, local noise and the
operator's own station do not. So the client does not learn "gateway X works at 21Z" from counts in
hour buckets, which would need years of attempts. It learns how far each path's measured SNR sits
from the physics model's prediction, and lets the model carry the time of day, the season and the
sunspot cycle:

```
measured SNR = predicted median SNR (model, at that hour and month) + δ + day-to-day variation
```

δ is learned for each gateway and band (HF), or each gateway and channel (VHF), each way. It
starts at the offset learned over all gateways on that band, with a spread of 8 dB at HF (VOACAP's
own day-to-day deciles are of that order) and 10 dB at VHF (the location variability of a
terrain-diffraction path). It is kept as a posterior on a grid: δ from −30 to +30 dB in 1 dB steps,
crossed with availability *A* in 19 steps, 1 159 cells in all. Each record multiplies the grid by
its likelihood, raised to its weight (§12.4):

| Record | Likelihood of (δ, *A*) |
|---|---|
| Linked, SNR *m* measured | *A* · f(*m* − μ − δ), f the day-to-day density (VOACAP's split normal from its deciles; at VHF a normal with the location spread) widened by 2 dB of measurement error |
| Linked, no SNR from the modem | *A* · P(μ + δ + ε ≥ θ_connect) |
| No answer, nothing heard | (1 − *A*) + *A* · P(the call fails at μ + δ) |
| No answer, station heard at *m* | *A* · f(*m* − μ_down − δ_down) · P(the uplink fails) |
| Busy, cancelled, local, unknown | 1: no information |

θ_connect is the modem's: for Aether, the threshold of its tone-floor call and acceptance on the
channel class (§10.3). A failure is a *censored* observation: it says the SNR was below the
threshold, not what it was, and the likelihood uses it exactly that way. The grid makes this exact
with no approximation and no fitting: 1 159 multiplications a record, microseconds.

Direction. With `peer_snr_db` (§12.2) the uplink is measured directly. Without it, δ_up is δ_down
plus a direction offset learned over all gateways (power and noise differ between the user's station
and gateways in general), with a 6 dB spread.

**The place term at VHF.** A VHF path's offset changes over a few hundred metres when a vehicle
moves behind a building. The VHF offset is δ for the gateway (its real power and antenna, learned
from every place) plus a residual for the place, pooled from records within 1 km and weighted by
exp(−distance / 300 m). Terrain carries the place dependence the model can see (§8); the residual
carries what it cannot (buildings, trees, the vehicle).

**Why this is the right split.** It separates, as the brief asks, what propagation did from what the
modem did with it. The SNR offset says whether the model's view of the path is right. The modem
efficiency η (§12.6) says whether the modem turned that SNR into the expected rate. A path that
measures as predicted but carries half the expected data is a modem or channel-class problem, not a
propagation one, and the two are corrected in different places.

### 12.6 Throughput and stability

Throughput is modelled jointly with the path, through the SNR, and separately in what the SNR does
not explain:

```
goodput each way = η × g_modem(SNR, channel class)
```

g_modem is the modem profile's expected goodput at that SNR (§10.3). Each session measures both the
SNR (the `quality` readings) and the goodput (bytes `delivered` or received over the link's own
time, the handshake left out), so each session updates δ from its SNR and η from its goodput
relative to g_modem at the SNR measured. η is pooled from each gateway toward the modem and
bandwidth over all gateways.

Link loss is a rate per second of link, λ, from a global curve of the SNR margin above the modem's
floor (losses are mostly fades below it), times a factor for each gateway and band learned by
Gamma–Poisson counting. A loss whose SNR readings stayed high before it is not a fade; it adds to
the gateway's service record instead.

Calibrating the physics prior. Before a candidate has its own evidence, its connection probability
comes from the model. Over all attempts, the client fits two numbers, *a* and *b*, in

```
logit P(link) = a + b · logit(P_model)
```

(Platt scaling) so that the model's probabilities match outcomes on this station. With tens of
attempts this is well determined, and it corrects a model that is consistently optimistic or
pessimistic for this station's antenna and noise.

### 12.7 Drift

Forgetting (§12.4) handles slow change. A sudden one (a gateway's antenna down, a new noise source
at home) shows as outcomes the posterior found unlikely: three episodes in a row with predictive
probability under 0.05 together, with the station's health not in doubt. The candidate is then
marked "different lately", its half-lives are cut to a quarter until it agrees with its evidence
again, and the explanation says so. A change the operator knows of (a new antenna) is told to the
client by starting a new station profile (§6.5): records carry the profile, and a new profile's
offsets start from the old ones with a wider spread.

### 12.8 Exploration, and probing

Ranking by the best estimate alone means candidates never tried keep their prior for ever, and a
better gateway is never found. The remedy is under the operator's control:

* **Little history is shown**, on every row that has it (§16.1), and the explanation names the
  model as the main evidence.
* **"Suggest one to try"** picks, among candidates with little evidence and a real chance of being
  best, the one with the best sample from its posterior (Thompson sampling), and says why. The
  operator decides whether to call it.
* **"Check this gateway"** (with `caps.probe`, §12.2) sends one probe on the operator's click and
  records both directions' SNR, which updates δ without a session.

There is **no automatic probing and no automatic calling**, in any phase (the author's decision,
§21, Q9). The client never transmits to learn, or to try another gateway, on its own.

### 12.9 Seeing and resetting what was learned

* Each gateway's details show its evidence: attempts by class, the last ten with their outcome, the
  learned offsets each way against the model, availability, throughput against the modem's
  expectation, and when each was last updated (§16.4).
* The operator can forget a gateway's history, a place's, everything before a date (after a station
  change), or everything.
* Forgetting removes records from the learning, not from the file: the record is marked, and
  "show forgotten records" brings it back. Deleting the file is the operator's.
* Because estimates are recomputed from records, a change to the learning itself (new priors, a
  better model) is applied to all past evidence on the next start. Nothing learned is lost by
  improving the method.

---

## 13. The transaction model

### 13.1 What the transaction is

Before a call the client knows most of what it will move:

| Part | Known? | From |
|---|---|---|
| Outbound messages | Exactly: each air form's bytes | The outbox (RP-2 §4: the air form is made before the call) |
| Session overhead | Closely: `HELLO` 201 bytes, 361 with the whole credential, `WELCOME`, receipts, `DONE` (RP-3 §11); whether this gateway holds the credential | RP-3 §11; `Store::gateway` |
| Inbound, already listed | The size of each waiting message the fetch rule will take | `Store::waiting` from the last call |
| Inbound, new since | Not known | Estimated: the median inbound bytes of this user's recent sessions not already listed, shown as an estimate |
| Held attachments asked for | Known when listed | `Store::files` |
| Resumable pieces | Known: what remains of partial transfers | RP-3 §7, `partials/` |
| Priority | Known: the highest priority waiting to go | RP-2 `priority` (0 routine, 1 priority, 2 emergency) |

The transaction is therefore *B*_out (known), *B*_in (part known, part estimated), and a priority.
The estimate for new inbound mail is shown as one ("about 3 kB more may be waiting"), never
treated as known.

### 13.2 One attempt, and the whole transaction

For a candidate, from §12's estimates:

| Symbol | Meaning |
|---|---|
| *p* | Probability a call ends in a link: availability × P(the call's frames get through each way) |
| *t*_f | How long a failed call takes (the modem's call design; a scanning gateway's `call_s`) |
| *t*_c | Time from the call's start to the link |
| *h* | The session's opening: `HELLO`, `WELCOME` and the first turnarounds |
| *e* | The ending: last receipts, `DONE`, the disconnect |
| *r*_up, *r*_down | Goodput each way, bytes a second |
| λ | Link losses per second of link |
| *S* | Probability the gateway serves the session once linked |

Because RP-3 resumes (§2.3), a lost link costs a new call and a new opening, and nothing already
carried. Getting a working session costs, on average,

```
C = ( t_c + h + ((1 − p) / p) · t_f ) / S
```

and the data takes τ = *B*_out / *r*_up + *B*_in / *r*_down of link time, during which λτ losses
are expected, each costing another *C*. So the expected time to finish is

```
E[T] = C · (1 + λτ) + τ + e  =  K + B_out · s_up + B_in · s_down

K    = C + e                                  the fixed cost: getting connected
s_up = (1 + λC) / r_up,  s_down = (1 + λC) / r_down    the cost of each byte, reconnections included
```

**Expected time is a straight line in the transaction's size.** Each candidate is a line; its
intercept is what it costs to get connected, its slope what each byte costs. For a given size the
best candidate is the lowest line there, and two candidates swap places at

```
B* = (K_2 − K_1) / (s_1 − s_2)
```

This is the whole answer to "a reliable slow gateway or an unreliable fast one?": it depends on the
size, and the size where it changes is computable and can be shown.

The straight line assumes the rate and loss rate hold for the transaction. On HF a long transfer
crosses hours whose predictions differ; for those, and for the probabilities below, the client
simulates instead (§13.4).

### 13.3 A worked example

Three candidates, with times common to all (*t*_c = 25 s, *t*_f = 40 s, *h* = 20 s, *e* = 10 s,
*S* = 1, the same rate each way):

| | *p* | Rate | Links lost | Evidence |
|---|---|---|---|---|
| A | 0.95 | 40 B/s (320 bit/s) | one per 30 min of link | Long history |
| B | 0.45 | 150 B/s (1 200 bit/s) | one per 10 min | Long history |
| C | about 0.8 | about 100 B/s | about one per 15 min | Model only: *p* ~ Beta(1.6, 0.4), worth two attempts; rate log-normal, spread 0.7; loss rate Gamma, shape 2 |

Closed form (§13.2) and a simulation of 40 000 transactions each (appendix A), sizes in bytes:

| | *K* | *s* (s per byte) | 2 kB: E[T] | 2 kB: 80 % done by | 50 kB: E[T] | 50 kB: 80 % done by | 50 kB: P(≤ 10 min) |
|---|---|---|---|---|---|---|---|
| A | 57 s | 0.0257 | 108 s | 105 s | 1 340 s (22 min) | 1 350 s | 0 % |
| B | 104 s | 0.0077 | 119 s | 148 s | 489 s (8 min) | 553 s | 88 % |
| C, as estimated | 82 s | 0.0138 | 108 s | 117 s | 760 s (13 min) | 1 057 s | 49 % |
| C, if the guesses were exact | 65 s | 0.0106 | 86 s | 115 s | 596 s | 640 s | 75 % |

The closed form and the simulation agree to within half a per cent wherever both apply. What the
example shows:

* **A is best for a short message and worst for a long one.** A and B cross at 2.6 kB.
* **Uncertainty costs C time, honestly.** The same guesses held with no doubt would put C first at
  2 kB and close behind B at 50 kB. Held as two attempts' worth, they cost a quarter more on
  average and two-thirds more at the 80th percentile at 50 kB: a model-only candidate is not
  preferred over a proven one on equal guesses, without any rule saying so. One session on C
  narrows it quickly.
* **"Fewest failures" and "fastest link" each pick wrongly at one of the two sizes.**

### 13.4 Simulation, and why the day matters

For the probability of finishing within a time, and for transfers long enough to cross hours, the
client simulates (appendix A's model, extended):

1. Draw the parameters from their posteriors once for the simulated transaction.
2. Draw **one** day-level SNR offset for the day from the model's day-to-day spread. A bad day on a
   path is bad for every retry that day; retries are not independent draws.
3. Step through attempts: a call succeeds with *p* at the hour's predicted SNR; a link lasts an
   exponential time; data moves at the hour's rate; a lost link costs a new call and opening.
4. Stop when done or past the operator's budget.

Two thousand draws a candidate take well under a millisecond each in Rust; a list of fifty
candidates is a fraction of a second.

Step 2 has a consequence for fallback (§14.6): **after a failure, the next candidate should not be
the same path again.** A second gateway in the same direction on the same band shares the day's
ionosphere; one on another band, or in another direction, does not. The simulation can score a
*pair* (first choice, then fallback) with that correlation, which a ranking of single candidates
cannot.

### 13.5 Objectives

| Objective | Good for | Fails when |
|---|---|---|
| Highest completion probability within a deadline | Urgent traffic | With a generous deadline everything scores 1; slow candidates win ties |
| Shortest expected time | Routine traffic | Ignores risk: a candidate that is usually quick but sometimes hopeless can win |
| Highest throughput | Large transfers on good paths | Small messages: connection dominates (A in §13.3) |
| Fewest failed attempts | Short messages, crowded channels | Large transfers (A at 50 kB) |
| Shortest expected time among those meeting a reliability floor | Most cases | Needs two numbers, a floor and a deadline, to explain |

**The default, as the author chose it (§21, Q6): the time by which the transaction is done four
times in five** (the 80th
percentile of the simulated completion time, "T80"). It is one number in minutes that an operator
reads directly ("likely done within 9 minutes"), it grows with the size, it counts failed calls and
broken links, and it penalises thin evidence more than the mean does, which suits a choice made
once. It is the reliability-floor objective in one figure: a candidate that finishes within *x*
minutes four times in five has met an 80 % floor at *x*.

**Priority and emergency traffic** (RP-2 `priority` 1 or 2) switch the objective to the highest
probability of being done within a deadline (default 10 minutes for emergency, 30 for priority,
both settings), T80 breaking ties. The panel says which objective ordered the list.

The operator can choose the median instead of T80 in Setup ("typical rather than safe"); nothing
else is exposed.

### 13.6 What the numbers are not

* **Not precise.** The panel rounds: times to the half minute under five minutes and the minute
  above; probabilities in words with a band ("likely, about 3 in 4"). A difference smaller than the
  candidates' own spread is shown as a tie (§14.4).
* **Not measured yet.** The default overheads, the modem efficiency prior (0.6 for Aether, wide)
  and the loss-rate curve are starting values. The RadioPost sessions of 2026-11-12 on 80 m (22.6 kB
  in 183 s at 2 300 Hz, 22.5 kB in 465 s at 500 Hz, both ways reading +4 to +10 dB) and of
  2026-11-17 on 40 m (51.9 kB at about 920 bit/s) are the first points to set them from; the
  attempt records of Phase 1 replace them.

---

## 14. The ranking

### 14.1 One framework for HF and VHF

The physics differs completely between the two; the ranking does not. Each physical model answers
the same question, in the same form, and everything after it is shared:

```
PathEstimate {                       for one candidate, one direction, one time
    snr: distribution in dB (3 kHz reference),   from the model, before learning
    open: probability the path exists at all     (HF: MUFday; VHF: 1)
    channel_class: AWGN | Good | Moderate | Poor | NVIS | FM   (what fading to expect)
    reasons: [Reason]                            (what the model saw: §14.3)
    assumed: [Assumption]                        (inputs it had to guess: §6.5)
    model: name and version
}
```

HF fills it from VOACAP (§10), VHF and UHF from terrain and a link budget (§8). Learning (§12)
shifts the distribution by δ and widens or narrows it; the modem profile turns it into a
probability of connecting and a rate (§10.3); the transaction model turns those into time (§13).

### 14.2 The steps

```
rank(transaction, position, station, time, objective):
    candidates = eligible(directory, station, position, time)            # 14.2.1
    for c in candidates (cheapest first, best-looking first):
        est_down, est_up = path_model(c).estimate(position, station, c.gateway, time)
        post = learned(c, position, station)                              # §12: δ, A, S, η, λ
        snr_down = est_down.snr + post.delta_down;  snr_up = est_up.snr + post.delta_up
        p      = post.A * P_connect(modem(c), snr_up, snr_down, class)
        r_up   = post.eta * g_modem(modem(c), snr_up, class)
        r_down = post.eta * g_modem(modem(c), snr_down, class)
        lam    = post.lambda(margin(snr_down, snr_up))
        dist   = simulate(transaction, p, r_up, r_down, lam, post.S,
                          overheads(modem(c)), day_spread(est), draws=2000)
        c.score     = objective(dist)            # T80, or P(done within deadline)
        c.reasons   = reasons(est_down, est_up, post, dist)
        c.evidence  = evidence_level(post)       # much, some, little, model only
    order by score; mark ties (14.4); apply favourites within ties (14.5)
    return candidates with score, reasons, evidence, the objective's name
```

**14.2.1 Eligibility** removes, with a reason the operator can see on request: a modem this station
does not have; a band or frequency its radio or antenna does not cover (station profile, §6.5); a
channel outside the operator's rules for the control they run (the dial's segment, RP-4 §8); a
gateway the operator blocked; a channel whose `hours` or published windows say it is not listening
at that time (§7.3); an FM channel at HF or the reverse. Eligibility never removes a gateway for
being predicted poor: that is the ranking's job, and a removed row teaches nothing.

**Cost.** Physics runs are the expensive part, and are staged: a cheap pass for everything (HF: the
coverage grid at the hour, §10.2; VHF: distance and the radio horizon, §8.9), then full estimates
for the thirty or so that pass, both cached (§15). Learning and simulation are cheap. A ranking from
warm caches takes well under a second; from cold, a few seconds at HF (engine runs) and, at VHF,
however long the elevation tiles take to arrive (§8.9).

### 14.3 Explanations

Each row carries its two or three strongest reasons, as plain phrases, chosen by how much each moved
the score from what the model alone would give:

| Evidence | Phrases (examples) |
|---|---|
| Terrain (VHF) | "Clear terrain profile, antennas well placed"; "Blocked by a ridge 12 km out (about 25 dB lost)"; "Fresnel zone partly obstructed" |
| Propagation (HF) | "40 m predicted strong both ways at 21Z"; "Near the top of the usable range: may close within the hour"; "Long path predicted better than short" |
| Observations | "FT8 from near the gateway heard here in the last hour"; "Heard the gateway itself at −6 dB 20 minutes ago" |
| History | "Connected 9 of 10 times from here"; "Last 3 calls unanswered"; "Usually carries about 900 bit/s from here" |
| Transaction | "Faster for this 48 kB transfer"; "Best for short messages: answers almost always" |
| Evidence level | "Little history: prediction based mainly on the model"; "Model and history disagree: history trusted" |
| Data age | "Gateway record 40 days old"; "Elevation data not downloaded: distance only" |

Numbers in explanations are rounded the same way as the scores (§13.6). Every phrase can be opened
to the evidence behind it (§16.4).

### 14.4 Confidence, kept apart from the prediction

The score is a prediction; the evidence level says how much stands behind it. They are shown
separately and never multiplied together:

* **Evidence level**, from the effective number of weighted records behind the candidate's own
  estimates: *model only* (none), *little* (under 3), *some* (3 to 10), *much* (more).
* **Ties.** Two rows whose scores differ by less than the overlap of their 50 % intervals are
  shown as equal ("about the same"). The operator is not told 6.8 minutes beats 7.1.

### 14.5 Favourites

A favourite is ordered first among its ties, is offered first in the call box, and appears in the
Favourites view (§16.1) whatever its score. It never moves above a candidate clearly better, and a
favourite predicted poor keeps its warning ("Your favourite, but 10 m is closed toward it now").

### 14.6 Fallback order

When the operator asks for "the next one" after a failure, the next candidate is chosen by scoring
pairs (§13.4): the second choice that, after the first's failure, gives the best chance of finishing,
counting that the first failure makes the same path today less likely. In plain terms: try another
band or another direction before another gateway beside the first. After a failure the client
suggests that candidate with its reason (§18, Phase 6); the operator decides whether to call it.

### 14.7 Time other than now

The same ranking runs for a chosen time ("tonight at 20:00 local"). HF uses the model at that hour
and month, learned offsets unchanged (they are not tied to the hour), availability for that block
of the day, and today's conditions only as they are forecast (§10.6). VHF predictions do not change
with time; availability does. The panel says what is assumed ("forecast: monthly climatology, no
storm expected; your position as saved").

---

## 15. Persistence and interfaces

### 15.1 Where things are kept

RadioPost's client keeps its state as JSON in its home, written whole by way of a temporary file
(`store.rs`). This design keeps that style, adds append-only JSON Lines for the records that grow,
and puts large re-fetchable data in the operating system's cache directory. No database: the client
makes tens of attempts a month, and a few thousand records are read in milliseconds.

```
<first home>/selection/
    places.json           saved places, the default, the GPS reader's settings
    stations.json         station profiles
    gateways.json         the operator's own gateways, corrections, favourites, blocks
    attempts/2026-11.jsonl    one line per attempt (§12.1): raw evidence, never rewritten
    heard/2026-11.jsonl       signals heard and probes (§10.5): raw evidence
    forgotten.json        records the operator excluded from learning (§12.9)
    learned.json          what was learned: a cache, rebuilt from the records when missing or
                          when the learning's version changes
<cache dir>/radiopost/
    terrain/copernicus-glo30/v2021/N33W085.bin      converted tiles (§9.3)
    terrain/profiles/<hash>.json                    profiles by both ends and the data's version
    hf/voacapl-0.7.7/<hash>.json                    predictions by every input (§10.2)
```

The first home is where the client already keeps what belongs to the computer rather than to one
callsign (the address list, the log): places, stations and attempts belong to the station.

**Raw and derived are apart.** `attempts/` and `heard/` are evidence. `learned.json` is a
computation over them, carrying the learning's version; a new version of the learning recomputes it
from all the evidence (§12.9). Recalibrating never destroys evidence.

### 15.2 The records

An attempt (§12.1), one line, here spread out:

```json
{"v": 1, "id": "a-20261120-031522-7f3a", "start": 1795144522, "end": 1795145001,
 "kind": "gateway", "remote": "KK4ODA-1", "as": "WD5EMA",
 "channel": {"dial_hz": 7101000, "bandwidth_hz": 2300, "call_s": null},
 "modem": "Aether HF 0.2.0-beta.168",
 "place": {"grid": "EM73tr", "source": "typed"},
 "station": null, "predicted": null,
 "transaction": {"messages": 3, "out_bytes": 48210, "in_listed_bytes": 3120, "priority": 0},
 "call": {"busy_before": false, "duration_s": 24.6, "result": "connected", "text": null,
          "heard_remote": true, "heard_snr_db": 4.5, "refused": null},
 "link": {"bandwidth_hz": 2300, "duration_s": 452.3, "end": "local",
          "quality": [[3.1, 5.0, 1100, false], [61.0, 7.5, 1650, true]],
          "snr_db": {"min": 3.5, "median": 6.5, "max": 9.0, "readings": 74},
          "bytes_handed": 48450, "bytes_unacknowledged": 0, "deliveries": 12, "bytes_in": 5480},
 "session": {"online": true, "errors": [], "notices": 0, "over": true, "refused_other": false,
             "taken": 3, "queued": 0, "received": 2},
 "outcome": {"class": "completed", "plan_done": true, "left_out": 0, "why": null},
 "classifier": 1}
```

`quality` entries are `[seconds since the link came up, snr_db, rate_bps, receiving]`. `classifier`
is the version of the rules that set `outcome.class`; a later version re-classifies from the
evidence kept, so the class is recomputable too.

That is the record as Phase 1 writes it (`app/client/src/selection/attempts.rs`). A line with
`"stub": true` and only the call's identity is written first, when the call starts, so a call the
program never finished still counts; a reader takes the last line of each `id`. `link` and
`session` are `null` for a call that brought no link; `call.refused` names a request of the call's
own (`connect`, `tune`, `dial`) the modem refused, and `outcome.why` the error a call ended in.
Later phases fill what Phase 1 leaves empty: `place` gains the position and its source and age
(§6), `station` the profile in use (§6.5), `predicted` the prediction made before the call (§10,
§14), and `transaction` the estimate of new mail coming in (§13.1).

A place, a station profile and an own gateway:

```json
{"id": "pl-home", "name": "Home", "lat": 33.75, "lon": -84.39, "ground_m": 320,
 "default": true, "station": "st-home", "source": "typed"}

{"id": "st-mobile-ts480", "name": "Mobile, TS-480", "power_w": 65,
 "hf_antenna": {"kind": "whip", "agl_m": 2.5}, "vhf_antenna": {"gain_dbi": 2, "agl_m": 1.8},
 "feedline_db": {"hf": 1, "vhf": 2}, "noise": "residential", "bands": ["80m", "40m", "20m", "2m"]}

{"callsign": "N0CALL-10", "own": true, "site": {"lat": 34.01, "lon": -84.40, "agl_m": 18},
 "channels": [{"dial_hz": 145050000, "mode": "FM", "modem": "ax25", "bandwidth_hz": 12500}],
 "favourite": true}
```

A learned estimate (in `learned.json`, keyed by candidate), shown because it is what the details
view reads:

```json
{"KK4ODA-1/7101000/aether/2300": {
   "attempts_weighted": 7.4, "availability": {"mean": 0.93, "n": 9.1},
   "delta_down_db": {"mean": 2.1, "sd": 2.8}, "delta_up_db": {"mean": -1.4, "sd": 3.9},
   "service": {"mean": 0.97, "n": 6.0}, "eta": {"median": 0.71, "sd_log": 0.25},
   "loss_per_hour": {"mean": 0.6, "n_hours": 2.3}, "changed_lately": false,
   "evidence": "some", "as_of": 1795145001, "learning": 1}}
```

### 15.3 Interfaces between the parts

In Rust, in the order data flows (names provisional):

```rust
/// Where the station is (radioprop-place).
pub trait PositionSource {
    fn current(&self) -> Option<Fix>;            // None when silent or stale (§6.3)
    fn status(&self) -> SourceStatus;            // reading, silent since, error
}
pub struct Fix { pub lat: f64, pub lon: f64, pub alt_m: Option<f64>, pub at: u64,
                 pub source: SourceKind, pub quality: FixQuality }

/// A physical prediction for one direction (radioprop-hf, radioprop-terrain).
pub trait PathModel {
    fn name(&self) -> String;                    // with its version and data's version
    fn estimate(&self, from: &End, to: &End, freq_hz: u64, at: Time) -> PathEstimate;
}
pub struct End { pub position: LatLon, pub station: StationProfile, pub assumed: Vec<Assumed> }
pub struct PathEstimate { pub snr_db: SplitNormal, pub open: f64, pub class: ChannelClass,
                          pub reasons: Vec<Reason>, pub assumed: Vec<Assumed> }

/// What a modem does with an SNR (radioprop-hf; data files, one per modem and bandwidth).
pub struct ModemProfile { pub name: String, pub bandwidth_hz: u32,
                          pub call_db: ByClass, pub answer_db: ByClass,
                          pub rungs: Vec<Rung>,       // threshold by class, net bit/s
                          pub generic: bool }         // true: no model, learned only

/// Evidence and learning (RadioPost client).
pub trait Evidence {
    fn begin(&mut self, stub: AttemptStub) -> AttemptId;
    fn finish(&mut self, id: AttemptId, record: AttemptRecord);
    fn heard(&mut self, sighting: Sighting);
    fn forget(&mut self, what: Forget);
}
pub fn learn(records: &[AttemptRecord], heard: &[Sighting], cfg: &Learning) -> Learned;

/// The transaction and the ranking (RadioPost client).
pub fn transaction(store: &Store) -> Transaction;
pub fn rank(cands: &[Candidate], tx: &Transaction, learned: &Learned, objective: Objective,
            at: Time) -> Vec<Ranked>;
pub struct Ranked { pub candidate: Candidate, pub score: Score, pub median_s: f64, pub t80_s: f64,
                    pub p_within: Option<f64>, pub evidence: EvidenceLevel,
                    pub reasons: Vec<Reason>, pub tie_group: u32 }
```

The client's panel gains, behind its existing rules (loopback, the `Host` checked, changes with
`X-RadioPost: 1`):

| Endpoint | Gives |
|---|---|
| `GET /api/candidates?when=…&objective=…` | The ranked list with reasons and evidence levels |
| `GET /api/candidate?id=…` | One candidate's details and evidence (§16.4) |
| `GET /api/profile?id=…&agl_here=…&agl_there=…` | A VHF terrain profile, recomputed for the heights given |
| `GET /api/places`, `POST /api/places` | Saved places |
| `GET /api/position` | The position in use, its source and age |
| `GET /api/stations`, `POST /api/stations` | Station profiles |
| `GET /api/attempts?gateway=…` | History |
| `POST /api/forget` | Forgetting, as §12.9 |
| `POST /api/terrain/prepare` | Fetch an area's elevation data ahead |

### 15.4 Interoperability

Nothing here changes what crosses the air or what another RadioPost implementation must do.
Selection is a client's own business. Three things touch the specifications, all optional and
MINOR: the gateway record's members (§7.2, RP-5) and the modem interface's additions (§12.2, RP-4),
each proposed in the phase that uses it (§21, Q3 and Q4), and, if wanted later, an informative document describing the
attempt record so that other clients (RadioPostKit) could read and share the same evidence.

---

## 16. The user interface

The client's panel (`app/ui`, no build step) keeps its look: a list with the details a click away.
The default view is simple; everything in §12 to §14 is behind "why" and "details". Wireframes
below are layout, not styling. As the manual rule requires, the manual and its screenshots change
with each piece when it is built.

### 16.1 The gateway selector

Replaces today's Gateways view.

```
┌ Gateways ─────────────────────────────────────────────────────────────────────────────────────┐
│ Position  GPS (phone, TCP) · EM73tr · 3 s ago   [Places ▾]     Station  Mobile, TS-480 [▾]     │
│ This call  48 kB to send · 3 kB waiting, maybe 2 kB more · routine                              │
│ Ordered by  likely done within (4 times in 5)                          [At another time…]       │
│                                                                                                 │
│ [Recommended] [Favourites] [Recent] [Worked from here]   Band [all ▾] Modem [all ▾] [Map]       │
│                                                                                                 │
│    Gateway       Channel                   Likely done   Connects      Rate         Evidence    │
│ 1 ★ KK4ODA-1     7.101 MHz · Aether 2300   7 min         very likely   ~900 bit/s   much  [Choose]
│     40 m strong both ways now · connected 9 of 10 from here                         [why]       │
│ 2   N4XYZ-10     3.5915 MHz · Aether 500   9 min         likely        ~400 bit/s   some  [Choose]
│     Answers almost always · slower, steadier                                         [why]       │
│ 3 = W4ABC-1      14.103 MHz · Aether 2300  9 min         about even    ~1.6 kbit/s  model [Choose]
│     Little history: mostly the model's view · 20 m may close within the hour         [why]       │
│ 4   K4QRS-1      145.050 MHz · packet      14 min        likely        ~800 bit/s   little[Choose]
│     Blocked by a ridge 9 km out, about 22 dB lost · within margin                    [why]       │
│ ⋯                                                                                               │
│ Left out (3): 10 m channels (closed toward them now); VARA FM (no such modem here)  [show]      │
│ Gateway lists fetched 2 h ago · elevation data: this area held · [Suggest one to try]           │
└─────────────────────────────────────────────────────────────────────────────────────────────────┘
```

* **Columns** stay few: the score in minutes, connection and rate in words with a rough figure, the
  evidence level. "=" marks a tie with the row above (§14.4).
* **Views** are filters of one list: *Recommended* (ranked), *Favourites* (the operator's, in
  rank order, warnings kept), *Recent* (last attempts, newest first, with outcome), *Worked from
  here* (candidates with successful sessions within 50 km at HF or 2 km at VHF of the position).
* **Filters**: band, modem, distance, favourites only.
* **[why]** opens the reasons and the evidence summary in the row; **the row** opens the details
  (§16.4); **[Choose]** does what it does today (W14: channel into the call box, the radio tuned).
* With nothing learned and no prediction (no position, no engine), the list is today's: by
  distance, saying so.

### 16.2 The map

```
┌ Map ─────────────────────────────────── HF: 40 m at 21Z ▾ ── [List] ┐
│                                                                     │
│        ·  ·        ○ W4ABC-1                                        │
│            ╲      ╱                                                 │
│   ● KK4ODA-1 ━━━━◎ you (EM73tr)                                     │
│                   ╲                                                 │
│                    ┄┄┄○ N4XYZ-10        ● recommended  ○ listed     │
│                                        ★ favourite    ◆ worked      │
│  Day and night shaded · lines coloured by "likely done"            │
└─────────────────────────────────────────────────────────────────────┘
```

* **HF**: the world from bundled vector data (as hf-predict draws it: Natural Earth through
  `world-atlas`), the terminator, great-circle lines to the candidates coloured by score, and, on
  request, hf-predict's coverage shading for one band. No tile server.
* **VHF**: a local map drawn as hillshade from the elevation data already cached (§9.3), with
  paths coloured by terrain class and the dominant obstacle marked. No map tiles are needed, so
  the map works offline wherever the terrain does. Online map tiles are an option the operator
  turns on, used under their provider's policy (no prefetching of OpenStreetMap's tiles).
* Selecting on the map selects in the list, and the reverse.

### 16.3 The VHF terrain profile

```
┌ K4QRS-1 · 145.050 MHz · 31.4 km at 287° ─────────────── terrain: Copernicus GLO-30 (surface) ┐
│ m                                                                                             │
│ 600 ┤                                                              ▲ K4QRS-1 mast 18 m        │
│ 500 ┤                  ▄▄▄                                        ███                         │
│ 400 ┤ ·  ·  ·  · · ·▄█████▄· · · · · · · · · · · · · · · · · · · ·███  direct ray             │
│ 300 ┤   ┄┄┄┄┄┄┄┄┄▄██████████▄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄▄▄████  60 % of first Fresnel    │
│ 200 ┤▲ you  ▄▄▄▄██████████████████▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄█████████                         │
│     └──────────────┬───────────────────────────────────────────────────────────── km         │
│                    9 km: ridge 41 m above the direct ray                                     │
│                                                                                              │
│ Blocked by terrain · diffraction about 22 dB (P.526) · margin about 30 dB · likely to work  │
│ Your antenna  [====|========]  1.8 m       Gateway antenna  18 m (from its record)           │
│ At 6 m: ridge 33 m above the ray · about 19 dB · +3 dB                                       │
│ Earth curvature included (k = 4/3) · trees and buildings as the radar saw them, 2010–2015    │
└──────────────────────────────────────────────────────────────────────────────────────────────┘
```

* Ground (with Earth's bulge), both antennas, the direct ray, the 60 % Fresnel envelope, the
  dominant obstacle with its height above the ray, the class, the loss, the margin and its words.
* The **antenna slider** recomputes at once (§8.8) and states the gain in decibels.
* What the data is (surface or bare earth, its source and years), and every assumed input, are on
  the view.

### 16.4 Gateway details

```
┌ KK4ODA-1 ── listed by radiopost.org · record 3 days old · reached its post office 4 min ago ───┐
│ Channels  7.101 MHz Aether 2300 (scanned with 3.590) · 3.590 MHz Aether 2300 / 500             │
│ Site  EM73 (no precise site published) · station not published: assumed 100 W, dipole 10 m    │
│                                                                                                 │
│ From here, 7.101 MHz                       Model        Learned        Measured (last 30 days) │
│   Signal there → here (21Z)                +4 dB        +6 dB (±3)     +5 … +9 dB, 7 sessions  │
│   Signal here → there                      +2 dB        +1 dB (±4)     from its reports        │
│   Answers when called                      —            93 % (9 calls) 9 of 10                 │
│   Rate                                     1 400 bit/s  ~900 bit/s     700 … 1 100 bit/s       │
│   Links lost                               —            ~1 an hour     1 in 7 sessions          │
│                                                                                                 │
│ Last attempts  20 Nov 21:02 completed 48 kB in 7 min · 19 Nov 13:40 no answer, nothing heard … │
│ Through the day (40 m, this month)  ▁▂▄▆██▇▆▄▂▁▁▁▁▂▃▅▇██▇▅▃▂   hours UTC 0 … 23                  │
│ [Check this gateway (probe)]  [Forget its history]  [Correct its record]  [★ Favourite]        │
└─────────────────────────────────────────────────────────────────────────────────────────────────┘
```

The three columns are the point: what the model says, what has been learned, and what was
measured, side by side, so a disagreement is visible rather than averaged away.

### 16.5 Places, GPS and station

A Setup card: the list of places (name, position, grid, default, station profile) with *Add from
GPS*, *Add from the map* and *Add from a grid square*; the position source (a place, a GPS reader,
or automatic) with the reader's settings (serial port and speed with *Find*; host and port;
UDP port; gpsd); and the reader's state in words ("reading: 3D fix, 2 s ago"; "silent for 6
minutes: using Home"). Station profiles alongside, every field optional and its default shown.

### 16.6 History

The Recent view (§16.1) with a filter by gateway, place, band and outcome, and per attempt: when,
from where (place name or grid), the channel, the outcome class in words, the time taken, the
bytes each way, and the prediction made at the time beside what happened. Exporting it writes the
records with positions cut to the grid square unless the operator asks otherwise.

### 16.7 Another time

*At another time…* opens a time picker (today, tonight, a date and hour), reruns the ranking for
it, and heads the list with what is assumed: "Tonight 20:00 local (01Z): monthly prediction; today's
conditions not included; no storm forecast; your position as saved (Home)".

---

## 17. Architecture and data flow

```
                        ┌──────────── post offices (RP-5 §4.4, signed lists; `seen`, `site`…) ┐
                        │                                                                      │
 GPS: serial · TCP ·    │   RadioPost client                                                   │
 UDP · gpsd             ▼                                                                      │
   │        ┌─────────────────────┐   ┌──────────────────────┐                                 │
   └──────▶ │ Position (§6)       │   │ Gateway directory    │◀── own gateways, corrections,   │
 places ──▶ │ radioprop-place     │   │ (§7) client          │    favourites (gateways.json)   │
            └─────────┬───────────┘   └──────────┬───────────┘                                 │
                      │ position, station        │ candidates                                  │
                      ▼                          ▼                                             │
            ┌───────────────────────────────────────────────┐                                  │
            │ Physical models (§8, §10)                      │                                  │
            │  HF: radioprop-hf ──▶ voacapl (bundled engine) │◀── sunspot table                 │
            │  VHF: radioprop-terrain ──▶ elevation cache ◀──┼─── Copernicus GLO-30/90 (HTTPS)  │
            │  → PathEstimate each way                       │                                  │
            └───────────────────────┬───────────────────────┘                                  │
                                    │                         ┌────────────────────────────┐   │
                                    ▼                         │ hf-predict (if running):   │   │
            ┌───────────────────────────────────────────────┐ │ GET /v1/evidence (§11.4)   │   │
            │ Learning (§12)  ◀── attempts/*.jsonl, heard/   │◀┤ FT8 tiers toward a place   │   │
            │  classify · δ, A, S, η, λ, O, H · pooled,      │ └────────────────────────────┘   │
            │  discounted, explainable  → learned.json       │                                  │
            └───────────────────────┬───────────────────────┘                                  │
                                    ▼                                                          │
            ┌───────────────────────────────────────────────┐                                  │
            │ Modem profiles (§10.3) + transaction (§13)     │◀── outbox, waiting list, priority│
            │ simulate → T80, P(done within), reasons        │                                  │
            └───────────────────────┬───────────────────────┘                                  │
                                    ▼                                                          │
            ┌───────────────────────────────────────────────┐   choose ┌───────────────────┐   │
            │ Ranking (§14) → panel (§16)                    │────────▶│ session::call      │   │
            └───────────────────────────────────────────────┘          │ RPMI (RP-4) ──▶ modem
                                    ▲                                  └─────────┬─────────┘   │
                                    │        attempt record: begin, events, outcome │            │
                                    └────────────────────────────────────────────┘            │
```

| Component | Lives in | Responsibility | Depends on |
|---|---|---|---|
| Position (places, GPS readers, station profiles) | `radioprop-place` (hf-predict's workspace) | A current, honest position and station | Serial, sockets |
| Geometry, locators | `radioprop-geo` | One implementation of Maidenhead and great circles | — |
| HF model | `radioprop-hf` | VOACAP both ways, batched, cached; modem profiles | The engine binary |
| VHF model | `radioprop-terrain` | Elevation tiles, profiles, P.526, ITM later, link budgets | HTTPS for tiles |
| Observations | `radioprop-observe`, served by hf-predict | FT8 evidence tiers | hf-predict running |
| Gateway directory | RadioPost client (`store.rs`, `online.rs`) | Lists (as today) plus local entries and the new optional members | RP-5 |
| Attempt records, classification, learning | RadioPost client, new module `selection` | Evidence in, estimates out | The records |
| Transaction model, ranking | RadioPost client, `selection` | Candidates in, ranked and explained out | All the above |
| Panel views | `app/ui` | §16 | The panel API (§15.3) |
| Next-candidate suggestion (Phase 6) | RadioPost client | After a failure, proposes the next candidate in pair order, with its reason; never calls | The ranking |

The client calls the libraries in-process: no new program, no new port, no new service. The one
cross-program link (hf-predict's evidence API) is optional and read-only.

---

## 18. Roadmap

Each phase ships something an operator can use on its own. Attempt records start in Phase 1,
because every later phase learns from them and evidence not recorded is lost.

| Phase | Delivers | Depends on | Accepted when |
|---|---|---|---|
| **1. Position, directory, history** | Places, the GPS readers (serial, TCP, UDP, gpsd) with freshness; station profiles; distance and bearing from the real position; the operator's own gateways, favourites, blocks, ages shown; **the attempt record for every call, with the failure class**; the Recommended, Favourites, Recent and Worked-from-here views; the world map | Nothing | NMEA parser tests from recorded streams of each reader, GPS2IP's fixed HDOP included; staleness and fallback tests with an injected clock; a record for every exit path of `session::call`, failures and cancels included, checked by driving the loopback adapter through each RPMI event (the conformance tester's adapter can produce them) |
| **2. VHF terrain** | Copernicus tiles fetched and cached; profiles; clearance, Fresnel, P.526 §4.5; the link budget with defaults; the profile view and antenna slider; the local hillshade map; "prepare this area" | Phase 1 (positions) | Geometry against hand-worked cases; P.526 against worked knife-edge and smooth-Earth cases; on 50 real paths, P.526's loss compared with NTIA's ITM run on the same profiles, differences reported; 100 candidates ranked in under a second from cached tiles |
| **3. HF** | hf-predict's crates (§11.3 steps 1–3); both-way VOACAP for each channel, batched and cached; Aether's modem profile; the time picker; the evidence API from hf-predict | hf-predict's workspace | The crates pass hf-predict's own tests and engine reference cases; the same prediction from the app and the library; RadioPost's HF ranking works with networking off |
| **4. Learning** | Classification rules versioned; δ, A, S, η, λ, O, H estimated with pooling and forgetting; the details view's three columns; forgetting; the station-health warning | Phase 1's records; Phases 2–3 for δ | Learning replayed on recorded and simulated attempt streams reproduces known offsets within their stated spread; a planted station fault is flagged and not blamed on gateways; the same records always give the same estimates |
| **5. Transaction ranking** | The transaction (§13.1); simulation; T80 and the priority objectives; reasons and ties; ranking pairs for "next one" | Phase 4 | The closed form and simulation agree (appendix A); the validation of §19 run on all records so far, reported with intervals, before the ranking becomes the default order |
| **6. Refinement** | Probing on request (`caps.probe`, RP-4, §21 Q4); "suggest one to try"; obstruction data experiments; after a failure, the next candidate suggested with its reason | Phase 5 | Each experiment judged on recorded outcomes before it changes the default; no path in the client starts a call or a probe without the operator's click |

**After a failure (Phase 6).** The client names the next candidate of the pair order (§14.6), with
why ("another band: 40 m failed, 80 m does not share today's ionosphere"), and puts it in the call
box for one click. It never calls, retries or retunes by itself: the author's decision (§21, Q9).
Unattended operation (a station calling with no one present) is automatic control, allowed in the
US only in the segments RP-7 §5.1 lists, and is not part of this design.

**Effort, roughly.** Phase 1 and Phase 3 are each a few weeks; Phase 2 is the largest (elevation
handling and its tests); Phases 4 and 5 are modest in code and long in calendar time, because they
wait for records.

---

## 19. Validation

Nothing in this design is claimed to work better until records show it. This section says what
would show it, against what, and how to avoid fooling ourselves.

### 19.1 The methods compared

| | Method | What it uses |
|---|---|---|
| M0 | Distance (today's client) | Grid squares |
| M1 | HF propagation only | VOACAP reliability at the modem's call threshold |
| M1w | Winlink Express's method, as published | VOACAP reliability at 40 dB-Hz, 50 W; then distance (§4.1) |
| M2 | VHF distance and terrain | §8, no learning |
| M3 | Propagation and history | M1 or M2 plus success counts per candidate (Beta, pooled) |
| M4 | Propagation and observations | M1 plus FT8 evidence (§10.5) |
| M5 | Adaptive reliability | §12 in full |
| M6 | Transaction-aware | §12 to §14 |

M1w is the only comparison with Winlink this design makes: Winlink's *method* applied to
RadioPost's own gateways and attempts. Comparing networks (two thousand Winlink gateways and
RadioPost's pilot; VARA and Aether) would measure the networks, not the selection.

### 19.2 Outcomes

**Primary:** the transaction completed (within 30 minutes, or the priority deadline); time to
complete; calls that failed before the one that worked; whether the top-ranked candidate was a
working one.

**Secondary:** goodput; top-1 and top-3 success; calibration (Brier score and log loss of P(link),
reliability diagrams for P(link) and P(done within)); error of the predicted SNR each way, before
and after learning; error of the predicted time; all of these with little history (under three
records) and with much; by place; by band; on days of disturbed conditions.

### 19.3 Three sources of evidence

1. **Aether's scenario harness, for the machinery.** Its channel server already runs several
   stations on several dials (Aether's ADR-0129), with fading, noise, outages, QRM and gateways that
   come and go. There the truth is known and every candidate can be run on the same draw, so the
   best choice in hindsight exists. It checks that the learning converges to the offsets the
   scenario set, that the ranking finds the best candidate when the evidence allows, and the cost of
   a cold start. It does not validate the physics (the harness sets SNRs, it does not propagate
   them). Aether's own lessons apply: steps are taken by wall clock, so one build on one draw can
   differ between runs; judge on paired draws, and repeat a surprising one.
2. **Replay of recorded attempts, for the predictions.** Every recorded attempt is predicted again
   by each method using **only records before it** (rolling origin), and scored against what
   happened. This is the honest test of calibration and of whether learning improves the forecast
   (M5 against M1 and M3; for VHF, M2 against M0). It cannot score candidates nobody called.
3. **A field study, for the choices.** With volunteers' consent and for a set period, when there
   are two or more eligible candidates within a tie or near it, the client puts one of the two methods' top
   choices first at random (M6 against M0 or M1w), and records which; the operator still presses
   Call (§21, Q9). Randomised, so
   the comparison is fair; limited to near-ties, so no operator is sent to a poor choice.

### 19.4 Pitfalls, and what is done about them

| Pitfall | Remedy |
|---|---|
| Repeated calls in a burst are not independent | The episode (calls to one candidate within ten minutes) is the unit |
| One operator's station dominates | Results reported per operator and pooled with operators as clusters (cluster bootstrap for intervals) |
| Leakage from the future | Rolling origin; time-blocked splits by week; never random splits |
| Selection bias: only chosen candidates are observed | The field study's randomisation; replay scores predictions, not choices |
| Small numbers | Effect sizes with intervals; no claim at 90 % from ten attempts. To see top-1 success rise from 70 % to 80 % with the usual 5 % significance and 80 % power takes about 290 episodes for each method; months of several active stations |
| Hardware failures read as propagation | The classification (§12.3) is itself checked: a sample of failures is labelled by hand from the logs and both sides' recordings, as Aether's reports do, and its accuracy reported |
| A model tuned on the data it is judged on | Priors and rules frozen before each evaluation period; changes judged on the next period |

### 19.5 For VHF and HF in particular

* **VHF: does terrain beat distance?** On VHF attempts, the area under the ROC curve of predicted
  margin against "connected", against distance alone; false "unreachable" (predicted blocked,
  connected) and false "viable" rates; separately for gateways that publish a site and antenna
  height and those that do not. If terrain does not beat distance, it stays as information (§8.10).
* **HF: do outcomes improve on the model?** Log loss of P(link) and RMS error of the SNR with and
  without the learned offsets, by band and hour; and whether the offsets learned in one season still
  help in the next (the model is meant to carry the season; if the offsets drift with it, the model
  is missing something).

Results go in a report under `docs/` in the style of Aether's research reports: the data, the
method, the numbers with their intervals, and what changes because of them.

---

## 20. Risks, trade-offs and open questions

### 20.1 Risks

| Risk | What it would do | Answer |
|---|---|---|
| Too few records for the learning to matter | Rankings stay model-driven for months | The model and the classification carry the early value; the learning is built so that few records help a little and many help a lot, never so that few records mislead (pooling, priors, evidence levels) |
| Numbers trusted too much | Operators skip a gateway that would have worked | Rounding, ties, evidence levels, the three columns (§16.4), and no candidate ever hidden for a poor score |
| Surface model at VHF (trees as walls) | Pessimistic loss in forests and towns | Labelled; the place term learns it; bare earth plus canopy as a measured experiment (§8.7) |
| Elevation download size | 40 MB a GLO-30 tile; slow in the field | GLO-90 first; "prepare this area" ahead; range reads (§9.3) |
| Copernicus terms | Notices and a liability sentence that must travel with redistribution | In the licence files and About; the data is fetched, not bundled |
| The engine bundle | A few MB more to download, a Fortran build in another repository | Built once by hf-predict's CI and pinned by checksum; a Rust port (`hfcast-engine`) could remove it after hf-predict validates it |
| Three repositories to keep in step | A RadioPost release waits on a crate release | Crates pinned by tag; RadioPost upgrades when it chooses |
| Position privacy | Precise positions on disk, in records | Kept local, shared only as a grid square; an option to keep only the square (§12.1) |
| A gateway's site in its record | Publishes where an operator's antenna is | Optional, at the precision the operator chooses (§7.2) |
| Classification wrong | Learning blames the wrong cause | Rules versioned and re-applied; accuracy measured by hand-labelled samples (§19.4) |
| Complexity grows | A design no one can explain | Every estimate has a parent it shrinks to and a sentence it explains itself with; anything that does not beat a simpler method on records is dropped (§19) |

### 20.2 Trade-offs taken

* **Explainable estimates over a better-fitting black box.** A gradient-boosted model would fit
  more, would need data RadioPost will not have for years, and could not tell an operator why.
* **Physics as the clock, learning as the level.** Learning by the hour of day directly would need
  far more records and would forget every season.
* **The 80th percentile as the default score.** One number, risk-aware, honest about thin evidence.
  The mean is faster on average and riskier; it is available as a setting.
* **No automatic transmission at all.** Probing and calling only on the operator's click (§21,
  Q9): the learning is slower, a failed call costs the operator a click, the band is quieter, and
  the rules are simpler.
* **No central collection of outcomes.** Each client learns alone. A shared data set could help new
  users a great deal and is the obvious next question (§20.4), but it is a privacy and governance
  choice, not a technical one.

### 20.3 The design, criticised

Asked of every part: does it earn its place, and what would be lost by cutting it?

1. **Is selection worth this today?** RadioPost has a handful of gateways. But a gateway that scans
   two bands and runs two bandwidths is already four candidates, and choosing among them is the
   decision every HF call makes today. VHF gateways (packet, VARA FM, Aether FM to come) make terrain
   matter as soon as they exist. The design is phased so that Phase 1 alone (real position, history,
   the right channel shown first) pays for itself.
2. **ITM is cut from the first VHF release.** P.526 §4.5 is one standard method, deterministic and
   explainable; ITM is added only if the records show P.526 ranking worse.
3. **Obstruction datasets are cut** until terrain alone is measured.
4. **The two-dimensional grid posterior** is the most technical piece. It is kept, because a failed
   call is censored evidence and treating it as a measured SNR (or ignoring it) would bias every
   offset. For modems with no SNR it falls back to counts.
5. **The simulation** could be replaced by the closed form for routine ranking. It is kept because
   T80, the priority objective and correlated retries (§13.4) need a distribution, and it is
   eighty lines.
6. **hf-predict's refactor** could be skipped by writing a small VOACAP runner inside RadioPost. That
   duplicates a validated engine integration and its CI, and helps neither app. The refactor's first
   step is mechanical and is checked by hf-predict's own tests.
7. **The optional protocol additions** are not needed for any phase to work. They are proposed
   because each one turns a guess into a measurement, and each is something Aether already knows.

### 20.4 Open questions

* **A shared outcome data set.** Clients could contribute anonymised attempt records (grid square,
  gateway, band, hour, outcome) to a published data set, from which every client's priors would
  start. Who keeps it, what is published, and whether a gateway operator may object, are the
  author's questions, not this design's.
* **The modem profile's channel class** from VOACAP's outputs (§10.3) is a first mapping. Whether a
  better one exists (from MPROB, the mode, or the delay) is for Phase 4's records to say.
* **FT8 as evidence for Aether.** How much an FT8 "open" says about an Aether link is unknown; the
  ratio starts fixed and is fitted later (§10.5).
* **Packet and VARA FM thresholds** are not published as measurements (§8.6). A bench measurement
  through real radios would replace an assumption with a number.
* **Gateways that scan.** A scanning gateway's channel is listened to part of the time; whether its
  availability should be learned per channel or for the gateway (the scan's dwell is known from
  `scan_s`) is open until scanning gateways have records.
* **The wishlist.** W17 (*Gateways ranked by propagation*) recommended a prediction only; this
  design goes further. Once the proposal is accepted, W17 is rewritten to point here, and the new
  wishes (places and GPS, terrain, learning, transaction ranking) are added with their
  compatibility notes (the author's answer, §21, Q10).

---

## 21. Decisions for the author

Asked one at a time and answered by the author on 2026-11-20. Nine answers are the recommendation;
Q9's is not, and the design follows it (§12.8, §14.6, §17, §18).

| # | Question | Recommendation | The author's answer |
|---|---|---|---|
| Q1 | Relicense hf-predict's extracted crates MIT OR Apache-2.0? (The app may stay Apache-2.0.) | Yes: RadioPost's binaries then carry one licence choice throughout | As recommended: the crates MIT OR Apache-2.0 |
| Q2 | Where do the shared crates live: hf-predict's repository as a workspace, a new repository, or RadioPost's? | hf-predict's: the engine build and its CI are there; move them to their own repository only if a third program needs them | As recommended: hf-predict's repository, as a workspace |
| Q3 | Propose RP-5's optional record members: `site`, a channel's `station`, `listens`, and `seen` from the post office? | `seen` and `listens` with Phase 1; `site` and `station` with Phase 2, when terrain can use them. Each MINOR and announced (RP-0 §8.4) | As recommended: phased |
| Q4 | Propose RP-4's optional additions: `quality.peer_snr_db`, `failed.heard` and `snr_db`, `probe`? | The first two with Phase 4 (learning is sharper with them); `probe` with Phase 6 | As recommended: phased |
| Q5 | Copernicus GLO-30 (fetched, with its notices) as the elevation source? | Yes; GLO-90 for the first pass; 3DEP and canopy data as a US experiment later | As recommended: Copernicus everywhere |
| Q6 | The default score: the time done 4 times in 5? Deadlines for priority (30 min) and emergency (10 min)? | Yes, as settings | As recommended: T80; 30 and 10 minutes |
| Q7 | Bundle the VOACAP engine in the client (about 9 MB installed, a few MB to download)? | Yes: HF ranking must work with no Internet | As recommended: bundled |
| Q8 | Attempt records local only, with full positions, and an option for squares only? | Yes. A shared outcome data set is a separate question (§20.4) | As recommended: local, full positions |
| Q9 | Attended automatic fallback in Phase 6, within the operator's limits? | Yes, attended only; unattended calling stays out of this design | **Suggest, never call**: after a failure the client proposes the next candidate and why; the operator presses Call. No automatic transmissions at all |
| Q10 | Rewrite W17 and add the new wishes once this is accepted? | Yes, with each wish's compatibility notes, as the wishlist does | As recommended, once the proposal is accepted |

---

## Appendix A. The transaction model's worked example

The script behind §13.3's table: the closed form and the simulation. Standard library only; `python3 txmodel.py` prints both for the three candidates at 2 kB and 50 kB.

```python
"""The transaction model of the gateway-selection proposal, worked through for three candidates.

Closed form: E[T] = K + B * s, with C = t_c + h + (1-p)/p * t_f, K = C + e, s = (1 + lam*C) / r.
Monte Carlo: calls succeed with p; a link breaks after an exponential time (rate lam); data moves at
r and is kept across breaks (RP-3 section 7); each new link pays t_c + h again; the run stops at
done or at the budget. Parameter uncertainty is drawn once per simulated transaction.
"""

import math
import random

T_C, T_F, H, E = 25.0, 40.0, 20.0, 10.0  # connect, failed call, handshake, ending (s)


def closed(p, r, lam, size):
    c = T_C + H + (1 - p) / p * T_F
    return c + E + size * (1 + lam * c) / r


def one(p, r, lam, size, budget, rng):
    t, left, failed = 0.0, float(size), 0
    while t < budget:
        if rng.random() >= p:
            t += T_F
            failed += 1
            continue
        t += T_C + H
        need = left / r
        life = rng.expovariate(lam) if lam > 0 else math.inf
        if life >= need:
            t += need + E
            return t, failed
        t += life
        left -= life * r
    return math.inf, failed


def sim(draw, size, budget=3600.0, n=40000, seed=1):
    rng = random.Random(seed)
    times, fails = [], 0
    for _ in range(n):
        p, r, lam = draw(rng)
        t, f = one(p, r, lam, size, budget, rng)
        times.append(t)
        fails += f
    times.sort()
    q = lambda x: times[int(x * n)]
    mean = sum(min(t, budget) for t in times) / n
    return {
        "p10min": sum(t <= 600 for t in times) / n,
        "p30min": sum(t <= 1800 for t in times) / n,
        "median": q(0.5),
        "t80": q(0.8),
        "mean_capped": mean,
        "failed_calls": fails / n,
    }


A = lambda rng: (0.95, 40.0, 1 / 1800)
B = lambda rng: (0.45, 150.0, 1 / 600)


def C(rng):
    # model only: p ~ Beta(1.6, 0.4) (mean 0.8, worth two attempts); r log-normal, median 100 B/s,
    # sigma 0.7; lambda ~ Gamma with mean 1/900 per second, shape 2
    p = rng.betavariate(1.6, 0.4)
    p = max(p, 0.02)
    r = 100.0 * math.exp(rng.gauss(0, 0.7))
    lam = rng.gammavariate(2.0, (1 / 900) / 2.0)
    return p, r, lam


C_point = lambda rng: (0.8, 100.0, 1 / 900)

for size in (2000, 50000):
    print(f"--- {size} bytes")
    print("closed A", round(closed(0.95, 40, 1 / 1800, size)),
          "B", round(closed(0.45, 150, 1 / 600, size)),
          "C(point)", round(closed(0.8, 100, 1 / 900, size)))
    for name, d in (("A", A), ("B", B), ("C", C), ("C point", C_point)):
        r = sim(d, size)
        print(name, {k: (round(v, 3) if v != math.inf else "inf") for k, v in r.items()})
```

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.1.0 | 2026-11-20 | First draft, for the author's review: the selection problem; position and GPS; the directory and optional RP-5 members; VHF terrain (Copernicus, P.526 §4.5, link budgets); HF (VOACAP both ways through the modem's ladder); hf-predict's crates; attempt records, failure classification and learning; the transaction model and T80; the ranking; the panel; roadmap, validation, risks and ten decisions. Research of 2026-11-20 on Winlink Express, Pat, GPS sources, elevation data and propagation models. |
| 0.2.0 | 2026-11-20 | §21: the author's answers to the ten questions, asked one at a time. Q1 to Q8 and Q10 as recommended; Q9, after a failure the client suggests the next candidate and never calls by itself, so §1, §12.8, §14.6, §17 and §18's Phase 6 no longer have an automatic fallback. |
| 0.2.1 | 2026-11-20 | The passages that left a decision open now give the author's answer (§3, §7.2, §9.1, §11.2, §12.2, §13.5, §15.4, §19.3, §20.2, §20.4); §5.2's table, where 0.2.0 had added §21's answers by mistake, restored; three references to §19 corrected. No change of design. |
| 0.3.0 | 2026-11-20 | Accepted by the author, with §21's answers; Phase 1 begun. The wishlist's W17 points here, and W55 to W58 are its parts. |
| 0.3.1 | 2026-11-20 | §15.2: the attempt record as Phase 1 writes it (member names, the stub line, what later phases fill); the example's times corrected to 2026-11-20. |
