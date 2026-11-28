# SPDX-License-Identifier: MIT OR Apache-2.0
"""The licences of every crate compiled into RadioPost's programs, in one text file.

    python tools/third_party_licenses.py [OUT]

The programs are built from a few hundred crates, and most of their licences (MIT, BSD, ISC,
Apache 2.0) ask that their notice travel with a binary made from them. This reads `cargo
metadata` for the three workspaces that ship (`core/`, every member: the tools and the
adapters; `app/src-tauri/`, RadioPost; `app/gateway/src-tauri/`, RadioPost Gateway), follows
the shipped packages' dependencies (normal and build, not dev; every platform, so one file
serves every installer and archive), and writes each crate's own licence and notice files as
the crate ships them. Crates that ship the same text share one copy. The release puts the file
in `docs/` beside the manual (`stage_docs.py`).

A crate that ships no licence file gets, in this order: the files of another shipped crate
from the same repository under the same licences (the repository's own); or the text of a
licence its expression offers that this tool holds (Apache 2.0 and MPL 2.0 as written, MIT
with the crate's authors as the copyright holders). The list says which. A crate none of
those covers stops the run, named. Without OUT the file goes to standard output.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# (workspace manifest, the packages of it that ship: None for every member)
SHIPPED: list[tuple[Path, set[str] | None]] = [
    (ROOT / "core" / "Cargo.toml", None),
    (ROOT / "app" / "src-tauri" / "Cargo.toml", {"radiopost-app"}),
    (ROOT / "app" / "gateway" / "src-tauri" / "Cargo.toml", {"radiopost-gateway-app"}),
]
PRODUCT = "RadioPost"
OWN = (
    "MIT OR Apache-2.0 for the programs (LICENSE-MIT.txt and LICENSE-APACHE.txt, beside this "
    "file), CC BY 4.0 for the documents (LICENSE-CC-BY-4.0.txt)"
)

# the licences' own texts, for a crate that ships none; Apache 2.0 first, since it names no
# copyright holder and is offered by most of them
APACHE = ROOT / "LICENSE-APACHE"
MPL = ROOT / "tools" / "licences" / "MPL-2.0.txt"
MIT = ROOT / "LICENSE-MIT"
HELD = ("Apache-2.0", "MPL-2.0", "MIT")

# the file names a crate's licence and notices go by, at its top level or in LICENSES/
NAMES = re.compile(r"^(licen[cs]e|copying|copyright|notice|unlicense)", re.IGNORECASE)
TEXT_SUFFIXES = {"", ".txt", ".md", ".mit", ".apache", ".apache2", ".bsd", ".rst", ".markdown"}


@dataclass
class Crate:
    name: str
    version: str
    licence: str
    repository: str
    authors: list[str]
    files: list[Path]
    # (heading, text) of each licence text this crate is covered by
    texts: list[tuple[str, str]] = field(default_factory=list)
    note: str = ""

    @property
    def label(self) -> str:
        return f"{self.name} {self.version}"


def metadata(manifest: Path) -> dict:
    out = subprocess.run(
        [
            "cargo",
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--manifest-path",
            str(manifest),
        ],
        check=True,
        capture_output=True,
        encoding="utf-8",
    ).stdout
    return json.loads(out)


def shipped_ids(meta: dict, roots: set[str] | None) -> set[str]:
    """Every package a shipped one depends on, other than through dev-dependencies."""
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    if roots is None:
        roots = {packages[pid]["name"] for pid in meta["workspace_members"]}
    todo = [pid for pid, p in packages.items() if p["name"] in roots and p["source"] is None]
    missing = roots - {packages[pid]["name"] for pid in todo}
    if missing:
        sys.exit(f"no package named {', '.join(sorted(missing))} in {meta['workspace_root']}")
    seen: set[str] = set()
    while todo:
        pid = todo.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid]["deps"]:
            if any(kind["kind"] != "dev" for kind in dep["dep_kinds"]):
                todo.append(dep["pkg"])
    return seen


def licence_files(package: dict) -> list[Path]:
    here = Path(package["manifest_path"]).parent
    candidates = [p for p in here.iterdir() if p.is_file()]
    if (here / "LICENSES").is_dir():
        candidates += [p for p in (here / "LICENSES").iterdir() if p.is_file()]
    found = [p for p in candidates if NAMES.match(p.name) and p.suffix.lower() in TEXT_SUFFIXES]
    named = package.get("license_file")
    if named:
        path = (here / named).resolve()
        if path.is_file() and path not in found:
            found.append(path)
    return sorted(found, key=lambda p: p.name.lower())


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace").strip()


def repository_key(url: str) -> str:
    url = url.strip().lower().removesuffix("/").removesuffix(".git")
    return re.sub(r"^https?://(www\.)?", "", url)


def sibling_key(crate: Crate) -> tuple[str, frozenset[str]]:
    return repository_key(crate.repository), frozenset(offered(crate.licence))


def offered(expression: str) -> list[str]:
    """The licences an expression lets a user pick from, as SPDX identifiers.

    `A OR B` and the old `A/B` offer either; a `WITH` exception or an `AND` is kept whole, so
    an expression that needs two licences at once offers neither on its own here.
    """
    choices = re.split(r"\s+OR\s+|\s*/\s*", expression.strip().strip("()"))
    return [c.strip("() ") for c in choices if c.strip()]


def holders(crate: Crate) -> str:
    names = ", ".join(re.sub(r"\s*<[^>]*>", "", a).strip() for a in crate.authors)
    who = names or (crate.repository or "its repository")
    return f"Copyright (c) the authors of {crate.name}: {who}"


def held_text(crate: Crate) -> tuple[str, str] | None:
    """The text of a licence the crate offers that this tool holds, or None."""
    choices = offered(crate.licence)
    for spdx in HELD:
        if spdx not in choices:
            continue
        if spdx == "Apache-2.0":
            return "Apache-2.0 (the licence's text)", read(APACHE)
        if spdx == "MPL-2.0":
            return "MPL-2.0 (the licence's text)", read(MPL)
        text = re.sub(r"^Copyright \(c\).*$", holders(crate), read(MIT), count=1, flags=re.M)
        return "MIT (the licence's text, the crate's authors as its holders)", text
    return None


def crates() -> list[Crate]:
    found: dict[tuple[str, str], Crate] = {}
    for manifest, roots in SHIPPED:
        meta = metadata(manifest)
        packages = {p["id"]: p for p in meta["packages"]}
        for pid in shipped_ids(meta, roots):
            package = packages[pid]
            if package["source"] is None:  # this repository's own
                continue
            key = (package["name"], package["version"])
            if key not in found:
                found[key] = Crate(
                    name=package["name"],
                    version=package["version"],
                    licence=package.get("license") or "(no licence expression)",
                    repository=package.get("repository") or "",
                    authors=package.get("authors") or [],
                    files=licence_files(package),
                )
    listed = sorted(found.values(), key=lambda c: (c.name, c.version))
    # a sibling is a crate of the same repository under the same licences
    by_repository: dict[tuple[str, frozenset[str]], Crate] = {}
    for crate in listed:
        if crate.files and crate.repository:
            by_repository.setdefault(sibling_key(crate), crate)
    for crate in listed:
        if crate.files:
            crate.texts = [(p.name, read(p)) for p in crate.files]
            continue
        sibling = by_repository.get(sibling_key(crate)) if crate.repository else None
        if sibling:
            crate.texts = [(p.name, read(p)) for p in sibling.files]
            crate.note = f"ships no licence file; the files of {sibling.label}, same repository"
            continue
        held = held_text(crate)
        if held:
            crate.texts = [held]
            crate.note = f"ships no licence file; used under {held[0].split()[0]}"
    return listed


def normalised(text: str) -> str:
    return " ".join(text.split())


def render(listed: list[Crate]) -> str:
    texts: dict[str, tuple[str, str, list[Crate]]] = {}
    for crate in listed:
        for heading, text in crate.texts:
            key = normalised(text)
            if key not in texts:
                texts[key] = (heading, text, [])
            if crate not in texts[key][2]:
                texts[key][2].append(crate)
    rule = "-" * 78
    lines = [
        "Third-party licences",
        "====================",
        "",
        f"{PRODUCT} is built from its own source and the {len(listed)} Rust crates listed here.",
        f"Its own licence is {OWN}.",
        "",
        "Each crate's source is on crates.io under its name and version, and at the repository",
        "named. The licence and notice files follow the list, as each crate ships them; crates",
        "that ship the same text share one copy.",
        "",
        "Crates",
        "------",
        "",
    ]
    for crate in listed:
        lines.append(f"{crate.label}: {crate.licence}")
        if crate.repository:
            lines.append(f"    {crate.repository}")
        if crate.note:
            lines.append(f"    ({crate.note})")
    lines.append("")
    for heading, text, users in sorted(texts.values(), key=lambda t: (t[2][0].name, t[0].lower())):
        lines += [rule, f"{heading}, for:", *(f"  {c.label}" for c in users), rule, "", text, ""]
    return "\n".join(lines) + "\n"


def main() -> int:
    listed = crates()
    bare = [c for c in listed if not c.texts]
    if bare:
        print("no licence text for these crates:", file=sys.stderr)
        for crate in bare:
            print(f"  {crate.label} ({crate.licence})", file=sys.stderr)
        print(f"a text for one of their licences belongs in {MPL.parent}", file=sys.stderr)
        return 1
    page = render(listed)
    if len(sys.argv) > 1:
        out = Path(sys.argv[1])
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(page, encoding="utf-8", newline="\n")
        print(f"{out}: {len(listed)} crates")
    else:
        sys.stdout.write(page)
    return 0


if __name__ == "__main__":
    sys.exit(main())
