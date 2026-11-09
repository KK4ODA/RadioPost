# RadioPost core

The Rust workspace of RadioPost's programs (design D27, D28). So far it holds one crate:

| Crate | What it is |
|---|---|
| `radiopost` | RadioPost's objects as the specifications define them: deterministic CBOR (RP-0 §4.1), key IDs, signatures with a context and COSE_Sign1 (RP-0 §5), content IDs (RP-0 §6.3), messages and their air forms (RP-2), credentials (RP-1 §4, §5), session statements (RP-1 §6), PDU framing (RP-3 §2, §4), and the session protocol's engines (RP-3): the caller (`session::client`) and a client that answers a call (`session::called`, RP-3 §10). |

The engines take bytes and give back bytes and events, with no input or output, clock or random
numbers of their own: the program that drives them brings the link, the time, the nonces and the
user's choices. Phase 1 adds the modem interface's host and adapters (RP-4), the client, and the
conformance tester's first half ([its outline](../docs/design/01-conformance.md)).

## Checks

```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

`tests/vectors.rs` replays the test vectors in `../vectors/`: it makes every object and compares
the bytes, checks every case and compares the verdict, and refuses every invalid item.
`tests/sessions.rs` plays each side of `session-01` to `session-03`, fed the other side's bytes one
at a time, and requires its own side's bytes exactly, and nothing early; it also joins the two
engines with no vector between them. A mismatch
is a bug here, never a reason to change a vector (`vectors/README.md`). A new check is shown to
fail on a planted fault before it is trusted, as `make.py`'s are.

Code is under MIT or Apache 2.0, at the user's option; every file starts with an SPDX line.
