# RP-5: Post office API

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.18.0 |
| **Updated** | 2026-11-13 |
| **Editor** | KK4ODA |
| **Licence** | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| **Depends on** | RP-0, RP-1, RP-2, RP-3 |
| **Protocol version** | 1 (`/rp/v1/`) |

The HTTPS interface of a post office. **Gateways** use it to carry their users' sessions, report
pickups, hand in mail queued while offline and receive copies placed ahead. **Clients** use it to
enroll, renew credentials and change their settings. **Applicants** ask for an account, and the
post office's **operators** decide, look after its accounts and issue codes. **Anyone** may read a
post office's public documents: its keys, revocations, the post offices it knows, its gateways and
its radio routes.

Reading and writing the mailbox itself, with a phone or a laptop when there is Internet, uses the
standard mail protocols (JMAP, IMAP, SMTP submission) of the post office's mail server and is not
part of this API.

---

## 1. Conventions

* **Base URL.** The `url` of the domain's `t=api` DNS record (RP-1 §7.1), ending in `/rp/v1`. Paths
  below are relative to it.
* **HTTPS only**, TLS 1.2 or later.
* **JSON** request and response bodies (RP-0 §4.2), `Content-Type: application/json`.
* **Binary values** in base64url without padding; **content IDs** in base32 text (RP-0 §6.3);
  **times** in Unix seconds.
* **Errors** are problem details (RFC 9457, `application/problem+json`) with two extra members:
  `code`, the number from RP-0 §9.3, and `name`, its name. Example:

  ```json
  {"type": "about:blank", "title": "Credential expired", "status": 422,
   "code": 4, "name": "credential-expired"}
  ```

* **The status** of each code:

  | Status | Codes |
  |---|---|
  | 400 | `unsupported-version`, `malformed` |
  | 401 | `not-authorized`, when the request has no token, or one unknown or expired |
  | 403 | `not-authorized` otherwise (the token may not do this), `refused-by-policy` |
  | 404 | `unknown-message`: no such serial in the session, no such session of this gateway's, or no such application (§6.7) |
  | 409 | `already-have` |
  | 413 | `too-large` |
  | 422 | `credential-invalid`, `credential-expired`, `credential-revoked`, `credential-required`, `unknown-post-office`, `signature-invalid`, `time-skew` |
  | 429 | `busy` |
  | 500 | `internal` |
  | 502 | `post-office-unreachable` |
  | 504 | `timeout` |
  | 507 | `quota-exceeded` |

* **Rate limits** are answered `429` with `Retry-After`.
* **Signed documents** (§4) are returned as `application/jose` (a JWS compact serialisation, RP-0
  §5) signed by one of the post office's keys, `kid` in the JWS header as the key's hex ID.

---

## 2. Who calls what

| Endpoint | Caller | Auth |
|---|---|---|
| §4 Public documents | Anyone | None |
| §3 Logging in | Gateways, clients | Credential and signature |
| §5 Gateway endpoints | Gateways | A token from a credential with the `gateway` flag |
| §6 Client endpoints | Clients | A token from the account's credential (enrollment: a code) |
| §6.7 Applications | Anyone asking for an account | None: the new device's signature |
| §7 Operator endpoints | The post office's operators | A token from an operator's credential (§7.1) |

A request with no token, or an unknown or expired one, is answered `401` `not-authorized`; a token
without the `gateway` flag on a gateway endpoint, or one with it on a client endpoint other than
`POST /credentials` (§6.2), `403` `not-authorized`.

A post office accepts gateways credentialed by **any post office it knows** (RP-1 §7.5), unless its
info document says `gateways: "listed"`, in which case only gateways in its own list (§4.4).

---

## 3. Logging in

### 3.1 `POST /auth/challenge`

No body. Answer:

```json
{"nonce": "q7m2x9a4…", "expires": 1794326700}
```

`nonce` is 32 random bytes; it is good for one login, until `expires` (at most 5 minutes).

### 3.2 `POST /auth/token`

```json
{"credential": "gqNk…", "nonce": "q7m2x9a4…", "signature": "9b1f…"}
```

`signature` is the device key's signature (RP-0 §5) with context `RadioPost-api-login-v1` over the 32
nonce bytes followed by the UTF-8 bytes of the base URL.

The post office verifies the nonce (one it issued, unexpired, not used: otherwise `401`
`not-authorized`), the credential (RP-1 §5) and the signature (`422` `signature-invalid`), and
answers:

```json
{"token": "tok_8Hc…", "expires": 1794330000, "address": "n1call@hampost.org",
 "operator": "N1CALL-10", "flags": 9}
```

The answer has `"office": true` when the token may use the operator endpoints (§7.1), and no
`office` member otherwise.

The token is sent as `Authorization: Bearer tok_8Hc…` on later requests. It lasts one hour, or
until the credential expires if that is sooner, and is bound to the credential: a revocation ends
it.

---

## 4. Public documents

Each is a signed JWS (RP-0 §5), signed by a key the verifier accepts by RP-1 §7.2: one in DNS, or
one the info document lists when a key in DNS signed that. A gateway SHOULD fetch and keep them for
every post office it knows (RP-1 §7.3). Test vectors: `vectors/jws.json`. The payloads:

### 4.1 `GET /info`

