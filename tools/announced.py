# SPDX-License-Identifier: MIT OR Apache-2.0
"""The announced changes a release carries, for its notes (RP-0 §8.4).

A change to a Stable specification is announced: its change-log row says what an implementation
must do, and the release notes of the programs that carry it name it. This lists, as Markdown,
every change-log row added to a specification since the previous release's tag whose version is
1.0.0 or above. A Stable document's version is (RP-0 §8.2); a Draft's stays below 1.0.0, and its
changes need no announcing.

    python tools/announced.py                       # since the newest v* tag behind HEAD
    python tools/announced.py --since v0.1.0-beta.37
    python tools/announced.py --exclude v0.1.0-beta.39 --repo KK4ODA/RadioPost --ref <sha>
    python tools/announced.py --self-test

`--exclude` leaves out the tag of the release being made, which a pushed tag puts on HEAD; a
release started by hand has no tag yet. `--repo` and `--ref` link each version to its document.
Prints nothing when there is nothing to announce. Standard library only.
"""

from __future__ import annotations

import argparse
import posixpath
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = "spec"

TITLE = re.compile(r"^# (RP-\d+): (.+)$", re.MULTILINE)
# a Markdown link's target that is relative to the specification's own folder
RELATIVE = re.compile(r"\]\((?!https?://|mailto:|#)([^)\s]+)\)")
ROW = re.compile(r"^\| (\d+)\.(\d+)\.(\d+) \| (\d{4}-\d{2}-\d{2}) \| (.*) \|\s*$")


@dataclass(frozen=True)
class Change:
    """One change-log row: the document, its version and what the row says."""

    number: str  # RP-2
    title: str  # Messages
    file: str  # rp-0002-messages.md
    version: tuple[int, int, int]
    date: str
    text: str

    @property
    def stable(self) -> bool:
        return self.version[0] >= 1

    def markdown(self, repo: str | None, ref: str | None) -> str:
        v = ".".join(map(str, self.version))
        name = f"{self.number} {v}"
        text = self.text
        if repo and ref:
            base = f"https://github.com/{repo}/blob/{ref}"
            name = f"[{name}]({base}/{SPEC}/{self.file})"
            # a release's notes are read on GitHub, not beside the document: links point at the
            # commit the release was built from
            text = RELATIVE.sub(
                lambda m: f"]({base}/{posixpath.normpath(posixpath.join(SPEC, m.group(1)))})", text
            )
        return f"- **{name}** ({self.title}, {self.date}): {text}"


def changes(text: str, file: str) -> list[Change]:
    """Every row of a specification's change log, in the order written."""
    title = TITLE.search(text)
    number, name = (title.group(1), title.group(2)) if title else (file, file)
    log = text.split("\n## Change log", 1)
    if len(log) < 2:
        return []
    out = []
    for line in log[1].splitlines():
        m = ROW.match(line)
        if m:
            version = (int(m.group(1)), int(m.group(2)), int(m.group(3)))
            row = m.group(5).replace("\\|", "|").strip()
            out.append(Change(number, name, file, version, m.group(4), row))
    return out


def announced(before: dict[str, str], now: dict[str, str]) -> list[Change]:
    """The rows of Stable versions in `now` that `before` did not have, by document and version.
    `before` and `now` map a specification's file name to its text; a document new since `before`
    has every row new."""
    out = []
    for file in sorted(now):
        had = {c.version for c in changes(before.get(file, ""), file)}
        out.extend(c for c in changes(now[file], file) if c.stable and c.version not in had)
    return sorted(out, key=lambda c: (c.file, c.version))


def git(*args: str) -> str | None:
    done = subprocess.run(
        ["git", *args], cwd=ROOT, capture_output=True, text=True, encoding="utf-8", check=False
    )
    return done.stdout if done.returncode == 0 else None


def previous_tag(exclude: str | None) -> str | None:
    """The newest `v*` tag behind HEAD, leaving out `exclude`."""
    args = ["describe", "--tags", "--abbrev=0", "--match", "v*"]
    if exclude:
        args += ["--exclude", exclude]
    found = git(*args, "HEAD")
    return found.strip() if found else None


def specs_at(ref: str) -> dict[str, str]:
    """Each specification's text as `ref` had it."""
    listing = git("ls-tree", "--name-only", f"{ref}:{SPEC}") or ""
    out = {}
    for name in listing.split():
        if name.startswith("rp-") and name.endswith(".md"):
            text = git("show", f"{ref}:{SPEC}/{name}")
            if text is not None:
                out[name] = text
    return out


