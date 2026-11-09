# RP-4: Modem interface

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.1.1 |
| **Updated** | 2026-11-09 |
| **Editor** | KK4ODA |
| **Licence** | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| **Depends on** | RP-0 |
| **Protocol version** | 1 |

How a RadioPost program (a client or a gateway, here the **host**) drives a modem: calling,
answering, sending and receiving bytes, hanging up, and hearing what the modem reports. The host
never talks to a modem's own interface; it talks to an **adapter** in one common language, the
**RadioPost Modem Interface** (RPMI), so that any modem can carry RadioPost once someone writes an
adapter for it, and a modem may speak RPMI itself.

![The modem interface](fig/rp4-modem-interface.svg)

---

## 1. Model

* An **adapter** is a program, or a part of one, that speaks RPMI on one side and a particular
  modem's interface on the other. It MAY run as its own process, be built into the host, or be the
  modem itself.
* A **link** is one connection to one remote station: a reliable, ordered, two-way byte stream, as
  RP-3 §1 needs. The adapter names each link with a string ID.
* A modem that carries datagrams (one-to-many frames, no connection) MAY offer **broadcasts**.

What the host is told about the radio, and when it is told, is all in RPMI. Keying the radio,
choosing frequencies and modulation, retransmission and turnaround are the modem's business.

---

## 2. Transport and framing

The adapter listens; the host connects to it, over **TCP on the loopback interface** (default port
**8600**) or a **Unix domain socket**. Each host keeps one connection.

Messages are **JSON Lines**: one JSON object (RP-0 §4.2) per line, UTF-8, ended by LF. Every object
has an `op` member naming it. Byte data is in a `data` member in **standard base64** (RFC 4648 §4,
with padding), because adapters are often small scripts and every language has it.

Requests from the host MAY carry an `id` (a string or integer). The adapter answers each request
that has an `id` with `ok` or `error` carrying the same `id`. Events from the adapter carry no `id`
unless they answer a request.

An adapter MUST ignore members it does not know, and answer an unknown `op` with `error`
`unsupported`.

---

## 3. Opening

The host speaks first:

```json
{"op": "hello", "id": 1, "version": [1, 1], "app": "RadioPost gateway 0.1.0"}
```

The adapter answers:

```json
{"op": "hello", "id": 1, "version": 1, "modem": "Aether HF 0.2.0",
 "caps": {"bandwidths": [500, 2300], "max_links": 1, "delivery": true, "broadcast": true,
          "quality": true, "busy": true, "regulatory": true, "max_queue": 65536}}
```

| `caps` member | Meaning |
|---|---|
| `bandwidths` | Bandwidths in Hz the host may ask for in `connect`, if the modem has a choice. |
| `max_links` | How many links may be up at once (HF modems: 1). |
| `delivery` | The adapter reports `delivered` (§5): when the other station holds the bytes. |
| `broadcast` | The adapter carries broadcasts (§6). |
| `quality` | The adapter reports `quality` (§5). |
| `busy` | The adapter reports `busy` (§5). |
| `regulatory` | The modem checks the station's band rules before every transmission (§8). |
| `max_queue` | The most bytes the host should have queued on a link at once (§4, `send`). |

If no version is in common the adapter answers `error` `unsupported-version` and closes.

---

## 4. Requests from the host

| `op` | Members | What the adapter does |
|---|---|---|
| `hello` | `version`, `app` | §3. |
| `callsigns` | `callsigns` (list) | The callsigns the station answers to and may call as; the first is the default. |
| `listen` | `on` (bool) | Whether calls to the station's callsigns are answered. Off until the host turns it on. |
| `connect` | `remote`, `as`?, `bandwidth`?, `id` | Calls `remote` as `as` (default: the first callsign). `ok` means the call is under way; `connected` or `failed` follows. |
| `send` | `link`, `data`, `ref`? | Queues bytes on the link. With `ref`, the adapter later reports `delivered` (if it can) for the last byte of this send. |
| `disconnect` | `link` | Closes the link in order: what is queued goes first, then the modem disconnects. |
| `abort` | `link`? | Ends the link, or a call under way, now. Queued bytes are dropped. |
| `broadcast` | `data`, `ref`? | §6. |
| `status` | — | The adapter answers `status` (below). |

**Flow control.** The host SHOULD keep no more than `max_queue` bytes queued on a link; the adapter
reports `queued` (§5) as the queue drains. RP-3 sessions are small enough that this rarely binds.

`status` answer:

```json
{"op": "status", "id": 7, "listening": true, "callsigns": ["N1CALL-10"], "busy": false,
 "links": [{"link": "a1", "remote": "KK4ODA", "direction": "in", "queued": 0}]}
```

---

## 5. Events from the adapter

| `op` | Members | When |
|---|---|---|
| `incoming` | `remote`, `local` | A call to the station is being answered (optional; for display). |
| `connected` | `link`, `remote`, `local`, `direction` (`in` or `out`), `bandwidth`? | A link is up. For an outgoing call, it answers the `connect` (and carries its `id`). |
| `failed` | `id`, `code`, `text`? | An outgoing call ended without a link (no answer, refused, aborted). |
| `received` | `link`, `data` | Bytes arrived on a link, in order. |
| `queued` | `link`, `bytes` | Bytes still queued on the link, when it changes. `0` means everything sent has been handed over (and, with `delivery`, acknowledged). |
| `delivered` | `link`, `ref` | The other station holds every byte up to the end of the send with this `ref`. Only with `delivery`. |
| `disconnected` | `link`, `reason`, `text`? | The link is gone. `reason`: `local` (the host asked), `remote` (the other station hung up), `timeout`, `failure`. |
| `quality` | `link`?, `snr_db`?, `rate_bps`?, `receiving`? | What the modem knows about the path: the last SNR heard, the data rate it is using, and whether it is receiving right now (RP-3 §8 uses this to tell a slow link from a dead one). Only with `quality`. |
| `busy` | `busy` (bool) | Whether the channel is in use by others. Only with `busy`. |
| `heard` | `callsign`, `snr_db`?, `bandwidth`? | A station was heard (a beacon, a call to someone else). Optional. |
| `broadcast` | `source`, `data`, `snr_db`? | §6. |
| `broadcast-sent` | `ref`, `sent` (bool), `text`? | §6. |
| `error` | `id`?, `code`, `text`? | A request failed, or something went wrong. `code` is from §7. |

