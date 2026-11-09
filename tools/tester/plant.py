# SPDX-License-Identifier: MIT OR Apache-2.0
"""Shows that each of the conformance tester's tests can fail (01-conformance.md §5).

For each test, it copies the repository, plants one fault in the copy of the program the test is
for (the client, or the TCP adapter), builds it, and runs the tester's test against it: the test
must fail. Run it by hand after changing a test or those programs; it builds once a fault, so it
takes a few minutes, and C-11 one more (its fault makes the client hang up after 60 s).

    python3 tools/tester/plant.py [C-01 A-02 ...]

It needs `core/target/debug/radiopost-tester` (`cargo build -p radiopost-tester` in `core/`).
"""

import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
ENGINE = "core/radiopost/src/session/client.rs"
CALLED = "core/radiopost/src/session/called.rs"
TCP = "adapters/tcp/src/lib.rs"

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
    "A-01": (TCP, '("busy", false.into()),', ""),
    "A-02": (TCP, '("direction", "out".into()),', '("direction", "in".into()),'),
    "A-03": (TCP, "if owner.is_some_and(|o| o != c) {", "if false {"),
    "A-04": (TCP, "rpmi::base64::encode(&buf[..n])", "rpmi::base64::encode(&buf[n / 2..n])"),
}


def plant(test: str, scratch: Path) -> str:
    """Builds the program with `test`'s fault and runs the test; what came of it."""
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
    package = "radiopost-tcp" if test.startswith("A-") else "radiopost-client"
    built = subprocess.run(
        ["cargo", "build", "-q", "-p", package],
        cwd=work / "core",
        env={**os.environ, "CARGO_TARGET_DIR": str(target)},
        capture_output=True,
        text=True,
        check=False,
    )
    if built.returncode != 0:
        return "MISSED: the faulty program did not build: " + built.stderr[-600:]
    tester = str(REPO / "core/target/debug/radiopost-tester")
    if package == "radiopost-client":
        driver = str(REPO / "core/tester/drivers/radiopost.json")
        ran = run_test(
            [tester, "client", "--driver", driver, "--bin", str(target / "debug/radiopost")], test
        )
    else:
        ran = run_adapter(tester, str(target / "debug/radiopost-tcp"), test)
    lines = ran.stdout.splitlines()
    caught = ran.returncode == 1 and any(ln.startswith(f"{test} FAIL") for ln in lines)
    why = lines[1].strip() if len(lines) > 1 else ran.stderr.strip()
    return ("caught: " if caught else "MISSED: ") + why


def run_test(command: list[str], test: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run([*command, "--only", test], capture_output=True, text=True, check=False)


def run_adapter(tester: str, adapter: str, test: str) -> subprocess.CompletedProcess[str]:
    """Two instances of the adapter, plain TCP on free ports, and the test against them."""
    ports = []
    for _ in range(3):
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            ports.append(s.getsockname()[1])
    near, far, calls = (f"127.0.0.1:{p}" for p in ports)
    quiet = {"stdout": subprocess.DEVNULL, "stderr": subprocess.DEVNULL}
    running = [
        subprocess.Popen([adapter, "--plain", "--rpmi", far, "--listen", calls], **quiet),
        subprocess.Popen([adapter, "--plain", "--rpmi", near], **quiet),
    ]
    try:
        time.sleep(1)
        command = [tester, "adapter", "--at", near, "--far", far, "--reach", f"N1CALL-10@{calls}"]
        return run_test(command, test)
    finally:
        for p in running:
            p.kill()
            p.wait()


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
