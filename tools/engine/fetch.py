# SPDX-License-Identifier: MIT OR Apache-2.0
"""The VOACAP engine RadioPost bundles (choosing a gateway, §11.2): hf-predict builds it and
publishes it with its checksums as a release of its own; `voacapl.json` beside this file pins
one release and each platform's archive by its SHA-256.

    python tools/engine/fetch.py get PLATFORM DEST    # linux-x64, windows-x64 or macos-arm64
    python tools/engine/fetch.py pin RELEASE          # pin another release, from its SHA256SUMS

`get` downloads the platform's archive, refuses it unless its SHA-256 is the pinned one, and
puts the archive's `engine/` folder at DEST (bin/voacapl, the itshfbc data tree and the notices
that travel with it). Nothing in the archive is written outside DEST. `pin` reads a release's
SHA256SUMS and rewrites `voacapl.json`. Standard library only.
"""

import hashlib
import json
import shutil
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path

PIN = Path(__file__).with_name("voacapl.json")
REPOSITORY = "KK4ODA/hf-predict"
PLATFORMS = ("linux-x64", "windows-x64", "macos-arm64")
TOP = "engine"


def url(repository: str, release: str, name: str) -> str:
    return f"https://github.com/{repository}/releases/download/{release}/{name}"


def download(address: str) -> bytes:
    request = urllib.request.Request(address, headers={"User-Agent": "radiopost-engine-fetch"})
    # an https address of the pinned repository's releases, built here, never one given
    with urllib.request.urlopen(request, timeout=120) as answer:
        return answer.read()


def checked(data: bytes, sha256: str, name: str) -> bytes:
    """`data` if its SHA-256 is `sha256`; otherwise the fetch stops."""
    found = hashlib.sha256(data).hexdigest()
    if found != sha256:
        sys.exit(f"{name}: SHA-256 {found}, not the pinned {sha256}: not used")
    return data


def extract(archive: Path, dest: Path) -> None:
    """The archive's `engine/` folder as `dest`, refusing any member outside it, any link and any
    device: an archive is data, never a way to write elsewhere."""
    with tarfile.open(archive, "r:gz") as tar:
        members = tar.getmembers()
        for m in members:
            parts = Path(m.name).parts
            if not parts or parts[0] != TOP or ".." in parts or Path(m.name).is_absolute():
                sys.exit(f"{m.name}: outside {TOP}/: not used")
            if not (m.isfile() or m.isdir()):
                sys.exit(f"{m.name}: not a file or a folder: not used")
        with tempfile.TemporaryDirectory() as scratch:
            # Python 3.12's filter, where there is one: the members are checked above either way
            if hasattr(tarfile, "data_filter"):
                tar.extractall(scratch, filter="data")
            else:
                tar.extractall(scratch)
            if dest.exists():
                shutil.rmtree(dest)
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.move(str(Path(scratch) / TOP), str(dest))


def get(platform: str, dest: Path) -> None:
    pin = json.loads(PIN.read_text(encoding="utf-8"))
    entry = pin["files"].get(platform)
    if entry is None:
        sys.exit(f"no engine pinned for {platform}: {', '.join(pin['files'])}")
    address = url(pin["repository"], pin["release"], entry["name"])
    data = checked(download(address), entry["sha256"], entry["name"])
    with tempfile.TemporaryDirectory() as scratch:
        archive = Path(scratch) / entry["name"]
        archive.write_bytes(data)
        extract(archive, dest)
    exe = dest / "bin" / ("voacapl.exe" if platform.startswith("windows") else "voacapl")
    if not exe.is_file() or not (dest / "itshfbc").is_dir():
        sys.exit(f"{entry['name']}: no engine in it")
    print(f"{pin['release']} {platform}: {dest}")


def pin(release: str) -> None:
    if not release.startswith("engine-"):
        sys.exit(f"{release}: an engine release's name begins engine-")
    sums = download(url(REPOSITORY, release, "SHA256SUMS")).decode("utf-8")
    files = {}
    for line in sums.splitlines():
        sha256, _, name = line.partition("  ")
        for platform in PLATFORMS:
            if name.strip() == f"voacapl-{platform}.tar.gz":
                files[platform] = {"name": name.strip(), "sha256": sha256.strip()}
    missing = [p for p in PLATFORMS if p not in files]
    if missing:
        sys.exit(f"{release}: no archive for {', '.join(missing)}")
    text = json.dumps({"repository": REPOSITORY, "release": release, "files": files}, indent=2)
    PIN.write_text(text + "\n", encoding="utf-8")
    print(f"pinned {release}")


def main() -> None:
    match sys.argv[1:]:
        case ["get", platform, dest]:
            get(platform, Path(dest))
        case ["pin", release]:
            pin(release)
        case _:
            sys.exit(__doc__)


if __name__ == "__main__":
    main()