```json
{
  "version": 1,
  "domains": ["radiopost.org"],
  "operator": "KK4ODA",
  "contact": "postmaster@radiopost.org",
  "country": "US",
  "keys": [{"kid": "3f9a1c22", "public": "Hk3sW…9Q",
            "not_before": 1794268800, "not_after": 1857340800}],
  "limits": {"max_air_bytes": 65536, "max_email_bytes": 10485760},
  "gateways": "any",
  "mail": {"jmap": "https://po.radiopost.org/jmap/session", "imap": "po.radiopost.org:993"},
  "updated": 1794312000
}
```

| Member | Meaning |
|---|---|
| `keys` | Every current post office key (RP-1 §7.2). |
| `limits.max_air_bytes` | The largest air form this post office sends or takes by radio (RP-2 §5). |
| `limits.max_email_bytes` | The largest ordinary email it accepts (RP-6). |
| `gateways` | `any`: gateways credentialed by any known post office; `listed`: only its own (§4.4). |
| `mail` | Where its users' ordinary mail programs connect. |
| `applications` | `open` when it takes applications for accounts (§6.7). Absent, it does not. |

### 4.2 `GET /revocations`

The revocation list of RP-1 §8.

### 4.3 `GET /post-offices`

The post offices this one knows (RP-1 §7.5), for gateways to merge (RP-1 §7.3) and for gateway
lists (§4.4):

```json
{"issued": 1794312000,
 "post_offices": [{"domain": "hampost.org", "api": "https://po.hampost.org/rp/v1",
                   "keys": [{"kid": "71c2e0a4", "public": "…",
                             "not_before": 1794268800, "not_after": 1857340800}]}]}
```

Each entry has `domain`, `api` (the base URL its DNS record gives, RP-1 §7.1) and `keys`, as in
the info document (§4.1). A post office that serves several domains has an entry for each, with
the same `api`.

### 4.4 `GET /gateways`

The gateways registered with this post office (§6.4), for clients' gateway lists:

```json
{"issued": 1794312000,
 "gateways": [{
   "callsign": "KK4ODA-1", "operator": "KK4ODA", "grid": "EM73", "place": "Atlanta, GA",
   "channels": [{"dial_hz": 3590000, "mode": "USB", "modem": "aether", "bandwidth_hz": 2300},
                {"dial_hz": 7101000, "mode": "USB", "modem": "aether", "bandwidth_hz": 2300}],
   "hours": "always", "internet": true,
   "copies": {"users": 50, "days": 7, "max_air_bytes": 16384},
   "updated": 1794300000}]}
```

`modem` names the modem from the registry of RP-0 §9.4. A reader ignores a channel whose `modem`
it does not know: the modem may have been added after the reader was written. `mode` is the
radio's: `USB` for a modem on single sideband, `FM` for one on an FM radio. On an `FM` channel,
`dial_hz` is the channel's carrier frequency and `bandwidth_hz` the channel's width, 12 500 or
25 000: a modem working on an FM radio's audio cannot measure the deviation, so the channel is what
can be stated. A gateway that takes calls from cut-off partners lists its windows in `forwarding`
(RP-7 §5).

The post office that registered a gateway (§6.4) is its **own post office**: the domain of its
credential's `address` (RP-1 §3, §5).

**Gateways of other post offices.** A gateway serves the users of every post office that lets it
in (§2), not only those of its own post office. So a client's list of gateways, and the gateways a
post office places copies at (§5.8), come from several post offices' lists. The reader, a client
or a post office, takes:

* its **home's** list: for a client, its own post office's, checked as RP-1 §7.2 says; for a post
  office, the gateways registered with it;
* the list of each post office its home names in `/post-offices` (§4.3), fetched once for each
  `api` and checked with a key of that entry, or with one the post office's info document adds,
  signed by one of the entry's (RP-1 §7.2). A list that does not verify is left out.

When the home's info document says `gateways: "listed"`, only the home's list is taken: the home
lets no other gateway in (§2).

The reader keeps, with each record, the domain of the post office that listed it. A callsign in
more than one list takes the home's record when the home lists it. Otherwise it is **ambiguous**:
a client keeps each record; a post office places no copies at it (§5.8), since it could not tell
whose credential to serve them to.

The reader refreshes the lists at least every **6 hours** while online, keeps the last it verified
of each list it cannot fetch, and uses what it holds while offline.

Test vectors: `vectors/gateway-lists.json`.

### 4.5 `GET /routes`

This post office's radio routes, for forwarding when the Internet is down. The format is RP-7's
(§6).

---

## 5. Gateway endpoints

All need a gateway token (§3).

### 5.1 What a gateway may do

A gateway acts **for a user only within that user's session**: everything below hangs off a session
the user's own signed statement opened (§5.2). A gateway cannot list or fetch a mailbox on its own.

### 5.2 `POST /sessions`: a user called

Sent when a user's `HELLO` arrives (RP-3 §9), while the link turns around.

```json
{
  "statement": "pgF2ZmFjdW5kb0ByYWRpb3Bvc3Qub3JnAmZLSzRPREED…",
  "signature": "9b1f…",
  "credential": "gqNk…",
  "started": 1794326441,
  "gateway_session": "ec7b31841ec47c47",
  "cursor": 418,
  "partials": [{"id": "cpwfawr3mrq37rql7fza", "offset": 512}],
  "renew": true
}
```

* `statement`: the user's session statement (RP-1 §6.1), deterministically encoded as the gateway
  rebuilt it from `HELLO` (or `AUTH`), base64url.
* `started`: the gateway's clock when `HELLO` arrived.
* `gateway_session`: the gateway's session ID (RP-3 `WELCOME` key 2) in hex. The post office records
  it for the `Radio-Path` header of the mail the session sends (RP-6 §3.2).

