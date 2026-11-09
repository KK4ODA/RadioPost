# RP-7: Forwarding between gateways

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.4.0 |
| **Updated** | 2026-11-09 |
| **Editor** | KK4ODA |
| **Licence** | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| **Depends on** | RP-0, RP-1, RP-2, RP-3, RP-5 |

How gateways carry mail to each other **by radio** when the Internet is down: out of an area that
has lost it (*outward*), and back in (*inward*). It uses the session protocol (RP-3) unchanged,
adds one PDU and two keys to it, and adds a few members to the post office API (RP-5).

---

## 1. Model

* A gateway is **cut off** when it cannot reach the post offices whose users it serves. It keeps
  checking users (RP-1 §5, §6.3), queues what they send and serves the copies it holds (RP-5 §5.8),
  as RP-3 §9 already says.
* A cut-off gateway has **partners**: gateways it can reach by radio, named by its operator, at
  least one of which can usually reach the Internet.
* In a **window** (§5) the cut-off gateway **calls a partner**, as a client calls a gateway. That
  one session carries both directions:
  * **outward**, the cut-off gateway's queue, as its offers (§3);
  * **inward**, mail its post offices could not send it over the Internet, which waits at the
    partner (§4).
* A partner that is cut off too keeps what it receives and forwards it the same way to its own
  partners. A message may pass several gateways; each one's callsign joins its **path** (§3.1).

Only the cut-off gateway calls, so only the partner's windows need to be agreed, and only the
station that needs the link starts transmitting.

A cut-off gateway also hands mail between its own users without waiting for their post offices
(§4.4). Not in this version: routing beyond configured partners and published routes (§8).

---

## 2. The forwarding session

A forwarding session is an RP-3 session in which the caller is a gateway:

* `HELLO` `role` is 1. `address` and `operator` are the caller's gateway credential's (RP-1 §3).
  The credential MUST have the `gateway` flag, and `send` when the caller offers messages; the
  partner checks it as RP-1 §5 says, asking for those in step 6. It does not need `receive`: the
  mail that comes back is not the gateway's own.
* The statement (RP-1 §6) signs the caller's offers: the messages it forwards. It is the caller's
  own; the senders' statements travel in `PROOF` PDUs (§3.2).
* `cursor` is the highest **forwarding serial** (§4.3) the caller has received from this partner;
  `auto` is the caller's choice, usually as large as it can take.
* The partner answers as a gateway answers a client, with these differences:
  * it does not hand the offers to a post office in the caller's name: they are forwarded mail
    (§3.3, §3.4);
  * its `waiting` lists the inward mail it holds for the caller (§4.3).

Everything else is RP-3: the five transmissions, resuming (RP-3 §7), and the caller hanging up
(RP-3 §8).

---

## 3. Outward: a gateway's queue

### 3.1 Session records

For every session it carried while cut off, a gateway keeps a **session record**: what RP-5 §5.6
would report.

