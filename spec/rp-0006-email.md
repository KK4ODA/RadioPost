# RP-6: Email interoperation

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.7.0 |
| **Updated** | 2026-11-13 |
| **Editor** | KK4ODA |
| **Licence** | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| **Depends on** | RP-0, RP-1, RP-2, RP-5 |

How a post office meets ordinary email: what it must publish, what it takes in and what it
refuses, how an email becomes a RadioPost message, and how a RadioPost message becomes an email.
Ordinary email here means Internet mail as RFC 5321 and RFC 5322 define it.

---

## 1. What a post office must publish

For every domain it serves, a post office MUST publish:

| Record | Example | Why |
|---|---|---|
| MX | `radiopost.org. MX 10 po.radiopost.org.` | Where mail for the domain goes. A backup server MAY be listed at a lower priority. |
| SPF (RFC 7208) | `radiopost.org. TXT "v=spf1 mx -all"` | Which servers may send for the domain; name a relay too if one is used. |
| DKIM (RFC 6376) | `po2026._domainkey.radiopost.org. TXT "v=DKIM1; k=rsa; p=…"` | Outgoing mail is signed. RSA, because many receivers do not check Ed25519 DKIM. |
| DMARC (RFC 7489) | `_dmarc.radiopost.org. TXT "v=DMARC1; p=quarantine; …"` | Receivers know what to do with forgeries. |
| RadioPost | `_radiopost.radiopost.org. TXT "v=RP1; …"` | RP-1 §7.1. |

Each sending server's address MUST have reverse DNS naming the server, and that name MUST resolve
back to the address.

---

## 2. Mail arriving from the Internet

### 2.1 At the door

A post office decides during the SMTP conversation, never after it. A message is either taken or
refused with an SMTP reply the sending server passes back to its sender. A post office MUST NOT
accept a message and send a refusal later: that sends bounces to forged senders (*backscatter*).

The door judges the recipients that are accounts of the post office. Any other address at its
domains (`postmaster@`, which RFC 5321 §4.5.1 requires, `abuse@`, the operator's own) is not
RadioPost's to judge: the door lets the mail server have it, and the mail server delivers it to a
mailbox of its own or refuses it as unknown.

It makes the checks below in order, and the first that fails gives the refusal. They rest on the
DMARC result for the message's `From:` domain and on which domains a DKIM signature passes for:

* The mail server's verdicts, when it hands them to the door in fields of its own.
* Otherwise the post office's own: SPF (RFC 7208) for the envelope sender, DKIM (RFC 6376) and
  DMARC (RFC 7489) for the message, from the connecting client's address and the name it gave in
  EHLO, the envelope sender and the message as the client sent it. Stalwart 1.1 hands its MTA
  hooks no verdicts.
* Never from `Authentication-Results` headers in the message itself: its sender can write them.

A check that cannot be finished for now (a DNS lookup failed, or the door was given neither
verdicts nor the client) refuses the message with `451 4.7.1`, and the sending server tries again
later.

| When | Check | Refusal |
|---|---|---|
| `RCPT TO` | The envelope sender, or its domain, is on the recipient's accept list (§2.2), or the sender's domain is a RadioPost post office the receiver knows (RP-1 §7.5), or the sender is null (`MAIL FROM:<>`, as delivery reports are sent). | `550 5.7.1` (§2.3) |
| `DATA` | The message passes DMARC for its `From:` domain, when that domain publishes DMARC. | `550 5.7.1` |
| `DATA` | The `From:` address, or its domain, is on the accept list, or is a known RadioPost post office's domain passing DKIM. (The header sender can differ from the envelope sender.) With a null sender instead: the message is a delivery status report (RFC 3464) about a message this post office sent, named by its `Message-ID` (§3.1). | `550 5.7.1` |
| `DATA` | Size within the post office's `max_email_bytes` (RP-5 §4.1). | `552 5.3.4` |
| `DATA` | Not encrypted: no `multipart/encrypted` part, no `application/pkcs7-mime` part other than `smime-type=signed-data`, and no `-----BEGIN PGP MESSAGE-----` in a text part. Signed mail is not encrypted, and is taken. | `554 5.7.1` |
| `DATA` | No executable attachments: no part of type `application/x-msdownload`, `application/x-dosexec` or `application/x-msi`, and no file name ending, in any case, in `.exe`, `.com`, `.scr`, `.pif`, `.bat`, `.cmd`, `.msi`, `.js`, `.jse`, `.vbs`, `.vbe`, `.wsf`, `.hta`, `.jar`, `.ps1` or `.lnk`. | `554 5.7.1` |

A message taken is answered `250 2.0.0`, a recipient taken `250 2.1.5`.

The `DATA` checks are made for each recipient that is an account here, but SMTP gives one reply
to `DATA` for the whole transaction. When the recipients' answers differ (one is on the `From:`
address's accept list and another is not), the post office refuses the message for every
recipient with the first refusal, in the order of `RCPT TO`; the sender's server reports it, and a
sender who writes to each recipient alone reaches those who take it. Recipients at other domains
the mail server serves are not RadioPost's to judge.

A post office MAY apply ordinary spam filtering as well.

Test vectors: `vectors/email-door.json`.

### 2.2 The accept list

Each account has an accept list (RP-5 §6.5) of addresses and domains (`@domain`).

* An entry matches an address ignoring case. `@domain` matches addresses at that domain, not at
  its subdomains.
* Writing to an address adds it (RP-5 §6.5), unless the user's `accept_on_send` is false (RP-5
  §6.3). Test vectors: `vectors/api-account.json`, where an email refused at the door passes once
  the user has written to its sender.
