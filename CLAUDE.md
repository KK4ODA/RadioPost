# RadioPost — working notes for Claude Code

## What this is
RadioPost: a federated email system for amateur radio (gateways anyone runs, post offices groups
run, ordinary email addresses, no central server). The author is KK4ODA. Its first modem is Aether
HF (`KK4ODA/aether-hf`), but RadioPost is modem-independent.

## Layout
- `spec/` — **the protocols, normative**: RP-0 conventions, RP-1 identity, RP-2 messages, RP-3
  session protocol, RP-4 modem interface (RPMI), RP-5 post office API, RP-6 email, RP-7 forwarding
  (outline). `spec/README.md` is the index. Where a program and a spec disagree, the spec wins.
- `docs/concept/` — the plain-language concept, with schematics.
- `docs/design/` — the detailed design: the map, decisions D1–D18, the programs' parts, the order of
  work, the open questions.
- `tools/figures/` — `style.py` (the shared drawing kit), `concept.py`, `design.py`. Standard
  library only; `ruff.toml` holds the lint settings.

## Conventions
- **Living documents.** Every spec has a status, a version (`MAJOR.MINOR.PATCH`, RP-0 §8.2) and a
  change log. A change updates, in the same commit: the text, the version, the change log, the RP-0
  §9 registries if numbers are added, the test vectors (once they exist), and the design document
  if a decision changes.
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
- Stalwart as the post office's mail server (others possible behind JMAP/SMTP).
