# RP-2: Messages

| | |
|---|---|
| **Status** | Stable |
| **Version** | 1.1.0 |
| **Updated** | 2026-11-15 |
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
| 13 | `form` | map | no | A filled-in form: its template's fingerprint and its values (§8). |

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
uses.

**Dictionary 1.** Zstandard with a dictionary should do much better on the short messages that
make up most HF traffic. A dictionary holds verbatim strings from what it was trained on, and is
published like every encoding on the air, so it is trained only on a corpus RadioPost writes and
publishes with this specification, never on real mail: the CBOR structure of messages, common
domains and words, forms in the style of ICS-213, and short notes in English and Spanish. It is
adopted only if, on a held-out part of that corpus, it makes messages of under 1 000 bytes at least
20 % smaller than DEFLATE at their median. Then a MINOR version of this specification publishes the
dictionary and its SHA-256, and code 3 is an option a station uses only with one that says it
supports it (`caps`, RP-3 §5.1), like code 2: never a requirement, so a station without it keeps
working. Until then code 3 is not used.

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
when opened. A message with `trimmed` set SHOULD say so, and SHOULD show the message its `reply_to`
names when the client holds it: that is the history the post office trimmed. The full text stays
at the post office, where the user reads it by email (RP-6). Content IDs SHOULD be shown, if at all, as the first 6 base32 characters (RP-0 §6.3).

---

## 7. Example *(informative)*

The message Facundo sends Maria from the concept document, written as diagnostic notation. Its
`reply_to` is the content ID of Maria's message (`maria-419` in the vectors):

```
{
  1: 1,
  2: "facundo@radiopost.org",
  3: ["maria@example.com"],
  5: 1794234600,
  6: "Re: Are you safe?",
  7: "We are fine. Power is out, but the radio works.\n",
  10: h'47342a407f37d3189af7f7c1'
}
```

Its canonical encoding is 136 bytes and its content ID `kdj2eowb2e7wqq4y5ttq`. Uncompressed, its
air form is 137 bytes; DEFLATE saves a single byte (136). A longer message shrinks much more: the
shelter roster (`eoc-420`) is 1 297 bytes, and 547 as a DEFLATE air form.

Test vectors: `vectors/messages.json` holds these and other messages with their exact encodings,
content IDs and air forms, and air forms that must be refused.

---

## 8. Forms

A form, such as an ICS-213 General Message, travels as its **values alone**. Its wording, the labels
and the order of its fields, stays in a **template** each station holds, named in the message by
the template's fingerprint. Each end makes the form's text from the template (§8.4), so the labels
never go on the air.

Measured on `shelter3-213` (`vectors/messages.json`), an ICS-213 with five fields filled in: 253
bytes on the air with DEFLATE as a form, against 320 for the same text sent as a message's `body`
(333 and 513 bytes uncompressed). The longer a form's wording and the shorter its answers, the more
it saves.

### 8.1 The template

A template is a CBOR map:

| Key | Name | Type | Required | Meaning |
|---|---|---|---|---|
| 1 | `name` | text | yes | The form's name, as shown: `ICS-213 General Message`. |
| 2 | `fields` | array of field | yes | In the order the form shows them. |
| 3 | `note` | text | no | Where the form comes from. |

A **field** is a map:

| Key | Name | Type | Required | Meaning |
|---|---|---|---|---|
| 1 | `n` | uint | yes | Its number, 1 to 255, used once in the template. |
| 2 | `label` | text | yes | As shown. |
| 3 | `type` | uint | yes | Below. |
| 4 | `required` | bool | no | `true`: the form is not filled in without it. |
| 5 | `choices` | array of text | for `choice` | The choices, in order. |
| 6 | `bound` | uint | no | 1: the message's `subject`; 2: the date of its `date`; 3: the time of its `date`. A bound field's value is the message's own, and is never among the values. |
| 7 | `max` | uint | no | The most characters (Unicode code points) of a `line` or `text` value. |

| Type | Name | A value is |
|---|---|---|
| 0 | `line` | text with no line break |
| 1 | `text` | text, lines joined by LF, not ending with one |
| 2 | `date` | uint: days since 1970-01-01, UTC |
| 3 | `time` | uint: minutes after midnight UTC, 0 to 1439 |
| 4 | `number` | int |
| 5 | `choice` | uint: the index of a choice |
| 6 | `check` | bool |

