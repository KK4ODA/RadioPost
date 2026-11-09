# RP-2: Messages

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.1.1 |
| **Updated** | 2026-11-09 |
| **Editor** | KK4ODA |
| **Licence** | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| **Depends on** | RP-0 |

What a RadioPost message is, how it is encoded and compressed for the air, and how its content ID
is computed. How a message is converted to and from ordinary email is RP-6's; how it is carried is
RP-3's and RP-5's.

---

## 1. The message

A RadioPost message is a mail message cut down to what is worth radio time: addresses, a date, a
subject, plain text and attachments. It carries nothing that changes on the way, so its content ID
(§3) is the same at every station that holds it.

---

## 2. Encoding

A message is a CBOR map (RP-0 §4.1):

| Key | Name | Type | Required | Meaning |
|---|---|---|---|---|
| 1 | `v` | uint | yes | Format version: 1. |
| 2 | `from` | text | yes | The sender's address. |
| 3 | `to` | array of text | yes | Recipients; at least one in `to` or `cc`. |
| 4 | `cc` | array of text | no | Copy recipients. |
| 5 | `date` | uint | yes | When the sender wrote it. |
| 6 | `subject` | text | yes | May be empty. |
| 7 | `body` | text | yes | Plain text, may be empty (RP-0 §4.3). |
| 8 | `files` | array of file | no | Attachments carried in the message. |
| 9 | `priority` | uint | no | 0 routine (default), 1 priority, 2 emergency. |
| 10 | `reply_to` | bstr .size 12 | no | The content ID of the message this answers. |
| 11 | `flags` | uint | no | Bits: 1 `trimmed` (quoted history was removed; the full text is held at the post office), 2 `held` (some attachments are held at the post office; see key 12). |
| 12 | `held` | array of held-file | no | Attachments not carried, which the recipient may ask for. |
| 13 | `form` | map | no | Reserved for forms (a later specification). |

A **file** is a map: `1` `name` (text), `2` `type` (text, an Internet media type such as
`image/jpeg`), `3` `data` (bstr).

A **held-file** is a map: `1` `name` (text), `2` `type` (text), `3` `size` (uint, bytes), `4` `ref`
(bstr, the post office's reference for asking for it; RP-5 §5.3).

Keys 0 and 14–63 are reserved. Receivers MUST ignore keys they do not know (RP-0 §4.1) but MUST
keep them: they are part of the message and of its content ID.

**Priority.** `priority` orders transfers (RP-3 §6) and is shown to the recipient. `2` is for
emergency traffic only, as in the operator's traffic-handling practice.

**Addresses** follow RP-0 §6.2. **Text** follows RP-0 §4.3: a sender MUST normalise `subject`,
`body` and file names to NFC with LF line endings before encoding.

---

## 3. The content ID

A message's **canonical encoding** is its deterministic CBOR encoding (RP-0 §4.1), uncompressed.

```
content_id = SHA-256(canonical encoding)[0:12]
```

A station that receives a message MUST decode it, re-encode it deterministically, and check that
the content ID matches the one it was told (RP-3 §5.4, RP-5 §5.4). A message whose encoding is not
already canonical MUST be rejected (`malformed`), so that every station computes the same ID.

The content ID is what the session statement lists (RP-1 §6), what stations use to recognise a
message they already hold, and how a reply refers to the message it answers.

---

## 4. The air form

A message travels as its **air form**: one byte naming the compression, then the compressed
canonical encoding.

```
air_form = compression (1 byte) || compressed bytes
```

| Byte | Compression | Requirement |
|---|---|---|
| 0 | None | MUST be supported. |
| 1 | DEFLATE, raw (RFC 1951), no zlib or gzip header | MUST be supported. |
| 2 | Zstandard (RFC 8878), no dictionary | MAY be supported. |
| 3 | Zstandard with RadioPost dictionary 1 | Reserved: the dictionary is not yet published. |

A sender chooses a compression both ends support (RP-3 §5.1, `caps`) and SHOULD choose the one that
gives the fewest bytes, including `0` when compression would not help (photos already compressed).

The content ID is computed on the uncompressed encoding, so a station may recompress a message
without changing its ID.

**Why DEFLATE is the baseline:** every platform has it, and it is the method Aether's link already
uses. Zstandard with a dictionary trained on amateur traffic should do much better on short
messages; it becomes a requirement once the dictionary is published with this specification
(*open*).

---

## 5. Size

| Limit | Value |
|---|---|
| A station MUST be able to handle an air form of at least | 64 KiB |
| A post office states its own limit for radio delivery | RP-5 §4.1, `max_air_bytes` |

Larger attachments are **held**: the post office keeps them, the message lists them under `held`,
and the recipient asks for them only if it wants them (RP-3 §6, RP-5 §5.3).

---

## 6. Text shown for a message *(informative)*

Clients show `from`, `subject`, `date` and `priority` in lists, and the `body` with any attachments
when opened. A message with `trimmed` set SHOULD say so, and SHOULD let the user ask for the full
text. Content IDs SHOULD be shown, if at all, as the first 6 base32 characters (RP-0 §6.3).

---

## 7. Example *(informative)*

The message Facundo sends Maria from the concept document, written as diagnostic notation:

```
{
  1: 1,
  2: "facundo@radiopost.org",
  3: ["maria@example.com"],
  5: 1794326400,
  6: "Re: Are you safe?",
  7: "We are fine. Power is out, but the radio works.\n",
  10: h'a3f9c27e15d0b84466e2c901'
}
```

Its canonical encoding is 136 bytes. DEFLATE does not shorten a message this short, so the sender
sends it uncompressed: an air form of 137 bytes, compression byte `0`. A message of a few paragraphs
roughly halves (469 bytes to 234 with DEFLATE).

*To come:* exact bytes, content IDs and air forms as test vectors in `vectors/`, which every
implementation checks itself against.

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.1.0 | 2026-11-09 | First draft. |
| 0.1.1 | 2026-11-09 | Licence stated: CC BY 4.0. |
