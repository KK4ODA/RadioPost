# RP-7: Forwarding between gateways

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.9.0 |
| **Updated** | 2026-11-12 |
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
(§4.4). Mail travels only through configured partners and published routes: gateways do not
announce what they reach (§6.2).

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
| 1 | `credential` | bstr | one of 1, 9 | The user's credential. |
| 2 | `statement` | bstr | yes | The user's statement's encoding. |
| 3 | `signature` | bstr .size 64 | yes | Its signature. |
| 4 | `started` | uint | yes | When the user's session started. |
| 5 | `session` | bstr .size 8 | yes | The gateway session ID. |
| 6 | `path` | array of text | yes | The record's path, ending with the caller. |
| 7 | `pickups` | array of uint | no | The serials picked up. |
| 8 | `local` | array of bstr .size 12 | no | The content IDs picked up by local delivery (§4.4). |
| 9 | `credential_hash` | bstr .size 8 | one of 1, 9 | The credential's hash (RP-1 §4.3), when the partner holds the credential. |

The caller offers, in `HELLO`, every message of every record it forwards that the partner has not
yet receipted. In its third transmission it sends, before the `MSG` PDUs, one `PROOF` for each
record whose messages it sends, and one for each record with no messages (pickups or local pickups
only).

The credential is about half of a `PROOF` (§9), and one user's records often travel together. A
partner keeps each credential that verifies in a `PROOF` until it expires, as a gateway keeps a
client's (RP-3 §5.2, `cached`). So the caller sends `credential_hash` in place of `credential` when
an earlier `PROOF` to the same partner carried that credential: in the same transmission, or in an
earlier session whose `RECEIPT` listed that record in `records`.

### 3.3 What the partner checks

For each `PROOF`, the partner checks, and refuses the record at the first failure:

1. The credential: exactly one of `credential` and `credential_hash`, or `malformed`. With
   `credential_hash`, the one the partner holds with that hash, or `credential-required`. A caller
   that sent a record by its hash and does not find it in `records` sends it with its credential
   the next time.
2. The proof, as RP-1 §6.3 says for a statement relayed later: the credential (RP-1 §5) as it was
   at `started`, with `send` if the statement lists messages and `receive` if the record has
   pickups or local pickups; steps 2 and 4; and the statement's `time` within 600 seconds of
   `started`. As RP-5 §5.6 checks a report.
3. The path: its first callsign is the statement's `gateway`, its last is the caller, it holds at
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

A post office counts a gateway as **cut off** when the gateway has polled its `GET /copies` (RP-5
§5.8) before, but not for **15 minutes**: a gateway polls without stopping each post office that
places items at it, its own or another (RP-5 §5.8). A gateway that has never polled, or that has
not polled since the post office last answered it `placing` false, is not counted: the post office
cannot tell whether it is cut off or has not yet started. From then on, for each of its
routes (§6) whose `users_at` names that gateway, it places **forward items** at the route's `via`
gateway: one for each message that is waiting for a user who named the cut-off gateway as usual
(RP-5 §6.3), and for each that arrives while the gateway stays cut off.

Copies for the cut-off gateway itself are still placed, and wait for its next poll. When it polls
again, the post office has the forward items it placed for it dropped at the `via` gateways (RP-5
§5.8, `drop`). The gateway may then receive copies of mail it already got by radio: they have the
same content ID, and it keeps one.

### 4.2 The placement

Each forward item carries a **placement**, signed by the post office, so that the cut-off gateway
can tell its post office's mail from mail any station could make up. A placement is a COSE_Sign1
signed by the post office (RP-0 §5), like a credential (RP-1 §4.1):

```
placement = [ protected, {}, payload, signature ]
protected = bstr .cbor { 1: -19, 4: kid }   ; Ed25519; the ID of the post office key that signed
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

The signature is the post office key's, over COSE's `Sig_structure` with the context
`RadioPost-placement-v1` as external data (RP-0 §5).

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

1. It decodes as §4.2, a COSE_Sign1 in RP-0 §5's form, with `v` 1. Otherwise `malformed`.
2. The protected header's `kid` names a key of the address's post office that the caller holds (RP-1 §7.3), and the
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
                            "bandwidth_hz": 2300, "occupied_hz": [7101240, 7103760]},
                "every_s": 3600, "at_s": 900, "length_s": 600}]
```