The template's **fingerprint** is the first 8 bytes of SHA-256 of its deterministic CBOR encoding
(RP-0 §4.1). Any change to a template makes another template, with another fingerprint: a published
template is never changed, a new one is published.

RadioPost publishes its templates in its repository's `forms/` directory, as JSON with the names of
the tables above (a type and a `bound` by name), each with its fingerprint in `vectors/forms.json`.
The first is ICS-213, after FEMA's public-domain form. A client SHOULD hold every published
template, and MAY hold others: a group's own forms, shared as files.

### 8.2 The form in a message

Key 13, `form`, is a map:

| Key | Name | Type | Meaning |
|---|---|---|---|
| 1 | `template` | bstr .size 8 | The template's fingerprint. |
| 2 | `values` | map of uint to value | Each field filled in, by its number, a value of its type. |

The values **fill the template** when every number is a field of it that is not bound, every value
is of its field's type and within its `max` and `choices`, no `line` value has a line break, no
`text` value ends with one, no value is empty text (an empty field is left out), and every required
field that is not bound has a value. A client MUST NOT send values that do not fill their template.

A form message's `subject` is the form's subject, whether or not a field is bound to it, so that
lists of messages read as usual. Its `body` holds anything the sender adds to the form, and is
usually empty.

A station that does not hold the template, or finds values that do not fill it, keeps the message
as it is: RP-2's rules for the message do not change, and the content ID covers the form. It shows
the form as §8.4 says.

### 8.3 Templates and the air

Only the fingerprint and the values go on the air. A template is shared beforehand: published with
the programs, or passed between stations as a file. A station that receives a form it has no
template for shows its values by number (§8.4); its operator can ask the sender for the template.

### 8.4 The text a form shows as

With its template, a form message shows as this text, which is also what a post office writes in
the email it makes of it (RP-6 §3.1):

```
<the template's name>

<label>: <value>
<label>:
<a text value, its lines as they are>
```

One line for each field in the template's order that has a value: a bound field the message's
(the subject when it is not empty; the date as `YYYY-MM-DD`; the time as `HH:MM UTC`), any other its
value from `values`. A `text` field's label stands alone and its value follows on the lines after.
A `date` value is written `YYYY-MM-DD`, a `time` `HH:MM UTC`, a `choice` as its choice's text, a
`check` `Yes` or `No`, a `number` in decimal. A field with no value is left out. The text ends with
a line break, and a non-empty `body` follows after an empty line.

Without its template, or with values that do not fill it, the form shows as:

```
Form <the fingerprint in lowercase hex>

Field <n>: <value>
Field <n>:
<a value with line breaks, its lines as they are>
```

the values in the order of their numbers, text as it is, an integer in decimal, a bool `Yes` or `No`;
then the `body` in the same way.

Test vectors: `vectors/forms.json`: ICS-213's encoding and fingerprint, `shelter3-213`'s text with
and without its template, and ten sets of values checked against the template.

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.1.0 | 2026-11-09 | First draft. |
| 0.1.1 | 2026-11-09 | Licence stated: CC BY 4.0. |
| 0.1.2 | 2026-11-09 | The example is the test vector's, its sizes measured; test vectors. |
| 0.2.0 | 2026-11-09 | Dictionary 1: trained only on a published corpus, adopted only if it beats DEFLATE by 20 % on short messages (§4). |
| 1.0.0 | 2026-11-15 | Stable (D41). §4: dictionary 1, if adopted, is an option both ends say they support, never a requirement (gap G7). §6: a trimmed message points to the one it answers; nothing on the air asks for the full text (gap G1). |
| 1.1.0 | 2026-11-15 | Forms (§8, key 13, wishlist W32): a form travels as its template's fingerprint and its values by field number, the template held by each station; the template's CBOR map, its fingerprint, the values that fill it and the text a form shows as. An implementation that does not show forms keeps key 13 as before: nothing it must do. Test vectors: `forms.json`, and `shelter3-213` in `messages.json`. |