The post office verifies the credential and the statement in RP-1 §6.3's order, with the
gateway's callsign as the station called and `started` in place of its clock, and then checks
`started` against its own clock within 600 seconds (`time-skew`). A statement in challenge form
(RP-3 §5.2: the client's clock was off) holds the challenge the gateway issued in `WELCOME`, which
the gateway has checked as RP-1 §6.3 step 6 requires: the post office takes it as issued, once.
It keeps the pair (`address`, `challenge`) in its replay cache as it keeps a time-form statement's
(`address`, `nonce`), and answers a challenge already there `signature-invalid`. It answers a
failure with its code (§1), and otherwise `201`:

```json
{
  "session": "s_4Nf…",
  "expires": 1794330041,
  "waiting": [{"serial": 419, "id": "c1…", "from": "maria@example.com",
               "subject": "Are you safe?", "size": 1104, "priority": 0, "held": false,
               "third_party": true}],
  "more": 0,
  "accept": [{"id": "a3f9…", "offset": 0}],
  "decline": [],
  "credential": "gqNk…"
}
```

* `accept` / `decline`: the post office's view of the user's offers, which the gateway passes on
  in `WELCOME`. A `decline` entry is `{"id", "code", "name"}`, with `already-have`, `too-large` or
  `refused-by-policy`.
* `waiting`: every message not yet picked up, in RP-3 §6.1's order. `held` is set when the message
  lists attachments held at the post office (RP-2 `flags` 2): the gateway sets RP-3's `held` flag.
  `third_party` marks mail whose sender is not shown to be licensed (RP-6 §5.1); the gateway lists
  it only as RP-6 §5.1 allows.
* `credential`: a renewed credential (§6.2), when `renew` was asked and the post office issued
  one. The gateway hands it to the client in `CREDENTIAL` (RP-3 §5.8).

### 5.3 Messages for the user

| Request | Does |
|---|---|
| `GET /sessions/{s}/waiting?after={serial}&limit={n}` | The next page of the waiting list (RP-3 `REQUEST` `more`). |
| `GET /sessions/{s}/messages/{serial}` | One waiting message: `{"serial": 419, "id": "c1…", "air": "AXic…"}`. A serial the session's `waiting` did not list: `404` `unknown-message`. |
| `POST /sessions/{s}/files` | `{"files": [{"serial": 422, "ref": "…"}]}` (`ref` in base64url): send these held attachments. The post office answers `202` and puts them in the mailbox as RP-6 §2.5 says, in new messages that the next waiting list shows. `malformed` when `files` is not a non-empty list of such entries; `404` `unknown-message` for a serial that is not in the user's mailbox, picked up or not, or a `ref` its message does not list; `413` `too-large` for an attachment too large for the air even alone. A refused request sends nothing. |

The post office makes a message's air form once and serves those same bytes every time it is
asked, and in every copy placed ahead (§5.8). A user whose download broke resumes it through any
gateway by its offset (RP-3 §7), which works only if every gateway sends the same bytes.

**Compression.** That one air form goes to every gateway, and the post office never sees the
caller's `caps` (RP-3 §5.1). So it uses compression 0 or 1, which every station takes (RP-2 §4),
and SHOULD use 1 when that gives fewer bytes. It MUST NOT use 2 or 3. A post office that holds
attachments back to keep a message within `max_air_bytes` (§4.1) or an account's `attach_bytes`
(§6.3) judges the air form it will serve.

### 5.4 `POST /sessions/{s}/messages`: mail the user sent

```json
{"messages": [{"id": "a3f9…", "air": "AXic…"}]}
```

For each message the post office checks that its content ID is in the session's statement, that
the air form decodes to that ID (RP-2 §3), and that its `from` is the statement's address. It
answers:

```json
{"results": [{"id": "a3f9…", "status": "accepted"}]}
```

`status` is `accepted` (the post office has it and will send it on; RP-6), `duplicate` (it already
had it), or `refused` with `code`, `name` and `text`: `not-authorized` when the ID is not in the
statement or the `from` is not its address, `malformed` when the air form does not decode to the
ID. The checks go in the order above, one result per message, in the order sent. Only `accepted` and `duplicate` let the gateway list
the message `complete` in its `RECEIPT` before it holds the message itself; a gateway that stores
the message first MAY receipt it at once and hand it in later.

### 5.5 `POST /sessions/{s}/pickups`

```json
{"serials": [419, 420]}
```

Sent when the user's `RECEIPT` lists the messages complete (RP-3 §5.5). The post office marks them
picked up and has any copies placed ahead dropped (§5.8). `204`. If any serial is not in the
session's `waiting`, it marks none and answers `404` `unknown-message`.

### 5.6 `POST /reports`: what happened while offline

A gateway that carried sessions while it could not reach a post office reports them when it can,
oldest first:

```json
{"sessions": [{
  "statement": "pgF2…", "signature": "9b1f…", "credential": "gqNk…",
  "started": 1794326441, "gateway_session": "ec7b31841ec47c47",
  "messages": [{"id": "a3f9…", "air": "AXic…"}],
  "pickups": [419, 420],
  "local_pickups": ["3ic5zafw7yhvse5zt5iq"],
  "path": ["N2CALL-5", "N1CALL-10"]
}]}
```

`path` is there when the session reached the reporting gateway by forwarding (RP-7 §3.4): the
gateways that held it, from the one the user called to the reporting gateway. `local_pickups` are
the content IDs of messages the user picked up from a cut-off gateway's local delivery (RP-7 §4.4):
the post office marks each picked up in the user's mailbox once it holds it, has its copies placed
ahead dropped, and does not offer it to the user again.

The post office verifies each statement as in §5.2, with the station called being the path's first
callsign (without a path, the reporting gateway). A path whose last callsign is not the reporting
gateway is answered, for that session, `{"code", "name"}` with `not-authorized`.
It checks the credential and `time` against `started` only, not its own clock. It takes
the messages as in §5.4 and the pickups as in §5.5, and answers one entry per session, in order:
`{"results": [...]}` as in §5.4, or `{"code", "name"}` when the session's statement fails, in which
case nothing of it is taken. Messages for other post offices go there (§5.4 applies at each).

### 5.7 `DELETE /sessions/{s}`

The session is over. `204`. Sessions also end on their own at `expires`.

### 5.8 Copies placed ahead

A post office places copies of a user's waiting mail at each gateway the user named as usual
(§6.3), when that gateway's record says it holds copies. The record is the one §4.4's lists give:
the gateway may be registered with this post office or with another it knows. A callsign no list
names, or an ambiguous one (§4.4), gets no copies.

**`GET /copies?after={cursor}&wait={seconds}`**: long poll, `wait` at most 60; `wait=0` answers at
once. Answer:

```json
{"cursor": "c_9…",
 "placing": true,
 "items": [
   {"kind": "copy", "address": "facundo@radiopost.org", "serial": 421, "id": "4e…",
    "from": "maria@example.com", "subject": "Photo", "size": 310522, "priority": 0,
    "third_party": true, "expires": 1794931200},
   {"kind": "copy", "address": "facundo@radiopost.org", "serial": 419, "id": "c1…",
    "from": "maria@example.com", "subject": "Are you safe?", "size": 1104, "priority": 0,
    "third_party": true, "air": "AXic…", "expires": 1794931200},
   {"kind": "drop", "address": "facundo@radiopost.org", "serial": 418}
 ]}
```

* A `copy` carries `air` when the message fits the gateway's `copies.max_air_bytes`; otherwise only
  its list entry, and the gateway lists it with the `held` flag while offline. `third_party` is as
  in `waiting` (§5.2), and the gateway applies RP-6 §5.1 while offline too.
* A `drop` means the message was picked up elsewhere, or expired: the gateway deletes its copy.
* A `forward` item is a copy for a cut-off gateway, placed at the gateway its route names (RP-7
  §4.1): as a `copy`, with `gateway`, the cut-off gateway's callsign, and `placement`, the post
  office's signed placement (RP-7 §4.2). The gateway holds it for that gateway's next call, and a
  `drop` naming the same address and serial ends it.
* `placing` is true while this post office places items at the gateway, or may: a user of it names
  the gateway as usual and the gateway's record holds copies, a route of it names the gateway as
  `via` (RP-7 §6.1), or items for it are not yet acknowledged. Otherwise false.

**`POST /copies/ack`** `{"cursor": "c_9…"}`: the gateway holds everything up to the cursor. The
post office sends nothing twice after an acknowledged cursor.

**Whose items.** The items for a gateway are served only to a gateway token whose credential has
that gateway's callsign as `operator` and was issued by the post office whose list gave its record
(§4.4): the credential's `address` is at one of that post office's domains. To any other gateway
token the post office answers as to a gateway it places nothing at: no items, `placing` false, and
the poll does not count as the gateway's (RP-7 §4.1). Another post office it knows can issue a
credential for any callsign (RP-1 §9); this is what keeps it from collecting a gateway's copies, or
from hiding that the gateway is cut off.

**Which post offices a gateway polls.** A gateway whose record holds copies, or lists `forwarding`
windows (RP-7 §5), polls without stopping:

* its own post office (§4.4), whatever `placing` says;
* each other post office whose last answer said `placing` true.

It asks each other post office it knows (RP-1 §7.3) and does not poll, with `wait=0`: at each
refresh of that post office's documents, and after a session of one of that post office's users,
when it has not asked in the last hour. When a post office it polls answers `placing` false, the gateway acknowledges the
answer's cursor and stops polling it. A post office that refuses its login (§2, §3.2) places
nothing at it.

A gateway behind a home router uses only outgoing connections: the long poll is how copies reach it.

Test vectors: `vectors/api-exchanges.json` (copies at N1CALL-10, a gateway registered with
hampost.org), `vectors/forwarding.json` (`placing`).

---

## 6. Client endpoints

### 6.1 `POST /enroll`: a new device

No token. The post office's operator gives the user a one-time **enrollment code** (by its website,
by email, in person), once the user has shown they hold the callsign (RP-1 §1). The client generates a device key and sends:

