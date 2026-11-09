# RadioPost: detailed design

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.1.5 |
| **Updated** | 2026-11-09 |
| **Editor** | KK4ODA |
| **Licence** | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| **Follows** | [The concept](../concept/00-concept.md) |

The concept says what RadioPost is for and how it feels to use. This document says how it is
built: the programs, the connections between them, the specification that governs each
connection, and the decisions behind them.

**The specifications in [`spec/`](../../spec/README.md) are the contract.** They are what anyone
building a client, a gateway, a post office or a modem adapter implements, and where this document
and a specification disagree, the specification is right. This document is the map and the
reasoning.

---

## 1. The parts and their specifications

![Which specification governs which connection](fig/d1-interfaces.svg)

*Figure D1.* Every connection in RadioPost is governed by one specification. Identity (RP-1) and
messages (RP-2) run through all of them.

| Program | What it does | Implements |
|---|---|---|
| **Client** | Holds the user's mail and key; talks to gateways by radio; manages the account when there is Internet. | RP-1, RP-2, RP-3 client side; RP-4 as host; RP-5 §6 |
| **Gateway** | A radio station that carries users' sessions to their post offices, checks them offline, queues mail and holds copies when the Internet is down. | RP-1, RP-2, RP-3 gateway side, RP-5 §5; RP-4 as host; RP-7 later |
| **Post office** | Holds mailboxes, issues credentials, serves gateways and clients, meets ordinary email. An off-the-shelf mail server plus the RadioPost service. | RP-1 issuing, RP-2, RP-5 serving, RP-6 |
| **Modem adapter** | Lets a client or gateway use a modem: Aether, VARA, ARDOP, AX.25 packet, or TCP. | RP-4 adapter side |

RP-0 §3 makes this formal: the roles, and what each must implement. **People can use the RadioPost
programs or not**: a gateway written by someone else serves RadioPost clients, a client written by
someone else uses RadioPost gateways, and an adapter for a new modem plugs into both.

---

## 2. Principles

1. **Specifications first.** A behaviour two programs depend on is written in a specification
   before it is relied on. The reference programs have no special standing.
2. **Open on the air.** Nothing on the air is encrypted, and every encoding is published, so anyone
   can decode any transmission.
3. **Small on the air.** Every byte costs time on a weak HF path: about a fifth of a second at
   Aether's slowest rung. Formats are compact; lists send content IDs, not messages; nothing is
   sent twice.
4. **Few turnarounds.** Every change of direction costs seconds. A session is five transmissions.
5. **Standards where they exist.** Ordinary email (SMTP, IMAP, JMAP, DKIM, SPF, DMARC), DNS, HTTPS,
   CBOR, JSON, JWS, Ed25519. RadioPost invents only what has no standard: the session on the air,
   the credential, the modem interface.
6. **Works without the Internet.** Gateways check users offline, hold copies, queue mail, and later
   forward by radio.
7. **Nobody guesses.** Every exchange has a defined end: who hangs up, when a message counts as
   delivered, what an error means.
8. **Federated.** Any number of post offices and gateways, run by different people, with no central
   server.

---

## 3. Decisions

Each decision names what was chosen, why, and what was turned down. They are the design's
reasoning; the specifications hold the result.

