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
  role, conformance mode, the test adapter, what each phase adds).
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
  RPMI adapter; `rpmi-loopback` serves the simulated channel for running two clients by hand.
  Station-to-station mail moves to `sent` and its RP-5 §6.6 report is kept (D30), from the proofs
  the engines now keep (`Client::proofs`, `Called::proofs`); the replay test matches session-03
  byte for byte and `direct.json`'s two reports. The engines report their own refusals
  (`Event::Refused`), found by running two clients that trusted no post office.
- `tools/figures/` — `style.py` (the shared drawing kit), `concept.py`, `design.py`. Standard
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
- The name RadioPost; the project's post office domain `radiopost.org` (registered 2026-11-09,
  null-MX/SPF/DMARC records until the post office is live).
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
