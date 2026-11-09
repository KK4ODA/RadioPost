# RP-0: Conventions

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.3.1 |
| **Updated** | 2026-11-09 |
| **Editor** | KK4ODA |
| **Licence** | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |

This document holds what every other RadioPost specification relies on: the words that make a
requirement, the data formats, the cryptography, how identifiers are written, how the
specifications themselves change, and what each kind of program must implement.

---

## 1. Requirement words

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**, **SHOULD**, **SHOULD
NOT**, **RECOMMENDED**, **NOT RECOMMENDED**, **MAY** and **OPTIONAL** are used as described in BCP 14
(RFC 2119, RFC 8174) when, and only when, they appear in capitals.

Sections marked *informative* explain and give examples; they set no requirements.

---

## 2. The specifications

| Number | Title | What it governs |
|---|---|---|
| RP-0 | Conventions | This document. |
| RP-1 | Identity | Accounts, keys, credentials, how a post office publishes its keys, revocation. |
| RP-2 | Messages | The message object, its encoding on the air, its content ID, compression. |
| RP-3 | Session protocol | What a client and a gateway (or two gateways) say over a radio link. |
| RP-4 | Modem interface | How a RadioPost program drives a modem through an adapter. |
| RP-5 | Post office API | HTTPS between gateways or clients and a post office. |
| RP-6 | Email interoperation | How a post office meets ordinary email in both directions. |
| RP-7 | Forwarding | Store-and-forward between gateways by radio. *Outline only.* |

---

## 3. Roles and conformance

A program conforms to RadioPost in one or more **roles**. Each role names what it must implement;
anything else is the implementer's choice. A program may take several roles (a gateway that is also
a post office, a client that talks to a modem directly).

| Role | MUST implement | MAY implement |
|---|---|---|
| **Client** | RP-1 §4–§6 (credentials, verifying a post office), RP-2, RP-3 (client side) | RP-4 (as host), RP-5 §6 (client endpoints) |
| **Gateway** | RP-1, RP-2 §2–§5, RP-3 (gateway side), RP-5 §5 (gateway endpoints, as caller) | RP-4 (as host), RP-7 |
| **Post office** | RP-1 (issuer side), RP-2, RP-5 (server), RP-6 | — |
| **Modem adapter** | RP-4 (adapter side) | — |

The reference programs (the RadioPost client, gateway and post office service) are one
implementation each. They have no special standing: where a reference program and a specification
disagree, the specification is right and the program has a bug.

---

## 4. Data formats

### 4.1 CBOR on the air and in signed objects

Everything sent over the air (RP-3), and every object that is signed (RP-1), is encoded in **CBOR**
(RFC 8949) using the **core deterministic encoding** of RFC 8949 §4.2.1:

* integers and lengths in their shortest form;
* definite lengths only;
* map keys sorted by their encoded bytes;
* no duplicate map keys;
* no floating-point values, no tags, unless a specification allows them.

Objects are CBOR **maps with small unsigned-integer keys**. A key from 0 to 23 costs one byte; names
would cost several on every message.

**Extension rule.** A receiver MUST ignore map keys it does not know, unless the specification for
that object says otherwise. Keys 0–63 are assigned by the specifications; keys from 64 upward are
for experiments and MUST NOT be relied on between independent implementations.

### 4.2 JSON elsewhere

The post office API (RP-5) and the modem interface (RP-4) use **JSON** (RFC 8259), UTF-8, because
they run over fast links and are easy to read and debug. Binary values in JSON are written in
**base64url without padding** (RFC 4648 §5), except where a specification says plain base64.

A receiver MUST ignore JSON members it does not know.

### 4.3 Text

Text is UTF-8, normalised to Unicode NFC. Line endings inside text are a single LF (U+000A).

### 4.4 Time

Times are **Unix seconds** (seconds since 1970-01-01T00:00:00Z, ignoring leap seconds), as unsigned
integers in CBOR and as integers in JSON. Human-readable times in documents use ISO 8601 in UTC.

---

## 5. Cryptography

RadioPost **signs; it never encrypts** anything sent over the air (§7).

| Use | Algorithm |
|---|---|
| Signatures | **Ed25519** (RFC 8032), pure, no prehash. Public keys 32 bytes, signatures 64 bytes. |
| Hashes | **SHA-256** (FIPS 180-4). |
| Signed JSON documents | **JWS** compact serialisation (RFC 7515) with `alg` `EdDSA` (RFC 8037). |
| Transport over the Internet | **TLS** 1.2 or later (HTTPS). |

