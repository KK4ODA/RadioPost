# RP-1: Identity

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.2.0 |
| **Updated** | 2026-11-09 |
| **Editor** | KK4ODA |
| **Licence** | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| **Depends on** | RP-0 |

How RadioPost knows who is who: accounts, the keys of devices and post offices, the credential a
post office issues, the statement a client signs at the start of a session, how a post office
publishes its keys, and how credentials are revoked. Requirement words, CBOR rules, Ed25519 and the
signature contexts are those of RP-0.

The design goal: **a gateway can check any user of any post office it knows, with no Internet
connection**, and no server ever holds a user's secret.

---

## 1. Accounts *(informative)*

A post office holds **accounts**. Each has an address (`facundo@radiopost.org`). There are three
kinds:

* **Personal**: owned by one licensed amateur.
* **Role**: `shelter3@`, `eoc@`. Owned by a licensed trustee, who authorises the licensed operators
  that may use it.
* **Email only**: a member who does not transmit. They use ordinary mail programs (RP-6) and hold no
  credential.

How a post office creates accounts and checks a licence is its own business. It SHOULD check that
the operator holds the callsign it puts in a credential, for example with an ARRL Logbook of the
World certificate or by the sysop's own check.

---

## 2. Keys

| Key | Held by | Purpose |
|---|---|---|
| **Device key** | Each client device or gateway, generated there | Signs session statements (§6) and API logins (RP-5 §3). The private half never leaves the device. |
| **Post office key** | The post office | Signs credentials (§4) and the post office's published documents (§7). |

All are Ed25519 keys (RP-0 §5). A post office MAY hold several keys at once, to rotate them (§7.4).

---

## 3. Who a credential can describe

A credential binds together:

* an **address** (the account);
* an **operator callsign** (the licensee who uses this device for this account);
* a **device key**;
* what the holder may do (**flags**);
* a validity period.

A role mailbox has one credential per authorised operator, each with that operator's callsign and
device key. A gateway has a credential too, with the `gateway` flag, its station callsign as the
operator, and its operator's account as the address.

---

## 4. The credential

### 4.1 Format

A credential is a CBOR array of two byte strings:

```
credential = [ payload, signature ]
payload    = bstr .cbor credential-body
signature  = bstr .size 64
```

`credential-body` is a CBOR map:

| Key | Name | Type | Meaning |
|---|---|---|---|
| 1 | `v` | uint | Format version: 1. |
| 2 | `serial` | uint | Unique among the credentials this post office has issued. |
| 3 | `address` | text | The account (RP-0 §6.2). |
| 4 | `operator` | text | The operator's callsign (RP-0 §6.1). |
| 5 | `device` | bstr .size 32 | The device's Ed25519 public key. |
| 6 | `issued` | uint | Time of issue. |
| 7 | `expires` | uint | The credential is not valid at or after this time. |
| 8 | `kid` | bstr .size 4 | The ID of the post office key that signed it. |
| 9 | `flags` | uint | Bits: 1 `send` (may send mail), 2 `receive` (may pick up mail), 4 `role` (a role mailbox), 8 `gateway` (may act as a gateway: relay sessions and forward mail). |

The signature is the post office key's Ed25519 signature (RP-0 §5) with context
`RadioPost-credential-v1` over the `payload` bytes as they are, not re-encoded.

A credential is typically about 160 bytes.

### 4.2 Lifetime

A post office SHOULD issue credentials valid for no more than **90 days**. A client renews its
credential whenever it can reach its post office, by the Internet (RP-5 §6) or during a radio
session (RP-3 §5.8). Short lifetimes bound how long a revoked credential can still be accepted by
a gateway that has been offline (§8).

### 4.3 The credential hash

Where a credential is referred to rather than sent, it is identified by the first 8 bytes of
SHA-256 of the whole credential (the CBOR array). Gateways index the credentials they hold by it.

---

## 5. Verifying a credential

A verifier (a gateway, a post office, a client checking a gateway) MUST check, in order, and reject
the credential at the first failure:

