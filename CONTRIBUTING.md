# Contributing to RadioPost

Thank you for helping. RadioPost is specifications first: the protocols in `spec/` are what
everyone builds to, so most contributions are to them, and they change carefully.

## Questions and ideas

Open an issue. Say what you are building or trying to do; a concrete case is the best argument for
a change.

## Changing a specification

1. **Open an issue first** for anything beyond wording, so the change can be discussed before it is
   written.
2. **One pull request per change.** In it, update together:
   * the specification's text;
   * its **version** (RP-0 §8.2: patch for wording, minor for an addition others can ignore, major
     for one they cannot) and its **change log**;
   * the **registries** in RP-0 §9, if you add a number, a code or a signature context;
   * the **test vectors** (`vectors/`), if the change alters any: change the reference in
     `tools/vectors/`, rerun `python tools/vectors/make.py`, and commit the files it writes. Never
     edit a vector by hand;
   * the **detailed design**, if a decision changes or a new one is made.
3. **Say what it removes** as well as what it adds. Every rule has a cost for every implementer.
4. A specification marked **Candidate** or **Stable** changes only as RP-0 §8.1 and §8.3 allow.

## Writing style

Plain words, short sentences. Requirement words in capitals only where they are requirements (RP-0
§1). Examples are marked informative. Numbers come from measurement or calculation, not guesses.

## Figures

Figures are drawn by `tools/figures/` (Python, standard library only):

```
python tools/figures/concept.py     # docs/concept/fig/
python tools/figures/design.py      # docs/design/fig/ and spec/fig/
python tools/figures/manual.py      # docs/manual/fig/
python tools/figures/server.py      # docs/user/fig/ (the server guide's)
```

Edit the script, run it, and commit the script and the SVGs together.

## The user's manual

[`docs/manual/manual.md`](docs/manual/manual.md) is the manual for the RadioPost client and
RadioPost Gateway. **It is part of the programs.** A change to anything a user or a gateway
operator sees or sets (a window, a box, a button, a setting or its default, a message, a file
kept) updates the manual in the same commit:

1. Change the text. Keep it plain: short sentences, the words on the screen in **bold**, what the
   program says in *italics*.
2. Raise the manual's **Version** (the last number for a correction, the middle one for something
   added or changed, the first for a reorganisation), set **Updated**, and add a row to its change
   log. `check_docs.py` holds the three together.
3. When a window changed, take its screenshots again (`node tools/manual/screens.mjs [NAME…]`,
   Node with Playwright and its Chromium) and look at each picture before committing it. The
   examples' data is in that script.
4. `python tools/manual/build.py toc` when a heading changed, then
   `python tools/manual/build.py check`: the contents, the pictures, their numbers and the links.

The **Describes** row is the release version: `tools/release.py bump` moves it. Each release
carries the manual as a PDF (`python tools/manual/build.py pdf manual`, which needs Chrome or
Chromium).

**The server guide**, [`docs/user/post-office.md`](docs/user/post-office.md), is held to the same
rule for what a post office's operator sets up, types or sees on the server: an installer step, a
command, a setting, a file, a message. It has its own **Version**, change log and **Describes**,
its schematics come from `tools/figures/server.py`, and it shows the manual's screenshots of the
desk. `build.py toc` and `check` cover both books; `build.py pdf server` prints it.

## Test vectors

`vectors/` is made by `tools/vectors/make.py` from a small reference (Python, standard library
only). `vectors/README.md` says what each file holds and how an implementation uses it. A new case
comes with its verdict, read from the specification and written in `make.py`'s table for that
list; never copy it from what the reference answers.

## Code

Besides the figures, the test vectors' reference and a checker for the documents, the programs are
in `core/`, a Rust workspace ([`core/README.md`](core/README.md)). Each crate says how to build and
test it.

## Checks

Every pull request and every push to `main` runs `.github/workflows/check.yml`. Run the same
before you open a pull request (Python 3.12 or later; ruff 0.16.9, because `ruff.toml` turns on
preview rules):

```
ruff check . && ruff format --check .
python tools/vectors/make.py --check    # the vectors are what the reference makes
python tools/figures/concept.py         # the figures are what their generators draw:
python tools/figures/design.py          #   git status shows no change after these
python tools/figures/manual.py          #   (the manual's schematics too)
python tools/figures/server.py          #   (and the server guide's)
python tools/check_docs.py              # versions, change logs, the index, links, RP-n §x.y
python tools/manual/build.py check      # the manual's and server guide's contents and links
```

and, in `core/`, with a stable Rust:

```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test                              # the library against vectors/
```

The desktop application is a package of its own: in `app/src-tauri`, the same three commands, with
WebKitGTK's development packages installed on Linux ([the client's README](app/client/README.md)).
`python tools/figures/icons.py --check` holds its icons to their generator.

`rustup update stable` first: CI lints with the newest stable clippy. The Aether adapter's tests
against real daemons run only when `AETHERD` names an `aetherd` of 0.2.0-beta.146 or later
(`core/README.md`).

`check_docs.py` holds each specification's header to the last row of its change log and to the
index, follows every relative link, and checks that every `RP-n §x.y` names a section that exists.

## Licences of contributions

By contributing, you agree that what you contribute is published under the licence of the part it
goes into: CC BY 4.0 for the specifications and documents, MIT or Apache 2.0 (at the user's option)
for code. New code files start with `SPDX-License-Identifier: MIT OR Apache-2.0` in a comment.

## Conduct

Be kind, be specific, and assume good faith. Amateur radio's own traditions of courtesy apply.