**Domain separation.** Every signature in RadioPost is made over a fixed ASCII context string, one
zero byte, then the signed bytes:

```
signed_input = context || 0x00 || bytes
```

The contexts are registered in §9.2. A signature made for one purpose can then never be accepted
for another.

**Signed JSON documents** are plain JWS, with no context: the signature is over the JWS signing
input of RFC 7515, which is base64url text and a dot. It never holds the zero byte that ends every
context, so it cannot be taken for any other RadioPost signature, and any JWS library can check it.
The protected header has `alg` `EdDSA` and `kid`, the key's ID in hex. A verifier answers
`malformed` for a JWS that is not three base64url parts with a JSON header and payload, and
`signature-invalid` for any other `alg`, a `crit` header, a `kid` it does not accept (RP-1 §7.2),
or a signature that does not verify. Test vectors: `vectors/jws.json`.

**Key ID.** A key's ID (`kid`) is the first 4 bytes of SHA-256 of its 32-byte public key. It is
written as 8 lowercase hexadecimal digits in text (`3f9a1c22`).

---

## 6. Identifiers

### 6.1 Callsigns

A callsign is written in uppercase ASCII letters and digits, optionally with a prefix or suffix
separated by `/`, and optionally an SSID from `-0` to `-15`:

```
callsign = [prefix "/"] base ["/" suffix] ["-" ssid]
```

`base`, `prefix` and `suffix` are 1 to 10 letters and digits; the whole is at most 20 characters.
Comparison is case-insensitive. A program MUST store and send callsigns in uppercase.

### 6.2 Addresses

A RadioPost address is an Internet mail address (RFC 5321 `Mailbox`, the `addr-spec` of RFC 5322).
The domain part is compared case-insensitively and sent in lowercase.

For accounts a post office creates, the local part SHOULD use only lowercase letters, digits, `.`,
`-` and `_`, so that it is short on the air and cannot be confused.

### 6.3 Content IDs

A message's **content ID** is the first **12 bytes** of SHA-256 of the message's canonical encoding
(RP-2 §3). In text it is written in **base32**, lowercase, RFC 4648 alphabet, no padding: 20
characters (`k3q7m2x9a4d8f6h1j5n0`). A display MAY show the first 6 characters.

Why 12 bytes: 16 would be safer than needed and cost 4 bytes per message in every list sent on the
air; 8 would allow collisions once a network carried billions of messages. With 96 bits, a
collision is not expected before about 10^14 messages.

---

## 7. Amateur radio rules *(informative)*

RadioPost is designed to be used under amateur radio rules: in the US, 47 CFR Part 97. In
particular:

* nothing sent over the air is encrypted, and every encoding is published, so that anyone can
  decode any transmission (§97.113(a)(4), §97.309(a)(4));
* every transmission is made by a licensed operator's station, and every message sent by radio can
  be traced to the licensee who sent it (§97.219);
* stations under automatic control keep to the bands and segments the rules allow (§97.221).

Each specification says what it does toward these. None of this is legal advice, and other
countries' rules differ.

---

## 8. How these specifications change

These are **living documents**. Each one carries a status, a version and a change log.

### 8.1 Status

| Status | Meaning |
|---|---|
| **Draft** | Being written. Anything may change. Implement it to try it out, not to deploy. |
| **Candidate** | Complete, and being tried by at least two independent implementations. Changes only for problems those implementations find. |
| **Stable** | Deployed. Changes only as §8.3 allows. |
| **Outline** | A placeholder: the scope is known, the content is not written yet. |

### 8.2 Document versions

Each document has a version `MAJOR.MINOR.PATCH`:

* **PATCH**: wording, examples or explanations; no requirement changes.
* **MINOR**: an addition an existing implementation can ignore (a new optional key, a new error
  code, a new optional endpoint).
* **MAJOR**: a change an existing implementation cannot ignore.

While a document is a **Draft** its version stays below 1.0.0: a change to its requirements
raises MINOR, anything else PATCH. A Draft's next MINOR may break what the last one allowed.

### 8.3 Protocol versions

The protocols themselves carry small integer versions, negotiated by the programs:

