# SPDX-License-Identifier: MIT OR Apache-2.0
"""RadioPost's one version number, in every place it is written.

Every program of a release carries the release's version (`0.1.0-beta.5`): the core workspace, the
two desktop applications, their lockfiles, and the operator's guide's download link. The apps
compare it with the update channel's to say when a newer release is out.

    python tools/release.py check [--tag v0.1.0-beta.5]   # every site agrees (and with the tag)
    python tools/release.py bump 0.1.0-beta.5             # write it everywhere
    python tools/release.py version                       # print it

A release: `bump`, commit, push, then run the Release workflow with the tag `v` + the version. The
workflow runs `check --tag` first and builds nothing when they differ. Standard library only.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# a semantic version, as the programs compare them (semver 2.0.0)
VERSION = re.compile(r"^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$")

MANIFESTS = [
    ROOT / "core" / "Cargo.toml",  # [workspace.package]: every member inherits it
    ROOT / "app" / "src-tauri" / "Cargo.toml",
    ROOT / "app" / "gateway" / "src-tauri" / "Cargo.toml",
]
LOCKFILES = [
    ROOT / "core" / "Cargo.lock",
    ROOT / "app" / "src-tauri" / "Cargo.lock",
    ROOT / "app" / "gateway" / "src-tauri" / "Cargo.lock",
]
TAURI_CONFIGS = [
    ROOT / "app" / "src-tauri" / "tauri.conf.json",
    ROOT / "app" / "gateway" / "src-tauri" / "tauri.conf.json",
]
GUIDE = ROOT / "docs" / "user" / "post-office.md"
GUIDE_LINK = re.compile(r"(releases/download/v)([^/]+)(/radiopost-tools-linux-x64\.zip)")

# the first `version = "…"` line of a manifest is its package's (or the workspace's) own
MANIFEST_VERSION = re.compile(r'^version = "([^"]*)"$', re.MULTILINE)


def ours(name: str) -> bool:
    """The repository's own packages, as a lockfile names them."""
    return name == "rpmi" or name.startswith("radiopost")


def lock_versions(text: str) -> dict[str, str]:
    """The version a lockfile records for each of the repository's own packages."""
    found = {}
    for block in text.split("[[package]]")[1:]:
        name = re.search(r'^name = "([^"]*)"$', block, re.MULTILINE)
        version = re.search(r'^version = "([^"]*)"$', block, re.MULTILINE)
        if name and version and "source = " not in block and ours(name.group(1)):
            found[name.group(1)] = version.group(1)
    return found


def set_lock_versions(text: str, version: str) -> str:
    def one(block: str) -> str:
        name = re.search(r'^name = "([^"]*)"$', block, re.MULTILINE)
        if not name or "source = " in block or not ours(name.group(1)):
            return block
        return re.sub(
            r'^version = "[^"]*"$', f'version = "{version}"', block, count=1, flags=re.MULTILINE
        )

    head, *blocks = text.split("[[package]]")
    return "[[package]]".join([head, *map(one, blocks)])


def sites() -> list[tuple[str, str | None]]:
    """Each place the version is written, and the version found there."""
    found: list[tuple[str, str | None]] = []
    for path in MANIFESTS:
        m = MANIFEST_VERSION.search(path.read_text())
        found.append((str(path.relative_to(ROOT)), m.group(1) if m else None))
    for path in LOCKFILES:
        for name, version in sorted(lock_versions(path.read_text()).items()):
            found.append((f"{path.relative_to(ROOT)} ({name})", version))
    for path in TAURI_CONFIGS:
        # no version of its own: the bundler takes the package's (Cargo.toml)
        if "version" in json.loads(path.read_text()):
            found.append((f"{path.relative_to(ROOT)} (has its own version)", None))
    m = GUIDE_LINK.search(GUIDE.read_text())
    found.append((f"{GUIDE.relative_to(ROOT)} (download link)", m.group(2) if m else None))
    return found


def check(tag: str | None) -> int:
    found = sites()
    versions = {v for _, v in found}
    wrong = []
    if len(versions) != 1 or None in versions:
        wrong.append("the version sites disagree:")
        wrong += [f"  {site}: {v}" for site, v in found]
    version = next(iter(versions)) if len(versions) == 1 else None
    if version and not VERSION.match(version):
        wrong.append(f"{version} is not a version such as 0.1.0-beta.5")
    if tag is not None and version is not None and tag != f"v{version}":
        wrong.append(f"the tag {tag} is not v{version}: bump first (tools/release.py bump)")
    for line in wrong:
        print(line, file=sys.stderr)
    if not wrong:
        print(f"version {version} in {len(found)} places")
    return 1 if wrong else 0


def bump(version: str) -> int:
    if not VERSION.match(version):
        print(f"{version} is not a version such as 0.1.0-beta.5", file=sys.stderr)
        return 1
    for path in MANIFESTS:
        text = path.read_text()
        path.write_text(MANIFEST_VERSION.sub(f'version = "{version}"', text, count=1))
    for path in LOCKFILES:
        path.write_text(set_lock_versions(path.read_text(), version))
    for path in TAURI_CONFIGS:
        config = json.loads(path.read_text())
        if config.pop("version", None) is not None:
            path.write_text(json.dumps(config, indent=2) + "\n")
    GUIDE.write_text(GUIDE_LINK.sub(rf"\g<1>{version}\g<3>", GUIDE.read_text()))
    return check(None)


def main() -> int:
    args = sys.argv[1:]
    if args[:1] == ["check"]:
        tag = args[args.index("--tag") + 1] if "--tag" in args else None
        return check(tag)
    if args[:1] == ["bump"] and len(args) == 2:
        return bump(args[1])
    if args[:1] == ["version"]:
        found = {v for _, v in sites()}
        if len(found) != 1 or None in found:
            return check(None)
        print(next(iter(found)))
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