`occupied_hz` is the lowest and highest radio frequency of the signal, as its modem measures its
occupied bandwidth (US: §97.3(a)(8), 26 dB down). Here Aether's 2 300 Hz signal, which occupies 240
to 2 760 Hz of audio by the wider reading of that rule, on a USB dial of 7.101 MHz.

A window opens `at_s` seconds after each multiple of `every_s` since the Unix epoch, and lasts
`length_s` seconds: here at :15 past every hour, for ten minutes. During a window the gateway
listens on its `channel`.

A cut-off gateway calls a partner only inside one of the partner's windows. If the call is not
answered it tries again at most twice in the same window, a minute apart, then waits for the next
window. It does not call on a channel its modem reports busy (RP-4 §5). An attended station MAY
call a partner at any time.

**The rules.** Both stations start transmissions in a forwarding session, each under its own
country's rules. So a gateway publishes a window, and a cut-off gateway calls in one, only when the
window's `occupied_hz` lies inside one segment of its own table (§5.1), ends included. The modem's
checks, or the gateway's own, enforce it as well (RP-4 §8). An attended station keeps to the rules
for its own control.

### 5.1 Segments open to unattended forwarding

A gateway holds the segments where its country's rules let a station under automatic control start
a data transmission. RadioPost keeps here the tables it knows, each read from the rules' text, with
the date it was read. A gateway in a country with no table here uses the segments its operator
enters from that country's rules; with none, it publishes no window and calls no partner while
unattended. A new country or a correction comes with the rule cited.

**United States**, ITU Region 2: 47 CFR §97.221(b), with §97.305(c) for where data is allowed;
read from the e-CFR text current to 2026-09-23 (6 m to 2026-09-24). Hertz.

| Band | From | To |
|---|---|---|
| 80 m | 3 585 000 | 3 600 000 |
| 40 m | 7 100 000 | 7 105 000 |
| 30 m | 10 140 000 | 10 150 000 |
| 20 m | 14 095 000 | 14 099 500 |
| 20 m | 14 100 500 | 14 112 000 |
| 17 m | 18 105 000 | 18 110 000 |
| 15 m | 21 090 000 | 21 100 000 |
| 12 m | 24 925 000 | 24 930 000 |
| 10 m | 28 120 000 | 28 189 000 |
| 6 m | 50 100 000 | 54 000 000 |

§97.221(b) opens the 6 m and shorter bands whole; data starts at 50.1 MHz on 6 m (§97.305(c)), and
the bands of shorter wavelength are not listed yet. §97.221(c), automatic control anywhere in the data segments
for a signal of 500 Hz or less, needs a station under local or remote control asking: it does not
cover two unattended gateways.

---

## 6. Routes

### 6.1 Radio routes

A post office publishes its routes at `GET /routes` (RP-5 §4.5), a signed document:

```json
{"issued": 1794312000,
 "routes": [{"users_at": ["N2CALL-5", "KK4ODA-1"], "via": "N1CALL-10"}]}
```

Read: "when the gateways in `users_at` are cut off, their users' mail goes to `via`, which holds it
for them." The `via` gateway MUST publish windows (§5), and the gateways in `users_at` SHOULD have
it among their partners: they call it, inside its windows, to collect the mail.

The gateways a route names may be registered with this post office or with another it knows,
their records as RP-5 §4.4 finds them; a route naming an ambiguous callsign (RP-5 §4.4) places
nothing. `via` learns to poll this post office from `placing` (RP-5 §5.8), which a route that
names it makes true.

### 6.2 Choosing a partner

Mail goes only to the partners an operator named, and on from them hop by hop (§3.4). Gateways do
not announce what they reach. An announcement would have to be trusted: one false one draws an
area's mail to a station that cannot pass it on, and every gateway would have to check them all.
Partners are stations their operators know (§1), and the path's limit bounds the hops (§3.3).

Records go furthest from a partner that reaches the post offices. So a cut-off gateway remembers,
for each partner, whether that partner's last `WELCOME` said `online`. In the window of a partner
whose last `WELCOME` said `online` false, it sends its records only if no partner whose last said
`online` true opens a window within the next hour (one open now counts). Otherwise it calls with
no offers, to collect its mail, and keeps its records for that window. A partner it has never
called counts as neither.

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