* Mail from RadioPost post offices this one knows (RP-1 §7.5) needs no entry: their users are
  licensed and their mail is signed.
* A role mailbox's list is shared by its operators.

### 2.3 What a refusal says

The reply text SHOULD tell a sender, in plain words, why and what to do. A post office SHOULD use or
adapt:

```
550 5.7.1 <facundo@radiopost.org> reads mail by amateur radio and takes it only
550 5.7.1 from senders on his list. Ask him to add you. Amateur radio carries no
550 5.7.1 business content.
```

```
554 5.7.1 Mail to radiopost.org travels by amateur radio, where encryption is not
554 5.7.1 allowed. Please send it unencrypted.
```

### 2.4 Making the RadioPost message

An accepted email is stored in the mailbox as received; the post office also makes its RadioPost
message (RP-2) for the radio, once, and gives it the account's next **serial** (RP-3 §6.2).

1. **Addresses and date.** `from`: the `From:` address. `to`, `cc`: the `To:` and `Cc:` addresses.
   Display names are dropped and domains written in lowercase (RP-0 §6.2). `date`: the `Date:`
   header, with its time zone, or the arrival time if there is none or it is unreadable.
2. **Subject.** The `Subject:` header, decoded (RFC 2047), as text.
3. **Text.** The `text/plain` part if there is one (in `multipart/alternative`, the plain one),
   otherwise the `text/html` part converted to text: paragraphs and line breaks as new lines,
   entities decoded, and each link followed by `[n]`, its address listed at the end as
   `[n] https://…`. How HTML becomes text is otherwise the post office's own. The text is normalised
   (RP-0 §4.3), each line without trailing spaces, without blank lines at the end, and ends with
   one LF.
4. **Quoted history.** The earlier messages a reply carries are cut at the first line that
   matches, and the post office keeps the full text and sets `trimmed` (RP-2 §2):
   * `-----Original Message-----`, or a `From:` line with a `Sent:` line among the next three
     (with the line of underscores just above it, if there is one): everything from there;
   * `On … wrote:`, on one line or two, when every non-empty line after it begins with `>`:
     everything from there;
   * otherwise, a run of lines at the end that are empty or begin with `>`, when one begins with
     `>`.

   Quotations between the writer's own lines are kept: they are part of the answer.