1. It decodes as §4.1, and `v` is a version it supports. Otherwise `credential-invalid`.
2. The address's domain names a post office whose keys the verifier holds (§7). Otherwise
   `unknown-post-office`.
3. One of that post office's keys has the `kid`, is in its validity period at `issued`, and its
   signature over `payload` verifies. Otherwise `credential-invalid`.
4. The verifier's clock is before `expires`, and not more than 600 seconds before `issued`.
   Otherwise `credential-expired` (or `credential-invalid` for a future `issued`).
5. The `serial`, and the `kid`, are not in the post office's revocation list the verifier holds
   (§8). Otherwise `credential-revoked`.
6. The flags allow what is being asked. Otherwise `not-authorized`.

A verifier that is offline uses the keys and revocation list it last fetched.

---

## 6. The session statement

At the start of each session (RP-3) the client proves who it is by signing a **session
statement** with its device key. The same statement later proves, to the post office and to anyone
who carries the messages onward, that this account sent these messages through this gateway.

### 6.1 Format

A CBOR map:

| Key | Name | Type | Meaning |
|---|---|---|---|
| 1 | `address` | text | The account. |
| 2 | `operator` | text | The operator's callsign, as on the air. |
| 3 | `gateway` | text | The callsign of the gateway called. |
| 4 | `time` | uint | The client's clock when it built the statement. Absent in challenge form. |
| 5 | `nonce` | bstr .size 8 | Random. Absent in challenge form. |
| 6 | `messages` | array of bstr .size 12 | The content IDs of every message the client offers in this session, in the order offered. May be empty. |
| 7 | `challenge` | bstr .size 16 | The gateway's challenge. Present only in challenge form. |

There are two forms:

* **Time form** (keys 4 and 5): built before the session starts and sent in the client's first
  transmission. Needs the two clocks to agree within the window of §6.3.
* **Challenge form** (key 7): used when the gateway asks for it, because the clocks disagree or the
  first transmission was unsigned.

### 6.2 Signature

The device key's Ed25519 signature with context `RadioPost-session-v1` over the statement's
deterministic CBOR encoding (RP-0 §4.1).

A statement and its signature are carried on the air in pieces (RP-3 §5.1); the receiver rebuilds
the map from them and checks the signature over the rebuilt encoding.

### 6.3 Verifying a statement

A verifier MUST check, in this order, and stop at the first failure:

1. The credential, by §5, with the flags `send` if `messages` is not empty, and `receive` if mail
   is to be handed over. Otherwise the error §5 gives.
2. `address` and `operator` equal the credential's. Otherwise `not-authorized`.
3. `gateway` is the verifier's own callsign, when the verifier is the gateway called. Otherwise
   `not-authorized`.
4. The signature, with the credential's device key. Otherwise `signature-invalid`.
5. In time form: `time` is within **600 seconds** of the verifier's clock (600 is within), and the
   pair (`address`, `nonce`) has not been seen in the last 1 200 seconds; the gateway keeps a replay
   cache for that long. Otherwise `time-skew`: the gateway asks for a challenge (RP-3 §5.2).
6. In challenge form: `challenge` is the one the verifier issued for this session. Otherwise
   `signature-invalid`: a signature over another challenge is a signature over another statement.

A post office verifying a statement relayed later (RP-5 §5.6) checks 1, 2 and 4, and checks `time`
against the gateway's reported session time instead of its own clock.

Test vectors: `vectors/credentials.json` (§4, §5) and `vectors/statements.json` (§6.1–§6.3).

### 6.4 Proof of submission

When a message enters RadioPost by radio, the gateway keeps its **proof**: the sender's
credential, the statement, the statement's signature, and the session's start time. Anyone holding
the message and its proof can check that the account in the statement sent it, because the
statement lists the message's content ID and the content ID is a hash of the message (RP-2 §3).
Gateways pass the proof on with the message (RP-5 §5.4, RP-7) so that a forwarding station can
carry a message but never alter it or forge its sender.

---

## 7. How a post office publishes its keys

### 7.1 DNS records

