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
  the gaps found in them; what is set aside and why).
- **Code** (Phase 1 on, D27, D28): Rust throughout, in this repository — `core/`, a Cargo
  workspace (the core library, the gateway, the service), `app/` (the Tauri client) and `adapters/`.
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
  home (RP-5 §6.4). `tests/online.rs` is the whole path: client → loopback adapter → gateway program
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
  to ask. A gateway's credential names the gateway as its operator (RP-5 §6.4) and lasts 90 days:
  registering again replaces it. `tests/common` holds the gateway tests' post office and clients.
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
  shells give it an installer (`tauri-plugin-updater`, the key and endpoints set in code). Its
  tests use a throwaway key whose private half was discarded. **One version**: `tools/release.py
  bump X` writes it in every Cargo manifest, lockfile and the guide's download link (the Tauri
  configurations take Cargo's); `check` runs in CI and in the release's preflight, which also
  refuses a tag that is not `v` + the version. A release: bump, commit, push, then the Release
  workflow with that tag; `tools/update_manifests.py` writes the manifests from the signed files.
  No MSI: it takes no pre-release version, and NSIS's setup.exe updates in place.
  `core/api` (`radiopost-api`) is RP-5 over HTTPS (login, calls, enrollment, DNS over HTTPS), shared
  by the gateway and the client; the client's `online` module uses it (enroll, renew, settings —
  `PUT` replaces them whole, so the client merges —, accept list, `report`). `app/src-tauri` has its
  own lockfile: a new dependency of the client changes it (`cargo metadata` there refreshes it).
  `app/ui` is the client's panel (no build step), served with a JSON API by `radiopost panel`
  (`panel.rs`: loopback only, the `Host` checked, changes need `X-RadioPost: 1`; a session on its
  own thread, the home unchanged by the panel while it runs). `app/src-tauri` (`radiopost-app`, its
  own package and lockfile, outside the core workspace; CI's `app · desktop` job) is the Tauri
  window around it: the panel's server in-process on 8620 or a free port, the window on it, no IPC.
  It needs `libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libxdo-dev
  libayatana-appindicator3-dev`; `xvfb-run` and ImageMagick's `import` show the real window here.
  Its icons come from `tools/figures/icons.py` (`--check` compares pixels, not zlib's bytes).
  Check the panel in headless Chromium (Playwright, `NODE_PATH=$(npm root -g)`); CI checks the
  script parses as a module.
- `tools/figures/` — `style.py` (the shared drawing kit), `concept.py`, `design.py`, `icons.py`. Standard
  library only; `ruff.toml` holds the lint settings.
- `vectors/` — the test vectors (`vectors/README.md`), **generated** by `tools/vectors/make.py`
  from `tools/vectors/rpref/` (deterministic CBOR, Ed25519 after RFC 8032, RadioPost's objects,
  RPMI adapters on a simulated channel in `rpmi.py`, JWS in `jws.py`, a post office in `po.py`,
  email in and out in `mail.py`, forwarding in `fwd.py`; standard library only, self-tested against RFC 8032 §7.1 and
  RFC 8949 Appendix A at every run).
- `tools/check_docs.py` — headers against change logs and the index, relative links, `RP-n §x.y`
  references, SPDX lines. `.github/workflows/check.yml` runs it with ruff (pinned 0.16.9: preview
  rules), `make.py --check` and the figure generators (no diff), on Python 3.12 and 3.13, for
  pushes to `main` and pull requests. Run the same locally before pushing (CONTRIBUTING.md
  *Checks*).

## Conventions
- **Living documents.** Every spec has a status, a version (`MAJOR.MINOR.PATCH`, RP-0 §8.2) and a
  change log. A change updates, in the same commit: the text, the version, the change log, the RP-0
  §9 registries if numbers are added, the test vectors, and the design document if a decision
  changes.
- **Test vectors are generated, never edited by hand.** A change that alters one is a spec change:
  change the reference or `make.py`, run `python tools/vectors/make.py`, and commit the files with
  the spec. `python tools/vectors/make.py --check` must pass before a push. A vector that disagrees
  with a spec is a bug in the reference. DEFLATE air forms are marked `"exact": false` (zlib
  versions differ); `--check` inflates them. The session vectors send uncompressed so they stay
  byte-exact. RPMI messages and API bodies compare as JSON values, not text. Writing a vector is
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
- Priorities for the Internet-down features: copies placed ahead, the gateway's queue, then
  forwarding by radio.
- Stalwart as the post office's mail server (others possible behind JMAP/SMTP); the door answers
  its MTA hooks (D29).
- (2026-11-09) Rust throughout, specs and code in this repository (D27, D28); credentials and
  placements as COSE_Sign1; station-to-station mail reported to the post office over the Internet;
  radiopost.org enrolls on an LoTW certificate or the sysop's check; third-party mail flagged by the
  post office and checked by the gateway against the receiving station's country (§97.115); any
  post office found in DNS trusted, the operator deciding; the Zstandard dictionary trained only on
  a published corpus, adopted only if it beats DEFLATE by a measured margin.