```json
{"address": "facundo@radiopost.org", "operator": "KK4ODA", "code": "7Q4F-AZ9K",
 "device": "Hk3sW…", "signature": "…"}
```

`signature` is the new device key's signature with context `RadioPost-api-enroll-v1` over the UTF-8 bytes
of the code followed by those of the base URL: it proves the client holds the key. The post office
checks the code and the operator (a code that is unknown, used, or for another account or
operator: `403` `not-authorized`) and the signature (`422` `signature-invalid`), and answers
`{"credential": "gqNk…"}`. The code is then used: only an enrollment that succeeds uses it, so a
refused attempt (a typing error in the callsign) leaves it good until it expires.

### 6.2 `POST /credentials`: renewal

With a token (§3) from the current credential, a client's or a gateway's. Body `{}`. Answers `200`
`{"credential": "gqNk…"}`: a new credential with the same address, operator, device key and flags,
the post office's next serial, issued now and expiring as RP-1 §4.2 says, signed with its current
key.

* The credential the token came from stays valid until it expires. The post office keeps the
  serials of every credential it issued for a device key, so that revoking the device revokes
  them all (RP-1 §8).
* A credential that is expired or revoked cannot log in (§3.2), so it cannot be renewed: the device
  enrolls again with a new code (§6.1). A new device enrolls too.
