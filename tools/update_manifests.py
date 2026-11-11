# SPDX-License-Identifier: MIT OR Apache-2.0
"""The update channel's manifests, from a release's files.

    python tools/update_manifests.py DIST --tag v0.1.0-beta.5 --repo KK4ODA/RadioPost

DIST holds the release's files as published (no spaces in their names) and, beside each file an
update installs, its signature (`NAME.sig`, as Tauri's signer writes it). It writes, in DIST:

    client.json    RadioPost: the setup.exe, the AppImage and the .deb, the macOS .app.tar.gz
    gateway.json   RadioPost Gateway: the same
    tools.json     the command-line programs: each platform's tools archive

each in the shape Tauri's updater reads: the version, notes, the date, and for each platform
(`linux-x86_64`, `windows-x86_64`, `darwin-aarch64`, and the installer's own key, such as
`linux-x86_64-deb`) the file's address and its signature. A program asks its manifest, and takes a
file only when the project's update key signed it (`core/update`). Standard library only.
"""

from __future__ import annotations

import json
import sys
from datetime import UTC, datetime
from pathlib import Path

# each product's files start with its name as the release publishes it
PRODUCTS = {"client": "RadioPost_", "gateway": "RadioPost.Gateway_"}

# a file's ending, and the updater's targets it serves (the first is the platform's default)
INSTALLERS = [
    ("_x64-setup.exe", ["windows-x86_64", "windows-x86_64-nsis"]),
    ("_amd64.AppImage", ["linux-x86_64", "linux-x86_64-appimage"]),
    ("_amd64.deb", ["linux-x86_64-deb"]),
    ("_aarch64.app.tar.gz", ["darwin-aarch64", "darwin-aarch64-app"]),
]
TOOLS = {
    "radiopost-tools-linux-x64.zip": "linux-x86_64",
    "radiopost-tools-windows-x64.zip": "windows-x86_64",
    "radiopost-tools-macos-arm64.zip": "darwin-aarch64",
}


def entry(dist: Path, name: str, base: str) -> dict[str, str] | None:
    """A platform's entry: the file's address and its signature, when it was signed."""
    sig = dist / f"{name}.sig"
    if not (dist / name).is_file() or not sig.is_file():
        return None
    return {"url": f"{base}/{name}", "signature": sig.read_text().strip()}


def installers(dist: Path, names: list[str], prefix: str, base: str) -> dict[str, dict[str, str]]:
    """A product's platforms: each signed installer under every target it serves."""
    platforms: dict[str, dict[str, str]] = {}
    for ending, targets in INSTALLERS:
        signed = [
            found
            for name in names
            if name.startswith(prefix) and name.endswith(ending)
            if (found := entry(dist, name, base))
        ]
        for target in targets:
            if signed:
                platforms.setdefault(target, signed[0])
    return platforms


def manifests(dist: Path, tag: str, repo: str) -> dict[str, dict]:
    version = tag.removeprefix("v")
    base = f"https://github.com/{repo}/releases/download/{tag}"
    page = f"https://github.com/{repo}/releases/tag/{tag}"
    head = {
        "version": version,
        "notes": f"RadioPost {version}: {page}",
        "pub_date": datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
    }
    out: dict[str, dict] = {}
    names = sorted(p.name for p in dist.iterdir())
    for product, prefix in PRODUCTS.items():
        out[product] = {**head, "platforms": installers(dist, names, prefix, base)}
    tools: dict[str, dict[str, str]] = {}
    for name, target in TOOLS.items():
        found = entry(dist, name, base)
        if found:
            tools[target] = found
    out["tools"] = {**head, "platforms": tools}
    return out


def main() -> int:
    args = sys.argv[1:]
    try:
        dist = Path(args[0])
        tag = args[args.index("--tag") + 1]
        repo = args[args.index("--repo") + 1]
    except (IndexError, ValueError):
        print(__doc__, file=sys.stderr)
        return 2
    made = manifests(dist, tag, repo)
    empty = [p for p, m in made.items() if not m["platforms"]]
    for product, manifest in made.items():
        (dist / f"{product}.json").write_text(json.dumps(manifest, indent=2) + "\n")
        print(f"{product}.json: {', '.join(sorted(manifest['platforms'])) or 'nothing'}")
    if empty:
        print(f"nothing signed for {', '.join(empty)}: no update offered", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
