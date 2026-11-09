# RadioPost core

The Rust workspace of RadioPost's programs (design D27, D28). So far it holds one crate:

| Crate | What it is |
|---|---|
| `radiopost` | RadioPost's objects as the specifications define them: deterministic CBOR (RP-0 §4.1), key IDs, signatures with a context and COSE_Sign1 (RP-0 §5), content IDs (RP-0 §6.3), messages and their air forms (RP-2), credentials (RP-1 §4, §5), session statements (RP-1 §6), and PDU framing (RP-3 §2, §4). |

Phase 1 adds the session engines (RP-3), the modem interface's host and adapters (RP-4), the
client, and the conformance tester's first half ([its outline](../docs/design/01-conformance.md)).

## Checks

```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

`tests/vectors.rs` replays the test vectors in `../vectors/`: it makes every object and compares
the bytes, checks every case and compares the verdict, and refuses every invalid item. A mismatch
is a bug here, never a reason to change a vector (`vectors/README.md`). A new check is shown to
fail on a planted fault before it is trusted, as `make.py`'s are.

Code is under MIT or Apache 2.0, at the user's option; every file starts with an SPDX line.
