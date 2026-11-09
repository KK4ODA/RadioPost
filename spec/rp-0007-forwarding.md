# RP-7: Forwarding between gateways

| | |
|---|---|
| **Status** | Outline |
| **Version** | 0.0.2 |
| **Updated** | 2026-11-09 |
| **Editor** | KK4ODA |
| **Licence** | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| **Depends on** | RP-0, RP-1, RP-2, RP-3, RP-5 |

How gateways carry mail to each other **by radio** when the Internet is down: out of an area that
has lost it, and back in. This is an outline: it fixes the scope and the parts already decided,
and lists what must be settled before it can be written. It comes last in the order of work
(concept §12), after the session protocol has been tried on the air.

---

## 1. Scope

* A gateway that cannot reach a post office forwards the mail in its queue to a gateway that can.
* A post office that cannot reach a user's usual gateways over the Internet sends their mail to
  the gateway its **radio route** names, which forwards it by radio.
* Two users of the same cut-off gateway exchange mail through it directly, and their post offices
  are told afterwards.

---

## 2. Already decided

* **The same session protocol.** A forwarding session is an RP-3 session with `role` 1: the calling
  gateway is the caller, signs with its own device key and gateway credential (RP-1 §3), and offers
  the messages it carries.
* **Proof travels with each message.** Every forwarded message is accompanied by its proof (RP-1
  §6.4): the original sender's credential, statement and signature. A `PROOF` PDU (RP-3 §4, type
  11) carries it. A forwarding gateway can carry a message but cannot alter it or forge its sender.
* **Duplicates are harmless.** Messages are named by content (RP-2 §3); a station that already
  holds a message declines it (`already-have`).
* **The path is recorded.** Each gateway that carries a message adds itself to the message's path,
  which ends up in the `Radio-Path` header (RP-6 §3.2) and stops loops.
* **The rules.** An unattended gateway starts forwarding transmissions only where automatic control
  is allowed (US: §97.221(b)); its modem's checks, or its own, enforce that (RP-4 §8).

---

## 3. Radio routes *(draft format)*

A post office publishes its routes at `GET /routes` (RP-5 §4.5):

```json
{"issued": 1794312000,
 "routes": [{
   "users_at": ["N2CALL-5", "KK4ODA-1"],
   "via": "N1CALL-10",
   "channel": {"dial_hz": 7101000, "mode": "USB", "modem": "aether", "bandwidth_hz": 2300},
   "schedule": {"every_s": 3600, "at_s": 900}
 }]}
```

Read: "when the gateways in `users_at` cannot be reached over the Internet, send their users'
mail to `via`, which forwards it by radio on `channel`, every `every_s` seconds starting `at_s`
past the hour."

---

## 4. To be settled

* The `PROOF` PDU's layout, and whether a proof covering several messages is sent once.
* Routing: how a gateway with no Internet chooses where to forward, beyond a post office's routes;
  whether gateways publish what they can reach.
* Schedules: who agrees them; how a gateway knows its partner is listening; what happens when a
  window is missed.
* Expiry: how long a forwarding gateway keeps mail it could not pass on, and what the sender is told.
* Reporting: how the post office learns, afterwards, what was delivered locally or forwarded.
* Bands and segments for unattended forwarding, per country.

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.0.1 | 2026-11-09 | Outline. |
| 0.0.2 | 2026-11-09 | Licence stated: CC BY 4.0. |
