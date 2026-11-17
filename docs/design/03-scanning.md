# RadioPost: gateways that scan (a proposal)

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.1.2 |
| **Updated** | 2026-11-17 |
| **Editor** | KK4ODA |
| **Licence** | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| **Follows** | [The wishlist](02-wishlist.md), W50 |

A gateway with one radio could listen on several channels, a while on each, as many Winlink
gateways do with RMS Trimode, and a client could find it on any of them. This document proposes
how. The author accepted it on 2026-11-17 (§8). RP-4 and RP-5, both Stable, change by RP-0 §8.4:
announced MINOR changes, each with its test vectors, once Aether's scan has passed its harness.

---

## 1. Why not Trimode's way

Trimode is a program separate from the modem. It tunes the radio and turns the modem's answering
off and on every 3.5 s or so, deaf for about half a second each time, even on one channel. Run
through Aether's VARA-compatible port, that design has needed five fixes in Aether (the
handshake between a held `CONNECTED`, `PENDING`, `PTT` and the program's `LISTEN`), and it still
loses calls that land in the deaf half second. Some modems end a call under way when answering is
turned off (Mercury's does, RP-4 §9.6).

The cause is that the scanner sits outside the modem. It cannot see a frame arriving. It cannot
see audio from the last channel still in the receiver. It can only switch answering off. RPMI has
the same view of the modem: `busy` and `incoming`, but no "a frame is arriving now". A RadioPost
gateway that scanned by cycling `listen` and `tune` would rebuild the same races.

Aether's ADR-0129
([docs/adr/0129-the-station-scans-for-a-program.md](https://github.com/KK4ODA/aether-hf/blob/master/docs/adr/0129-the-station-scans-for-a-program.md))
gives the analysis, the rules and their timing.

## 2. Who does what

* **The modem scans.** It moves the radio itself, on the same thread that hears frames and keys
  the radio. It never leaves a dial while a frame is arriving or anything is owed there. It stays
  on each dial for a time computed from its own call, and lingers after a frame it could not read.
  Answering stays on throughout.
* **The gateway says where.** It hands the modem its channels, publishes how long a call must
  keep trying to find it, and stops the scan for calls of its own.
* **The client calls long enough.** It asks its modem to keep calling for the time the gateway
  publishes.

In Aether's timing model (its `docs/research/scanning/00-timing.md`), a call of *n* + 2 tries
finds a gateway scanning *n* channels every time on a good path. On a path where each part of a
try decodes half the time, 98–99 % of calls are found. Aether's default of two tries finds such a
gateway 26–63 % of the time. The cost is in the call: at four channels a call nobody answers lasts
about 91 s, not about 42.

## 3. The drafts

### 3.1 RP-4, the modem interface

**§3, `caps`.** Two rows:

> | `caps` member | Meaning |
> |---|---|
> | `scan` | The adapter scans the radio over a list of dials (§4.2). Absent means false. Only with `dial`. |
> | `call_s` | The adapter keeps a call going as long as `connect`'s `call_s` asks (§4). Absent means false. |

**§4, requests.** One row, and a member for `connect`:

> | `op` | Members | What the adapter does |
> |---|---|---|
> | `scan` | `dials` (list), `id` | Listens on each dial in turn (§4.2); an empty list stops it. Answered with `scan`. |
> | `connect` | `remote`, `as`?, `bandwidth`?, `call_s`?, `id` | … With `call_s` and `caps.call_s`, the call's tries keep starting for at least `call_s` seconds. |

*Types:* `dials` is a list of integers, each above 0. `call_s` is an integer, 0 or above.

**§1.1, the bullet *The dial is the station's*,** gains a sentence:

> A scan (§4.2) is the host's that asked. One host scans at a time, and a `tune` from another host
> ends it.

**A new section after §4.1:**

> ### 4.2 Scanning
>
> An adapter with `caps.scan` listens on several dials with one radio, a while on each. The host
> asks:
>
> ```json
> {"op": "scan", "id": 9, "dials": [3590000, 7101000, 10145000]}
> ```
>
> and is answered with the scan as it runs:
>
> ```json
> {"op": "scan", "id": 9, "dials": [3590000, 7101000, 10145000], "scan_s": 27}
> ```
>
> The first dial is the **home** dial. `scan_s` is how long a call's tries must keep starting for
> one of them to be heard, whichever of these dials it is on. A host publishes it (RP-5 §4.4).
>
> The modem chooses how long it stays on each dial, from its own calls. It never leaves a dial
> while a frame is arriving, while an answer, a link or a call is up, or while something owed on
> that dial has not gone, such as the station's identification. Answering is still each host's
> `listen`: scanning changes nothing about it.
>
> A `scan` is refused with `unsupported` without `caps.scan`, and with `busy` while another host's
> scan runs. It is refused with `refused` when a dial is outside rules the modem checks (§8), or
> the radio cannot be tuned there; `text` names the dial.
>
> A `scan` with no dials stops the host's scan, and the radio goes to the home dial once nothing
> holds it. A scan also ends:
>
> * when the host leaves (§1.1);
> * when another host's `tune`, the radio's own knob, or the modem's own operator moves the radio
>   or stops the scan: the operator always wins, and the host does not start the scan again by
>   itself;
> * when fewer than two of its dials are left that the radio will take.
>
> Each end is a `scan` event without an `id`, with no dials and a `why`: `asked`, `left`, `tuned`,
> `knob`, `operator` or `radio`. A dial the radio will not take is dropped from the scan. A `scan` event
> without an `id` then carries the dials left and the new `scan_s`. Every move is a `dial` event
> (§4.1) to every host.
>
> A host that judges the dial itself, because the modem checks no rules (§8), judges every dial
> before it asks. It does not change `listen` on the scan's `dial` events.

**§5, events.** One row:

> | `op` | Members | When |
> |---|---|---|
> | `scan` | `dials`, `scan_s`?, `why`?, `id`? | The answer to `scan`; and, without an `id`, when the scan's dials change or it ends (no dials, and `why`). Only with `caps.scan`. |

**§9.1, Aether.** Rows:

> | RPMI | Aether |
> |---|---|
> | `scan` | `scan.start {dials}`, or `scan.stop` for no dials. `scan_s` is the answer's. `caps.scan` and `caps.call_s` are true when `status` has `scan` (a version with Aether's ADR-0129) and `can_tune`. Aether's `claimed` is `busy`, and its `regulatory` and `unsupported` are `refused` and `unsupported`. |
> | `scan` event | Aether's `scan` event, with its `why` (`connection closed` is `left`; `tuned by another program` is `tuned`; `operator`, the panel's Stop, is `operator`). |
> | `connect` | … `call_s` is passed on as Aether's `connect {call_s}`. |

**The change log:**

> | Version | Date | Change |
> |---|---|---|
> | 1.1.0 | … | Scanning (§4.2, wishlist W50): `caps.scan`, the `scan` request and event, a scan the host's that asked (§1.1); `connect`'s `call_s` and `caps.call_s`; §9.1 Aether's scan (its ADR-0129). MINOR: nothing for an implementation to do. |

**The vectors:** `rpmi-conversations` gains a scan (started, a dial dropped, ended by another
host's `tune`, and ended by the host leaving). The simulated channel of `rpref/rpmi.py` gains a
scan of its own. Each station there already has a dial (`StationConfig.dial_hz`); stations would
hear each other only while on the same dial.

### 3.2 RP-5, the post office API

**§4.4.** A channel may carry `scan_s`:

> ```json
> "channels": [{"dial_hz": 3590000, "mode": "USB", "modem": "aether", "bandwidth_hz": 2300, "scan_s": 14},
>              {"dial_hz": 7101000, "mode": "USB", "modem": "aether", "bandwidth_hz": 2300, "scan_s": 14}]
> ```
>
> A channel with `scan_s` is one of several the gateway listens on in turn with one radio. A call
> on it is heard if its tries keep starting for at least `scan_s` seconds. A client calling the
> gateway there SHOULD ask its modem to keep calling that long (RP-4 §4, `call_s`). A gateway
> SHOULD list only channels it listens on: the dials it scans, and otherwise the dial each modem's
> radio sits on.

**The change log:**

> | Version | Date | Change |
> |---|---|---|
> | 1.1.0 | … | §4.4: a channel's `scan_s`; a gateway lists only channels it listens on (wishlist W50, gap G21). MINOR: nothing for an implementation to do. |

**The vectors:** a gateway with scanned channels in `gateway-lists` and in `api-exchanges`'
registration.

## 4. The gateway

* **Setup.** Setup's channel list (what the gateway registers, RP-5 §6.4) gains **Scan these
  channels** for a modem with `caps.scan` and two or more channels. The first channel is home.
* **Starting.** At start, and whenever the list changes, the gateway sends `scan` with the
  channels' dials. It takes `scan_s` from the answer onto each of those channels, and registers
  again (`PUT /gateways/{callsign}`) when the dials or `scan_s` change, a dial the radio dropped
  included. The log says when a scan starts, changes and ends, but not every move. The panel's
  status shows *Scanning 3 channels, on 7.101 MHz*.
* **Its own calls.** A call in a partner's window (RP-7 §5) stops the scan, tunes, calls, and
  starts the scan again. This replaces tuning back. Its own windows, when they name a dial, take
  the radio for their length the same way.
* **An end it did not ask for** (`tuned`, `knob`, `radio`) is shown and logged. The gateway does
  not start the scan again by itself: the operator moved the radio.
* **Several modems (W26).** Only a modem with `caps.scan` scans. While a session runs on one modem
  the others' `listen` is held, as today. A scan keeps moving while held, and that is harmless.
* **Its own rules check (W40).** With a modem that checks no rules, the gateway judges every dial
  before `scan`, and refuses the list naming the dial outside the segments. During the scan it
  does not change `listen` on `dial` events.

## 5. The client

* **The call.** For a gateway channel with `scan_s`, `connect` carries `call_s` = `scan_s`. The
  panel says *This gateway scans 3 channels: the call may take up to a minute and a half.*
* **A modem without `caps.call_s`.** The client says the call may end before the gateway comes
  round, and calls as today.

## 6. The conformance tester

When this is accepted, `01-conformance.md` gains:

| Test | Checks |
|---|---|
| A-06 | A scan: answered with `scan_s`; a dial outside the rules refused (with `caps.regulatory`); ended by another host's `tune` and by the host leaving, each with its `scan` event. |
| G-10 | A scanning gateway: its record lists only the dials it scans, each with `scan_s`; it stops the scan for a call in a partner's window and starts it again after. |
| C-13 | A client calling a channel with `scan_s` passes it as `call_s`. |

The test adapter needs stations that hear each other only on the same dial.

## 7. What it costs, and what it removes

* **Cost.** Aether's scan and the harness work behind it (its ADR-0129 §6); RP-4 and RP-5 1.1.0
  with their vectors; the gateway; the client; three tests.
* **It removes the listen dance from RadioPost's road.** Nothing tunes a station from outside its
  modem, and RPMI never needs a "do not answer while I tune".
* **It removes channels nobody listens on.** Today a gateway can register channels it never sits
  on (gap G21).
* **It adds no setting to the gateway** but **Scan these channels**. The modem computes the time on
  each dial, and the client takes the call's length from the record.

## 8. The author's answers (2026-11-17)

All four as proposed:


1. **The split:** the modem scans, the gateway says where, the client calls long enough.
2. **G21 is a SHOULD**, so that the change stays MINOR. As a MUST it would be a change a gateway
   cannot ignore, and so a new protocol version (RP-0 §8.3).
3. **The operator wins:** another host's `tune`, or the knob, ends a scan. This is the same
   question as Aether's ADR-0129 §7.
4. **The order:** Aether's scan first, judged on its harness; then RP-4 and RP-5 1.1.0 with their
   vectors; then the gateway, the client and the tests.

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.1.0 | 2026-11-17 | First draft: the author's question how the gateway should scan, as Winlink gateways do with Trimode. The modem scans (Aether's ADR-0129), the gateway says where, the client calls long enough; drafts for RP-4 (`caps.scan`, `scan`, `call_s`) and RP-5 (`scan_s`, gap G21); the gateway, the client, three tests. |
| 0.1.1 | 2026-11-17 | §8: the author accepted it, all four answers as proposed. |
| 0.1.2 | 2026-11-17 | §3.1: a sixth end of a scan, `operator`, as Aether built it: its panel's Stop ends any program's scan and leaves the radio where it is. |