| Protocol | Where negotiated | Current |
|---|---|---|
| Session protocol (RP-3) | `HELLO` / `WELCOME` | 1 |
| Post office API (RP-5) | the URL path, `/rp/v1/` | 1 |
| Modem interface (RP-4) | `hello` | 1 |
| Credential format (RP-1) | the credential's `v` key | 1 |
| Message format (RP-2) | the message's `v` key | 1 |

Once a specification is Stable, a change that an implementation of the current protocol version
cannot ignore MUST come with a new protocol version, and programs SHOULD keep speaking the previous
version for at least a year.

### 8.4 Proposing a change

Changes are proposed as pull requests to the RadioPost repository, as described in
`CONTRIBUTING.md`. A change to a protocol must update, in the same pull request: the specification,
its change log, the affected registries (§9), and the test vectors (§8.6).

### 8.5 Licence

The specifications are published under the Creative Commons Attribution 4.0 International licence
(CC BY 4.0). Anyone may implement them, copy them, adapt them and build on them, for any purpose,
commercial or not, without asking, as long as they credit the RadioPost project. Code in the
RadioPost repository is under the MIT licence or the Apache License 2.0, at the user's option.

### 8.6 Test vectors

The files in `vectors/` hold exact inputs and outputs for RP-1 to RP-6, made from published
test keys by a reference written for the purpose
(`tools/vectors/`, standard library only). Every implementation SHOULD check itself against
them. A change that alters any vector updates the files in the same pull request, and
`python tools/vectors/make.py --check` must pass. Where a vector and a specification disagree,
the specification is right and the reference has a bug.

---

## 9. Registries

Numbers that more than one specification uses are kept here. A new entry is added by a MINOR
version change of this document.

### 9.1 Specification numbers

As in §2. Numbers are never reused.

### 9.2 Signature contexts

| Context string | Signs | Defined in |
|---|---|---|
| `RadioPost-credential-v1` | A credential's payload | RP-1 §4 |
| `RadioPost-session-v1` | A session statement | RP-1 §6 |
| `RadioPost-api-login-v1` | A post office API login | RP-5 §3.2 |
| `RadioPost-api-enroll-v1` | A new device's enrollment | RP-5 §6.1 |
| `RadioPost-api-gateway-v1` | A gateway's registration | RP-5 §6.4 |

### 9.3 Error codes

One numbering is shared by the session protocol (RP-3) and the post office API (RP-5), so a code
means the same thing wherever it appears.

| Code | Name | Meaning |
|---|---|---|
| 1 | `unsupported-version` | No protocol version in common. |
| 2 | `malformed` | A message could not be decoded. |
| 3 | `credential-invalid` | The credential's format or signature is wrong. |
| 4 | `credential-expired` | The credential is past its expiry. |
| 5 | `credential-revoked` | The credential or its key has been revoked. |
| 6 | `credential-required` | The receiver does not hold the sender's credential; send it. |
| 7 | `unknown-post-office` | No key is known for the credential's post office. |
| 8 | `signature-invalid` | A signature does not verify: a session statement's, or a signed document's (§5). |
| 9 | `time-skew` | The statement's time is outside the accepted window; sign a challenge instead. |
| 10 | `not-authorized` | The account or station may not do this. |
| 11 | `too-large` | A message or request is over the limit. |
| 12 | `quota-exceeded` | A mailbox, queue or store is full. |
| 13 | `refused-by-policy` | Refused by the post office's or gateway's policy (the text says why). |
| 14 | `already-have` | The receiver already has this message. |
| 15 | `unknown-message` | No such message or serial. |
| 16 | `post-office-unreachable` | The gateway cannot reach the post office now; work goes on from its store. |
| 17 | `busy` | Try again later. |
| 18 | `timeout` | Nothing arrived in time. |
| 19 | `internal` | Something failed inside the sender of the error. |

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.1.0 | 2026-11-09 | First draft. |
| 0.1.1 | 2026-11-09 | Licence stated: CC BY 4.0 (§8.5). |
| 0.2.0 | 2026-11-09 | Three API signature contexts in place of one (§9.2); Draft versions stay below 1.0.0 (§8.2); test vectors (§8.6). |
| 0.2.1 | 2026-11-09 | §8.6: the vectors cover RP-4 too. |
| 0.3.0 | 2026-11-09 | Signed JSON documents are plain JWS, with no context, and how one is checked (§5); `signature-invalid` covers them (§9.3); §8.6: the vectors cover RP-5. |
| 0.3.1 | 2026-11-09 | §8.6: the vectors cover RP-6 too. |