* By radio, a client asks with `renew` in `HELLO`'s caps (RP-3 §5.1), the gateway passes `renew` on
  (§5.2), and the post office answers in `credential` as here.

### 6.3 `GET /settings`, `PUT /settings`

```json
{"usual_gateways": ["KK4ODA-1", "N2CALL-5"],
 "auto_bytes": 1000,
 "attach_bytes": 16384,
 "radio_skips_read": true}
```

| Member | Meaning |
|---|---|
| `usual_gateways` | Where copies are placed ahead (§5.8): callsigns of gateways registered with this post office or with another it knows (§4.4). |
| `auto_bytes` | A default for the client's `auto` (RP-3 §5.1). |
| `attach_bytes` | Mail from the Internet carries its attachments up to this air-form size; above it they are held (RP-6 §2.4). The post office's default when unset is its own, at most its `max_air_bytes`. |
| `radio_skips_read` | Mail already read in an ordinary mail program is not offered by radio. |
| `accept_on_send` | Mail the user sends adds its recipients to the accept list (§6.5). Default true. |

### 6.4 Gateway registration

A licensed user registers a gateway they operate:

* **`POST /gateways`** with a user token: `{"record": {…}, "device": "…", "signature": "…"}`, the
  record as in §4.4 and the gateway's own device key, proved by the gateway key's signature with
  context `RadioPost-api-gateway-v1` over the UTF-8 bytes of the record's callsign followed by
  those of the base URL. The post office checks that the record's `operator` is the
  token's operator (otherwise `403` `not-authorized`) and the signature, and answers `201`
  `{"credential": "…"}` with the `send` and `gateway` flags, the gateway's callsign as
  `operator` and the user's address as `address`.
* **`PUT /gateways/{callsign}`** updates the record; **`DELETE`** withdraws it.

### 6.5 The accept list

| Request | Does |
|---|---|
| `GET /accept-list` | `200` `{"entries": ["maria@example.com", "@redcross.example"]}`: addresses, or whole domains written `@domain`. |
| `POST /accept-list` | `{"add": […], "remove": […]}`. Answers `200` with the list as it now stands. |

* Entries are written in lowercase, and keep the order they were added in. One already there is
  not added again; removing one that is not there changes nothing. `add` is applied first, then
  `remove`.
* An entry that is neither an address nor `@domain` is answered `400` `malformed`, and nothing
  changes.
* A role mailbox has one list, which every operator's token reads and changes.
* When the post office accepts a message the user sent (§5.4, §5.6), it adds each `to` and `cc`
  address the list does not already match (RP-6 §2.2), unless the user's `accept_on_send` is false
  (§6.3).

How the accept list is used at the door is RP-6's.

### 6.6 `POST /direct`: station-to-station mail

Mail two users exchange station to station (RP-3 §10) reaches no post office on the air. Each
client reports those sessions to its own post office when it has the Internet, oldest first, so
that its mailbox and sent mail agree with what crossed the air, and the message reaches its other
recipients:

```json
{"sessions": [{
  "started": 1794249647, "role": "caller",
  "mine":   {"statement": "pgF1…", "signature": "XT2U…", "credential": "hFgj…"},
  "theirs": {"statement": "pgF2…", "signature": "8kq1…", "credential": "hFgk…"},
  "sent":     [{"id": "haxmp4foh4557vtwhzlq", "air": "AKcB…"}],
  "received": [{"id": "bnidnck4opwdqyelsa4a", "air": "AKYB…"}]
}]}
```

* `started`: the station called's clock when the session started (its `WELCOME` `time`).
* `role`: `caller` or `called`, this user's part.
* `mine`, `theirs`: each station's proof (RP-1 §6.4): the caller's statement as its `HELLO` gave it,
  the station called's as RP-1 §6.5 builds it, each with its signature and credential, base64url.
* `sent`: the messages this user handed over and the other station listed `complete`.
* `received`: the messages this user received and listed `complete`.

The post office checks each session in this order and, at the first failure, answers it
`{"code", "name"}` and takes nothing of it:

1. Both proofs decode, and `role` is one of the two. Otherwise `malformed`.
2. `mine`'s credential (RP-1 §5) as it was at `started`, with `send` if `sent` is not empty and
   `receive` if `received` is not empty; then `theirs`', with `send` if `received` is not empty.
3. `mine`'s credential is for the token's account, an account of this post office. Otherwise
   `not-authorized`.
4. The statements, as RP-1 §6.3 checks a statement relayed later: the caller's with the station
   called as the station it names and `time` against `started`; the station called's with the
   caller as the station it names and the caller's `nonce` (RP-1 §6.5).

Then, one result per message, `sent` first, each in the order given:

* A sent message must be in `mine`'s statement and from its address, and a received one in
  `theirs`' and from its address, addressed (`to` or `cc`) to this user. Otherwise it is refused,
  `not-authorized`, as in §5.4.
