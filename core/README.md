# RadioPost core

The Rust workspace of RadioPost's programs (design D27, D28). So far it holds two crates:

| Crate | What it is |
|---|---|
| `radiopost` | RadioPost's objects as the specifications define them: deterministic CBOR (RP-0 §4.1), key IDs, signatures with a context and COSE_Sign1 (RP-0 §5), content IDs (RP-0 §6.3), messages and their air forms (RP-2), credentials (RP-1 §4, §5), session statements (RP-1 §6), PDU framing (RP-3 §2, §4), and the session protocol's engines (RP-3): the caller (`session::client`) and a client that answers a call (`session::called`, RP-3 §10). |
| `rpmi` | The modem interface (RP-4): its lines and requests (`wire`, standard base64 in `base64`), the host's side (`host`: requests numbered on a connection, the adapter's lines read as events, and `Carrier`, which joins one RP-3 session to one link), a simulated channel of stations and their adapters (`channel`: the loopback adapter, and the start of the conformance tester's test adapter), and that channel served on loopback TCP ports, with a host's connection to any adapter (`tcp`). |

The engines and the channel take bytes or lines and give back bytes, lines and events, with no
input or output, clock or random numbers of their own: the program that drives them brings the
link, the time, the nonces and the user's choices. Only `rpmi::tcp` does input and output. Phase 1
still adds the Aether adapter (RP-4 §9.1), the client program, and the conformance tester's first
half ([its outline](../docs/design/01-conformance.md)).

## Checks

```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

In `radiopost`, `tests/vectors.rs` replays the test vectors in `../vectors/`: it makes every object
and compares the bytes, checks every case and compares the verdict, and refuses every invalid item.
`tests/sessions.rs` plays each side of `session-01` to `session-03`, fed the other side's bytes one
at a time, and requires its own side's bytes exactly, and nothing early; it also joins the two
engines with no vector between them.

In `rpmi`, `tests/vectors.rs` has the channel play every adapter of `rpmi-lines.json` and
`rpmi-conversations.json`: given the host lines, it must write the adapter lines (as JSON values,
`text` aside), each before the next host line. The client's host plays its side of session-01 over
RPMI with the real session engine and must write the client's lines, each before the next adapter
line. `tests/stations.rs` carries session-03 between its two engines through the channel, in
memory with `received` cut at 1, 7, 128 and any number of bytes, and over TCP, and requires
session-03's transmissions byte for byte, each `send` numbered as the session's transmission.

A mismatch is a bug here, never a reason to change a vector (`vectors/README.md`). A new check is shown to
fail on a planted fault before it is trusted, as `make.py`'s are.

Code is under MIT or Apache 2.0, at the user's option; every file starts with an SPDX line.
