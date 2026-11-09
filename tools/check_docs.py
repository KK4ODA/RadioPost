# SPDX-License-Identifier: MIT OR Apache-2.0
"""Check that the living documents agree with themselves.

    python tools/check_docs.py

What it checks, and why:

* Each specification and each design document has a status, a version and an update date in its
  header, and a change log whose last row is that version on that date (RP-0 §8.2). A Draft or
  an Outline stays below 1.0.0, and the change log's versions only go up.
* The specification index (`spec/README.md`) lists each specification's status and version as
  its header does.
* Every relative link in a Markdown file leads to a file that exists.
* Every `RP-n §x.y` names a section that exists in RP-n, including a list such as
  `RP-1 §5, §6.3` and a reference broken across two lines.
* Every Python file starts with an SPDX licence line (CLAUDE.md, CONTRIBUTING.md).

Standard library only. Exits 1 and lists every problem if anything is wrong.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPDX = "# SPDX-License-Identifier: MIT OR Apache-2.0"
SKIP_DIRS = {".git", "__pycache__", ".venv", "node_modules"}

HEADER = re.compile(r"^\| \*\*(Status|Version|Updated)\*\* \| ([^|]+?) \|$", re.MULTILINE)
LOG_ROW = re.compile(r"^\| (\d+\.\d+\.\d+) \| (\d{4}-\d{2}-\d{2}) \|", re.MULTILINE)
HEADING = re.compile(r"^#{2,4} (\d+(?:\.\d+)*)\.? ", re.MULTILINE)
LINK = re.compile(r"\]\(([^)\s]+)\)")
# "RP-1 §5, §6.3": a specification, then one or more sections, each after "§" or a comma.
REFERENCE = re.compile(r"RP-(\d+)((?:\s*§\s*\d+(?:\.\d+)*)(?:\s*,\s*§\s*\d+(?:\.\d+)*)*)")
SECTION = re.compile(r"\d+(?:\.\d+)*")
INDEX_ROW = re.compile(
    r"^\| \[RP-(\d+)\]\(([^)]+)\) \| [^|]+ \| ([^|]+?) \| ([^|]+?) \|", re.MULTILINE
)


def version(text: str) -> tuple[int, ...]:
    return tuple(int(part) for part in text.split("."))


def files(pattern: str) -> list[Path]:
    return sorted(p for p in ROOT.rglob(pattern) if not SKIP_DIRS & set(p.parts))


def specifications() -> dict[int, Path]:
    return {int(p.name[3:7]): p for p in sorted((ROOT / "spec").glob("rp-*.md"))}


def check_living(path: Path, problems: list[str]) -> dict[str, str]:
    """The header, the change log and their agreement."""
    text = path.read_text(encoding="utf-8")
    name = path.relative_to(ROOT)
    header = dict(HEADER.findall(text))
    for field in ("Status", "Version", "Updated"):
        if field not in header:
            problems.append(f"{name}: the header has no {field}")
    log = LOG_ROW.findall(text)
    if not log:
        problems.append(f"{name}: no change log rows")
        return header
    versions = [version(v) for v, _ in log]
    if versions != sorted(set(versions)):
        problems.append(f"{name}: the change log's versions do not only go up")
    last, date = log[-1]
    if header.get("Version") != last:
        problems.append(f"{name}: the header says {header.get('Version')}, the change log {last}")
    if header.get("Updated") != date:
        problems.append(
            f"{name}: updated {header.get('Updated')}, the change log's last row {date}"
        )
    if header.get("Status") in {"Draft", "Outline"} and version(last) >= (1, 0, 0):
        problems.append(f"{name}: a {header['Status']} stays below 1.0.0 (RP-0 §8.2)")
    return header


def check_index(headers: dict[int, dict[str, str]], problems: list[str]) -> None:
    index = (ROOT / "spec" / "README.md").read_text(encoding="utf-8")
    listed = {int(n): (link, status, ver) for n, link, status, ver in INDEX_ROW.findall(index)}
    for n, header in headers.items():
        if n not in listed:
            problems.append(f"spec/README.md: RP-{n} is not in the index")
            continue
        _, status, ver = listed[n]
        if (status, ver) != (header.get("Status"), header.get("Version")):
            problems.append(
                f"spec/README.md: RP-{n} listed as {status} {ver}, "
                f"its header says {header.get('Status')} {header.get('Version')}"
            )
    for n in listed.keys() - headers.keys():
        problems.append(f"spec/README.md: lists RP-{n}, which does not exist")


def check_links_and_references(problems: list[str]) -> None:
    sections = {
        n: set(HEADING.findall(p.read_text(encoding="utf-8"))) for n, p in specifications().items()
    }
    for path in files("*.md"):
        text = path.read_text(encoding="utf-8")
        name = path.relative_to(ROOT)
        for link in LINK.findall(text):
            target = link.split("#", 1)[0]
            if not target or re.match(r"[a-z]+:", target):
                continue
            if not (path.parent / target).exists():
                problems.append(f"{name}: broken link {target}")
        flat = re.sub(r"\s+", " ", text)
        for n, refs in REFERENCE.findall(flat):
            spec = int(n)
            if spec not in sections:
                problems.append(f"{name}: RP-{spec} does not exist")
                continue
            for section in SECTION.findall(refs):
                if section not in sections[spec]:
                    problems.append(f"{name}: RP-{spec} §{section} does not exist")


def check_spdx(problems: list[str]) -> None:
    for path in files("*.py"):
        first = path.read_text(encoding="utf-8").split("\n", 1)[0]
        if first != SPDX:
            problems.append(f"{path.relative_to(ROOT)}: does not start with `{SPDX}`")


def main() -> int:
    problems: list[str] = []
    headers = {n: check_living(p, problems) for n, p in specifications().items()}
    for design in sorted((ROOT / "docs" / "design").glob("[0-9][0-9]-*.md")):
        check_living(design, problems)
    check_index(headers, problems)
    check_links_and_references(problems)
    check_spdx(problems)
    if problems:
        print("\n".join(problems), file=sys.stderr)
        print(f"{len(problems)} problems", file=sys.stderr)
        return 1
    print(f"{len(headers)} specifications, the designs, links, references and licence lines agree")
    return 0


if __name__ == "__main__":
    sys.exit(main())
