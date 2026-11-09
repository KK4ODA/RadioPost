# SPDX-License-Identifier: MIT OR Apache-2.0
"""Shows that each of the conformance tester's client tests can fail (01-conformance.md §5).

For each test, it copies the repository, plants one fault in the client's copy, builds that
client, and runs the tester's test against it: the test must fail. Run it by hand after changing
a test or the client; it builds the client once a fault, so it takes a few minutes, and C-11 one
more (its fault makes the client hang up after 60 s).

    python3 tools/tester/plant.py [C-01 C-02 ...]

It needs `core/target/debug/radiopost-tester` (`cargo build -p radiopost-tester` in `core/`).
"""

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
ENGINE = "core/radiopost/src/session/client.rs"
CALLED = "core/radiopost/src/session/called.rs"

# test: (file, text, the text with the fault)
FAULTS = {
    "C-01": (ENGINE, "(15, Value::bytes(&signature)),", "(15, Value::bytes(&[0u8; 64])),"),
    "C-02": (
        ENGINE,
        ".statement(Form::Challenge(challenge))",
        ".statement(Form::Time { time: self.cfg.time, nonce: self.cfg.nonce })",
    ),
    "C-03": (ENGINE, "if self.cfg.credential_cached {", "if false && self.cfg.credential_cached {"),
    "C-04": (ENGINE, "if !self.cfg.partials.is_empty() {", "if false {"),
    "C-05": (
        ENGINE,
        "let (_, offset) = accepted.iter().find(|(id, _)| *id == out.id())?;",
        "let offset = &accepted.iter().find(|(id, _)| *id == out.id()).map_or(0, |(_, o)| *o);",
    ),
    "C-06": (
        ENGINE,
        "Some(Err(code)) => self.fail(code),",
        "Some(Err(code)) => self.fail(if code == ErrorCode::Malformed "
        "{ ErrorCode::Internal } else { code }),",
    ),
    "C-07": (
        ENGINE,
        "statement::verify(&stmt, sig, &body, &self.me.operator, &freshness)?;",
        "let _ = statement::verify(&stmt, sig, &body, &self.me.operator, &freshness);",
    ),
    "C-08": (
        CALLED,
        "if message.recipients().contains(&self.me.address.as_str()) {",
        "if true {",
    ),
    "C-11": (
        "app/client/src/session.rs",
        "pub const QUIET: Duration = Duration::from_secs(300);",
        "pub const QUIET: Duration = Duration::from_secs(60);",
    ),
}


def plant(test: str, scratch: Path) -> str:
    """Builds the client with `test`'s fault and runs the test; what came of it."""
    path, text, faulty = FAULTS[test]
    work = scratch / "tree"
    shutil.rmtree(work, ignore_errors=True)
    shutil.copytree(
        REPO, work, ignore=shutil.ignore_patterns("target", ".git", "node_modules", "__pycache__")
    )
    source = work / path
    original = source.read_text()
    if original.count(text) != 1:
        return f"MISSED: the fault's text is not in {path} exactly once"
    source.write_text(original.replace(text, faulty))
    target = scratch / "target"
    built = subprocess.run(
        ["cargo", "build", "-q", "-p", "radiopost-client"],
        cwd=work / "core",
        env={**os.environ, "CARGO_TARGET_DIR": str(target)},
        capture_output=True,
        text=True,
        check=False,
    )
    if built.returncode != 0:
        return "MISSED: the faulty client did not build: " + built.stderr[-600:]
    ran = subprocess.run(
        [
            str(REPO / "core/target/debug/radiopost-tester"),
            "client",
            "--driver",
            str(REPO / "core/tester/drivers/radiopost.json"),
            "--bin",
            str(target / "debug/radiopost"),
            "--only",
            test,
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    lines = ran.stdout.splitlines()
    caught = ran.returncode == 1 and any(ln.startswith(f"{test} FAIL") for ln in lines)
    why = lines[1].strip() if len(lines) > 1 else ran.stderr.strip()
    return ("caught: " if caught else "MISSED: ") + why


def main() -> int:
    tests = sys.argv[1:] or list(FAULTS)
    missed = []
    with tempfile.TemporaryDirectory(prefix="radiopost-plant-") as scratch:
        for test in tests:
            verdict = plant(test, Path(scratch))
            print(test, verdict, flush=True)
            if not verdict.startswith("caught"):
                missed.append(test)
    print("missed:", ", ".join(missed) or "none")
    return 1 if missed else 0


if __name__ == "__main__":
    sys.exit(main())
