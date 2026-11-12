# RadioPost core

The Rust workspace of RadioPost's programs (design D27, D28). So far it holds ten crates; the
adapters in `../adapters/` and the client in `../app/` are members here, so that one lockfile and
one CI job hold them all:

| Crate | What it is |
|---|---|
| `radiopost` | RadioPost's objects as the specifications define them: deterministic CBOR (RP-0 §4.1), key IDs, signatures with a context and COSE_Sign1 (RP-0 §5), content IDs (RP-0 §6.3), messages and their air forms (RP-2), credentials (RP-1 §4, §5), session statements (RP-1 §6), PDU framing (RP-3 §2, §4), and the session protocol's engines (RP-3): the caller (`session::client`), a client that answers a call (`session::called`, RP-3 §10), and a gateway online (`session::gateway`, RP-3 §5, §9: it reaches the post office through an `Upstream` the program supplies, called in line). The caller and the client called count what has crossed each way (`session::Progress`), for a program to show while a session runs. Also the DNS records and the trust in post offices they give (`dns`, RP-1 §7.1, §7.3), JWS (`jws`), base64url (`base64url`), and which stations a gateway may hand third-party mail to (`thirdparty`, RP-6 §5.1). |
| `radiopost-api` (`api`) | RP-5 over HTTPS for gateways and clients: logging in with a credential and logging in again when the token runs out, requests and their problem codes, enrollment, and a domain's records read by DNS over HTTPS (RP-1 §7.1). |
| `radiopost-client` (`../app/client`) | The client ([its README](../app/client/README.md)): the device key and credential, the trusted post offices, the mail and its folders, and RP-3 sessions over any RPMI adapter, as the `radiopost` program, on the command line or as a panel in a browser (`../app/ui`). Over the Internet (`online`): enrollment, renewal, settings, the accept list, and the station-to-station sessions reported (RP-5 §6). |
| `radiopost-aether` (`../adapters/aether`) | The RPMI adapter for Aether HF (RP-4 §9.1): each host that connects gets its own WebSocket to Aether's control API, and its callsigns are a claim there, so several hosts share one station. `bridge` maps one host onto one Aether connection with no input or output; `serve` and the `radiopost-aether` program put it on a port. |
| `radiopost-postoffice` (`postoffice`) | A post office's service (design §5.3): the RP-5 API (`office`: accounts, enrollment codes, credentials and renewal, logins and tokens, sessions, mail handed in, pickups, offline gateways' reports, copies placed ahead, the accept list, station-to-station reports), its public documents signed as JWS (`documents`, RP-1 §7, §8), and the service (`serve`): plain HTTP behind an HTTPS front, a loopback port for its operator's commands, its state saved after each request. Email (RP-6): `mail` is the door, an email made into a message, a message into an email, the message part between post offices and bounces (MIME read by Stalwart's `mail-parser`); `email` puts them to work, answering the mail server's MTA hooks and submitting outgoing mail by SMTP (`lettre`). Its operators' side (RP-5 §6.7, §7, design D35): applications and the operator endpoints (`operator`), and around them the service's desk (`desk`): the evidence for each applicant from a copy of the FCC's licence database (`licences`) and QRZ.com (`qrz`), judged in `evidence` and gathered by `lookups`; the mailbox made at approval and the emails (`mailboxes`, `notices`, `aftercare`); the hourly checks and their alerts (`health`), backups sealed with a passphrase (`backups`), and the jobs a root-owned runner does (`jobs`). `radiopost-po` makes a post office (`init` prints its DNS records, `install` sets up a whole Debian server step by step), serves it, and gives enrollment codes ([running one](../docs/user/post-office.md)). |
| `radiopost-gateway` (`gateway`) | A gateway, online (design §5.2): `upstream` reaches a post office's API over HTTPS with the gateway's own credential (or a post office in this process, for tests), `station` answers calls on any RPMI modem (an adapter's port, or Aether's control API through the Aether adapter in this process) and carries each session to the caller's post office, found in its settings or by DNS over HTTPS, keeping the callers' credentials and broken uploads between sessions; `operator` registers the gateway with its operator's post office (RP-5 §6.4), logged in from the operator's own client home. `radiopost-gateway` makes, registers and runs one. Copies placed ahead, the offline queue and forwarding are Phases 3 and 4. |
| `radiopost-mercury` (`../adapters/mercury`) | The RPMI adapter for the Mercury HF modem (RP-4 §9.6), through Mercury's VARA-style TNC interface: commands on its control port, the session's bytes on the next. One host at a time, since Mercury serves one program; the adapter connects when a host says `hello` and lets Mercury go once what the host left on the air has ended. `bridge` maps one host onto Mercury's two ports with no input or output; `serve` and the `radiopost-mercury` program put it on a port. |
| `radiopost-tcp` (`../adapters/tcp`) | The TCP adapter (RP-4 §9.5): RPMI to its host, and links that are TCP connections to other TCP adapters, opened with one line each way. TLS on the Internet (`tls`: the answering adapter's certificate, its own if it has none, checked by the web's roots or by a fingerprint the operators exchange); plain on an amateur mesh, which is on the air. For trying RadioPost between two stations over the Internet before going on the air. |
| `radiopost-issue` (`issuer`) | A post office's key and the credentials it issues (RP-1 §4), for on-air tests before the post office service exists (Phase 2): `radiopost-issue key`, `issue`, `trust`. The vectors' keys are published and never go on the air. |
| `radiopost-tester` (`tester`) | The conformance tester ([its outline](../docs/design/01-conformance.md)): it plays a gateway, or another client's station, against a client through its own RPMI adapter (`harness::TestAdapter`), scripted from RP-3's tables with this workspace's encodings and none of its session engines, and says test by test whether the client did what RP-3 and RP-1 require (C-01 to C-08, C-11, which takes five minutes, and C-12, its station-to-station reports). A driver file says how to run the program under test; `drivers/radiopost.json` is this client's. It plays the caller and the post office behind a gateway (G-01, G-02, G-04, G-07 to G-09; `office`, the test post office and its DNS zone), the users, a gateway, another post office's user and the mail server around a post office (P-01 to P-04, P-07 to P-10; `postoffice`, with a stand-in SMTP port in `mailserver`), and the host, and through a second instance the far station's host, of any modem adapter (A-01 to A-04: RP-4's lines, a conversation, a second host, session-01 carried across). |
| `rpmi` | The modem interface (RP-4): its lines and requests (`wire`, standard base64 in `base64`), the host's side (`host`: requests numbered on a connection, the adapter's lines read as events, and `Carrier`, which joins one RP-3 session to one link), a simulated channel of stations and their adapters (`channel`: the loopback adapter, and the conformance tester's test adapter), and that channel served on loopback TCP ports, with a host's connection to any adapter and a host inside the adapter's own process (`tcp`); `rpmi-loopback` is that channel as a program. |

The engines and the channel take bytes or lines and give back bytes, lines and events, with no
input or output, clock or random numbers of their own: the program that drives them brings the
link, the time, the nonces and the user's choices. Only `rpmi::tcp` and the adapter's `serve` do
input and output, besides the programs and the tester.

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

In `radiopost-mercury`, `tests/bridge.rs` checks each row of RP-4 §9.6 with Mercury's lines given
by hand, and replays `rpmi-lines.json` through the bridge. `tests/conformance.rs` runs A-01 to
A-04 through two adapters on `tests/common`, a stand-in for two Mercury modems on one channel that
keeps to what Mercury's documentation and source say, and logs what a real Mercury would answer
badly (`LISTEN OFF` with a session up, a second `DISCONNECT`, bytes with no session); the test
requires that log empty. `tests/mercury.rs` runs A-01 to A-04 over two real Mercury processes,
their audio joined by named pipes, and so only when `MERCURY` names one (1.9.23 or later, built
with `make HAVE_HAMLIB=0`):

```
MERCURY=/path/to/mercury cargo test -p radiopost-mercury --test mercury -- --ignored
```

In `radiopost-client`, `tests/two_clients.rs` has two clients replay session-03 byte for byte
through the loopback adapter, ending with the folders the session says and the reports
`direct.json` has, and runs the `radiopost` program itself between two homes.

In `radiopost-client`, `tests/conformance.rs` runs the conformance tester's client tests against
the `radiopost` program (all but C-11); in `radiopost-gateway` and `radiopost-postoffice`,
`tests/conformance.rs` runs the gateway's and the post office's against their programs (G-07's
300-second wait by hand, with `--slow`). `tools/tester/plant.py` shows each of those tests failing:
it builds the client once for each with a fault planted, and runs the test against it. Run it by
hand after changing a test or the client; and C-11, which takes five minutes:

```
cargo build -p radiopost-tester -p radiopost-client -p radiopost-tcp
target/debug/radiopost-tester client --driver tester/drivers/radiopost.json \
    --bin target/debug/radiopost --slow
python3 ../tools/tester/plant.py
```

In `radiopost-postoffice`, `tests/vectors.rs` replays every exchange of `api-exchanges.json`,
`api-account.json`, `api-office.json` and `direct.json` from the state each starts from, with the vectors' published
randomness, so tokens, nonces and session IDs come out the same; `tests/service.rs` drives the real
HTTP service through an enrollment, a login, settings, the signed documents and a restart;
`tests/desk.rs` the operator's desk (applications opened, decided, looked up in an FCC copy, jobs,
sealed backups, a restart); `tests/aftercare.rs` a mailbox made in a stand-in Stalwart and the
emails at a stand-in mail server. The
core's `tests/vectors.rs` also replays `jws.json`, `dns.json` and `third-party.json`.
`tests/email.rs` replays `email-door.json`, `email-in.json` (every email, the bounce and the
message part between post offices) and `email-out.json`, and `tests/vectors.rs` judges the door's
`smtp` entries of `api-account.json` against the accept lists as the replay leaves them.
`tests/email_service.rs` drives the running service: the door over HTTP in both shapes of MTA
hook, email out through `lettre` to a stand-in mail server, a recipient refused, and the bounce
that comes back.

In `radiopost-gateway`, `tests/session01.rs` drives the gateway's engine with the client's side of
`session-01`, the post office itself behind it in this process, and requires the gateway's side
byte for byte; then `session-02`'s two sessions through two gateways, the second resuming what the
first held. `tests/online.rs` runs it all for real: the gateway registered from its operator's
client home over HTTP, a user's client calling it through the loopback adapter, the session
carried to the post office service over HTTP, mail going both ways and marked picked up.

In `radiopost-tcp`, `tests/two_adapters.rs` joins two adapters, plain and over TLS, and checks
the links and each refusal; `tests/conformance.rs` runs A-01 to A-04 against two of them. The
fault script covers A-01 to A-04 too, planted in the TCP adapter. The same tests run against any
adapter by hand, here the Aether adapter over two daemons joined by Aether's simulated channel
(the waits allow a radio's timing, so A-02 and A-04 take minutes there):

```
target/debug/radiopost-tester adapter --at 127.0.0.1:8600 --far 127.0.0.1:8601
```

A mismatch is a bug here, never a reason to change a vector (`vectors/README.md`). A new check is shown to
fail on a planted fault before it is trusted, as `make.py`'s are.

Code is under MIT or Apache 2.0, at the user's option; every file starts with an SPDX line.
