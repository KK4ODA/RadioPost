# RadioPost — working notes for Claude Code

## What this is
RadioPost: a federated email system for amateur radio (gateways anyone runs, post offices groups
run, ordinary email addresses, no central server). The author is KK4ODA. Its first modem is Aether
HF (`KK4ODA/aether-hf`), but RadioPost is modem-independent.

## Layout
- `spec/` — **the protocols, normative**: RP-0 conventions, RP-1 identity, RP-2 messages, RP-3
  session protocol, RP-4 modem interface (RPMI), RP-5 post office API, RP-6 email, RP-7 forwarding
  (a draft). `spec/README.md` is the index. Where a program and a spec disagree, the spec wins.
- `docs/concept/` — the plain-language concept, with schematics.
- `docs/design/` — `00-detailed-design.md`: the map, decisions D1–D33, the programs' parts, the order of
  work, the open questions; `01-conformance.md`: the conformance tester's outline (its tests by
  role, conformance mode, the test adapter, what each phase adds); `02-wishlist.md`: the wishlist
  (every wish for the programs, grouped and prioritized, each checked against the specifications;
  the gaps found in them; what is set aside and why); `03-scanning.md`: gateways that
  scan their channels (W50, gap G21; accepted and built 2026-11-17, its §9 the details). The modem
  scans (Aether's ADR-0129, 0.2.0-beta.162) and the gateway says where: RP-4 1.1.0 (`caps.scan`,
  `scan`, `connect`'s `call_s` with `caps.call_s`) and RP-5 1.1.0 (a channel's `scan_s`). The
  gateway's `Settings.scan` (*Scan these channels*) is scanned at start, partner calls stop and
  restart it, an end it did not ask for stays ended, and `record.json` (the record and the home it
  was registered from) lets it register again when the modem's `scan_s` changes: there is no
  `PUT /gateways/{callsign}`. The client takes `call_s` from its kept list (`Store::scan_s_of`).
  The simulated channel scans with no clock (`SCAN_DWELL_S` 10 s a dial, as the reference); the
  tester's A-06 runs on it and passes over adapters without `caps.scan`; C-13 and G-10 wait for a
  gateway list in the test post office and dials in the test adapter.
