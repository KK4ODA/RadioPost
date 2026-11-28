# SPDX-License-Identifier: MIT OR Apache-2.0
"""Put the manual, the server guide and the licences where every build carries them.

    python tools/stage_docs.py [--no-books]

Writes `app/docs/`: `manual.pdf` and `server-guide.pdf` (`tools/manual/build.py pdf`),
`LICENSE-MIT.txt`, `LICENSE-APACHE.txt` and `LICENSE-CC-BY-4.0.txt` (the repository's), and
`THIRD-PARTY-LICENSES.txt` (`tools/third_party_licenses.py`). The release puts the folder among
both desktop applications' resources and in the tools archive beside the programs, so each
panel's **Manual and licences** opens them with no Internet (`radiopost_update::docs`). The
folder is not in git. The release builds it once (its `docs` job, on Linux, where the
schematics' font is installed) and every build takes it from there.

A book that does not build is reported and left out, as the release page has always done: the
licences are not optional, and a failure there stops the run. `--no-books` skips the PDFs (a
checkout without Chrome).
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "app" / "docs"
BOOKS = (("manual", "manual.pdf"), ("server", "server-guide.pdf"))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--no-books", action="store_true", help="leave the PDFs out")
    args = parser.parse_args()

    if DOCS.exists():
        shutil.rmtree(DOCS)
    DOCS.mkdir(parents=True)
    for name in ("LICENSE-MIT", "LICENSE-APACHE", "LICENSE-CC-BY-4.0"):
        shutil.copyfile(ROOT / name, DOCS / f"{name}.txt")
    licences = ROOT / "tools" / "third_party_licenses.py"
    subprocess.run(
        [sys.executable, str(licences), str(DOCS / "THIRD-PARTY-LICENSES.txt")], check=True
    )
    if not args.no_books:
        build = ROOT / "tools" / "manual" / "build.py"
        for book, name in BOOKS:
            made = subprocess.run(
                [sys.executable, str(build), "pdf", book, str(DOCS / name)], check=False
            )
            if made.returncode != 0:
                print(f"{name} did not build: these files go without it", file=sys.stderr)
    for path in sorted(DOCS.iterdir()):
        print(f"{path.relative_to(ROOT).as_posix()}: {path.stat().st_size:,} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
