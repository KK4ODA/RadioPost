# RP-7: Forwarding between gateways

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.1.0 |
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

Not in this version: delivery between two users of the same cut-off gateway, and routing beyond
configured partners and published routes (§8).

---

## 2. The forwarding session

A forwarding session is an RP-3 session in which the caller is a gateway:

* `HELLO` `role` is 1. `address` and `operator` are the caller's gateway credential's (RP-1 §3).
  The credential MUST have the `gateway` flag; the partner checks it as RP-1 §5 says, asking for
  `gateway` in step 6.
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

The caller offers, in `HELLO`, every message of every record it forwards that the partner has not
yet receipted. In its third transmission it sends, before the `MSG` PDUs, one `PROOF` for each
record whose messages it sends, and one for each record with no messages (pickups only).

### 3.3 What the partner checks

For each `PROOF`, the partner checks, and refuses the record at the first failure:

1. The proof, as RP-1 §6.3 says for a statement relayed later: the credential (RP-1 §5) as it was
   at `started`, steps 2 and 4, and the statement's `time` within 600 seconds of `started`. As RP-5
   §5.6 checks a report.
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
partner has listed it in `records`. Until then it offers them again in later windows.

---

## 4. Inward: mail for a cut-off area

### 4.1 When a post office forwards

A post office counts a gateway as **cut off** when it has not polled `GET /copies` (RP-5 §5.8) for
**15 minutes**; a gateway that holds copies polls without stopping. From then on, for each of its
routes (§6) whose `users_at` names that gateway, it places **forward items** at the route's `via`
gateway: one for each message that is waiting for a user who named the cut-off gateway as usual
(RP-5 §6.3), and for each that arrives while the gateway stays cut off.

When the cut-off gateway polls again, the post office has the forward items it placed for it
dropped (RP-5 §5.8, `drop`), and goes on placing copies at it as before.

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
**forwarding serials** of its own, increasing for each caller, and sends them as RP-3 says (under
the caller's `auto`, or when the caller asks). Each `MSG` that carries a forward item has a further
key:

| Key | Name | Type | Meaning |
|---|---|---|---|
| 6 | `placement` | bstr | The item's placement (§4.2). |

The caller checks each placement: it decodes, `kid` names a key of the address's post office that
it holds (RP-1 §7.3), the signature verifies, `id` is the message's content ID, `gateway` is its
own callsign, and `expires` is still ahead. Then it holds the message as a copy placed ahead for
`address`, with `serial` and `expires` (RP-5 §5.8), and lists it `complete`. If a check fails it
lists it failed with `signature-invalid`, or `not-authorized` for another gateway's item.

The partner deletes a forward item once the caller has listed it `complete`, or when it expires.

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
  (RP-5 §5.6), and for at least **30 days** otherwise.
* **Forward items** are kept until their placement's `expires`.
* **Every hop keeps its evidence.** A gateway keeps, with each record it takes, the caller's own
  statement and signature from the forwarding session, for at least 30 days. With the path, they
  show which station passed the mail to which (§97.219).
* **`Radio-Path`** (RP-6 §3.2) names the operator and every gateway of the path, in order.

---

## 8. Not yet settled

* **Local delivery.** Two users of the same cut-off gateway exchanging mail through it before their
  post offices hear of it.
* **Mail dropped unsent.** What the sender is told when a gateway drops a record at the end of its
  30 days.
* **Routing.** Gateways publishing what they reach; choosing among partners by more than their
  operators' lists.
* **Size.** Sending a credential's hash in `PROOF` when the partner holds it (as RP-3 `HELLO` key
  14 does), and how many bytes a forwarding session costs, once there are test vectors.
* **Bands.** A table, per country, of the segments open to unattended forwarding.

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.0.1 | 2026-11-09 | Outline. |
| 0.0.2 | 2026-11-09 | Licence stated: CC BY 4.0. |
| 0.1.0 | 2026-11-09 | First draft: the cut-off gateway calls its partner, one session both ways (§1, §2); session records and the `PROOF` PDU, the path, the partner's checks and `RECEIPT` `records` (§3); forward items, placements signed by the post office, forwarding serials and `MSG` `placement` (§4); windows (§5); routes (§6); keeping and accountability (§7); what is still open (§8). |
