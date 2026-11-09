# RadioPost core

The Rust workspace of RadioPost's programs (design D27, D28). So far it holds five crates; the
adapter in `../adapters/` and the client in `../app/` are members here, so that one lockfile and one
CI job hold them all:

| Crate | What it is |
|---|---|
| `radiopost` | RadioPost's objects as the specifications define them: deterministic CBOR (RP-0 §4.1), key IDs, signatures with a context and COSE_Sign1 (RP-0 §5), content IDs (RP-0 §6.3), messages and their air forms (RP-2), credentials (RP-1 §4, §5), session statements (RP-1 §6), PDU framing (RP-3 §2, §4), and the session protocol's engines (RP-3): the caller (`session::client`) and a client that answers a call (`session::called`, RP-3 §10). |
| `radiopost-client` (`../app/client`) | The client ([its README](../app/client/README.md)): the device key and credential, the trusted post offices, the mail and its folders, and RP-3 sessions over any RPMI adapter, as the `radiopost` program, on the command line or as a panel in a browser (`../app/ui`). A station-to-station session's report (RP-5 §6.6) is kept for when there is a post office to send it to. |
| `radiopost-aether` (`../adapters/aether`) | The RPMI adapter for Aether HF (RP-4 §9.1): each host that connects gets its own WebSocket to Aether's control API, and its callsigns are a claim there, so several hosts share one station. `bridge` maps one host onto one Aether connection with no input or output; `serve` and the `radiopost-aether` program put it on a port. |
| `radiopost-issue` (`issuer`) | A post office's key and the credentials it issues (RP-1 §4), for on-air tests before the post office service exists (Phase 2): `radiopost-issue key`, `issue`, `trust`. The vectors' keys are published and never go on the air. |
| `rpmi` | The modem interface (RP-4): its lines and requests (`wire`, standard base64 in `base64`), the host's side (`host`: requests numbered on a connection, the adapter's lines read as events, and `Carrier`, which joins one RP-3 session to one link), a simulated channel of stations and their adapters (`channel`: the loopback adapter, and the start of the conformance tester's test adapter), and that channel served on loopback TCP ports, with a host's connection to any adapter (`tcp`); `rpmi-loopback` is that channel as a program. |

The engines and the channel take bytes or lines and give back bytes, lines and events, with no
input or output, clock or random numbers of their own: the program that drives them brings the
link, the time, the nonces and the user's choices. Only `rpmi::tcp` and the adapter's `serve` do
input and output, besides the programs. Phase 1 still adds the conformance tester's first half
([its outline](../docs/design/01-conformance.md)).

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

In `radiopost-aether`, `tests/bridge.rs` checks each row of RP-4 §9.1 against a stand-in that
answers as Aether's control API specification says, and replays `rpmi-lines.json` through the
bridge. `tests/aetherd.rs` runs real Aether daemons, and so only when `AETHERD` names one (0.2.0-beta.146
or later): session-03 crosses two daemons joined by Aether's simulated channel, byte for byte, and
two hosts share one station.

```
AETHERD=/path/to/aetherd cargo test -p radiopost-aether --test aetherd -- --ignored
```

In `radiopost-client`, `tests/two_clients.rs` has two clients replay session-03 byte for byte
through the loopback adapter, ending with the folders the session says and the reports
`direct.json` has, and runs the `radiopost` program itself between two homes.

A mismatch is a bug here, never a reason to change a vector (`vectors/README.md`). A new check is shown to
fail on a planted fault before it is trusted, as `make.py`'s are.

Code is under MIT or Apache 2.0, at the user's option; every file starts with an SPDX line.