def specs_now() -> dict[str, str]:
    return {p.name: p.read_text(encoding="utf-8") for p in sorted((ROOT / SPEC).glob("rp-*.md"))}


def notes(found: list[Change], since: str, repo: str | None, ref: str | None) -> str:
    if not found:
        return ""
    lines = [
        f"**Announced changes to the specifications** since {since} (RP-0 §8.4). Each says "
        "what an implementation must do:",
        "",
    ]
    lines += [c.markdown(repo, ref) for c in found]
    return "\n".join(lines)


def self_test() -> None:
    """The rule on made-up documents, the verdicts written out by hand."""
    head = (
        "# RP-9: Example\n\n| | |\n|---|---|\n\nText.\n\n## Change log\n\n"
        "| Version | Date | Change |\n|---|---|---|\n"
    )
    old = head + "| 0.9.0 | 2026-01-01 | A draft. |\n| 1.0.0 | 2026-01-02 | Stable. |\n"
    new = (
        old
        + "| 1.0.1 | 2026-01-03 | Wording, with a pipe \\| kept. |\n"
        + "| 1.1.0 | 2026-01-04 | A key added ([why](../docs/why.md#a), RP-0 [§8](rp-0000-x.md),"
        + " [site](https://radiopost.org)). MINOR, announced. |\n"
    )
    draft_old = "# RP-8: Draft\n\n## Change log\n\n| 0.1.0 | 2026-01-01 | First. |\n"
    draft_new = draft_old + "| 0.2.0 | 2026-01-05 | Second. |\n"
    fresh = "# RP-10: New\n\n## Change log\n\n| 1.0.0 | 2026-01-06 | Born stable. |\n"
    found = announced(
        {"rp-0009-example.md": old, "rp-0008-draft.md": draft_old},
        {
            "rp-0009-example.md": new,
            "rp-0008-draft.md": draft_new,
            "rp-0010-new.md": fresh,
        },
    )
    got = [(c.number, c.version, c.text) for c in found]
    # by document, then version: the draft's row is left out, the new document's kept
    want = [
        ("RP-9", (1, 0, 1), "Wording, with a pipe | kept."),
        (
            "RP-9",
            (1, 1, 0),
            "A key added ([why](../docs/why.md#a), RP-0 [§8](rp-0000-x.md),"
            " [site](https://radiopost.org)). MINOR, announced.",
        ),
        ("RP-10", (1, 0, 0), "Born stable."),
    ]
    assert got == want, got
    # nothing new, nothing said
    assert announced({"rp-0009-example.md": new}, {"rp-0009-example.md": new}) == []
    assert not notes([], "v1", None, None)
    text = notes(found[1:], "v0.1.0-beta.1", "KK4ODA/RadioPost", "abc")
    # relative links resolve against the commit, from the specification's folder; others stay
    blob = "https://github.com/KK4ODA/RadioPost/blob/abc"
    assert f"([why]({blob}/docs/why.md#a)" in text, text
    assert f"[§8]({blob}/spec/rp-0000-x.md)" in text, text
    assert "[site](https://radiopost.org)" in text, text
    assert "[RP-10 1.0.0](https://github.com/KK4ODA/RadioPost/blob/abc/spec/rp-0010-new.md)" in text
    # every real specification has a title and a change log the parser reads
    for name, text in specs_now().items():
        rows = changes(text, name)
        assert rows and TITLE.search(text), f"{name}: no title or no change log read"
    print("announced.py: the rule holds on its cases, and every specification's change log reads")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    ap.add_argument("--since", help="the previous release's tag (default: the newest v* tag)")
    ap.add_argument("--exclude", help="a tag to pass over: the release being made")
    ap.add_argument("--repo", help="owner/name, to link each version to its document")
    ap.add_argument("--ref", help="the commit the links point at")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        self_test()
        return 0
    since = a.since or previous_tag(a.exclude)
    if not since:
        print("announced.py: no earlier v* tag to compare with", file=sys.stderr)
        return 0
    if git("rev-parse", "--verify", "--quiet", f"{since}^{{commit}}") is None:
        print(f"announced.py: no such tag or commit: {since}", file=sys.stderr)
        return 1
    found = announced(specs_at(since), specs_now())
    text = notes(found, since, a.repo, a.ref)
    if text:
        print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