The adapter MUST report every link it brings up with `connected` and every link it loses with
`disconnected`, so the host never has to guess.

---

## 6. Broadcasts

A modem that can send a frame to everyone listening (Aether's datagrams, AX.25 UI frames) MAY offer
`broadcast`. The host sends `{"op": "broadcast", "data": "…", "ref": "b7"}`; the adapter answers
`broadcast-sent` when the frame has gone out or will not. Broadcasts heard arrive as `broadcast`.

RadioPost does not use broadcasts yet. They are reserved for bulletins and position reports in a
later specification.

---

## 7. Error codes

Adapters use RP-0 §9.3's codes where one fits, and these words in `code` otherwise:

| `code` | Meaning |
|---|---|
| `unsupported` | The request is not known, or the modem cannot do it. |
| `unsupported-version` | No RPMI version in common. |
| `no-modem` | The adapter cannot reach its modem. |
| `not-connected` | No such link. |
| `refused` | The modem refused (a rule, a setting, the band). `text` says why. |
| `busy` | The modem is doing something else (a call already under way). |

---

## 8. The rules on the air

An adapter MUST NOT make the modem transmit except to carry out the host's requests (calls,
answers when `listen` is on, data, disconnects, broadcasts) and what the modem itself needs for
them (acknowledgements, identification).

Whether a transmission is allowed on this band, at this time, under this kind of control, is the
station's responsibility. When the modem checks it (`caps.regulatory`, as Aether does), a refusal
arrives as `error` or `failed` with `refused`. When it does not, the host (the gateway program)
MUST check it, or its operator must, before turning on `listen` or calling (RP-0 §7).

---

## 9. Mapping to modems *(informative)*

How an adapter can carry RPMI on today's modems. These notes guide adapter writers; each modem's
own documentation is the authority on its interface.

### 9.1 Aether

Aether's control API (JSON over WebSocket, `ws://127.0.0.1:8515/`) maps one to one:

| RPMI | Aether control API |
|---|---|
| `callsigns` | `callsigns.set {callsigns}` |
| `listen` | `listen {enabled: true}`. Aether answers every call to its callsigns while no host program is attached, and refuses `enabled: false` today (`unsupported`); until it can, the adapter answers `listen off` with `error` `unsupported`. |
| `connect` | `connect {remote, callsign}`; `state` events `connected` / `disconnected`. |
| `send` with `ref` | `send {data, ref}`; the `sent` event with `delivered: true` is RPMI's `delivered`. |
| `disconnect` / `abort` | `disconnect` / `abort`. |
| `received` | The `data` event. |
| `queued` | `metrics.queued_bytes`. |
| `quality` | `metrics` (`snr_db`, `throughput_bps`, `receiving`). |
| `busy` | `status.channel_busy`, the `metrics` events. |
| `broadcast` | `datagram.send`; the `datagram` and `datagram-sent` events. |

Aether reports `delivery`, `quality`, `busy` and `regulatory`. A future Aether may speak RPMI
natively.

### 9.2 VARA HF and VARA FM

VARA's published TNC command list (EA5HVK, *VARA Protocol Native TNC Commands*) uses a command port
(text lines) and a data port (raw bytes), on two TCP ports:

| RPMI | VARA |
|---|---|
| `callsigns` | `MYCALL <call> [<call>…]` |
| `listen` | `LISTEN ON` / `LISTEN OFF` |
| `connect` | `CONNECT <as> <remote>`; `CONNECTED …` or `DISCONNECTED` follows |
| `send` | write to the data port; `BUFFER <n>` gives `queued` |
| `received` | read from the data port |
| `disconnect` / `abort` | `DISCONNECT` / `ABORT` |
| `bandwidth` | `BW500`, `BW2300`, `BW2750` before the call |
| `busy` | `BUSY ON` / `BUSY OFF` |
| `quality` | `SN <dB>` lines, where sent |

VARA reports no delivery: `BUFFER 0` means sent, not acknowledged. VARA itself is proprietary; the
adapter only uses its published commands.

### 9.3 ARDOP

ARDOP's host interface also has a command port and a data port. Calls are `ARQCALL <remote>
<repeats>`, listening is `LISTEN TRUE|FALSE`, the bandwidth is `ARQBW`, and the data port carries
length-prefixed blocks. *To be checked against the ARDOP specification before an adapter is
written.*

### 9.4 AX.25 packet (VHF and UHF)

Connected-mode AX.25 gives the link RP-3 needs. Direwolf, and many packages that sit on a KISS TNC,
offer it through the **AGWPE** protocol: register a callsign (`X`), connect (`C`), send connected
data (`D`), disconnect (`d`), with `Y` reporting frames still outstanding. UI frames carry
broadcasts.

### 9.5 TCP with TLS

For the Internet and mesh networks (AREDN). `remote` is a host and port
(`gw.example.net:8774`) or a name the adapter maps to one; the link is a TLS connection; no
callsigns are keyed. Useful for testing, and for a client with Internet that wants the same protocol
as by radio.

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.1.0 | 2026-11-09 | First draft. |
| 0.1.1 | 2026-11-09 | Licence stated: CC BY 4.0. |
