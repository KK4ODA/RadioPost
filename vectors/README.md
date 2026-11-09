# RadioPost test vectors

Exact inputs and outputs for the RadioPost protocols. An implementation that produces these bytes
and gives these verdicts agrees with every other one that does. Check against them before going on
the air.

The vectors are **made, not typed**. `tools/vectors/make.py` builds every file from a small
reference written to read like the specifications (`tools/vectors/rpref/`: deterministic CBOR,
Ed25519 after RFC 8032 §6, and RadioPost's objects). It uses only the Python standard library.
Before it writes anything, the reference checks its Ed25519 against RFC 8032 §7.1 and its CBOR
against RFC 8949's own examples.

**The keys are test keys.** Their private halves are published in `keys.json`. Never use them for
anything but checking an implementation.

## The files

| File | Spec | What it holds |
|---|---|---|
| `keys.json` | RP-0 §5, RP-1 | The eleven test keys: four post office keys (radiopost.org's current, previous and new ones, hampost.org's) and seven device keys (Facundo's laptop and tablet, Ana's laptop, a shelter's laptop under two operators, two gateways). Each has its seed, public key and key ID, and a post office key its validity. |
| `cbor.json` | RP-0 §4.1 | Values and their deterministic encodings; encodings a decoder must refuse (a long form, an indefinite length, keys out of order or repeated, a float, a tag, bytes after the item). |
| `messages.json` | RP-2 | Twelve messages, from a two-line reply to a 6 kB photo. Each has its fields, canonical encoding, content ID (bytes and text) and air forms. Also air forms that must be refused as `malformed`. |
| `credentials.json` | RP-1 §4, §5 | Ten credentials with every step of their making, and sixteen verifications: valid, expired, revoked by serial or by key, signed by an unknown or retired key, issued too far ahead, missing a right, malformed. Each gives the RP-0 §9.3 error a verifier must return. |
| `statements.json` | RP-1 §6 | Session statements in time form and challenge form and a statement by the station called (§6.5), and fourteen verifications (wrong address, operator or gateway; bad signature; clock outside the window; a nonce seen before; a wrong challenge; a called station's statement checked by the caller's nonce, whatever its clock). |
| `pdus.json` | RP-3 §4 | One of each PDU type, with each kind of `WELCOME`, encoded and framed, and frames that must be refused. |
| `session-01.json` | RP-3 | The example session of RP-3 §12 byte for byte: five transmissions, both stations' byte streams, and what the gateway reports to the post office afterwards. |
| `session-02.json` | RP-3 §7 | Three sessions, byte for byte, with the links breaking: an upload broken and resumed through the same gateway, then a download broken and resumed through another gateway the next morning. |
| `session-03.json` | RP-3 §10 | Station to station, byte for byte: Facundo calls Ana's client directly; it takes the message for Ana, refuses the one for someone else, proves itself and hands over Ana's message. |
| `rpmi-lines.json` | RP-4 §2 | Lines an adapter reads (a string `id`, an unknown member) and lines it answers with an error: not JSON, no `op`, a bad `id`, `data` that is not standard base64 with padding, an unknown `op`. |
| `rpmi-conversations.json` | RP-4 | Conversations between hosts and a simulated modem's adapters, line by line: session-01 carried over RPMI on both stations, every error a host can meet, a modem shared by two programs, a broadcast. |
| `dns.json` | RP-1 §7.1 | `_radiopost` TXT records, and which of them are read and which ignored. |
| `api.json` | RP-5 §3.2, §6.1, §6.4 | The login, enrollment and gateway registration signatures. |
| `jws.json` | RP-0 §5, RP-1 §7.2, RP-5 §4 | The post office's four public documents as JWS, and eight verifications: by the key in DNS, by a new key only the info document lists, a changed payload, `alg` none, a key that lists itself, broken forms. |
| `email-door.json` | RP-6 §2.1, §2.2 | Eighteen emails at the SMTP door, with the replies to `RCPT TO` and `DATA`: lists and their matching, a known post office with and without DKIM, DMARC, size, every kind of encryption and a signed message, an executable, and delivery reports with their null sender. |
| `email-in.json` | RP-6 §2.4, §3.3, §4 | Seven emails and the messages they become (Maria's two become `maria-419` and `maria-421` exactly; replies trimmed, HTML, a held attachment), a delivery report that becomes a bounce, and Facundo's message reaching Ana's post office with its RadioPost part, then three parts that must not be used. |
| `email-out.json` | RP-6 §3, §4 | Facundo's two messages of session-01 and a priority message as email: headers, text, footer, `Radio-Path`, and the RadioPost part between post offices. |
| `api-exchanges.json` | RP-5 | Requests to a reference post office and its answers: a tablet enrolled, a gateway registered, copies placed ahead and dropped, logins and an expired token, session-01 at the post office, an offline gateway's report; and ten cases, from a session opened cleanly to a replayed statement and messages refused for each reason. |

Each file starts with the same header: what it holds (`radiopost_vectors`), the file format
(`format`, now 1), the sections it tests (`specs`), and a `rule` that says in one sentence what an
implementation must reproduce and what it must refuse.

## Conventions

* Bytes are lowercase hex in fields ending `_hex`.
* CBOR values are also given in RFC 8949 §8 diagnostic notation (`diagnostic`), for people reading
  the files. The hex is what counts.
* Times are Unix seconds (RP-0 §4.4).
* A verification case's `expect` is `ok` or an error name from RP-0 §9.3.
* RPMI messages (RP-4) and the post office API's bodies (RP-5) are JSON, which has no one
  spelling: they compare as JSON values, not as text, and a `text` or `title` member, written for
  people, need not match. Emails (RP-6) compare by header value, unfolded and decoded, and by
  decoded text; header order, folding and MIME boundaries are the writer's. Their `data` is standard base64,
  and exact. In a conversation, how a simulated `received` splits the bytes, and the link IDs, are
  the simulated modem's choices; a real adapter makes its own.

### Exact and inflate-only air forms

The bytes of a DEFLATE stream depend on the compressor and its settings, and any valid stream is
correct (RP-2 §4). So an air form with compression 1 is marked `"exact": false`: check that it
inflates to the canonical encoding, not that your compressor makes the same bytes. Everything else
is exact, including every air form in the session files, which send their messages uncompressed
so that whole sessions can be compared byte for byte.

### Broken links

In `session-02.json` a transmission marked `broken` was written whole, but the other station
received only its first `received_bytes`: the `complete_pdus` before the break, and part of the
next, which it drops (RP-3 §7). Each session's `after` says what each station then holds, and the
next session starts from that.

## Using them

Read the JSON with any library, and for each file:

1. **Make** each object from its inputs and compare the bytes: encodings, content IDs,
   signatures, frames.
2. **Check** each verification case: give your verifier the case's inputs and compare its verdict
   with `expect`.
3. **Refuse** each `invalid` item, with the error named.
4. **Replay** each conversation (`session-*.json`, `rpmi-conversations.json`,
   `api-exchanges.json`): play one side from the file and compare what your program says with the
   other side.

Ed25519 signatures are deterministic, so a correct signer reproduces `signature_hex` exactly.

## Changing them

The vectors follow the specifications: **never edit a file by hand.** Change the reference or
`make.py`, then run:

```
python tools/vectors/make.py            # write every file
python tools/vectors/make.py --check    # rebuild in memory and fail if a file differs
```

A change that alters a vector changes a specification. The pull request updates the specification,
its version and change log, and these files together (RP-0 §8.4, §8.6). Where a vector and a
specification disagree, the specification is right and the reference has a bug.

## Not yet covered

Forwarding (RP-7, an outline), DKIM signing (the mail server's), photo re-encoding (RP-6 §2.4
step 5, a SHOULD this reference skips), and the post office API's accept list and credential
renewal (RP-5 §6.2, §6.5). The modem interface's mapping to real modems (RP-4 §9) is informative and
has none.

## Licence

The vector files are data for checking implementations of the specifications, and are published
with them under CC BY 4.0. The reference code in `tools/vectors/` is under MIT or Apache-2.0.