| # | Decision | Why | Turned down |
|---|---|---|---|
| D1 | **CBOR with small integer keys** on the air and in signed objects; **JSON** off the air (RP-0 §4). | A key costs one byte; the encoding is a standard with libraries everywhere, and deterministic encoding makes hashes and signatures stable. JSON is easier to debug where bytes are cheap. | JSON on the air (several times the bytes); Protocol Buffers (needs schemas and tooling to read a capture); a custom binary format (no tools at all). |
| D2 | **Ed25519 and SHA-256**; sign, never encrypt (RP-0 §5). | Small keys (32 bytes) and signatures (64), fast, hard to misuse; encryption is not allowed on the air. | RSA (signatures four times larger); ECDSA (a bad random number leaks the key). |
| D3 | **Content IDs**: 12 bytes of SHA-256 of the message (RP-0 §6.3, RP-2 §3). | Names a message by what it is: duplicates and resumes work across gateways, and a signed list of IDs signs the messages. 12 bytes is safe for the life of the network. | Sequence numbers (not global, no deduplication); 16 or 32 bytes (bytes on every list). |
| D4 | **A compact credential** of our own: a signed CBOR pair, about 160 bytes (RP-1 §4). | Small enough to send over HF on a first visit; trivial to parse in a small program. | X.509 certificates (several times larger, complex parsing); COSE_Sign1 (close, a few bytes more; to be reconsidered before Candidate). |
| D5 | **The first transmission is signed with the time**; a challenge only if the clocks disagree (RP-1 §6, RP-3 §5.1). | Saves a whole exchange on every session: tens of seconds on a weak path. Clocks are usually right (GPS, the Internet). | Always a challenge (one more exchange every time). |
| D6 | **A gateway keeps credentials**; the client sends a hash on later visits (RP-3 §5.1). | About 160 bytes saved on every return visit: 35 seconds at 36 bit/s. | Sending the credential every time. |
| D7 | **The caller hangs up** (RP-3 §8). | Someone must, by rule. Winlink's sessions through RMS Trimode waited 30 s at every end because the program that should hang up did not. | Either side; the server side. |
| D8 | **Small PDUs** (1 kB unless both sides agree to more) (RP-3 §2). | A broken link loses at most one PDU; everything before it counts toward resuming. | Whole messages (a break loses it all). |
| D9 | **Post office keys in DNS** and in signed JWS documents (RP-1 §7). | Every post office publishes its own; DNS is already how mail is found; no registry to run. | A central key registry (central); a web of trust (hard to explain and operate). |
| D10 | **HTTPS with JSON** between gateways and post offices; bearer tokens from a signed login (RP-5 §3). | Simple, debuggable, works through any firewall; tokens are short-lived and tied to a credential. | HTTP Message Signatures (more to get right in every client); TLS client certificates (certificate handling for every gateway). |
| D11 | **A gateway acts for a user only inside that user's signed session** (RP-5 §5.1). | A stolen gateway token reads nobody's mail. | Gateways with standing access to mailboxes. |
| D12 | **Copies reach gateways by long poll**, gateway to post office (RP-5 §5.8). | Gateways sit behind home routers: outgoing connections only. | The post office pushing to gateways (needs them reachable from the Internet). |
| D13 | **The modem interface is JSON Lines over a local socket** (RP-4). | Any language can write an adapter in an afternoon; a modem can speak it natively; byte data is small. | In-process plugins (one language only); reusing VARA's interface (a proprietary vocabulary, no delivery receipts); gRPC (heavy for a script). |
| D14 | **An off-the-shelf mail server** (Stalwart) with the RadioPost service beside it (RP-6). | Email is a solved, hard problem; deliverability and security are years of work we do not need to redo. | Writing our own mail server. |
| D15 | **Refuse at the SMTP door**, never bounce later (RP-6 §2.1). | No backscatter to forged senders; the sender's own provider explains the refusal. | Accept and bounce; challenge-response email. |
| D16 | **Mail leaves from the sender's post office** (RP-6 §3.1). | Only that server can sign for the domain, so the mail is delivered, not filtered. | Sending from the gateway (fails DKIM, SPF and DMARC). |
| D17 | **DEFLATE as the baseline**; Zstandard optional; a dictionary later (RP-2 §4). | DEFLATE is everywhere; a trained dictionary should do much better on short messages but must be published first. | Zstandard only (not in every small environment yet). |
| D18 | **Forwarding uses the same session protocol** (RP-7). | One protocol to implement and test; proofs travel with messages. | A separate store-and-forward protocol. |
| D19 | **A modem may be shared**: each host holds its own callsigns, answering switch and links (RP-4 §1.1). | During the move from Winlink, a gateway runs both systems on one radio, and two modems cannot share one sound card. A call names a callsign, so the callsign decides whose call it is, as AGWPE already does. | One program at a time (the operator must choose a system); routing by an SSID convention (callsigns are the operator's choice); a switchboard program between them (one more program to run). |
| D20 | **In a station-to-station session the station called proves itself too**, with a statement over the caller's nonce (RP-1 §6.5, RP-3 §10). | Anyone can transmit any callsign, so the caller has no other way to know who answered, or whose mail it is taking. The caller's own nonce makes the statement fresh with no clock to compare. | Trusting the callsign (forged with a microphone); a challenge from the caller (one more exchange); a time-form statement checked against the clock (fails when the clocks disagree, with no way to ask again). |

---

## 4. A session end to end

![A session as specified](../../spec/fig/rp3-session.svg)

*The RP-3 figure.* Facundo calls N1CALL-10 in Texas.

1. **The client** builds its session statement (its address, its callsign, the gateway called,
   the time, a random nonce, the IDs of the two messages it will send) and signs it with its device
   key (RP-1 §6). It sends `HELLO` with the statement's parts, the signature, the hash of its
   credential (it has called N1CALL-10 before) and where its last pickup ended.
2. **The gateway** finds the credential in its cache, checks it and the signature (RP-1 §5,
   §6.3), and, while the modem turns the link around, calls Facundo's post office:
   `POST /sessions` (RP-5 §5.2). The post office checks the statement again and answers with the
   waiting list and its view of the offers.
3. **The gateway** sends `WELCOME` and the one waiting message small enough to send unasked.
4. **The client** sends its two messages, a `RECEIPT` for the one it got, and a `REQUEST` for one
   more. The gateway hands the two messages to the post office (RP-5 §5.4), reports the pickup
   (§5.5) and fetches the requested message.
5. **The gateway** sends `RECEIPT`s, the requested message and `DONE`.
6. **The client** sends its `RECEIPT` and `DONE`, and hangs up. The gateway reports the last
   pickup and closes the session at the post office.
7. **The post office** turns Facundo's messages into ordinary email and sends them, DKIM-signed for
   `radiopost.org`, with a `Radio-Path` header naming KK4ODA and N1CALL-10 (RP-6 §3).

**Offline**, steps 2, 4 and 6 change: the gateway checks the credential and statement against the
keys and revocation list it holds, lists the copies placed ahead for Facundo, keeps his messages in
its queue, and reports everything with `POST /reports` when the Internet returns (RP-5 §5.6). The
client sees the same protocol, with `online` false and a `NOTICE` saying why.

---

## 5. Inside each program

### 5.1 The client

| Part | Does | Spec |
|---|---|---|
| Keys and credentials | Generates the device key, enrolls, renews, remembers which gateways hold its credential. | RP-1, RP-5 §6 |
| Message store | Mail as standard `.eml` files, plus each message's air form and content ID. | RP-2 |
| Session engine | The client side of RP-3: `HELLO`, transfers, receipts, resume. | RP-3 |
| Modem host | Talks RPMI to an adapter; or embeds one. | RP-4 |
| Gateway list | From post offices' gateway documents; ranked by distance, band and hours. | RP-5 §4.4 |
| Interface | Folders, compose, attachments, the waiting list, where each message is. | — |

### 5.2 The gateway

| Part | Does | Spec |
|---|---|---|
| Session engine | The gateway side of RP-3. | RP-3 |
| Verifier | Checks credentials and statements, online or not. | RP-1 §5, §6 |
| Key cache | Post office keys, info documents, revocation lists, refreshed every 6 hours. | RP-1 §7, RP-5 §4 |
| Post office client | Sessions, submissions, pickups, reports, copies. | RP-5 §5 |
| Store | The outgoing queue, copies placed ahead, partial uploads, the replay cache, credentials seen. | RP-3 §7 |
| Modem host | Talks RPMI to one adapter per radio. | RP-4 |
| Rules check | Bands and segments for automatic control, when the modem does not check them. | RP-4 §8 |
| Forwarder | Later: scheduled forwarding by radio. | RP-7 |

### 5.3 The post office

**The mail server** (Stalwart): SMTP in and out, IMAP and JMAP for users' mail programs, storage,
spam filtering, DKIM signing, Sieve.

**The RadioPost service** beside it:

| Part | Does | Spec |
|---|---|---|
| Identity | Accounts, enrollment codes, credentials, the post office's keys, revocation, the DNS and JWS documents. | RP-1 |
| API | The HTTPS interface for gateways and clients. | RP-5 |
| Door | Answers the mail server's MTA hook at `RCPT TO` and `DATA`: the accept list, encryption, size. | RP-6 §2.1 |
| Converter | Email to RadioPost message and back; serials; the content-ID-to-Message-ID map. | RP-6 §2.4, §3 |
| Copies | Places and drops copies at users' usual gateways. | RP-5 §5.8 |
| Registry | Gateway records and radio routes. | RP-5 §4.4, §6.4, RP-7 |

The service reaches the mailboxes through JMAP and sends through SMTP submission, so another mail
server with those interfaces could replace Stalwart (concept §8.1).

### 5.4 Modem adapters

| Adapter | Modem's interface | Notes |
|---|---|---|
| Aether | Control API, WebSocket | Delivery receipts, quality, busy, rules checked by the modem. Could become native RPMI. Shareable once its ADR-0106 is built (RP-4 §9.1). |
| VARA HF, VARA FM | Published TNC commands, two TCP ports | No delivery receipts. |
| ARDOP | Host command and data ports | To be checked against ARDOP's specification. |
| AX.25 | AGWPE (Direwolf, TNCs) | VHF and UHF packet. |
| TCP with TLS | — | The Internet, AREDN mesh, and tests. |

---

## 6. Reference implementations *(proposal)*

To be confirmed:

* **A shared core library** in Rust: the encodings, content IDs, credentials, statements and the
  session state machines (RP-1, RP-2, RP-3), used by all three programs and checked against the
  test vectors. Rust matches Aether, builds single binaries for Windows, macOS, Linux and the
  Raspberry Pi, and can be wrapped for other languages later.
* **The gateway and the RadioPost service** in Rust, as services.
* **The client** as a desktop application, built like the Aether app (Tauri, a panel with no build
  step), later a phone.
* **Adapters** in whatever language suits the modem; the Aether adapter first, then VARA and AGWPE.

---

## 7. Testing and conformance

* **Test vectors** in `vectors/` (started): messages and their content IDs and air forms (RP-2);
  credentials, their verification and session statements made with published test keys (RP-1);
  every PDU and three sessions, byte for byte: an ordinary one, uploads and downloads broken and
  resumed, and station to station (RP-3); modem interface lines, and conversations between hosts
  and a simulated modem, shared and not (RP-4); the API's signatures (RP-5); deterministic
  CBOR and DNS records (RP-0, RP-1). They are made by a small reference in Python, standard
  library only (`tools/vectors/`), written to read like the specifications rather than to be
  fast. `python tools/vectors/make.py` writes them and `--check` compares the reference with the
  files. Every implementation checks itself against them; a specification change updates them in
  the same pull request. Writing them found five gaps in the drafts, fixed in RP-0 0.2.0, RP-1
  0.2.0, RP-2 0.1.2, RP-3 0.1.2 and RP-5 0.2.0. The resumed and station-to-station sessions
  found three more, fixed in RP-1 0.3.0, RP-3 0.2.0 and RP-5 0.3.0: a partial download was never
  said to be sent unasked; nothing kept a resumed air form the same bytes; and the station called
  never proved who it was (D20). The modem interface's conversations found seven unwritten rules,
  fixed in RP-4 0.3.0: what a bad line gets, that a host ignores what it does not know, `refused`
  before `hello`, which `connect` errors come at once, what `ok` carries, `failed`'s codes with a
  new `aborted`, and that `received` keeps no `send` boundaries. Owed: RP-5's requests and
  answers beyond its signatures, and RP-6.
* **A conformance tester** that plays the other side: a client to test a gateway, a gateway to
  test a client, over the TCP adapter, including broken links, resumes and wrong signatures.
* **A loopback adapter** joining two hosts through RPMI with delays and losses, for tests with no
  radio. Aether's simulated channel can carry the same sessions with real modem timing.
* **Interoperation before Candidate:** a specification becomes Candidate only when two independent
  implementations pass its vectors and complete sessions with each other (RP-0 §8.1).

---

## 8. Security, in brief

| Threat | Answer |
|---|---|
| Someone calls as another user | Every session is signed with the user's device key; the gateway and the post office both check it (RP-1 §6). |
| A lost or stolen device | The post office revokes its credential; gateways fetch the list; credentials expire within 90 days (RP-1 §4.2, §8). |
| A gateway alters or forges mail | It cannot: content IDs are hashes, and the signed statement lists them (RP-1 §6.4). |
| A gateway reads mailboxes | Only inside a user's own signed session (RP-5 §5.1). |
| Replay of a session's start | Time window and replay cache, or a challenge (RP-1 §6.3). |
| Forged email from a RadioPost domain | DKIM, SPF and DMARC (RP-6 §1). |
| Spam to radio users | Accept lists at the SMTP door (RP-6 §2). |
| A dishonest post office | Gateways and other post offices choose which post offices' keys they keep (RP-1 §7.3). |

---

## 9. Order of work

| Phase | Delivers | Specs |
|---|---|---|
| 0 | These documents brought to complete drafts; test vectors; the conformance tester's outline. | All |
| 1 | The core library; the client; the TCP and Aether adapters; station-to-station sessions between two clients. | RP-1–RP-4 |
| 2 | The RadioPost service with Stalwart; the gateway, online; ordinary email both ways. The pilot: `radiopost.org` with KK4ODA-1. | RP-5, RP-6 |
| 3 | Copies placed ahead; the offline queue; reports. | RP-5 §5.6, §5.8 |
| 4 | Forwarding by radio. | RP-7 |
| 5 | More adapters (VARA, ARDOP, AGWPE); forms; the phone. | RP-4, new |

---

## 10. Open questions

Collected from the specifications and the concept, each to be settled in the document named.

| Question | Where |
|---|---|
| COSE_Sign1 for credentials instead of the custom pair? | RP-1 |
| How a new post office comes to be trusted by gateways that have never met its users. | RP-1 §7.3 |
| Proof of callsign at enrollment: LoTW, the sysop's check, or both. | RP-1 §1 |
| The Zstandard dictionary: training data, publication. | RP-2 §4 |
| How a client reports station-to-station mail to its post office. | RP-3 §10, RP-5 |
| ARDOP's host interface, checked against its specification. | RP-4 §9.3 |
| Third-party traffic: the table of agreements, and knowing a sender's country. | RP-6 §5 |
| Stalwart: MTA hooks or milter; AGPL for operators; clustering's edition. | Concept §13 |
| Forwarding: routing, schedules, expiry, reporting. | RP-7 §4 |
| The reference implementations' languages and packaging. | §6 here |

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.1.0 | 2026-11-09 | First draft. |
| 0.1.1 | 2026-11-09 | Licence stated: CC BY 4.0. |
| 0.1.2 | 2026-11-09 | Test vectors started (§7). |
| 0.1.3 | 2026-11-09 | D19: a modem may be shared (RP-4 0.2.0). |
| 0.1.4 | 2026-11-09 | D20: the station called proves itself in a station-to-station session; the resumed and station-to-station test vectors (§7). |
| 0.1.5 | 2026-11-09 | The modem interface's test vectors (§7). |