* **Sent:** taken as mail the user sent (§5.4; the accept list fills as §6.5 says), and sent on to
  every recipient but the other station's account, which has it: `{"id", "status": "accepted",
  "sent_on": [...]}`, or `duplicate` when the post office already had it.
* **Received:** filed in the user's mailbox with the next serial, already picked up, so no gateway
  offers it again: `{"id", "status": "accepted", "serial"}`, or `duplicate` when the mailbox holds
  it.

The answer is `{"sessions": [...]}`, one entry per session, in order. Sending the same report again
changes nothing: every message is a `duplicate`.

### 6.7 Applications

Someone with a licence and no account asks for one, from a new device, and waits for the post
office's operators to decide (§7.3). Nobody has to send them a code first. A post office takes
applications only when its info document says `"applications": "open"` (§4.1).

**`POST /applications`**, no token:

```json
{"address": "jose@radiopost.org", "operator": "KJ4ABC", "name": "José Pérez",
 "contact": "jose.perez@example.net", "device": "Hk3sW…", "signature": "…"}
```

| Member | Meaning |
|---|---|
| `address` | The account asked for, in lowercase: a local part of letters, digits, `.`, `-` and `_` (RP-0 §6.2), at a domain the post office serves. |
| `operator` | The applicant's callsign as licensed: a base callsign (RP-0 §6.1), with no prefix, suffix or SSID, in uppercase. |
| `name` | The name the licence is in, 1 to 64 characters, for the operators' check; it becomes the account's display name (RP-6 §3.1). |
| `contact` | An email address the post office may write to about the application. |
| `device` | The new device's public key. |
| `signature` | The device key's signature with context `RadioPost-api-apply-v1` over the UTF-8 bytes of `address`, a line feed (`0x0A`), `operator`, a line feed, then the base URL. It proves the applicant holds the key the credential will be issued for. |

The post office checks, in this order:

1. Every member is there and of the form above. Otherwise `400` `malformed`.
2. It takes applications, and `address` is at one of its domains. Otherwise `403`
   `refused-by-policy`.
3. `operator` is not the callsign of one of its operators. Otherwise `403` `refused-by-policy` with
   `operates` (§7.1).
4. `address` is not an account already, nor asked for by an application still pending. Otherwise
   `409` `already-have`.
5. The signature. Otherwise `422` `signature-invalid`.

It answers `202`:

```json
{"application": "ap_Qm9vZ2xl…", "status": "pending", "expires": 1796918400}
```

`application` is the application's ID: `ap_` and 16 random bytes in base64url, which only the
applicant is given. `expires` is when a pending application is forgotten, at least 14 days on.

**`GET /applications/{id}`**, no token: where the application stands.

| `status` | Also | Means |
|---|---|---|
| `pending` | `expires` | Not decided yet. |
| `approved` | `credential` | The account is made and the credential issued, for the application's device key, with the flags the operator chose (§7.3). The applicant's client takes it as from an enrollment (§6.1). |
| `declined` | `text` | Refused; `text` says why, for the applicant. |
| `postcard` | | The operator sends an enrollment code by post to the address on the licence. The applicant enrolls with it (§6.1), from this device or another. |

An unknown ID, or one the post office has forgotten, is `404` `unknown-message`. A decided
application is answered for at least 7 days after the decision.

---

## 7. Operator endpoints

A post office MAY offer these. They let its operators run it from a client, over the API, with the
credential they already hold: decide applications, see the accounts, make enrollment codes and
revoke devices.

### 7.1 Who may

A post office names the callsigns that operate it: at least its info document's `operator`
(§4.1). A token may use the endpoints of this section when its credential:

* is for an account at one of the post office's own domains, so that the post office issued it;
* has the callsign of one of its operators as `operator`;
* does not have the `gateway` flag.

Any other token is answered `403` `not-authorized`; no token, `401`. A post office that does not
offer them answers `404` `unknown-message`. The login says which tokens may (§3.2).

**Who may become one.** Any credential the post office issues naming an operator's callsign gives
its holder these endpoints, so it issues one only on purpose. It refuses an application that names
one (§6.7), refuses to approve such an application or send it a postcard (§7.3), and makes a code
that names one for anyone but the operator asking only when they say so with `operates` (§7.5). An
operator adds their own devices with a code. Each of these refusals is `403` `refused-by-policy`
with one more member, `operates`, the callsign:

```json
{"type": "about:blank", "title": "Refused by policy", "status": 403,
 "code": 13, "name": "refused-by-policy", "operates": "KK4ODA"}