5. **Attachments.** All are carried in `files` if the message's air form, with them, stays within
   the account's `attach_bytes` (RP-5 §6.3), and never more than the post office's `max_air_bytes`
   (RP-5 §4.1). Otherwise each is listed under `held` with its name, its type, its decoded size and
   the post office's `ref`, and `held` is set in `flags`. Photos (`image/jpeg`, `image/png`,
   `image/heic`, `image/webp`) SHOULD first be re-encoded as JPEG at most 640 pixels on the long
   side.
6. **Priority.** 0. Only a RadioPost sender can mark a message priority or emergency (§4).
7. **Reply.** If `In-Reply-To:` names a message this post office sent from RadioPost (§3.1), set
   `reply_to` to that message's content ID.
8. **Content ID.** As RP-2 §3; the post office keeps the mapping between it and the email's
   `Message-ID`.

Test vectors: `vectors/email-in.json`. Its first two emails become `maria-419` and `maria-421` of
`vectors/messages.json` exactly.

### 2.5 Held attachments asked for

A post office keeps the attachments it held (§2.4 step 5) for as long as it keeps the message that
lists them, picked up or not: a user reads the list only once the message has come. The user asks
for some by the message's serial and each attachment's `ref` (RP-3 §5.6 `files`, RP-5 §5.3), and
the post office puts them in the user's mailbox as a new message:

* from `postmaster@` the domain, to the user, dated when asked;
* subject `Attachments: ` and the subject of the message that listed them;
* `reply_to` that message's content ID, and its priority;
* `files`: the attachments asked for, in the order asked, each with the name and type that message
  listed and its data;
* a body of one line for each: its name, `, `, and its size in bytes, as in `map.pdf, 2400 bytes`;
* marked third-party (§5.1) when that message is.

Attachments of different messages come in different messages, in the order first asked. When
the attachments of one message would make an air form larger than the post office's
`max_air_bytes` (RP-5 §4.1), they come in as many messages as it takes, each with as many of the
next ones as fit. An attachment that does not fit in a message alone cannot travel by radio: the
request is refused (`413` `too-large`, RP-5 §5.3) and nothing is sent. An attachment asked for
twice in one request is sent once.

Test vectors: `vectors/api-exchanges.json`, the case of held attachments asked for.

### 2.6 One mailbox: mail programs and the radio

A user's account has two views of one mailbox: the radio's (RP-5 §5) and the mail server's, which
ordinary mail programs read (RP-5 §4.1 `mail`). Mail that reaches either reaches the other:

* **Mail a user submits.** Mail a user sends from a mail program, signed in to the post office's
  mail server, is not judged at the door (§2.1): the user signed in, and the mail server's own
  rules govern what they may send. Each recipient who is an account of this post office gets its
  RadioPost message, made as §2.4 makes one and with the account's next serial. It is third-party
  mail by §5.1, as every message made from an email is. When the user who signed in is an account
  here, the submission's recipients join their accept list (RP-5 §6.5). A mail server that names
  the user to the door (Stalwart's MTA hooks give `context.sasl.login`) lets the post office tell
  this mail from mail arriving from the Internet.
* **Mail sent by radio.** A message handed in by radio (RP-5 §5.4, §5.6, §6.6) for an account of
  this post office is put in its radio mailbox at once, and is also handed to the mail server as
  the email §3.1 makes, without §4's part, so that a mail program shows it. A recipient at its
  domains that is no account (an email-only member, `postmaster@`) gets only the email. When the
  mail server refuses the email for good, the recipient still has the message by radio, and the
  sender is not told of a failure.
* **Mail read in a mail program** is not offered by radio when the user asks so (RP-5 §6.3
  `radio_skips_read`). The post office matches the two views by `Message-ID`: the one the email
  came with (§2.4 step 8), or the one §3.1 gives a RadioPost message.

What the post office itself submits to its mail server (the email of mail sent by radio, its
notices) is not made a RadioPost message again.

---

## 3. Mail leaving for the Internet

### 3.1 Making the email

A message handed in by radio (RP-5 §5.4) leaves from the post office of its sender's address, never
from a gateway, so that its DKIM signature and SPF match its domain.

| Header | From |
|---|---|
| `From:` | `from`, with the account's display name. |
| `To:`, `Cc:` | `to`, `cc`. |
| `Date:` | `date`, as RFC 5322 date-time in UTC, written `+0000`: `Mon, 09 Nov 2026 14:30:00 +0000`. |
| `Subject:` | `subject`, encoded if needed (RFC 2047). |
| `Message-ID:` | `<` the content ID in base32 `@` the sender's domain `>`: `<kdj2eowb2e7wqq4y5ttq@radiopost.org>`. |
| `In-Reply-To:`, `References:` | When `reply_to` names a message whose `Message-ID` the post office knows (§2.4, step 8). |
| `Radio-Path:` | §3.2. |
| `Importance:` | `high` when `priority` is 1 or 2. |
| `MIME-Version`, `Content-Type` | `text/plain; charset=utf-8` for the body; `multipart/mixed` with base64 parts when there are `files` or the part of §4. The text is `7bit` when it is ASCII with no line over 998 bytes, and `quoted-printable` UTF-8 otherwise. |

The body ends with a short footer, after the usual signature line of two dashes and a space, so
that mail programs leave it out of replies. The post office MAY reword it:

```
-- 
Sent by amateur radio through RadioPost. Please reply in plain text, under 20 kB.
Amateur radio carries no business content.
```

Then the post office DKIM-signs the message for the sender's domain and sends it, directly or
through a relay.

### 3.2 The `Radio-Path` header

Records which station sent the message by radio, through which gateway, and when:

```
Radio-Path: KK4ODA via N1CALL-10; at=2026-11-09T14:40:41Z; session=d4a91c07e2b35f68
```

* first, the operator's callsign from the session statement;
* `via`, the gateway's callsign and, after forwarding, `via` each further gateway of the path
  in order (RP-7 §3.1, RP-5 §5.6);
* `at`, the session's start (`started`, RP-5 §5.2), ISO 8601 in UTC;
* `session`, the gateway's session ID (RP-3 `WELCOME` key 2) in hex, as the gateway gave it in
  `gateway_session` (RP-5 §5.2).

Test vectors: `vectors/email-out.json`: Facundo's two messages of `vectors/session-01.json`, one
with §4's part, and a priority message with a subject and text that are not ASCII.

### 3.3 Bounces

A delivery failure report (RFC 3464) for a message sent from RadioPost becomes a short message in
the sender's mailbox: from `postmaster@` the domain, to the sender, dated when the report arrived,
subject `Not delivered: ` and the original subject, and `reply_to` the content ID of the message that
failed. Its body has one line for each recipient whose `Action` is `failed`: the address, a colon,
and the reason, which is the `Diagnostic-Code` without its type (`552 5.2.2 Mailbox full`), or the
`Status` when there is none. It is not itself bounced. Test vectors: `vectors/email-in.json`,
`bounce`.

---

## 4. Between RadioPost post offices

Mail between two RadioPost post offices is ordinary email (§3, then §2). To keep the original
message, its content ID and its proof across the hop, the sending post office SHOULD add one MIME
part when **every** recipient in that SMTP transaction is at a RadioPost post office it knows
(RP-1 §7.5):

```
Content-Type: application/radiopost-message
Content-Transfer-Encoding: base64
```

The part holds a CBOR map (RP-0 §4.1): `1` the air form (RP-2 §4), `2` the sender's credential, `3`
the session statement's encoding (RP-1 §6.2), `4` its signature, `5` `started` (RP-5 §5.2); all byte
strings but `5`, an integer.

The receiving post office checks the proof as RP-1 §6.3 says for a statement relayed later, with
the credential and `time` checked against `started`. Then it checks that the message's content ID
is in the statement, that its `from` is the statement's address, and that the recipient of this
SMTP transaction is in its `to` or `cc`. If all pass, it uses that message, with its priority and
content ID, instead of converting the email (§2.4). If any fails, it converts the email as usual and
drops the part. Test vectors: `vectors/email-in.json`, `between`.

---

## 5. The rules

### 5.1 Third-party traffic

Under US rules (47 CFR §97.115, e-CFR current to 2026-11-05) a station may transmit a message for a
third party to any station under US jurisdiction. To a station under a foreign government it may do
so only for emergency or disaster relief, or where that government has an arrangement with the US.
The restriction does not apply to a third party eligible to be a control operator, that is, a
licensed amateur. What decides is where the receiving station is, not where the sender is.

* **Flagged at the post office.** Mail whose sender is not shown to be licensed is **third-party
  mail**: every message made from an email (§2.4), mail a user submits from a mail program
  included (§2.6), unless it came with the RadioPost part of §4 and that part was used. Mail a RadioPost user handed in (RP-5 §5.4, §5.6, §6.6) is not. The post office
  marks it in waiting entries and copies (RP-5 `third_party`), and gateways carry the mark on the air
  (RP-3 §5.2, the flag `third-party`).
* **Checked at the gateway.** A gateway hands third-party mail to a station, a user or a cut-off
  gateway collecting forward items (RP-7 §4.3), only when that station is in the gateway's own
  country or in one with an arrangement, by the gateway's country's table below. Otherwise it
  leaves the message out of `waiting`, so that it stays at the post office, and says how many it
  held back in a `NOTICE` (RP-3 §5.10). Emergency or disaster relief abroad is the control
  operator's decision, never the program's.
* **Where the station is** comes from the callsign it operates under (`HELLO` `operator`). A part
  before or after a `/` that is not itself a whole callsign (a prefix, a digit, then one to four
  letters) names it: `VE3/KK4ODA` and `KK4ODA/VE3` are in Canada, `W4/G4ABC` in the US. `/P`, `/M`,
  `/QRP`, `/A` and a lone digit are ignored. `/MM` and `/AM` name no country, so such a station gets
  no third-party mail. Otherwise the callsign's own prefix names it. A station's prefix matches a
  table entry that it begins with.
* **Identification.** At the end of an exchange of third-party traffic with a station in another
  country, a US station also transmits that station's callsign (§97.115(d)): the modem's
  identification, or the operator, must carry both (RP-4 §8).

Tables of other countries' rules are owed; a gateway whose country has no table here hands
third-party mail only to stations of its own country, as its operator enters its prefixes.

**United States.** A station is under US jurisdiction when its prefix begins with K, N or W, or is
AA to AL. Arrangements, as the ARRL lists them
([arrl.org/third-party-operating-agreements](https://www.arrl.org/third-party-operating-agreements),
read 2026-11-09); `LO`–`LW` stands for every prefix from `LO` to `LW`:

| Prefixes | Country |
|---|---|
| V2 | Antigua and Barbuda |
| LO–LW | Argentina |
| VK | Australia |
| V3 | Belize |
| CP | Bolivia |
| E7 | Bosnia-Herzegovina |
| PP–PY | Brazil |
| VE, VO, VY | Canada |
| CA–CE | Chile |
| HJ–HK | Colombia |
| D6 | Comoros |
| TI, TE | Costa Rica |
| CM, CO | Cuba |
| HI | Dominican Republic |
| J7 | Dominica |
| HC–HD | Ecuador |
| YS | El Salvador |
| C5 | The Gambia |
| 9G | Ghana |
| J3 | Grenada |
| TG | Guatemala |
| 8R | Guyana |
| HH | Haiti |
| HQ–HR | Honduras |
| 4X, 4Z | Israel |
| 6Y | Jamaica |
| JY | Jordan |
| EL | Liberia |
| V7 | Marshall Islands |
| XA–XI | Mexico |
| V6 | Micronesia |
| YN | Nicaragua |
| HO–HP | Panama |
| ZP | Paraguay |
| OA–OC | Peru |
| DU–DZ | Philippines |
| V4 | St. Kitts and Nevis |
| J6 | St. Lucia |
| J8 | St. Vincent and the Grenadines |
| 9L | Sierra Leone |
| ZR–ZU | South Africa |
| 3DA | Eswatini |
| 9Y–9Z | Trinidad and Tobago |
| TA–TC | Turkey |
| GB | United Kingdom |
| CV–CX | Uruguay |
| YV–YY | Venezuela |
| 4U1ITU | ITU, Geneva |
| 4U1VIC | VIC, Vienna |

Pitcairn Island (VR6) has an informal arrangement for medical emergencies, urgent needs and its
residents' personal matters; a program cannot judge those, so it is not listed.

Test vectors: `vectors/third-party.json` (where twenty stations are, which may be handed
third-party mail, what a gateway lists to four of them); the flag in `vectors/api-exchanges.json`
and the session files.

### 5.2 Business and encryption

* **Business.** The refusal text and the footer say amateur radio carries no business; operators
  stop traffic that breaks the rules once they know of it (§97.219(c)).
* **Encryption** is refused at the door (§2.1), so nothing encrypted reaches the air.

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.1.0 | 2026-11-09 | First draft. |
| 0.1.1 | 2026-11-09 | Licence stated: CC BY 4.0. |
| 0.2.0 | 2026-11-09 | The door (§2.1): checks in order, the null sender and delivery reports, what is encrypted, which attachments are executable; how accept lists match (§2.2). Making the message (§2.4): addresses and dates, normalised text, a precise rule for quoted history, attachments by `attach_bytes`, held files. Mail out (§3): the date's form, the transfer encoding, the signature line `-- `, `Radio-Path`'s session from RP-5's `gateway_session`; a bounce's form (§3.3). Between post offices (§4): the part's types, and the checks, recipient included. Test vectors. |
| 0.2.1 | 2026-11-09 | §2.2: how writing to an address adds it, and how that is turned off, are RP-5 §6.5 and §6.3. |
| 0.2.2 | 2026-11-09 | §3.2: after forwarding, `Radio-Path` names each gateway of the path (RP-7). |
| 0.3.0 | 2026-11-09 | Third-party traffic (§5.1), from §97.115's text: the receiving station's country decides, not the sender's; the post office flags mail whose sender is not shown to be licensed; the gateway hands it only to stations of its own country or one with an arrangement, read from the operating callsign, with the US table from the ARRL's list; §5 becomes normative. |
| 0.3.1 | 2026-11-09 | §3.1: the example `Message-ID` holds a real content ID (RP-0 §6.3) |
| 0.4.0 | 2026-11-10 | §2.1: one `DATA` reply for every recipient: when they differ, the first refusal refuses the message. Found by building the door. |
| 0.5.0 | 2026-11-10 | §2.1: the door judges only the post office's accounts, so `postmaster@` and other mailboxes of the mail server reach it (the door had refused them). The DMARC and DKIM verdicts are the mail server's when it gives them, else the post office's own checks (Stalwart 1.1 gives its hooks none), and never the message's own `Authentication-Results`; a check that cannot be finished for now answers `451 4.7.1`. Found by setting up the pilot's server. |
| 0.6.0 | 2026-11-10 | §2.5: held attachments asked for: the message they come in, split by `max_air_bytes`, one too large for the air refused. RP-5 §5.3 had said they come later as a new message and nothing more. |
| 0.6.1 | 2026-11-12 | §2.1, §2.2, §4: the post offices a post office knows are RP-1 §7.5's (RP-1 0.6.0). |
| 0.7.0 | 2026-11-13 | §2.6, one mailbox: mail a user submits from a mail program is not judged at the door, becomes the RadioPost message of each recipient here (third-party, §5.1) and fills the sender's accept list; mail sent by radio to accounts here also reaches the mail server as §3.1's email, and a recipient here with no account gets only the email; mail read in a mail program is matched by `Message-ID`. Nothing said what became of mail a user sent from a mail program to another user of the same post office, which never reached the radio. |
