# RP-5: Post office API

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.3.0 |
| **Updated** | 2026-11-09 |
| **Editor** | KK4ODA |
| **Licence** | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| **Depends on** | RP-0, RP-1, RP-2, RP-3 |
| **Protocol version** | 1 (`/rp/v1/`) |

The HTTPS interface of a post office. **Gateways** use it to carry their users' sessions, report
pickups, hand in mail queued while offline and receive copies placed ahead. **Clients** use it to
enroll, renew credentials and change their settings. **Anyone** may read a post office's public
documents: its keys, revocations, the post offices it knows, its gateways and its radio routes.

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

A post office accepts gateways credentialed by **any post office it knows** (RP-1 §7.3), unless its
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

The post office verifies the credential (RP-1 §5) and the signature, and answers:

```json
{"token": "tok_8Hc…", "expires": 1794330000, "address": "n1call@hampost.org",
 "operator": "N1CALL-10", "flags": 9}
```

The token is sent as `Authorization: Bearer tok_8Hc…` on later requests. It lasts at most one hour
and is bound to the credential: a revocation ends it.

---

## 4. Public documents

Each is a signed JWS. A gateway SHOULD fetch and keep them for every post office it knows (RP-1
§7.3). The payloads:

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

### 4.2 `GET /revocations`

The revocation list of RP-1 §8.

### 4.3 `GET /post-offices`

The post offices this one knows and trusts, for gateways to merge (RP-1 §7.3):

```json
{"issued": 1794312000,
 "post_offices": [{"domain": "hampost.org", "api": "https://po.hampost.org/rp/v1",
                   "keys": [{"kid": "71c2e0a4", "public": "…"}]}]}
```

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

`modem` names the modem in RPMI terms (`aether`, `vara-hf`, `vara-fm`, `ardop`, `ax25`, `tcp`).

### 4.5 `GET /routes`

This post office's radio routes, for forwarding when the Internet is down. The format is RP-7's
(*outline*).

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
  "cursor": 418,
  "partials": [{"id": "k3q7m2x9a4d8f6h1j5n0", "offset": 512}],
  "renew": true
}
```

* `statement`: the user's session statement (RP-1 §6.1), deterministically encoded as the gateway
  rebuilt it from `HELLO` (or `AUTH`), base64url.
* `started`: the gateway's clock when `HELLO` arrived.

The post office verifies the credential and the statement (RP-1 §6.3, checking `time` against
`started` and `started` against its own clock within 600 seconds), and answers `201`:

```json
{
  "session": "s_4Nf…",
  "expires": 1794330041,
  "waiting": [{"serial": 419, "id": "c1…", "from": "maria@example.com",
               "subject": "Are you safe?", "size": 1104, "priority": 0, "held": false}],
  "more": 0,
  "accept": [{"id": "a3f9…", "offset": 0}],
  "decline": [],
  "credential": "gqNk…"
}
```

* `accept` / `decline`: the post office's view of the user's offers (`already-have`, `too-large`,
  `refused-by-policy`), which the gateway passes on in `WELCOME`.
* `credential`: a renewed credential, when `renew` was asked and the post office issued one.

### 5.3 Messages for the user

| Request | Does |
|---|---|
| `GET /sessions/{s}/waiting?after={serial}&limit={n}` | The next page of the waiting list (RP-3 `REQUEST` `more`). |
| `GET /sessions/{s}/messages/{serial}` | One waiting message: `{"serial": 419, "id": "c1…", "air": "AXic…"}`. |
| `POST /sessions/{s}/files` | `{"files": [{"serial": 419, "ref": "…"}]}`: send these held attachments. The post office answers `202` and later puts them in the mailbox as a new message, which the next waiting list shows. |

The post office makes a message's air form once and serves those same bytes every time it is
asked, and in every copy placed ahead (§5.8). A user whose download broke resumes it through any
gateway by its offset (RP-3 §7), which works only if every gateway sends the same bytes.

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
had it), or `refused` with `code` and `text`. Only `accepted` and `duplicate` let the gateway list
the message `complete` in its `RECEIPT` before it holds the message itself; a gateway that stores
the message first MAY receipt it at once and hand it in later.

### 5.5 `POST /sessions/{s}/pickups`

```json
{"serials": [419, 420]}
```

Sent when the user's `RECEIPT` lists the messages complete (RP-3 §5.5). The post office marks them
picked up and has any copies placed ahead dropped (§5.8). `204`.

### 5.6 `POST /reports`: what happened while offline

A gateway that carried sessions while it could not reach a post office reports them when it can,
oldest first:

```json
{"sessions": [{
  "statement": "pgF2…", "signature": "9b1f…", "credential": "gqNk…",
  "started": 1794326441,
  "messages": [{"id": "a3f9…", "air": "AXic…"}],
  "pickups": [419, 420]
}]}
```

The post office verifies each statement as in §5.2 but checks `time` against `started` only, takes
the messages as in §5.4 and the pickups as in §5.5, and answers with a result per message. Messages
for other post offices go there (§5.4 applies at each).

### 5.7 `DELETE /sessions/{s}`

The session is over. `204`. Sessions also end on their own at `expires`.

### 5.8 Copies placed ahead

A post office places copies of a user's waiting mail at each gateway the user named as usual
(§6.3), when that gateway's record says it holds copies (§4.4).

**`GET /copies?after={cursor}&wait={seconds}`**: long poll, `wait` at most 60. Answer:

```json
{"cursor": "c_9…",
 "items": [
   {"kind": "copy", "address": "facundo@radiopost.org", "serial": 421, "id": "4e…",
    "from": "maria@example.com", "subject": "Photo", "size": 310522, "priority": 0,
    "expires": 1794931200},
   {"kind": "copy", "address": "facundo@radiopost.org", "serial": 419, "id": "c1…",
    "from": "maria@example.com", "subject": "Are you safe?", "size": 1104, "priority": 0,
    "air": "AXic…", "expires": 1794931200},
   {"kind": "drop", "address": "facundo@radiopost.org", "serial": 418}
 ]}