- **Code** (Phase 1 on, D27, D28): Rust throughout, in this repository — `core/`, a Cargo
  workspace (the core library, the gateway, the service), `app/` (the Tauri client) and `adapters/`.
  Forms (W32, RP-2 1.1.0 §8): a form travels as its template's fingerprint (8 bytes of SHA-256 of
  the template's CBOR) and its values by field number; `forms/*.json` are the published templates
  (ICS-213 first), `radiopost::forms` reads them (`Template::published`, compiled in), checks values
  (`problem`) and makes the text (`shown`); `rpref/forms.py` and `vectors/forms.json` are its
  reference. RP-2 1.4.0 (D49, `docs/design/06-forms.md`, 2026-11-24) added `position` (`[lat,
  lon]` in ten-millionths of a degree, or with a distance in metres), `datetime`, `multi` (bits),
  `rows` (a field's `columns`), `heading`, and a number's `scale` and `unit`; `forms.json`'s
  `every_type` is a test template that uses them all, never published. The client reads each type
  with `compose::value_from_text` (the command line, and the panel's `form_of` with the template)
  and a position with `position_from_text` (degrees, or a grid square's centre with half its
  diagonal); `/api/form-position` fills a position field from W55 and shows what goes. The client's composer has **Form** (`/api/forms`), `compose --form --field N=VALUE`,
  `forms`; the post office writes a form's text into the email (RP-6 1.1.0 §3.1). A new template
  goes in `forms/` and in `make.py`, never a change to a published one.
  A form prints as its paper form (W53): `forms/*.print.html`, named by the template's
  fingerprint, each field placed by number (`radiopost::forms::layout`); the reader's **Print**
  prints in place by the page's print styles (the app's window prints itself at `/print`, Tauri's
  `print`: macOS's webview ignores `window.print()`), and **Save** writes a file to Downloads under
  a safe name, never over another, marked as downloaded on Windows (`downloads.rs`, `/api/save`).
  `core/radiopost` is the library of RadioPost's objects; `tests/vectors.rs` replays `vectors/`
  against it, and CI runs `cargo fmt`, `cargo clippy -D warnings` (Aether's lint levels) and
  `cargo test --locked` (`core/README.md`). Writing it found two gaps in the vectors: no ID text
  that must be refused, and no credential signed before its key's validity began. The session
  engines (`session::client`, `session::called`) found two in RP-3: a station called cannot tell
  from an offer whom a message is for (§10), and `session-01` left out the `DONE` that ends a
  gateway's turn after `WELCOME` (§5.2, §5.7). The engines are sans-IO: clock, nonces and the
  user's choices come in through their configuration, so the replays are byte for byte.
  `core/rpmi` is RP-4: the lines, the host's side (`host::Carrier` joins one session to one link,
  each `send` a transmission with `ref` `t<n>`), the simulated channel of `rpref/rpmi.py` (the
  loopback adapter) and that channel on TCP ports; its tests replay both RPMI vector files and
  carry session-03 between its engines. Writing it found two gaps, fixed in RP-4 0.4.0: what a
  request missing a member, or with one of the wrong type, gets (`malformed`), and what an adapter
  that is not shared does with a second host (`busy`, closed). RP-4 §9.5's TCP adapter, whose
  links are TLS connections between machines, is not built yet.
  The dial (RP-4 0.8.0 §4.1, W14, W40): `caps.dial`, `dial` and `tune` (refused `busy` while any
  link or call is up on the station), a `dial` event to every host; `StationConfig.dial_hz` and
  `Channel::dial` in the simulated channel (the vectors' modem starts at `modem.dial_hz`). The
  client's `Options.dial_hz` (`--dial`, the panel's **Channel** after **Choose**) tunes for a call
  and back; the gateway tunes to a partner's open window and back, and with a modem that checks
  no rules but has a dial judges it itself (`gateway/src/rules.rs`: the dial and 3 kHz above it
  inside an automatic segment; badge `outside`). The Aether adapter asks `status` before
  `frequency.set`, since another program's call is no event of Aether's.
  The tuner tone and a call's bandwidth (RP-4 1.2.0 §4.3, §1.1; from the 40 m mail of 2026-11-17):
  `caps.tone` and `tone {seconds}` (0 stops; Aether's `tune {duration_s}`); the Aether adapter's
  `caps.bandwidths` is `capabilities.bandwidths_hz`, and a call in a bandwidth not running moves
  the whole station (`status`, `bandwidth.set`, `connect` with `keep_bandwidth`), putting back the
  `host_hz` it kept when the call or link ends or the host leaves. The panel tunes as the operator
  picks a channel (`/api/tune`, `session::tune` over a connection with `hello` alone,
  `Modem::reach`), keys the tone (`/api/tone`), and leaves the radio there after the call
  (`Options::tune_back` false; the command line's `--dial` still goes back); a station call
  carries its **Dial, MHz** and **Bandwidth**. A station call a gateway answers (no key 13 or 15 in
  `WELCOME`) is `Event::NotAStation`, said in plain words, its `ERROR` carrying `NOT_A_STATION`;
  the gateway's monitor logs a caller's `ERROR`. A gateway whose scan ends asks where the radio
  stays (`Modem::stay_asked`: RP-4 says the end before the dial) and registers again with only
  what it listens on (`Store::scan_left`; `record.json` keeps all the operator's channels in
  `record` and what was sent in `sent`).
  `adapters/aether` (`radiopost-aether`, a member of the core workspace) is RP-4 §9.1: each host
  its own WebSocket to Aether's control API and its callsigns a claim there (Aether's ADR-0106);
  `bridge` is the sans-IO mapping, tested row by row against a stand-in and with `rpmi-lines`.
  `tests/aetherd.rs` runs real daemons (`AETHERD=<aetherd> cargo test -p radiopost-aether --test
  aetherd -- --ignored`): session-03 crosses two `aetherd` joined by `[sim]` byte for byte, and two
  hosts share one station. Build an `aetherd` from a worktree of `KK4ODA/aether-hf` (`cargo build
  --release -p aetherd`, which needs `libasound2-dev`). Writing it settled five details of §9.1
  (RP-4 0.4.1). `clippy.toml` is at the repository's root, so every crate finds it. CI's clippy is
  the newest stable (1.99 added `assert_is_empty`): run `rustup update stable` before linting.
  `app/client` (`radiopost-client`, the `radiopost` program, a member of the core workspace) is
  the client: home directory store, composing (NFC, LF), and `session::call`/`answer` over any
  RPMI adapter; `rpmi-loopback` serves the simulated channel for running two clients by hand. A
  modem given as `ws://…` is Aether's control API: the client runs the Aether adapter in-process.
  While the panel runs, `presence.rs` holds one more connection to that adapter with `hello` alone
  (RP-4 0.7.0 §3), which the bridge passes on as Aether's `hello {app}` (its ADR-0114), so Aether's
  header names the app between calls. No callsigns on it: under Aether's ADR-0108 any claim speaks
  for the station. The page tells the panel its modem (`POST /api/modem`); only `ws://` and only a
  shared Aether get the connection, never in conformance mode; retries every 5 s, logged once.
  **Listen for P2P** latches (W15, the 40 m sessions of 2026-11-18): `session::keep_listening`
  keeps one connection, its claims and `listen` on across sessions (Aether ends a claim with its
  connection, so closing it after each session had stopped the answering), opens a failed one
  again, tries a modem that is away every 5 s, and opens the home afresh at each call; the panel's
  `Activity.listen` is the switch and `held` why it cannot listen. `answer` (the command line's
  `listen`) still answers one call. Tested in `tests/listening.rs`.
  **P2P made plain** (D47, RP-4 1.6.0, 2026-11-24): the Aether adapter asks `bandwidth.set {hz:
  widest, listening: true}` while its host listens (Aether 0.2.0-beta.174 holds it per connection,
  below an external program's own, and drops it with the connection; `listen_wide` from
  `status.bandwidth.listening`), and puts back that request, not `host_hz`, after a call. RPMI
  `status` carries the station's `dial_hz`, `answers` and `scanning`, read from Aether's `status`
  (`Event::Status`, `session::Where`). **Listen for P2P** sends the P2P dial typed (`/api/listen
  {dial_hz}`): `tune_to_listen` tunes there unless the station scans; `wait_for_call` asks
  `status` every `WHERE_EVERY` (30 s) and follows `dial` events, and `Listening::Waiting(Where)`
  fills the page's `#listen-at` line. A P2P call to a listed gateway's own callsign is not placed
  (`Store::gateway_named`, `not_a_station`; the panel answers `{not_a_station}` and the page's
  toast offers the base callsign). `GET /api/p2p-chance` predicts the P2P dial at the site of the
  gateway listed under the station's base (`#p2p-chance`).
  Several callsigns (W46, W7; the author's decisions 2026-11-13): each a home of its own, the first
  the home as before and the others in `callsigns/<n>` under it, `chosen` naming the one in use
  (`callsigns.rs`); the panel opens the chosen home at each request and changes it only while idle,
  under the activity's lock; `--as` on the command line; one device key copied to each home added;
  the address list and the log stay in the first home; the open window renews every credential.
  Groups (W2) are `groups.json` beside `addresses.json` (a file of its own, so an older client
  leaves them alone); `contacts.rs` reads vCard and CSV exports for `addresses::import`.
  Separate accounts only: one account under two callsigns would split its mail between homes.
  *Listen* answers calls to any of them (W46, 2026-11-15): `Options::also` holds the other homes,
  every callsign is claimed, and the home whose callsign was called (the link's `local`) carries it.
  `core/testservice` (`radiopost-test-service`, wishlist W43, `docs/user/test-service.md`) is the
  public test service: a test post office (the core's own, in its directory, published nowhere in
  DNS) giving codes for any callsign, RPTEST-1 (the gateway engine, upstream `upstream::served`
  to that post office in-process) and RPTEST-2 (the station engine) answering calls through an
  in-process TCP adapter, and a report per session (`report`: four checks and every PDU decoded)
  on its page. One session at a time. Shipped in the tools archive; the author hosts it.
  `core/issuer` (`radiopost-issue`) makes a post office key and issues credentials for on-air
  tests before Phase 2's post office; the vectors' keys never go on the air.
  `core/tester` (`radiopost-tester`) is the conformance tester's first half: it plays a gateway or
  a station against a client through its own RPMI adapter (`rpmi::tcp::Server::attach`, a host in
  the adapter's process), scripted from RP-3's tables with the core encodings and none of the
  session engines, and drives the program by a driver file (`drivers/radiopost.json`; the
  client's `conformance --adapter` mode). `app/client/tests/conformance.rs` runs C-01 to C-08 in
  CI; C-11 takes five minutes (`--slow`). `tools/tester/plant.py` builds the client with one fault
  a test and shows each test failing. Writing it found that RP-3 §2 never said what a frame over
  `max_pdu` gets (`too-large`, RP-3 0.10.0), and that a station called answers the caller's last
  turn with a `RECEIPT` alone (session-03), so a turn ends on a `RECEIPT` that covers what was
  sent, not only on `DONE` or `REQUEST`. Its `adapter` command runs A-01 to A-04 against any
  adapter (`--at`, a second instance `--far`, `--reach` the far host's name in `connect`), with
  waits long enough for a radio: they pass on the loopback, the TCP adapter and the Aether adapter
  over two `aetherd` joined by `[sim]` (minutes there). Running them over Aether found that a host
  leaving mid-session left the Aether session up, the station busy for everyone until Aether's link
  timeout: RP-4 0.5.0 §1.1 now has a leaving host's call and links ended as `abort` would
  (`Bridge::host_left`), and A-02 checks it.
  `adapters/mercury` (`radiopost-mercury`) is RP-4 §9.6, written from Mercury's `docs/TNC.md`
  and its source (Rhizomatica's, GPL-3.0; the adapter talks to it over TCP only): one host at a
  time, since Mercury gives its control port to the newest program; `LISTEN OFF` held until
  nothing of the host's is up, since Mercury's ends a session unsaid; no `CONNECT` for a second
  after a command a stray `DISCONNECTED` may answer; `delivered` from `BUFFER`. `tests/common` is
  a stand-in for two Mercury modems that logs misuse; `tests/mercury.rs` runs two real ones
  (`MERCURY=<mercury> … --ignored`, Mercury built with `make HAVE_HAMLIB=0`, which needs
  `libasound2-dev libpulse-dev`): A-01 to A-04 pass on Mercury 1.9.23 in about five and a half
  minutes. Its pipes carry audio at the air's pace, a 20 ms block at a time, silence when nobody
  sends, and drop what the receiver does not take, as Mercury's own channel bridge does: passed
  on at once, a CALL is answered while the caller is still keyed, and no call connects. Writing
  the adapter found that Mercury does not identify the station: the called station's callsign
  goes on the air only as a CRC. So the adapter sends `CQFRAME <callsign>` after each session
  (the author's decision, 2026-11-12): the station called first, the caller 0.7 s after it hears
  it (a frame decodes before its sender unkeys, found on the real modem; RP-4 0.6.6); a `connect`
  waits for it, so its `ok` comes at once. The stand-in keeps the IDs it heard, decodes a CQ frame
  before its sender unkeys, and logs two transmissions at once.
  `adapters/vara` (`radiopost-vara`, `--fm` for VARA FM) is RP-4 §9.2: the same bridge and server
  speaking VARA's published commands (`bridge::Dialect`, `serve::listen_speaking`). `hello` asks
  `CHAT OFF` (`BUSYDETECT` is Mercury's alone); callsigns keep to VARA's published grammar
  (`vara_fault`); no `delivered` (VARA's `BUFFER` counts what is not yet sent); no `CQFRAME`.
  Its tests run on Mercury's stand-in in its VARA mode (`Air::vara`, reached by `#[path]`). VARA
  is never run: only its published commands are used.
  `adapters/packet` (`radiopost-packet`) is RP-4 §9.4 on a TNC's AGWPE port (default 8000; Dire
  Wolf's `server.c` read for what the API leaves open): `frame` (the 36-byte header), `bridge`
  (callsigns registered with `X` only while the host listens, or for a call's own callsign; `D`
  256 bytes at most, 7 ahead, paced by `Y` every 0.5 s; `disconnect` sends `d` once `Y` is 0, since
  AGWPE's `d` drops what the TNC holds; no `delivered`), `serve`. One host at a time: the shared
  lobby is `rpmi::lobby` (Mercury and VARA use it too). `tests/common` is a stand-in for two TNCs.
  `adapters/ardop` (`radiopost-ardop`) is RP-4 §9.3, checked against ardopcf's *TCP Host Interface
  Commands* and source (MIT; command port 8515 by default, as Aether's): replies are echoes of the
  command (`MYCALL now …`) or `FAULT`, matched in order; `MYAUX -` clears the aux list by a
  `FAULT`; data blocks have a 2-byte big-endian length (and a 3-letter tag from the TNC); `ABORT`
  says nothing more, so the bridge reports the end. Served by Mercury's `serve::listen_tnc`, generic
  over `TncBridge` (`host_line`, `modem_line`, `modem_data`, …) with a `Kind` naming the modem.
  `adapters/tcp` (`radiopost-tcp`) is RP-4 §9.5: links to other TCP adapters opened with one line
  each way (`RPTCP 1 <as> <called>`, `OK <callsign>` or `NO <code>`), TLS on the Internet with
  rustls and ring (a self-signed certificate made on first use, pinned by SHA-256 fingerprint, or
  the web's roots), plain TCP with `--plain` on an amateur mesh, which is on the air (RP-4 0.5.0:
  §9.5 and the design had put TLS on AREDN). One host, one link; for an Internet rehearsal before
  the air (`docs/user/on-air-tests.md` §3).
  `core/postoffice` (`radiopost-postoffice`, program `radiopost-po`) is Phase 2's post office: a
  port of `rpref/po.py` that answers every exchange of `api-exchanges`, `api-account` and `direct`
  exactly (its randomness comes through `Draw`, so the vectors' `stream` replays tokens and session
  IDs; each vector's setup in `tests/vectors.rs` mirrors `make.py`'s), its signed documents
  (`radiopost::jws`, replayed against `jws.json`), and the service: plain HTTP behind an HTTPS
  front, an operator's port on loopback (`code`, `deliver`, `revoke`, `outgoing`, `status`), state
  in `state.json` written whole after each request. Mail it accepts for other domains waits in
  `outgoing` for RP-6's email out. `radiopost::base64url` is RP-0 §4.2's encoding.
  Other post offices (RP-1 §7.5, D38): `postoffice/src/known.rs` fetches each post office the
  operators accepted (its DNS records, by `radiopost_api::records_signed` with DNSSEC's `AD`; its
  info document and revocation list) at start and every six hours; `assemble` checks them with no
  I/O, and `PostOffice::trust` lays what was fetched over the keys typed in `config.json`'s `known`
  (`know --public`, still the tester's way). Until then other post offices' revocation lists were
  never read. Domains met at the door, in mail sent out, on a refused credential
  (`PostOffice::take_met`) and in known post offices' lists are looked up on its thread and kept
  as `seen` when `radiopost::dns::meets` says so (replayed against `dns.json`'s `office_cases`);
  the desk's *Post offices* tab and `radiopost-po post-offices` accept, block and forget them, and
  a settings change rebuilds the trust at once. `tests/known.rs` runs two post offices in one
  process. Gateway lists across post offices (RP-5 §4.4, D36): `radiopost::gateway_list` is the
  rule with no I/O (`entries` of a `/post-offices`, `by_api`, `verify_list`, `merge`), replayed
  against `gateway-lists.json` (`rpref/po.py`'s `gateway_list`); the client's `online::gateways`
  fetches the home's list and each known post office's, keeps the last checked list of each in
  `gateway_list.lists`, and tags each record with its `post_office`; `tests/gateway_lists.rs`
  runs five post offices. Copies placed ahead across post offices (RP-5 0.16.0 §5.8, RP-7 0.9.0,
  D37), the post office's side: `PostOffice::record_of` finds a usual gateway's record in its own
  registry or in `State::listed` (each known post office's checked list, fetched by
  `known::refresh_list` with the keys held for it, fetched or typed) through `gateway_list::merge`;
  `/copies` answers `placing`, serves a gateway's items only to a credential from the post office
  that listed it (a stray poll gets `c_0`, uncounted), and takes acknowledgements (`acked`); a
  poll answered `placing` false is not counted for RP-7 §4.1. Replayed against `api-exchanges`'s
  case of copies at N1CALL-10 (its `setup`); `forwarding.json` now takes N1CALL-10's record from
  hampost.org's list, as its hampost.org credential requires, and has a `placing` story;
  `tests/copies.rs` runs two post offices. The gateway's side (which post offices it polls) waits
  for Phase 3's copies (W27): gateways do not poll yet.
  RP-6 is `postoffice/src/mail.rs` (the port of `rpref/mail.py`, MIME by Stalwart's `mail-parser`
  0.11: the newest needs Rust 1.98, over the workspace's 1.85) and `email.rs` (the door as an MTA
  hook, read in Stalwart's shape and the MTA Hooks draft's, `ietf.org/archive/id/draft-degennaro-mta-hooks-01.txt`;
  outgoing mail by `lettre`, which fails a whole transaction on one refused recipient, so a refused
  group is tried one by one). `mail-parser` strips a `Message-ID`'s angle brackets; the post office
  keeps them. `docs/user/post-office.md` is the operator's guide, written before the server.
  Station-to-station mail moves to `sent` and its RP-5 §6.6 report is kept (D30), from the proofs
  the engines now keep (`Client::proofs`, `Called::proofs`); the replay test matches session-03
  byte for byte and `direct.json`'s two reports. The engines report their own refusals
  (`Event::Refused`), found by running two clients that trusted no post office.
  `core/gateway` (`radiopost-gateway`) is Phase 2's gateway, online: the engine is
  `radiopost::session::gateway` (sans-IO but for an `Upstream` it calls in line; `tests/session01.rs`
  has it write session-01's and session-02's gateway sides byte for byte against the real post
  office in-process), `upstream::Http` logs in with the gateway's credential (ureq, again on a 401),
  `station` answers calls on an RPMI modem and finds post offices in `gateway.json` or by DNS over
  HTTPS (RP-1 §7.1, `radiopost::dns`), `operator::register` registers it from the operator's client
  home (RP-5 §6.4). Registration and its withdrawal (W54, RP-5 1.2.0 §6.4 and §7.7; a tester's
  gateway registered with radiopost.org by mistake, 2026-11-18): the operator chooses the client's
  account (`operator::accounts` reads the client's W46 layout, `account` finds one; Setup's
  **Account**, `register --account`); one post office at a time, so registering at another domain
  while registered is refused; `record.json` keeps the account, home and base URL, and after
  `operator::unregister` (Setup's **Unregister**, `unregister`; refused while records wait to be
  reported) only the record, which fills the form; a `404` reads the list (unchecked) before it
  forgets. The post office's `PostOffice::withdraw_gateway` drops the record, revokes every
  gateway credential issued for the callsign and drops its items and forward items, for the
  record's operator (`DELETE /gateways/{c}`), its operators (`DELETE /office/gateways/{c}`, the
  desk's **Gateways** tab over the desk's own `GET /office/gateways`, `registrations`) and the
  operator's port (`radiopost-po gateways`, `unregister --callsign`). `tests/online.rs` is the whole path: client → loopback adapter → gateway program
  → post office service over HTTP. A post office's base URL is what the gateway signs at login and
  registration, so a test's post office takes the URL of the port it listens on.
  The tester's Phase 2 roles: `gateway` (it plays the caller through the test adapter, with
  `office`'s test post office behind — the core's own post office on the test key — and a test DNS
  zone served as DNS over HTTPS; conformance mode is *given* the test post office's URL, since it
  is plain HTTP and RP-1 §7.1 ignores such a record) and `postoffice` (it starts the post office
  from its driver for each test and plays users, a gateway, a `hampost.org` user and the mail
  server: the door called in Stalwart's shape with verdicts in `serverHeaders`, `mailserver`'s
  stand-in SMTP). Writing them found the gateway's session loop handing the engine the last event
  again at each quiet read (fixed: `event.take()`), and enrollment codes used at a refused attempt
  (RP-5 0.12.1). The loopback channel drops what is in flight when a link breaks: a test that
  breaks a link after sending waits for it to cross.
  Setting up the pilot's server (Stalwart 1.1, RP-6 0.5.0) found three things about the door:
  1.1 gives each hook header value with its line ending; it gives hooks no verdicts, so the door
  checks SPF, DKIM and DMARC itself (`postoffice/src/auth.rs`, `mail-auth` 0.13, the last series
  on mail-parser 0.11; its tests fill the lookup cache and build with `mail-auth`'s `test`
  feature, which makes any other name NXDOMAIN and a name with `_dns_error.` fail for now); and
  the door had refused `postmaster@`. Stalwart's `authenticated_as` is the account's name, the
  full address there though the service signs in as `radiopost`: Must Match Sender's rule names
  both. Verdicts are never read from the message's own
  `Authentication-Results`. `radiopost-po check` (`postoffice/src/check.rs`) looks at a post
  office from outside: DNS (`radiopost_api::query`, any record type by DNS over HTTPS), reverse
  DNS, `info` over HTTPS, the door with its token, and the submission port signed in to and
  asked for `MAIL FROM` but never sent to; its judgements are functions of what was found. The
  door logs each decision with addresses and a reason (`mail::rcpt_why`, `data_why`), never
  the message.
  The backup mail server (W38, `backup_mx.rs`): `radiopost-po backup-door` on a second machine
  judges with `email::first_refusal` (shared with the door) over a snapshot fetched from
  `GET /door/snapshot` (a token made by `backup-mx --add`, kept as its hash) and seals its
  verdicts in `X-RadioPost-Door` (HMAC under a key derived from the token, over the envelope
  sender, the body and the time); the door takes a seal that checks in place of its own checks.
  Held attachments by radio (RP-3 §5.6 `files`, RP-5 §5.3, RP-6 §2.5): the client asks by serial
  and `ref` (`radiopost attachments`, kept in `state.json` until a gateway's `DONE` answers the
  `REQUEST`), the gateway passes them to the post office and says how it went in a `NOTICE`, and
  the post office puts them in new messages from `postmaster@` for the next call. A serial is
  asked for whether its message was picked up or not: the vector's three sessions are what catch
  the narrower rule. `gateway/tests/online.rs` carries it end to end, from an email at the door.
  RadioPost Gateway (`app/gateway`, the author's choice: its own app and installer): the gateway
  answers on a thread of its own (`runner`), its state in `monitor` (sessions in `sessions.json`,
  the log, the post offices' logins every ten minutes), its panel in `panel.rs` with the page in
  `app/gateway/ui` (port 8621; `radiopost-gateway panel` serves it headless), and the window in
  `app/gateway/src-tauri` (`radiopost-gateway-app`, its own lockfile, CI's `app · gateway` job).
  `station::run` waits for a modem that is away and stops when asked; `serve` is `run` with no one
  to ask. Several modems (W26): `Settings.more_modems` (Setup's **More modems**, `modem --add`), each
  opened by `Others` beside the first and read between sessions with a short wait; one session at
  a time, the other modems held (`listen` off) while one carries a session or a partner call, and a
  link one of them brought up meanwhile aborted at release (`Modem::hold`, `release`). Partner calls
  go on the first modem. `tests/several_modems.rs` taps two loopback channels' adapters to see the
  `listen`s. The registration's channels name their modem (the panel's `MODEMS`). A gateway's credential names the gateway as its operator (RP-5 §6.4) and lasts 90 days:
  registering again replaces it. `tests/common` holds the gateway tests' post office and clients.
  Callers refused (W24): `refused.rs`, `refused.json` beside the settings and read at each call
  (the settings are held while it answers; a refusal is wanted at once), handed to the engine as
  `GatewayConfig::refused`; `session::gateway::refused_caller` is the rule (an address whatever its
  case, a callsign without an SSID every SSID of it), checked at `HELLO` and again on the
  credential, answered `refused-by-policy` with a text, and `Note::Refused` marks the session.
  Local delivery while cut off (W31, RP-7 §4.4): `Store::copies_seen` (when a copy last arrived
  for each address) names the local users (`session::gateway::is_local`, `local_recipients`, a
  copy in the last 30 days); `offline.rs` keeps a message naming one in `Store::local`, lists it
  with serial 0 and the flag `local` after the copies of each priority, serves it by content ID
  (`Upstream::fetch_local`) and keeps its pickup in the user's record (`picked_up_local`,
  `Record::local_pickups`, reported only when there are some); what was kept goes once the
  sender's record is reported. The engines: the gateway's reads `REQUEST` key 4; the client's
  `Chooser` returns content IDs (`client::by_serials` for the old callers) and asks for local
  mail under key 4. `session-05` replays both sides (`radiopost/tests/sessions.rs`,
  `gateway/tests/session05.rs`, the cut-off `NOTICE` text given by `Offline::with_notice`); C-10,
  G-06 and P-05's local pickups; `plant.py` takes a second fault for a test as `"P-05 local"`.
  Forwarding between gateways (W30 stage 3, RP-7 §2 to §4.3): `radiopost::forwarding` has the
  records (`Proof`, `record_id`, `check_proof`) and placements (`Placed`, `check_placement`,
  replayed against `forwarding.json`'s cases). The caller is `Client::forwarding(Forwarding)`
  (role 1, `PROOF`s before the messages, a credential whole once then by hash, `Event::Placed`
  for a forward item whose placement checks, `Event::Records` for `RECEIPT` key 4); the partner is
  `Gateway::with_partner(Partners)`, whose `Upstream` is the partner's own store (`placement`,
  `trust`, `take_records`; the engine checks each `PROOF` and each message against the records
  taken). A record one of whose messages failed is not taken, and its other messages are listed
  failed with that code (RP-7 0.10.1), or the caller would wait for their receipt. In the gateway
  program: `forward.rs` (`Partner`, `outbox`, `Outcome::apply`), `Settings.partners` (a gateway
  answers forwarding calls from its partners only), `Store.partners` (cursor, the credentials a
  partner holds, its last `online`), `Record.path` (reported with it; RP-5 §5.6 has no `cursor`,
  so a report sends none), forwarding serials given per cut-off gateway as forward items arrive;
  `station::forward` runs between sessions when the monitor asks (`request_forward`, the panel's
  *Forward now*), `forward_once` for the command line. `session-04` replays byte for byte through
  the engines (`radiopost/tests/sessions.rs`) and through each gateway's store
  (`gateway/tests/session04.rs`, N1CALL-10's reports equal to `forwarding.json`'s);
  `gateway/tests/forwarding.rs` is a cut-off gateway (restarted with an address that does not
  answer, since both gateways must sign one base URL) forwarding to its online partner, which
  reports. P-05 checks a forwarded session's path (`"P-05 path"`). Windows, the published routes
  and F-01 to F-06 are stage 5.
  Mail for a cut-off area (W30 stage 4, RP-7 §4.1 to §4.3): the post office keeps `State.routes`
  (`RadioRoute`, set on the operator's port by `radiopost-po route --via --users-at` and listed by
  `routes`), `State.cut` and `State.forwarded`. `sweep()` runs before every request and every
  arrival: a gateway counted cut off for the first time (polled with `placing` true, not for
  `CUT_OFF_S`) has the copies it never acknowledged placed as forward items at each route's
  `via`, signed with `forwarding::Placed::sign`; `deliver` places one for each arrival while it
  stays cut off; its next poll drops them at the `via`. Forwarding only the unacknowledged copies
  is RP-7 0.11.0 (the reference had forwarded only arrivals, leaving the 15 minutes before the
  cut behind); `forwarding.json`'s placing story shows it. `postoffice/tests/vectors.rs` replays
  `forwarding.json`'s post office and placing stories and `local-delivery.json`'s exactly.
  `gateway/tests/forwarding.rs`'s second test carries such mail end to end. Its cut-off gateway
  never reaches the post office after registering: restarting a gateway in one process leaves its
  old poller's last long poll in flight, which the post office rightly takes as the gateway being
  back. P-06's forward items run with `--slow` (a real 15-minute wait); the post office's driver
  has `route`. A `via` gateway polls when it holds copies or publishes windows (RP-5 §5.8,
  `Settings::polls`).
  Windows, routes and calls by itself (W30 stage 5, RP-7 §5, §6): `radiopost::forwarding` has
  `Window` (`next`, `is_open`), `segments(country)` (RP-7 §5.1's US table), `window_allowed`,
  `occupied` (Aether's audio span, or an FM channel) and `records_to` (§6.2's choice), replayed
  against `forwarding.json`'s window and partner cases. The gateway keeps its own windows in
  `Settings.windows` (`WindowSettings::refused` says why one may not be published; the `window`
  command and the panel's `/api/windows` refuse those) and registers the allowed ones in its
  record's `forwarding`. Its partners' windows come from the gateway lists of the post offices
  it trusts (`upkeep::refresh_lists`, every six hours while it has partners, kept in
  `Store.lists`, read by `Store::record_of`). The answering loop's `calls_between_sessions` runs
  `forward::Caller::due` every 5 s: only while cut off (`Station::cut_off`, a login at its own
  post office, kept 60 s), in an open window, three tries a minute apart, never while the modem
  says busy; `records_to` decides whether the call forwards the queue or only collects mail. The
  post office publishes its routes at `GET /routes`. F-01 to F-06 are in
  `core/tester/src/forwarding.rs`: the tester plays the partners, the test adapter reads the calls
  a program places (`TestAdapter::calls_to`, from `rpmi::channel::Channel::placed`) and makes the
  channel busy with two bystander stations (`TestAdapter::busy`); the gateway's driver has
  `partner`, `window`, `register` and `forward`. F-01 (7 minutes) and F-05's calls run with
  `--slow`. A test that stops `serve` right after a session waits for it to be stored (`stop`).
  `plant.py` touches every file a fault names in each copy: copied files keep their times, and
  cargo had reused an earlier fault's library. The first trial on the air has its guide,
  `docs/user/on-air-forwarding.md`.
  The client's gateway side: `online::gateways` fetches RP-5 §4.4's list, its JWS checked with the
  keys the home trusts for the user's domain (and those its signed `info` adds, RP-1 §7.2), and
  keeps it in `state.json` for when there is no Internet; `grid.rs` ranks it by distance from the
  operator's grid square. A gateway call keeps what was listed (`Store::listed`, less what came) as
  the *Waiting* folder, and serials marked there (`fetch_next`) are fetched whatever `--fetch`
  says. `gateway/tests/online.rs` drives it all through the client's panel against the real
  gateway and post office.
  Updates (D34): `core/update` (`radiopost-update`) reads the update channel's manifests
  (`client.json`, `gateway.json`, `tools.json` on the rolling `channel-beta` release, Tauri's
  updater format), compares versions (semver), checks minisign signatures as Tauri's updater does
  with the project's key (`core/update/updater.pub`; the private half is the release's secret
  `TAURI_SIGNING_PRIVATE_KEY`, never in the repository) and replaces the command-line programs
  (`radiopost-po update` and the others' `update`); `Watch` is what a panel shows, and the Tauri
  shells give it an installer (`tauri-plugin-updater`, the key and endpoints set in code). A
  `Watch` asks at start and every six hours (`EVERY`), and `check_now` when the user presses
  **Check for updates** (Setup, **Updates**; `POST /api/update/check` in both panels), on a thread
  of its own since the client's panel answers one request at a time; `status()` carries
  `checking`. Each window shows its version beside its name (`#version`, from `update.current`);
  `screens.mjs` reads the version from `core/Cargo.toml`. Only shots that show the top bar or a
  changed card are retaken: the committed shots render differently on another machine, and a
  full rerun changes every PNG. Its
  tests use a throwaway key whose private half was discarded. **One version**: `tools/release.py
  bump X` writes it in every Cargo manifest, lockfile and the guide's download link (the Tauri
  configurations take Cargo's); `check` runs in CI and in the release's preflight, which also
  refuses a tag that is not `v` + the version. A release: bump, commit, push, then the Release
  workflow with that tag; `tools/update_manifests.py` writes the manifests from the signed files.
  No MSI: it takes no pre-release version, and NSIS's setup.exe updates in place.
  Renewal (W8): `radiopost::credential::RENEW_WITHIN` (30 days) and `renewal_due` are the one rule.
  The client's `renewal` module and its panel renew over the Internet when the panel starts and
  every hour, between sessions (activity `renewing`), and a gateway call asks `renew` in `HELLO`
  (`Options::renew_within`); the gateway's answering loop runs `station::Renewal` (a try every ten
  minutes on a thread of its own) and takes a renewed credential up before its next session
  (`Station::reload_credential`). Tests give them a hundred days' notice; with every try at once,
  a credential just renewed is due again.
  The operator's desk (D35, RP-5 0.14.0 §6.7 and §7, wishlist W49; 2026-11-12): applications
  and the operator endpoints are `core/postoffice/src/operator.rs`, replayed against
  `vectors/api-office.json` (the reference's `po.py` takes both; `office_post_office` sets the
  operators, which the older stories leave empty so their logins carry no `office`). The service
  adds its own under `/office/` in `desk.rs`, around `PostOffice::handle` so the replay stays exact:
  status members, outgoing, names, settings (applications, lookups, mailboxes, DKIM selectors),
  lookups, health, jobs, backups. `licences.rs` builds a sorted copy of the FCC's weekly
  `l_amat.zip` (HD, EN, AM joined; the real file of 2026-11-08: 1,606,615 callsigns in 21 s,
  170 MB, no email addresses in it) found by binary search on disk; `qrz.rs` is QRZ's XML
  interface; `evidence.rs` judges good/warn/bad; `lookups.rs` looks each new application up on a
  worker thread. Two red flags came from a probe of radiopost.org on 2026-11-18 ("Probe Name"
  asking for W1AW, written to at `probe@example.com`): a club callsign's trustee is looked up
  too, and an applicant who is neither the club (by its licence's name) nor the trustee is `bad`;
  an application's contact at an RFC 2606/6761 reserved domain is `bad`. Both are evidence, not
  refusals at `POST /applications`, so RP-5 and its vectors (which write to `example.*`) are
  untouched. `mailboxes.rs` makes the mailbox (Stalwart's JMAP `x:Domain/get` and
  `x:Account/set`; or a named program) wherever an account begins
  (`aftercare.rs`): at an approval (the welcome brings its password), with a code for an address
  that is no account yet (`/office/codes` and the operator's `/code` answer `mailbox` with the
  password, shown once), and at a postcard's enrollment (`after_enrollment`: the welcome then).
  A password never goes on the air and is never kept. `mailboxes::addresses` lists Stalwart's
  (`x:Account/get`): the desk's accounts gain `mailbox`, Health's `accounts_line` fails for an
  account without one, and **Make mailbox** (`POST /office/mailbox`, `radiopost-po mailbox`)
  makes it. Until 2026-11-18 a code made no mailbox, so Stalwart refused every email from
  outside for such an account (*550 5.1.1 Mailbox does not exist*, before the door is asked).
  **New password** (`POST /office/password`, the operator's `/password`, `radiopost-po
  password`; `mailboxes::set_password`) is for a user who lost theirs, since Stalwart has no
  reset: the account found by `emailAddress`, its credentials read, and only the `Password`'s
  `secret` changed by a JSON pointer (`credentials/<place>/secret`), so app passwords, passkeys
  and the second factor stay; a `command` way is refused. Stalwart's lists go over JMAP as
  objects keyed `"0"`, `"1"`, … and an array is refused (*invalidPatch, Invalid value for object
  property*; its `crates/registry/src/types/list.rs`, read 2026-11-18): up to 0.1.0-beta.28
  `make` sent `credentials` as an array, so every mailbox it made was refused. The API key needs
  `sysAccountUpdate` for it. The stand-in in `tests/mailboxes.rs` refuses an array and applies
  pointer updates as Stalwart does.
  `notices.rs` and `aftercare.rs` send the emails. One mailbox's second part (W36, RP-6 0.8.0 §2.6): `send_on` and
  `picked_up(…, by_radio)` queue `State::to_mailboxes` (`MailboxTask`), and `mirror.rs`, run with
  `seen.rs` every minute, imports §3.1's email into the sender's `sent`-role mailbox (uploaded to
  the session's upload path on the host the service reaches Stalwart at) and sets `$seen` on what a
  `Message-ID` finds, waiting for mail that reaches the mail server late, giving up after two days;
  tried against a stand-in only (`tests/email_service.rs`'s `mail_server`). DKIM rotation by
  hand (`dkim.rs`, the desk's `/office/dkim`, the server guide's chapter 12): Stalwart rotates only with automatic
  DNS, so the post office makes the keys (RSA 2048 by `rsa`, Ed25519 as RFC 8410's PKCS #8, both
  already in the tree through `mail-auth`; `num-bigint-dig` is optimized in debug builds so a
  test's RSA key takes milliseconds), gives them to Stalwart `pending` (`x:DkimSignature/set`),
  switches them `active` and the old `retiring` only once every record is in DNS, and `retired`
  a week later once the old records are gone; a Stalwart that makes a new key active at once has
  it destroyed again. `dkim-rotation.json` holds a rotation under way; Health reads the keys' age
  from the selectors' dates. Tried against a stand-in only (`tests/dkim.rs`). `health.rs` checks hourly and emails on change (an amber
  `Warn` sends nothing); `backups.rs` seals with ring's PBKDF2 and ChaCha20-Poly1305, and backs up
  Stalwart's `/var/lib/stalwart` and `/etc/stalwart` with Stalwart stopped (`mail_backup`, the jobs
  runner's nightly `mail-backup`; `restore_mail`; tests use a stand-in `MailServer`), and seals
  a backup to an operator's computer (`/office/backup-keys`, `seal_for`; `radiopost_api::sealing`:
  X25519, HKDF-SHA256, ChaCha20-Poly1305), which the client's `backups.rs` fetches by itself while
  its panel runs (W38); a backup is new by name, time and size, since one made again the same day
  keeps its name. `radiopost_api::Http` reads answers up to `ANSWER_LIMIT` (10 MB) unless told
  more (`reading_up_to`): ureq's own limit had made a larger download fail silently. `jobs.rs` is the fixed table a root-owned
  systemd path unit runs from `requests/new/`; `install.rs` plans a Debian set-up against a
  `Probe` (tests plan a fresh and a set-up server). The desk goes by callsign, so nobody becomes an
  operator by applying, and a code naming an operator's callsign for anyone but the asking operator
  needs `operates` (RP-5 0.15.0 §7.1; the desk's red box). The API answers each request on a thread
  of its own: the health check fetches the post office's own info document from it. The client's
  side: `online::apply`, `check_application`, `office` (a passthrough for `/office/` paths, its
  token kept by the panel), the panel's `/api/office`, `/api/office-backup` and `/api/terminal`
  (`terminal.rs`: the system's own terminal and ssh, the target checked to be user@host), and
  in `app/ui` the Post office view and Setup's *Apply for an account*. LoTW proof (W10) is not
  built: it needs the author's TQSL `.p12`, ARRL's CA certificates and a decision on legacy
  RSA-1024/SHA-1.
  `core/api` (`radiopost-api`) is RP-5 over HTTPS (login, calls, enrollment, DNS over HTTPS), shared
  by the gateway and the client; the client's `online` module uses it (enroll, renew, settings —
  `PUT` replaces them whole, so the client merges —, accept list, `report`). `app/src-tauri` has its
  own lockfile: a new dependency of the client changes it (`cargo metadata` there refreshes it).
  Alerts (W52, RP-8 Draft, D42; the author's ask of 2026-11-18) and sounds (W51):
  `radiopost::alert` is RP-8 sans clock (a circle's key by PBKDF2-HMAC-SHA256, 600 000
  iterations, its ID and check, the frame and its 8-byte HMAC tag, `hear` in RP-8 §4's order,
  `answer`); HMAC and PBKDF2 are hand-built on `sha2` in `crypto` (RFC 4231 and RFC 7914 tests),
  and `[profile.dev.package.sha2]` is optimized so tests that make a key take a second, not
  ten. `rpref/alert.py` and `vectors/alerts.json` are its reference. The client's `alerts` keeps
  `circles.json` (keys, never keywords), `alerts.json` (the log) and `notify.json` (the page's
  sound and level settings) in the first home; every connection that holds the modem hands it
  broadcasts (`presence::Carrier::heard`): the connection between calls (`presence`, which now also
  holds a shared non-Aether adapter while circles are kept, and lets go of one that is not
  shared), and a station listening (`session::wait_for_call`, which also sends what waits). To
  send between calls `presence` takes the station from the panel (activity `alerting`, the
  carrier's `gate`) and opens a second, short connection that claims the chosen callsign and
  broadcasts; the listening one stays open (closing it to drop the claim lost a `heard` that came
  in the gap, found by CI on Linux).
  **Dials (D46, RP-8 0.2.0, RP-4 1.5.0; Aether's ADR-0129 §14, §15):** an alert goes on a dial
  (`alerts::Sending`, `Alerts::send_on`): the page offers the listed gateway channels of the base of
  **To**, ranked by `hf.chance`, then the circle's watch dial (`Kept::watch_hz`), then where the
  radio is; the connection that sends it `tune`s first (`Due::tune`). When the gateway scans that
  dial, copies go on for its `scan_s` (`Store::outlast_s`; for the whole circle the longest of any
  listed gateway on the dial), stopped by a `heard`. A broadcast heard carries its `dial_hz`
  (`Alerts::heard_on`) and every answer names it (`broadcast_on`); a heard-again alert is answered
  unless an answer to it is still to go (`answer_pending`). `presence` keeps the radio's dial
  (`Presence::dial`); the page warns when it, or the own gateway's scanned dials
  (`Store::scanned_dials_of`), miss a circle's watch dial.
  The panel's `/api/alerts`, `/api/circles`, `/api/notify`; the page plays synthesized sounds by
  Web Audio on the output chosen (`setSinkId`: the radio's sound card would put them on the air),
  and decides mail's level from `notify.json`; the Tauri shell brings the window forward when
  `Panel::alarming` turns true. `tests/alerts.rs` runs two clients' connections over a shared
  loopback. On Windows `online.rs`'s renewal test fails: a closed local port takes ~2 s to refuse
  there, past its 1.5 s; CI is Linux.
  `app/ui` is the client's panel (no build step), served with a JSON API by `radiopost panel`
  (`panel.rs`: loopback only, the `Host` checked, changes need `X-RadioPost: 1`; a session on its
  own thread, the home unchanged by the panel while it runs). An outbox message is edited in place
  (2026-11-18): `GET /api/edit` fills the composer, `/api/compose` with `edit` saves it through
  `Store::replace` (the old one's serial and place, the old message gone); `Store::edit_refusal`
  refuses one queued at a gateway or sent, `Panel::busy_editing` any while a session runs or the
  station listens. Nothing on the air changes: the edited message is a new content ID. `app/src-tauri` (`radiopost-app`, its
  own package and lockfile, outside the core workspace; CI's `app · desktop` job) is the Tauri
  window around it: the panel's server in-process on 8620 or a free port, the window on it, no IPC.
  Write pops out (2026-11-18): at 720 px and under the panel stacks in one column (`STACKED`), and
  Write, Reply, Forward and writing to an address open the composer in a window of its own, the
  same page with `?compose=` (`ALONE`: `startAlone`, `popOut`; it tells the main window what it
  put in the outbox over `BroadcastChannel("radiopost")`). In a browser a pop-up; in the shell,
  `on_new_window` makes it a window `compose-<n>` (`composer_window`), which closes and comes
  forward when its page goes to `/closed` or `/front` (a page's `close()` and `focus()` do not
  work there on every system), and which the window-state plugin keeps under `compose`, its first
  restore skipped and done from a thread (`placed`): restored inside `window.open`, the program hung.
  It needs `libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libxdo-dev
  libayatana-appindicator3-dev`; `xvfb-run` and ImageMagick's `import` show the real window here.
  Its icons come from `tools/figures/icons.py` (`--check` compares pixels, not zlib's bytes).
  Check the panel in headless Chromium (Playwright, `NODE_PATH=$(npm root -g)`); CI checks the
  script parses as a module.
- `tools/map/` — the client's world map (the gateway selection design §16.2): `world.py` writes
  `app/ui/world.js` (Natural Earth's 1:110m land as world-atlas 2.0.2 gives it, one SVG path, rings
  across the antimeridian drawn twice, the ISC notice inside; `--check`), and `check.mjs` (CI)
  holds `app/ui/map.js`'s sun, night side, great circles and land to cases worked by hand.
- `tools/figures/` — `style.py` (the shared drawing kit), `concept.py`, `design.py`, `icons.py`, `manual.py`, `server.py`. Standard
  library only; `ruff.toml` holds the lint settings.
- `vectors/` — the test vectors (`vectors/README.md`), **generated** by `tools/vectors/make.py`
  from `tools/vectors/rpref/` (deterministic CBOR, Ed25519 after RFC 8032, RadioPost's objects,
  RPMI adapters on a simulated channel in `rpmi.py`, JWS in `jws.py`, a post office in `po.py`,
  email in and out in `mail.py`, forwarding in `fwd.py`; standard library only, self-tested against RFC 8032 §7.1 and
  RFC 8949 Appendix A at every run).
- `docs/manual/` — **the user's manual** for the client, its post office desk and RadioPost Gateway (`manual.md`, its
  schematics in `fig/` from `tools/figures/manual.py`, its screenshots in `screens/` from
  `node tools/manual/screens.mjs`: the real panels in headless Chromium, their API answered with
  made-up data; its `readme-…` shots are the README's pictures, in `docs/images/`, since
  `build.py check` refuses a picture in the manual's folders that the manual does not use). `tools/manual/build.py` writes its contents (`toc`), checks it (`check`, CI) and
  prints it (`html`, `pdf` through Chrome; the Release workflow attaches the PDF). Its header's
  **Version** and change log follow `check_docs.py`'s rules; **Describes** is a `tools/release.py`
  site. Aether's manual (`KK4ODA/aether-hf`, `docs/manual/manual.md`) is cross-referenced, not
  repeated. **The server guide**, `docs/user/post-office.md` (2026-11-15, the author: in the
  manual's vein), is the second book `build.py` knows (`server`): step by step around
  `radiopost-po install`, its schematics in `docs/user/fig/` from `tools/figures/server.py`, the
  manual's desk screenshots, its own Version, change log and Describes; the setup by hand is its
  chapter 17. The post office's messages name the desk's place (*Settings, Mailboxes*), not a
  section number.
- `tools/check_docs.py` — headers against change logs and the index, relative links, `RP-n §x.y`
  references, SPDX lines. `.github/workflows/check.yml` runs it with ruff (pinned 0.16.9: preview
  rules), `make.py --check` and the figure generators (no diff), on Python 3.12 and 3.13, for
  pushes to `main` and pull requests. Run the same locally before pushing (CONTRIBUTING.md
  *Checks*).

## Conventions
- **The manual is part of the programs** (the author's rule, 2026-11-15). Any change to what a user
  or a gateway operator sees or sets (a window, a box, a button, a setting or its default, a
  message, a file kept) updates `docs/manual/manual.md` in the same commit: its text, its
  Version and change log, and the screenshots of a window that changed (`screens.mjs`, looked at
  before committing). Plain words, the screen's words in bold, the program's in italics. The same
  holds for the server guide (`docs/user/post-office.md`) and what an operator does on the
  server.
- **Living documents.** Every spec has a status, a version (`MAJOR.MINOR.PATCH`, RP-0 §8.2) and a
  change log. A change updates, in the same commit: the text, the version, the change log, the RP-0
  §9 registries if numbers are added, the test vectors, and the design document if a decision
  changes.
- **Test vectors are generated, never edited by hand.** A change that alters one is a spec change:
  change the reference or `make.py`, run `python tools/vectors/make.py`, and commit the files with
  the spec. `python tools/vectors/make.py --check` must pass before a push. A vector that disagrees
  with a spec is a bug in the reference. DEFLATE air forms are marked `"exact": false` (zlib
  versions differ); `--check` inflates them. The session vectors send uncompressed so they stay
  byte-exact; the post office compresses its air forms when smaller (`radiopost_postoffice::air_form`,
  RP-5 §5.3, `compress` in `config.json`), so its vector replays run with `compress: false`. RPMI messages and API bodies compare as JSON values, not text. Writing a vector is
  how a spec gap is found: the first run found five, the resumed and station-to-station sessions
  three more, the modem interface's conversations seven unwritten rules, the post office's
  exchanges most of RP-5's error behaviour and JWS's missing context rule, the email vectors that
  bounces were refused at the door and `Radio-Path` lacked its session ID, renewal that a gateway
  could not renew its own credential, forwarding the flags and the placement's checks.
- **Every case's verdict is written out** in `make.py` (`confirm`, by case name; tables for the
  RPMI conversations, the API story and the accept list), as the spec states it, so the reference
  is tested rather than trusted. Work a new case's verdict out from the spec, never copy it from
  the reference's output. When changing a reference, plant a fault in a scratch copy and check
  `make.py` stops: a check that cannot fail tests nothing.
- **Numbers are measured.** Sizes and times in the specs come from encoding the examples, not
  estimates.
- **Cross-references** are written `RP-n §x.y`; check them when sections move.
- **Plain language**: short sentences, no filler, requirement words in capitals only where they are
  requirements.
- **No read timeouts on sockets.** A connection a thread polls (a host's to its adapter, an
  adapter's to its modem, the Aether adapter's WebSocket) is read through
  `rpmi::polled::PolledStream`: on Windows a read whose `SO_RCVTIMEO` runs out can fail with os
  error 997 and lose data (Aether found it on 2026-11-12; its `core/aetherd/src/polled.rs`). End
  such a connection from another thread with its `Closer`, not a `shutdown` on a clone. A one-shot
  request that gives up at its timeout (an HTTP call) may keep one.
- **Addresses are lowercase** (the author's rule, 2026-11-24; RP-0 1.5.0 §6.2): every program
  lowercases an address where it enters (typed, imported, read from an email, a file, a code, a
  request) with `radiopost::address::lowercase` (`rp.address` in the reference, `addressOf` in
  the panels), and keys every map by it. What is signed keeps its bytes, so an older program's
  message or credential may still hold capitals: compare those with `address::same`, never
  refuse them. The post office folds mixed-case keys in `state.json` at start
  (`State::lowercase_addresses`); the client's address list and groups merge on read.
- **Amateur radio rules**: nothing on the air is encrypted; every encoding is published; VARA is a
  black box — cite only its published documents (the TNC command list).
- Figures: edit the generator, run it, check the render, commit the script and the SVGs together.
- Conventional Commits. Work on a short-lived branch, fast-forward it into `main`, push both.
- **Licences** (the author's choice, 2026-11-09): specifications, documents and figures under
  CC BY 4.0 (`LICENSE-CC-BY-4.0`, stated in each spec's header table and RP-0 §8.5); code under
  MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`), each code file starting with an SPDX line.

## Decided by the author
- The name RadioPost; the project's post office domain `radiopost.org` (registered 2026-11-09;
  its mail live since 2026-11-11: MX `po.radiopost.org`, SPF `mx`, DMARC `quarantine` with reports
  to `postmaster@`, to go to `reject` once the reports are clean; Gmail checks the RSA DKIM key
  and ignores the Ed25519 one).
- No compatibility with Winlink's programs or B2F; a new client.
- (2026-11-10) RadioPost is not meant to replace Winlink. It is an environment for amateurs to
  experiment and develop their own applications on a common protocol; the documents say so
  (README, concept §1, the design's introduction) and never speak of a move from Winlink.
- Priorities for the Internet-down features: copies placed ahead, the gateway's queue, then
  forwarding by radio.
- (2026-11-14) The next stages (design §9): the pilot's mail protected first (backups off the
  server by themselves, blocking callers, one mailbox's second part, DKIM rotation), then local
  delivery (W31), then forwarding in three stages with its first trial on the air between two
  gateways on one shared channel, then the dial (W14, W40), then RP-0 to RP-6 frozen as 1.0 once
  forwarding is built, then Phase 5. *Delivered to the recipient's provider* is dropped (G5). LoTW
  enrollment (W10) is built when the author supplies a test certificate, RSA-1024 and SHA-1
  accepted for proof of licence.
- Stalwart as the post office's mail server (others possible behind JMAP/SMTP); the door answers
  its MTA hooks (D29).
- (2026-11-15) RP-0 to RP-6 are **Stable at 1.0.0** (D41), the open gaps settled first: a change an
  implementation cannot ignore now needs a new protocol version (RP-0 §8.3), and every change to
  them is announced (§8.4). RP-7 stays a Draft. Callsigns are compared by their base (D40, RP-0
  §6.1, RP-1 §6.3): a gateway's own exactly, a client's and a statement's `operator` by base.
- (2026-11-19) RadioPostKit, Georges Auberger's Swift implementation, asked 35 questions (GitHub
  issues 3 to 8; D43; RP-0 1.2.0 to RP-5 1.3.0): a client calls and answers stations; of three
  callsign parts the base is the middle one (`callsign::base`, `rp.base_callsign`); addresses
  compare without regard to ASCII case (`radiopost::address`, `rp.same_address`); two API URLs
  mean no API (`dns::api_of`); receivers check RP-2 §2's keys and types but take a message with no
  recipient; inflation to 16 MiB, past it `too-large` (`message::TOO_LARGE`); a caller that signed
  a challenge keeps that proof, and `/direct` takes it with the `HELLO` nonce (`Proofs::nonce`);
  RPMI lines are I-JSON, integers within ±(2^53−1); a list's repeated callsign counts once; the
  300 seconds of RP-3 §8 count from the last bytes (`rpmi::host::Silence`).
- (2026-11-24) P2P made plain (D47): a listener runs 2300 Hz and answers both bandwidths; Listen
  tunes to the P2P dial unless a gateway scans, and says where and how it answers; no P2P call to
  a listed gateway's callsign; a P2P call shows its band's prediction. No retry at 500 Hz.
- (2026-11-21) An alert to a station whose gateway scans (D46): it goes on the gateway's channel
  predicted best, else the circle's optional watch dial, else where the radio is; copies for the
  gateway's `scan_s`, also for a circle alert when any listed gateway scans the watch dial; answers,
  late ones too, go back to the dial the alert was heard on; at a non-gateway station the operator
  keeps the radio on the watch dial, with a reminder; at a scanning station the watch dial is one
  of the gateway's channels, which the client checks.
- (2026-11-15) The forwarding trial on the air (stage 5, `docs/user/on-air-forwarding.md`) is
  deferred until later: too early in the process.
- (2026-11-12) With more than one post office: a client's gateway list read from every post
  office its own knows, the home's record winning and a callsign listed twice elsewhere ambiguous
  (D36); copies placed ahead at gateways of other post offices, served only to the credential of
  the post office that listed the gateway (D37); a post office knows another only by its
  operators' choice, finding others as mail crosses (D38).
- (2026-11-09) Rust throughout, specs and code in this repository (D27, D28); credentials and
  placements as COSE_Sign1; station-to-station mail reported to the post office over the Internet;
  radiopost.org enrolls on an LoTW certificate or the sysop's check; third-party mail flagged by the
  post office and checked by the gateway against the receiving station's country (§97.115); any
  post office found in DNS trusted, the operator deciding; the Zstandard dictionary trained only on
  a published corpus, adopted only if it beats DEFLATE by a measured margin.
