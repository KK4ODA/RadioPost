# The RadioPost client

`radiopost` is the RadioPost client: the user's device key and credential (RP-1), their mail
(RP-2), and sessions (RP-3) over any modem that has an RPMI adapter (RP-4). It runs on the command
line, or as a panel in a browser (`radiopost panel`); the desktop application (Tauri, design D27)
will show the same panel in its own window.

Phase 1 carries mail **station to station** (RP-3 §10). Calling a gateway works as RP-3 §5 says,
but there is no gateway to call until Phase 2, nor yet enrollment (RP-5 §6.1): a credential is
imported as a post office issued it.

## Commands

```
radiopost [--home DIR] COMMAND
```

| Command | Does |
|---|---|
| `key` | Makes this device's key, once, and prints its public key for enrollment. |
| `init --credential FILE [--device-key FILE]` | Keeps the credential a post office issued for this device's key (raw or hex). |
| `trust DOMAIN PUBLIC_HEX [NOT_BEFORE NOT_AFTER]` | Trusts a post office key (RP-1 §7.3: the operator decides). |
| `whoami` | The account and callsign the credential is for. |
| `compose --to ADDR … [--cc ADDR] [--subject TEXT] [--priority 0\|1\|2] [--attach FILE]… [--body FILE]` | Writes a message into the outbox; the text from standard input without `--body`. |
| `import FILE` | Puts a message in its canonical encoding (RP-2) in the outbox. |
| `outbox`, `inbox`, `sent` | Lists a folder. |
| `show ID` | Shows a message; `ID` is the start of its content ID. |
| `reports` | The station-to-station sessions waiting to be reported to the post office (RP-5 §6.6). |
| `call CALLSIGN [--station] [--fetch all\|none\|BYTES]` | Calls a gateway, or with `--station` another client's station. |
| `listen` | Answers one station's call. |
| `panel [--port N] [--ui DIR]` | Serves the panel on this computer (port 8620) and says where to open it. |

`call` and `listen` take `--adapter HOST:PORT` (the modem's RPMI adapter, `127.0.0.1:8600` by
default) and `--bandwidth HZ`. `--plain` sends messages uncompressed; `--clock`, `--nonce`,
`--session-id` and `--challenge` fix the clock and the random draws, so that a session can be
replayed exactly (the conformance tester's §2.4).

## The panel

`radiopost panel` serves the page in `app/ui/` (plain HTML, CSS and JavaScript, no build step,
light or dark as the system is) and a JSON API under `/api/`, on the loopback interface only, as
the Aether app serves its own. It shows the folders, reads and writes messages with attachments,
calls a station or listens for one, and holds the setup: the device key, the credential and the
post offices trusted. A session runs on a thread of its own; while it runs, the panel changes
nothing in the home. `--ui DIR` serves the page from a directory, for working on it.

Only this computer's own pages reach it: a request must name the panel's own host, so a page that
rebinds its name to 127.0.0.1 is refused, and a change must carry `X-RadioPost: 1`, which another
site's page cannot send without a preflight the panel never grants.

## Two stations on one machine

`rpmi-loopback` (in `core/rpmi`) is a simulated channel whose stations are reached over TCP. With
the test identities of `vectors/` (their keys are published, so they are never real):

```
rpmi-loopback --station KK4ODA@127.0.0.1:8600 --station N0CALL@127.0.0.1:8601 &
radiopost --home facundo init --device-key facundo.key --credential facundo.cred
radiopost --home ana init --device-key ana.key --credential ana.cred
radiopost --home facundo trust radiopost.org <key> ; radiopost --home ana trust radiopost.org <key>
…                                                     # and hampost.org's key, for each
echo "All well here." | radiopost --home facundo compose --to ana@hampost.org --subject Hola
radiopost --home ana listen --adapter 127.0.0.1:8601 &
radiopost --home facundo call N0CALL --station --adapter 127.0.0.1:8600
radiopost --home ana inbox
```

On the air, the adapter is the modem's: `radiopost-aether` for Aether HF (`adapters/aether`).

## What is kept

The home (`--home`, else `$RADIOPOST_HOME`, else `.radiopost` in the user's home directory) holds
the device seed, the credential, the trusted keys, each message's canonical encoding named by its
content ID, partial messages to resume (RP-3 §7) and `state.json`: the folders, the next serial,
the station called's replay cache (RP-1 §6.3), what each gateway holds, and the reports. Files are
written whole or not at all, and a message received is on disk before its `RECEIPT` goes out (RP-3
§5.4).

A message a station takes moves to `sent`. Its other recipients get it when the session is
reported to the post office (D30, RP-5 §6.6), not over the air again; each session's report, with
both stations' proofs, waits in `state.json` until the client can send it (Phase 2).

## Tests

`tests/panel.rs`: the panel's guards, a message written with an attachment and carried by a call
made from the panel, and listening stopped. `tests/two_clients.rs`: two clients replay session-03
byte for byte through the loopback adapter
with the vector's clocks and draws, and end with the folders the session says and the reports
`vectors/direct.json` has; the program itself sets up two homes, composes, listens and calls; and it
refuses a credential for another device's key.