```

### 7.2 `GET /office/status`

```json
{"accounts": 41, "applications": 2, "codes": 1, "sessions": 1}
```

How many accounts, pending applications, enrollment codes neither used nor expired, and sessions
open. A post office MAY add members of its own, such as the mail waiting to go on by email (RP-6)
or how its own checks stand: a client shows them or ignores them.

### 7.3 Applications

**`GET /office/applications`**: every application the post office keeps (§6.7), in the order
received:

```json
{"applications": [
  {"application": "ap_Qm9vZ2xl…", "status": "pending", "address": "jose@radiopost.org",
   "operator": "KJ4ABC", "name": "José Pérez", "contact": "jose.perez@example.net",
   "device": "Hk3sW…", "received": 1794326400, "expires": 1796918400}
]}
```

A decided entry has `decided`, the time, and what the decision gave: `flags` when approved, `text`
when declined, `code` and `code_expires` when sent by postcard. A post office MAY add members of
its own, such as what it found of the callsign in a licence database; a client shows them or
ignores them.

**`POST /office/applications/{id}`** decides a pending application:

| Body | Does |
|---|---|
| `{"decision": "approve", "flags": ["send", "receive"]}` | Makes the account and issues its credential (RP-1 §4) for the application's device key, with these flags: any of `send`, `receive` and `role` (RP-1 §4.1); without `flags`, `send` and `receive`. The application's `name` becomes the account's display name. |
| `{"decision": "decline", "text": "…"}` | Refuses it; `text` is for the applicant. |
| `{"decision": "postcard"}` | Makes an enrollment code for the application's account and callsign, with `send` and `receive`, good for at least 30 days, to be sent by post to the address on the licence. |

It answers `200` with the application's entry as `GET /office/applications` lists it. An unknown
ID is `404` `unknown-message`; another decision, a flag that is not one of the three, or a body
without `decision`, `400` `malformed`. An application already decided, or one whose address has
become an account since, is `409` `already-have`, and nothing changes. An application whose
callsign has become an operator's since it arrived can only be declined: `approve` and `postcard`
are `403` `refused-by-policy` with `operates` (§7.1).

### 7.4 `GET /office/accounts`

```json
{"accounts": [
  {"address": "facundo@radiopost.org", "name": "Facundo", "waiting": 3,
   "credentials": [{"serial": 1042, "operator": "KK4ODA", "device": "Hk3sW…", "flags": 3,
                    "expires": 1802102400, "revoked": false}]}
]}
```

Every account, by address: its display name when it has one, how many messages wait in its mailbox
for a gateway, and the credentials the post office issued for it that have not expired.

### 7.5 `POST /office/codes`

`{"address": "ana@radiopost.org", "operator": "N2CALL", "flags": ["send", "receive"], "name": "Ana"}`
makes an enrollment code (§6.1) for an account, a new one or one that exists (a new device). `flags`
and `name` are optional, as in §7.3. Answers `200`:

```json
{"code": "7Q4F-AZ9K", "address": "ana@radiopost.org", "operator": "N2CALL", "flags": 3,
 "expires": 1794931200}
