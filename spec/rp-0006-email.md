# RP-6: Email interoperation

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.1.1 |
| **Updated** | 2026-11-09 |
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

| When | Check | Refusal |
|---|---|---|
| `RCPT TO` | The recipient exists. | `550 5.1.1` |
| `RCPT TO` | The envelope sender, or its domain, is on the recipient's accept list (§2.2), or the sender's domain is a RadioPost post office the receiver knows (RP-1 §7.3). | `550 5.7.1` (§2.3) |
| `DATA` | The message passes DMARC for its `From:` domain, when that domain publishes DMARC. | `550 5.7.1` |
| `DATA` | The `From:` address, or its domain, is on the accept list, or is a known RadioPost post office's domain passing DKIM. (The header sender can differ from the envelope sender.) | `550 5.7.1` |
| `DATA` | Size within the post office's `max_email_bytes` (RP-5 §4.1). | `552 5.3.4` |
| `DATA` | Not encrypted: no `multipart/encrypted`, no S/MIME `enveloped-data`, no inline `-----BEGIN PGP MESSAGE-----`. | `554 5.7.1` |
| `DATA` | No executable attachments. | `554 5.7.1` |

A post office MAY apply ordinary spam filtering as well.

### 2.2 The accept list

Each account has an accept list (RP-5 §6.5) of addresses and domains (`@domain`).

* Writing to an address adds it, unless the user turns that off.
* Mail from other RadioPost post offices needs no entry: their users are licensed and their mail
  is signed.
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
message (RP-2) for the radio, and gives it the account's next **serial** (RP-3 §6.2).

1. **Addresses and date.** `from`: the `From:` address. `to`, `cc`: the `To:` and `Cc:` addresses.
   `date`: the `Date:` header, or the arrival time if there is none or it is unreadable.
2. **Subject.** The `Subject:` header, decoded (RFC 2047), as text.
3. **Text.** The `text/plain` part if there is one, otherwise the `text/html` part converted to
   text, with each link's address listed at the end as `[1] https://…`.
4. **Quoted history.** Text quoted from earlier messages is trimmed: lines beginning with `>`, and
   everything after a line that introduces a quotation (`On … wrote:`, or a `From:` … `Sent:` header
   block), when that is followed only by quotation. The post office keeps the full text, and sets
   `trimmed` (RP-2 §2).
5. **Attachments.** Each attachment is carried in `files` if, after the step below, the message's
   air form stays within the post office's limit for messages placed ahead or sent unasked (RP-5
   §4.4 `copies.max_air_bytes`, and the user's `auto`); otherwise it is listed under `held`.
   Photos (`image/jpeg`, `image/png`, `image/heic`, `image/webp`) SHOULD first be re-encoded as JPEG
   at most 640 pixels on the long side.
6. **Priority.** 0. Only a RadioPost sender can mark a message priority or emergency (§4).
7. **Reply.** If `In-Reply-To:` names a message this post office sent from RadioPost (§3.1), set
   `reply_to` to that message's content ID.
8. **Content ID.** As RP-2 §3; the post office keeps the mapping between it and the email's
   `Message-ID`.

---

## 3. Mail leaving for the Internet

### 3.1 Making the email

A message handed in by radio (RP-5 §5.4) leaves from the post office of its sender's address, never
from a gateway, so that its DKIM signature and SPF match its domain.

| Header | From |
|---|---|
| `From:` | `from`, with the account's display name. |
| `To:`, `Cc:` | `to`, `cc`. |
| `Date:` | `date`, as RFC 5322 date-time in UTC. |
| `Subject:` | `subject`, encoded if needed (RFC 2047). |
| `Message-ID:` | `<` the content ID in base32 `@` the sender's domain `>`: `<k3q7m2x9a4d8f6h1j5n0@radiopost.org>`. |
| `In-Reply-To:`, `References:` | When `reply_to` names a message whose `Message-ID` the post office knows (§2.4, step 8). |
| `Radio-Path:` | §3.2. |
| `Importance:` | `high` when `priority` is 1 or 2. |
| `MIME-Version`, `Content-Type` | `text/plain; charset=utf-8` for the body; `multipart/mixed` with base64 parts when there are `files`. |

The body ends with a short footer, which the post office MAY reword:

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
* `via`, the gateway's callsign (and, after forwarding, `via` the further gateways in order,
  RP-7);
* `at`, the session's start (`started`, RP-5 §5.2);
* `session`, the gateway's session ID in hex.

### 3.3 Bounces

A delivery failure report (RFC 3464) for a message sent from RadioPost becomes a short message in
the sender's mailbox, from `postmaster@` the domain, subject `Not delivered: ` and the original
subject, body the failed address and the reason in plain words. It is not itself bounced.

---

## 4. Between RadioPost post offices

Mail between two RadioPost post offices is ordinary email (§3, then §2). To keep the original
message, its content ID and its proof across the hop, the sending post office SHOULD add one MIME
part when **every** recipient in that SMTP transaction is at a RadioPost post office it knows:

```
Content-Type: application/radiopost-message
Content-Transfer-Encoding: base64
```

The part holds a CBOR map: `1` the air form (RP-2 §4), `2` the sender's credential, `3` the session
statement, `4` its signature, `5` `started` (RP-5 §5.2). The receiving post office checks the proof
(RP-1 §6.4) and the content ID, and uses that message, with its priority and content ID, instead of
converting the email (§2.4). If the check fails it converts the email as usual and drops the part.

---

## 5. The rules *(informative)*

* **Third-party traffic.** Mail from someone who is not licensed, sent to a radio user, is
  third-party traffic. In the US, international third-party traffic is allowed only with countries
  that have an agreement (§97.115). A post office SHOULD keep a table of agreements and SHOULD NOT
  place copies of, or hand over, such mail through gateways where that is not allowed. How the
  sender's country is known is *open*.
* **Business.** The refusal text and the footer say amateur radio carries no business; operators
  stop traffic that breaks the rules once they know of it (§97.219(c)).
* **Encryption** is refused at the door (§2.1), so nothing encrypted reaches the air.

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.1.0 | 2026-11-09 | First draft. |
| 0.1.1 | 2026-11-09 | Licence stated: CC BY 4.0. |