| Part | Meaning |
|---|---|
| credential | The user's credential (RP-1 §4). |
| statement | The user's statement's encoding (RP-1 §6.2). |
| signature | Its signature. |
| started | When the session started, on the gateway's clock (RP-5 §5.2). |
| gateway session | The RP-3 session ID the gateway gave it (RP-5 §5.2, `gateway_session`). |
| messages | The messages the user sent, as air forms. |
| pickups | The serials the user picked up from copies (RP-5 §5.5). |
| local pickups | The content IDs of the messages the user picked up by local delivery (§4.4). |
| path | The callsigns of the gateways that have held the record, in order. The first is the gateway the user called (the statement's `gateway`); the last is the one holding it now. |

A record's **ID** is the first 12 bytes of SHA-256 of the statement's encoding.

### 3.2 The PROOF PDU

`PROOF` (RP-3 §4, type 11) carries one session record, without its messages. Sent by the caller of
a forwarding session.

| Key | Name | Type | Req. | Meaning |
|---|---|---|---|---|
| 1 | `credential` | bstr | yes | The user's credential. |
| 2 | `statement` | bstr | yes | The user's statement's encoding. |
| 3 | `signature` | bstr .size 64 | yes | Its signature. |
| 4 | `started` | uint | yes | When the user's session started. |
| 5 | `session` | bstr .size 8 | yes | The gateway session ID. |
| 6 | `path` | array of text | yes | The record's path, ending with the caller. |
| 7 | `pickups` | array of uint | no | The serials picked up. |
| 8 | `local` | array of bstr .size 12 | no | The content IDs picked up by local delivery (§4.4). |

The caller offers, in `HELLO`, every message of every record it forwards that the partner has not
yet receipted. In its third transmission it sends, before the `MSG` PDUs, one `PROOF` for each
record whose messages it sends, and one for each record with no messages (pickups or local pickups
only).

### 3.3 What the partner checks

For each `PROOF`, the partner checks, and refuses the record at the first failure:

1. The proof, as RP-1 §6.3 says for a statement relayed later: the credential (RP-1 §5) as it was
   at `started`, with `send` if the statement lists messages and `receive` if the record has
   pickups or local pickups; steps 2 and 4; and the statement's `time` within 600 seconds of `started`. As RP-5 §5.6
   checks a report.
2. The path: its first callsign is the statement's `gateway`, its last is the caller, it holds at
   most 8 callsigns, and it does not hold the partner's own (a loop). Otherwise
   `refused-by-policy`.

For each `MSG`, it checks the message as RP-3 §5.4 says, and then that its content ID is in the
statement of a `PROOF` it took and that its `from` is that statement's address. Otherwise it lists
the message as failed: `not-authorized`, or the code of the failed record.

The partner's `RECEIPT` (RP-3 §5.5) lists messages in `complete` as usual, and the IDs of the
records it has taken in a further key:

| Key | Name | Type | Meaning |
|---|---|---|---|
| 4 | `records` | array of bstr .size 12 | Session records received and stored: the `PROOF`, and every message of it that was offered. |

### 3.4 What the partner does with a record

The partner adds its own callsign to the record's path and stores it. Then:

* if it can reach the senders' post offices, it reports the record there with `POST /reports`
  (RP-5 §5.6), with the `path`;
* otherwise it is cut off too, and forwards the record to its own partners, as its own (§3.2).

### 3.5 When the caller lets go

The caller deletes a message once the partner has listed it `complete`, and a record once the
partner has listed it in `records`. Until then it offers them again in later windows. It keeps the
content ID of each message it passed on, to a partner or to a post office (RP-5 §5.6, `accepted`
or `duplicate`), for 30 days (§3.6).

### 3.6 The sender's side

A message a cut-off gateway receipts is **queued**, not sent: the post office has not seen it. The
gateway may be lost, or drop the record unsent after its 30 days (§7), and it has no way to reach
the sender. Only the sender's client knows what it sent, so the client keeps track.

* **Queued.** A client that hands a message to a gateway whose `WELCOME` said `online` false keeps
  the message's air form, the gateway's callsign and the time, and shows the message as queued
  there. It shows it as **sent** once a gateway that is online lists it `complete` (RP-3 §5.5) or
  declines it `already-have` (RP-3 §5.2): either way the post office holds it.
* **Offered again.** In each later session the client offers each queued message again (RP-3
  `HELLO` `offers`), unless the gateway it calls said `online` false the last time the client called
  it and the message was queued less than 30 days ago. An online gateway passes the offer to the
  post office like any other (RP-5 §5.2), so a message that already arrived by forwarding costs
  only its entries in `HELLO` and `WELCOME`.
* **At a cut-off gateway.** A cut-off gateway declines an offer `already-have` while the message is
  in its queue, and for 30 days after it passed it on (§3.5). Otherwise it takes the message as
  new: a gateway that dropped the message's record queues it again, and another gateway queues a
  second copy, which the post office answers `duplicate` (RP-5 §5.4).
* **Told.** When a message has been queued for 30 days, the client tells the sender that it may
  have been dropped unsent, says where and when it was queued, and goes on offering it.

The gateway cannot tell the sender itself: the sender may never call it again, and a gateway that
has dropped a record has nothing left to say. Nothing new goes on the air for it: a queued message
is confirmed or sent again by the offers a session already carries.

---

## 4. Inward: mail for a cut-off area

### 4.1 When a post office forwards

A post office counts a gateway as **cut off** when it has not polled `GET /copies` (RP-5 §5.8) for
**15 minutes**; a gateway that holds copies polls without stopping. From then on, for each of its
routes (§6) whose `users_at` names that gateway, it places **forward items** at the route's `via`
gateway: one for each message that is waiting for a user who named the cut-off gateway as usual
(RP-5 §6.3), and for each that arrives while the gateway stays cut off.

Copies for the cut-off gateway itself are still placed, and wait for its next poll. When it polls
again, the post office has the forward items it placed for it dropped at the `via` gateways (RP-5
§5.8, `drop`). The gateway may then receive copies of mail it already got by radio: they have the
same content ID, and it keeps one.

### 4.2 The placement

Each forward item carries a **placement**, signed by the post office, so that the cut-off gateway
can tell its post office's mail from mail any station could make up. A placement is a CBOR array
of two byte strings, like a credential (RP-1 §4.1):

```
placement = [ payload, signature ]
payload   = bstr .cbor placement-body
```

`placement-body` is a CBOR map:

| Key | Name | Type | Meaning |
|---|---|---|---|
| 1 | `v` | uint | Format version: 1. |
| 2 | `address` | text | The user the message is for. |
| 3 | `serial` | uint | The message's serial in that mailbox (RP-3 §6.2). |
| 4 | `id` | bstr .size 12 | The message's content ID. |
| 5 | `gateway` | text | The cut-off gateway it is for. |
| 6 | `expires` | uint | When the copy is to be dropped. |
| 7 | `kid` | bstr .size 4 | The ID of the post office key that signed it. |

The signature is the post office key's (RP-0 §5) with context `RadioPost-placement-v1` over the
`payload` bytes as they are.

### 4.3 In the session

The partner lists the forward items it holds for the caller in `WELCOME` `waiting`, numbered with
**forwarding serials** of its own, increasing for each caller, with flags as for a copy (RP-3
§5.2: `held` when the item came without its air form), and sends them as RP-3 says (under the
caller's `auto`, or when the caller asks). Each `MSG` that carries a forward item has a further
key:

| Key | Name | Type | Meaning |
|---|---|---|---|
| 6 | `placement` | bstr | The item's placement (§4.2). |

The caller checks each placement, in this order, and lists the message failed with the code at
the first failure:

1. It decodes as §4.2, with `v` 1. Otherwise `malformed`.
2. `kid` names a key of the address's post office that the caller holds (RP-1 §7.3), and the
   signature verifies with it. Otherwise `signature-invalid`.
3. `id` is the message's content ID, and `gateway` the caller's own callsign. Otherwise
   `not-authorized`.
4. The caller's clock is before `expires`. Otherwise `refused-by-policy`.

Then it holds the message as a copy placed ahead for `address`, with `serial` and `expires` (RP-5
§5.8), and lists it `complete`.

The partner deletes a forward item once the caller has listed it `complete`, or when it expires.

### 4.4 Local delivery

A cut-off gateway hands mail between its own users without waiting for their post offices. Its
**local users** are the addresses for which it has received a copy placed ahead (RP-5 §5.8) in the
last 30 days: their users named it as usual, whichever post office they belong to.

* **Keeping.** When a user hands the gateway a message (RP-3 §5.4) whose `to` or `cc` names a local
  user, the gateway keeps it for that user as well as in the sender's session record (§3.1). The
  message still goes to the post office with the record, for its other recipients and for the
  record.
* **Listing.** The gateway lists it in that user's `WELCOME` `waiting` with serial 0 and the flag
  `local` (4) (RP-3 §5.2), in RP-3 §6.1's order, after the copies of the same priority. It sends
  it like any waiting message: unasked under the user's `auto`, or when the user asks for it by
  content ID (RP-3 §5.6, `ids`). Its `MSG` has no serial.
* **Picked up.** When the user lists it `complete`, the gateway adds its content ID to the local
  pickups of that user's session record, and stops listing it to that user.
* **Told.** The post office learns of each local pickup with the record (RP-5 §5.6,
  `local_pickups`), and marks the message picked up for that user once it holds it, so that the
  user is not offered it again. A copy that comes anyway has the same content ID, and the client
  keeps one (RP-3 §7).

Local delivery happens only while the gateway is cut off: online, the message reaches the post
office at once and comes back as a copy. The user trusts a local message as a copy the gateway
serves offline: the gateway checked its sender (RP-1 §5, §6.3), and the post office checks it again
when the record arrives.

---

## 5. Windows

A gateway that takes calls from partners publishes its **windows** in its record (RP-5 §4.4):

```json
"forwarding": [{"channel": {"dial_hz": 7101000, "mode": "USB", "modem": "aether",
                            "bandwidth_hz": 2300},
                "every_s": 3600, "at_s": 900, "length_s": 600}]
```

A window opens `at_s` seconds after each multiple of `every_s` since the Unix epoch, and lasts
`length_s` seconds: here at :15 past every hour, for ten minutes. During a window the gateway
listens on its `channel`.

A cut-off gateway calls a partner only inside one of the partner's windows. If the call is not
answered it tries again at most twice in the same window, a minute apart, then waits for the next
window. It does not call on a channel its modem reports busy (RP-4 §5). An attended station MAY
call a partner at any time.

**The rules.** A window's channel MUST be in a segment where the stations' control allows them to
start transmissions: an unattended gateway only where automatic control is allowed (US:
§97.221(b)). The modem's checks, or the gateway's own, enforce it (RP-4 §8).

---

## 6. Radio routes

A post office publishes its routes at `GET /routes` (RP-5 §4.5), a signed document:

```json
{"issued": 1794312000,
 "routes": [{"users_at": ["N2CALL-5", "KK4ODA-1"], "via": "N1CALL-10"}]}
```

Read: "when the gateways in `users_at` are cut off, their users' mail goes to `via`, which holds it
for them." The `via` gateway MUST publish windows (§5), and the gateways in `users_at` SHOULD have
it among their partners: they call it, inside its windows, to collect the mail.

---

## 7. Keeping and accountability

* **Outward records** are kept until a partner has taken them (§3.5) or they have been reported
  (RP-5 §5.6), and for at least **30 days** otherwise. A gateway that then drops a record unsent
  forgets its messages, so their senders' clients can queue them again (§3.6).
* **Forward items** are kept until their placement's `expires`.
* **Every hop keeps its evidence.** A gateway keeps, with each record it takes, the caller's own
  statement and signature from the forwarding session, for at least 30 days. With the path, they
  show which station passed the mail to which (§97.219).
* **`Radio-Path`** (RP-6 §3.2) names the operator and every gateway of the path, in order.

---

## 8. Not yet settled

* **Routing.** Gateways publishing what they reach; choosing among partners by more than their
  operators' lists.
* **Size.** Sending a credential's hash in `PROOF` when the partner holds it, as RP-3 `HELLO`
  key 14 does: the credential is about half of every `PROOF` (§9).
* **Bands.** A table, per country, of the segments open to unattended forwarding.

---

## 9. Sizes *(informative)*

| What | Size |
|---|---|
| `HELLO` from a gateway, one offer, with its credential | 335 bytes |
| `PROOF`, a record with one message | 342 bytes |
| `PROOF`, a pickup only | 332 bytes |
| `PROOF`, a local pickup only | 342 bytes |
| `REQUEST` for one local message by content ID | 20 bytes |
| A placement, in `MSG` key 6 | 136 bytes |
| `RECEIPT` for one message and two records | 48 bytes |

All with framing, from `vectors/session-04.json`, `vectors/forwarding.json`,
`vectors/session-05.json` and `vectors/local-delivery.json`.

---

## 10. Test vectors

`vectors/session-04.json` is a forwarding session byte for byte: N2CALL-5, cut off, calls N1CALL-10
in its window, sends two records and Shelter 3's message, and receives Ana's message with its
placement. `vectors/forwarding.json` has the placement and seven checks of it, the records and
seven checks of a `PROOF`, and the post office's side: the forward item placed when N2CALL-5 fell
silent, N1CALL-10's report with its path, and the drop when N2CALL-5 came back.

`vectors/session-05.json` is local delivery byte for byte: Shelter 3 hands N2CALL-5, cut off, a
message for Facundo, who then finds it listed with serial 0 and the flag `local`, asks for it by
content ID and takes it. `vectors/local-delivery.json` has six messages and the local recipients a
gateway keeps them for, both records' `PROOF`s (Facundo's with key 8), and the post office's side:
N1CALL-10's report with `local_pickups`, the forward item placed for the message and dropped in the
same poll, and the message not offered to Facundo again.

`vectors/queued.json` has the sender's side (§3.6): when a client offers a queued message again,
how a cut-off gateway answers, and what the client shows, and Shelter 3's session with N1CALL-10,
online, which declines the message `already-have` because the post office already holds it.

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.0.1 | 2026-11-09 | Outline. |
| 0.0.2 | 2026-11-09 | Licence stated: CC BY 4.0. |
| 0.1.0 | 2026-11-09 | First draft: the cut-off gateway calls its partner, one session both ways (§1, §2); session records and the `PROOF` PDU, the path, the partner's checks and `RECEIPT` `records` (§3); forward items, placements signed by the post office, forwarding serials and `MSG` `placement` (§4); windows (§5); routes (§6); keeping and accountability (§7); what is still open (§8). |
| 0.2.0 | 2026-11-09 | From the test vectors: the caller's credential needs `gateway` and `send`, not `receive` (§2); the user's credential is checked with `send` and `receive` as the record needs (§3.3); copies still wait for a cut-off gateway (§4.1); the placement's checks in order, with their codes, and forward items flagged as copies (§4.3); sizes (§9); test vectors (§10). |
| 0.3.0 | 2026-11-09 | Local delivery (§4.4): local users, keeping, listing with serial 0 and the flag `local`, local pickups in the record and in `PROOF` key 8 (§3.1, §3.2), checked with `receive` (§3.3); its sizes (§9) and test vectors (§10). |
| 0.4.0 | 2026-11-09 | Mail dropped unsent (§3.6): a message a cut-off gateway takes is queued, and the sender's client keeps it, offers it again and tells the sender; a cut-off gateway declines what it holds or passed on in the last 30 days (§3.5), and forgets a record it drops (§7); test vectors (§10). |