```

A code that names one of the operators' callsigns gives the operator endpoints to whoever enrolls
with it (§7.1), and its answer has `"operates": true`. Unless it is for the account and callsign of
the token's own credential (a new device of the operator asking), the post office makes such a code
only when the body has `"operates": true`; otherwise it answers `403` `refused-by-policy` with
`operates` (§7.1), and an operator's client asks them before sending the request again.

An address that is not at one of its domains, or a member of the wrong form (`operates` other than
`true` or `false` included), is `400` `malformed`.

### 7.6 `POST /office/revoke`

`{"device": "Hk3sW…"}` revokes every credential the post office issued for a device key (RP-1 §8),
as when a device is lost, and ends their tokens. Answers `200` `{"revoked": [1042, 1077]}`, the
serials. A device key that is not 32 bytes is `400` `malformed`.

---

## 8. Security considerations *(informative)*

* A gateway can do nothing for a user without that user's fresh signed statement, so a stolen
  gateway token cannot read anyone's mail.
* Tokens are short-lived and bound to a credential; revoking the credential ends the token.
* Everything a gateway hands in carries the user's statement, so a post office can always say which
  account sent a message and through which gateway (RP-1 §6.4).
* Enrollment codes are one-time and SHOULD expire within days; one sent by post, within weeks.
* An application proves only that the applicant holds a device key. Whether they hold the callsign
  is for the operators to establish before approving (RP-1 §1): a licence database shows that a
  callsign exists and in whose name, which anyone can read, so it is evidence, not proof. A
  credential lets its holder's mail be taken as a licensee's (RP-6 §5.1).
* The operator endpoints can make accounts and revoke devices. Only a credential the post office
  issued itself can use them, so another post office cannot issue one that does.
* They go by callsign, not by account: whoever holds a credential naming an operator's callsign
  holds them. Anyone may apply naming any callsign, and the licence database would show an
  operator's callsign as real, so an application naming one is refused rather than left to an
  operator's attention, and a code naming one for someone else's address is made only when asked
  for in so many words (§7.1).
* Copies placed ahead reach a gateway without a user's session, so they go only to the credential
  of the post office that listed the gateway (§5.8). A post office that lists a callsign another
  already lists makes it ambiguous wherever neither is the home, and copies stop there (§4.4): it
  can deny a gateway copies but not collect them, and its own signed list shows what it did.

---

## 9. Test vectors

`vectors/api-office.json` (§6.7 and §7: an application approved, its credential collected and
used, another declined, one sent by postcard and enrolled with its code; the operator's status,
accounts, codes and revocation; and the refusals of both), `vectors/direct.json` (§6.6: session-03 reported by both stations, sent again, refused to a
gateway, and nine cases), `vectors/api-account.json` (renewal over the Internet and by radio, a gateway's, an expired and a
revoked credential; the accept list read and changed, shared by a role mailbox's operators, filled
by mail sent, and its effect at the door), `vectors/api-exchanges.json` (enrollment, a gateway registered, logins, session-01
at the post office, copies placed ahead and dropped, copies at a gateway registered with another
post office and for whose credential, an offline gateway's report, and the failures above), `vectors/jws.json` (§4), `vectors/gateway-lists.json` (§4.4: a gateway list
read from the home's and every post office it knows) and `vectors/api.json` (the signatures of
§3.2, §6.1, §6.4 and §6.7).

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.1.0 | 2026-11-09 | First draft. |
| 0.1.1 | 2026-11-09 | Licence stated: CC BY 4.0. |
| 0.2.0 | 2026-11-09 | Login, enrollment and gateway registration each sign under a context of their own; test vectors (`vectors/api.json`). |
| 0.3.0 | 2026-11-09 | A message's air form is made once and served the same every time (§5.3), so that downloads resume across gateways (RP-3 §7). |
| 0.4.0 | 2026-11-09 | The HTTP status of each code, 401 and 403 (§1, §2); login and enrollment failures (§3.2, §6.1); public documents checked by RP-0 §5 and RP-1 §7.2 (§4); the order of a session's checks, `decline` entries, `waiting`'s order (§5.2); unknown serials (§5.3, §5.5); the codes of a refused message (§5.4); a report's answer (§5.6); test vectors. |
| 0.5.0 | 2026-11-09 | `gateway_session` in a session and a report, for RP-6's `Radio-Path` (§5.2, §5.6); the setting `attach_bytes` (§6.3). |
| 0.6.0 | 2026-11-09 | Renewal (§6.2): what a renewed credential holds, gateways renew too (§2), the old one stays valid, a device's serials revoked together, expired or revoked means enrolling again, by radio (§5.2). The accept list (§6.5): `POST`'s answer, lowercase entries in order, `malformed`, one list for a role mailbox, recipients added when mail is sent; the setting `accept_on_send` (§6.3). Test vectors. |
| 0.7.0 | 2026-11-09 | For forwarding (RP-7): `forwarding` windows in a gateway's record (§4.4), routes (§4.5), `path` in a report (§5.6), `forward` items (§5.8). |
| 0.8.0 | 2026-11-09 | §5.6: a report whose path does not end at the reporting gateway is refused, `not-authorized`. |
| 0.9.0 | 2026-11-09 | `local_pickups` in a report (§5.6), for RP-7's local delivery. |
| 0.10.0 | 2026-11-09 | `POST /direct`: a client reports its station-to-station sessions with both proofs; received mail is filed as picked up, sent mail sent on to every recipient but the other station (§6.6); test vectors (§8). |
| 0.11.0 | 2026-11-09 | `third_party` in waiting entries and copies, for RP-6 §5.1 (§5.2, §5.8). |
| 0.11.1 | 2026-11-09 | §6.1: the enrollment code comes once the callsign is shown (RP-1 §1). |
| 0.11.2 | 2026-11-09 | §5.2, §5.6: the examples' content IDs are real ones (RP-0 §6.3) |
| 0.12.0 | 2026-11-10 | §5.2: a statement in challenge form, whose challenge the gateway issued and checked, is taken once; a second time it is `signature-invalid`. Found by building the gateway: no post office could open such a session. Test vectors (`api-exchanges.json`). |
| 0.12.1 | 2026-11-10 | §6.1: a refused enrollment leaves its code good; only one that succeeds uses it. Found by the conformance tester (P-03): the reference used a code at any attempt. Test vectors. |
| 0.13.0 | 2026-11-10 | §5.2: `held` in a waiting entry is set when the message lists held attachments (it had always been false). §5.3: `files`' entries, `ref` in base64url, and its refusals; what is sent is RP-6 §2.5. |
| 0.14.0 | 2026-11-12 | Applications (§6.7): an account asked for from a new device, signed under `RadioPost-api-apply-v1`, and collected once an operator decides; `applications` in the info document (§4.1). Operator endpoints (§7): the post office's operators run it with their own credentials (status, applications, accounts, codes, revocation); `office` in the login's answer (§3.2). Security considerations and test vectors renumbered §8 and §9. Test vectors (`api-office.json`). |
| 0.15.0 | 2026-11-12 | Who may become an operator (§7.1): an application naming an operator's callsign is refused (§6.7) and cannot be approved or sent a postcard (§7.3); a code naming one for anyone but the operator asking needs `operates` (§7.5); each refusal carries `operates`; security considerations (§8); test vectors. |
| 0.16.0 | 2026-11-12 | Gateways across post offices. §4.3: each entry's `api` and keys, one entry per domain. §4.4: a gateway's own post office; a client's gateway list, and the gateways a post office places copies at, come from its home's list and those of the post offices its home knows, checked with their keys; the home's record wins, and a callsign otherwise listed twice is ambiguous; `listed` keeps to the home's. §5.8: copies at gateways registered elsewhere; `placing` in the answer; `wait=0`; items served only to the credential of the post office that listed the gateway; which post offices a gateway polls. §6.3: `usual_gateways` may name them. §2: known post offices are RP-1 §7.5's. A client could list only its own post office's gateways, and a post office placed copies only at the gateways registered with it. Test vectors (`gateway-lists.json`; `api-exchanges.json`'s copies at N1CALL-10; `placing` in every copies answer). |
| 0.17.0 | 2026-11-13 | §4.4: a channel's `modem` comes from RP-0 §9.4's registry, and a reader ignores a channel whose modem it does not know (the list of six had no way to grow and no rule for a name a client did not know); `mode` `USB` or `FM`; on an `FM` channel `dial_hz` is the carrier and `bandwidth_hz` the channel's width. |
| 0.18.0 | 2026-11-13 | §5.3: the post office's air form uses compression 0 or 1, 1 when it gives fewer bytes, never 2 or 3, and the sizes that decide what is held are judged on it. Nothing said which: the air form is made once for every gateway and never sees the caller's `caps`, so a post office using 2 would serve mail some clients cannot read. |
