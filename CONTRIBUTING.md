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
```

Edit the script, run it, and commit the script and the SVGs together.

## Test vectors

`vectors/` is made by `tools/vectors/make.py` from a small reference (Python, standard library
only). `vectors/README.md` says what each file holds and how an implementation uses it.

## Code

The only code so far is the figures, the test vectors' reference and a checker for the documents.
When there are programs, each will say how to build and test it.

## Checks

Every pull request and every push to `main` runs `.github/workflows/check.yml`. Run the same
before you open a pull request (Python 3.12 or later; ruff 0.16.9, because `ruff.toml` turns on
preview rules):

```
ruff check . && ruff format --check .
python tools/vectors/make.py --check    # the vectors are what the reference makes
python tools/figures/concept.py         # the figures are what their generators draw:
python tools/figures/design.py          #   git status shows no change after these
python tools/check_docs.py              # versions, change logs, the index, links, RP-n §x.y
```

`check_docs.py` holds each specification's header to the last row of its change log and to the
index, follows every relative link, and checks that every `RP-n §x.y` names a section that exists.

## Licences of contributions

By contributing, you agree that what you contribute is published under the licence of the part it
goes into: CC BY 4.0 for the specifications and documents, MIT or Apache 2.0 (at the user's option)
for code. New code files start with `SPDX-License-Identifier: MIT OR Apache-2.0` in a comment.

## Conduct

Be kind, be specific, and assume good faith. Amateur radio's own traditions of courtesy apply.