A post office publishes TXT records at `_radiopost.<domain>` for every domain it serves:

```
_radiopost.radiopost.org.  TXT  "v=RP1; t=api; url=https://po.radiopost.org/rp/v1"
_radiopost.radiopost.org.  TXT  "v=RP1; t=key; kid=3f9a1c22; p=Hk3sW…9Q="
```

| Record | Tags |
|---|---|
| `t=api` | `url`: the base URL of the post office API (RP-5). Exactly one. |
| `t=key` | `kid`: the key ID in hex; `p`: the 32-byte public key in standard base64. One per current key. |

Tags are separated by `;`; spaces around a tag, its name and its value are ignored. Unknown tags
MUST be ignored. A whole record MUST be ignored when:

* its `v` is not `RP1`, or its `t` is missing or unknown;
* a tag appears more than once;
* an API record's `url` does not begin `https://`;
* a key record's `p` is not 32 bytes in base64, or its `kid` is not that key's ID (RP-0 §5).

The domain SHOULD be signed with DNSSEC, and a verifier that validates DNSSEC SHOULD use it.

Test vectors: `vectors/dns.json`.

### 7.2 The info document

`GET {api}/info` (RP-5 §4.1) returns a JWS (RP-0 §5) signed by one of the keys in DNS, whose
payload lists, among other things, every current key with its validity period:

```json
{
  "keys": [
    { "kid": "3f9a1c22", "public": "Hk3sW…9Q", "not_before": 1794268800, "not_after": 1857340800 }
  ]
}
```

A verifier MUST accept a key only if it appears in DNS **or** in an info document signed by a key
that appears in DNS. This lets a post office announce a new key before the DNS change has spread.

### 7.3 What gateways keep

A gateway SHOULD keep, for every post office it knows: its keys, its info document, and its
revocation list. It refreshes them at least every **6 hours** while online, and keeps using what it
holds while offline. It learns a post office:

* when one of its users first calls while the gateway is online (it looks up the address's domain);
* from the **post office lists** that post offices publish (RP-5 §4.3), which it merges for the
  post offices it trusts.

*Open:* how a new post office comes to be trusted by gateways that have never met its users.

### 7.4 Rotating keys

To rotate, a post office adds the new key (in DNS and its info document), signs new credentials
with it, and keeps the old key listed until every credential it signed has expired. A key that is
compromised is revoked at once (§8).

---

## 8. Revocation

A post office publishes a **revocation list** at `GET {api}/revocations` (RP-5 §4.2): a JWS whose
payload is

```json
{
  "issued": 1794312000,
  "next": 1794333600,
  "serials": [ 1041, 1077 ],
  "kids": [ ]
}
```

* `serials`: credentials revoked (a lost device, an operator removed from a role mailbox).
* `kids`: post office keys revoked; every credential they signed is revoked with them.
* `next`: when the list will next be updated; verifiers SHOULD fetch it again by then when online.

A serial MAY be dropped from the list once the credential it names has expired.

---

## 9. Security considerations *(informative)*

* **What a stolen device can do.** Send and pick up mail as its account until the credential is
  revoked and gateways have fetched the list, or until it expires (§4.2).
* **What a gateway can do.** It sees all traffic it carries (as does anyone listening on the air).
  It cannot forge a sender, because it does not hold the device key, and it cannot alter a message,
  because the content ID would no longer match the statement.
* **What a post office can do.** It vouches for its users. A dishonest post office could issue
  credentials for callsigns its users do not hold; gateways and other post offices trust a post
  office by choosing to keep its keys (§7.3).
* **Replay.** A statement in time form can be replayed only within its window and only before the
  gateway's replay cache has seen it (§6.3); a challenge-form statement only in its own session.

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.1.0 | 2026-11-09 | First draft. |
| 0.1.1 | 2026-11-09 | Licence stated: CC BY 4.0. |
| 0.2.0 | 2026-11-09 | Each statement check names its error, a replay answers `time-skew` (§6.3); when a DNS record is ignored (§7.1); test vectors. |