```

* A `copy` carries `air` when the message fits the gateway's `copies.max_air_bytes`; otherwise only
  its list entry, and the gateway lists it with the `held` flag while offline.
* A `drop` means the message was picked up elsewhere, or expired: the gateway deletes its copy.

**`POST /copies/ack`** `{"cursor": "c_9…"}`: the gateway holds everything up to the cursor. The
post office sends nothing twice after an acknowledged cursor.

A gateway behind a home router uses only outgoing connections: the long poll is how copies reach it.

---

## 6. Client endpoints

### 6.1 `POST /enroll`: a new device

No token. The post office's operator gives the user a one-time **enrollment code** (by its website,
by email, in person). The client generates a device key and sends:

```json
{"address": "facundo@radiopost.org", "operator": "KK4ODA", "code": "7Q4F-AZ9K",
 "device": "Hk3sW…", "signature": "…"}
```

`signature` is the new device key's signature with context `RadioPost-api-enroll-v1` over the UTF-8 bytes
of the code followed by those of the base URL: it proves the client holds the key. The post office
checks the code and the operator, and answers `{"credential": "gqNk…"}`.

### 6.2 `POST /credentials`: renewal

With a token (§3) from the current credential. Body `{}`. Answers `{"credential": "gqNk…"}` for the
same device key. A new device enrolls with a new code instead.

### 6.3 `GET /settings`, `PUT /settings`

```json
{"usual_gateways": ["KK4ODA-1", "N2CALL-5"],
 "auto_bytes": 5000,
 "radio_skips_read": true}
```

| Member | Meaning |
|---|---|
| `usual_gateways` | Where copies are placed ahead (§5.8). |
| `auto_bytes` | A default for the client's `auto` (RP-3 §5.1). |
| `radio_skips_read` | Mail already read in an ordinary mail program is not offered by radio. |

### 6.4 Gateway registration

A licensed user registers a gateway they operate:

* **`POST /gateways`** with a user token: `{"record": {…}, "device": "…", "signature": "…"}`, the
  record as in §4.4 and the gateway's own device key, proved by the gateway key's signature with
  context `RadioPost-api-gateway-v1` over the UTF-8 bytes of the record's callsign followed by
  those of the base URL. The post office checks that the record's `operator` is the
  token's operator, and answers `{"credential": "…"}` with the `gateway` flag, the gateway's
  callsign as `operator` and the user's address as `address`.
* **`PUT /gateways/{callsign}`** updates the record; **`DELETE`** withdraws it.

### 6.5 The accept list

| Request | Does |
|---|---|
| `GET /accept-list` | `{"entries": ["maria@example.com", "@redcross.example"]}`: addresses, or whole domains written `@domain`. |
| `POST /accept-list` | `{"add": […], "remove": […]}` |

How the accept list is used is RP-6's.

---

## 7. Security considerations *(informative)*

* A gateway can do nothing for a user without that user's fresh signed statement, so a stolen
  gateway token cannot read anyone's mail.
* Tokens are short-lived and bound to a credential; revoking the credential ends the token.
* Everything a gateway hands in carries the user's statement, so a post office can always say which
  account sent a message and through which gateway (RP-1 §6.4).
* Enrollment codes are one-time and SHOULD expire within days.

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.1.0 | 2026-11-09 | First draft. |
| 0.1.1 | 2026-11-09 | Licence stated: CC BY 4.0. |
| 0.2.0 | 2026-11-09 | Login, enrollment and gateway registration each sign under a context of their own; test vectors (`vectors/api.json`). |
| 0.3.0 | 2026-11-09 | A message's air form is made once and served the same every time (§5.3), so that downloads resume across gateways (RP-3 §7). |