* **Tables.** Segments for countries other than the United States, and for the US bands of
  shorter wavelength than 6 m (§5.1).

---

## 9. Sizes *(informative)*

| What | Size |
|---|---|
| `HELLO` from a gateway, one offer, with its credential | 340 bytes |
| `PROOF`, a record with one message | 347 bytes |
| `PROOF`, a pickup only | 337 bytes |
| `PROOF`, a local pickup only, the credential by its hash | 187 bytes |
| `PROOF`, a record with one message, the credential by its hash | 186 bytes |
| `PROOF`, a pickup only, the credential by its hash | 177 bytes |
| `REQUEST` for one local message by content ID | 20 bytes |
| A placement, in `MSG` key 6 | 141 bytes |
| `RECEIPT` for one message and two records | 48 bytes |

All with framing, from `vectors/session-04.json`, `vectors/forwarding.json`,
`vectors/session-05.json` and `vectors/local-delivery.json`.

---

## 10. Test vectors

`vectors/session-04.json` is a forwarding session byte for byte: N2CALL-5, cut off, calls N1CALL-10
in its window, sends two records and Shelter 3's message, and receives Ana's message with its
placement. `vectors/forwarding.json` has the placement and eight checks of it, the records and
eleven checks of a `PROOF` (four of them with the credential's hash), eleven windows checked against
the United States' table (§5, §5.1), eight choices of whether to send records to a partner (§6.2),
and the post office's side: the forward item placed when N2CALL-5 fell
silent, N1CALL-10's report with its path, and the drop when N2CALL-5 came back.

`vectors/session-05.json` is local delivery byte for byte, after session-04: Shelter 3 picks up
Ana's message from N2CALL-5, still cut off, and hands it a message for Facundo, who then finds it
listed with serial 0 and the flag `local`, asks for it by content ID and takes it.
`vectors/local-delivery.json` has six messages and the local recipients a gateway keeps them for,
both records' `PROOF`s for N1CALL-10's next window, each credential by its hash (Facundo's with key
8), and the post office's side, going on from `forwarding.json`'s: N1CALL-10's report with
`local_pickups`, the forward item placed for the message and dropped in the same poll, and the
message not offered to Facundo again.

In `forwarding.json`, N1CALL-10 is hampost.org's gateway: radiopost.org takes its record from
hampost.org's gateway list (§6.1, RP-5 §4.4), and serves its forward items to its credential from
hampost.org. `placing` is §4.1's count: N2CALL-5, answered `placing` false and silent after, is
not cut off; once it has polled while radiopost.org places items at it, it is.

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
| 0.5.0 | 2026-11-09 | Size: `PROOF` key 9 `credential_hash` in place of the credential when the partner holds it, which halves a `PROOF` (§3.2); the partner's checks begin with the credential, `credential-required` for a hash it does not hold (§3.3); sizes (§9); test vectors (§10). |
| 0.6.0 | 2026-11-09 | Routing: gateways do not announce what they reach; a cut-off gateway keeps its records for a partner that reaches the post offices when one opens a window within the hour (§6.2); §6 split into radio routes (§6.1) and the choice (§6.2); test vectors (§10). |
| 0.7.0 | 2026-11-09 | Bands: a window states `occupied_hz`, and each station checks it against its own country's table (§5); the segments open to unattended forwarding, with the United States' table (§5.1); test vectors (§10). |
| 0.7.1 | 2026-11-09 | The local delivery vectors follow session-04: the sessions at 14:35 and 14:55, the records in the 15:15 window by their credentials' hashes (§9, §10). |
| 0.8.0 | 2026-11-09 | A placement is an untagged COSE_Sign1 (RP-0 §5): its key's ID moves from the body (key 7, removed) to the protected header (§4.2, §4.3); sizes (§9). |
| 0.9.0 | 2026-11-12 | Gateways across post offices (RP-5 0.16.0). §4.1: a gateway is cut off only after it has polled, as the reference already read it, and not since a `placing` false; it polls every post office that places items at it. §6.1: a route may name gateways registered with another post office this one knows, and `via` polls it. Test vectors (`forwarding.json`: N1CALL-10's record from hampost.org's list; `placing`). |
